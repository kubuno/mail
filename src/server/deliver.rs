//! Local delivery: turning the bytes an SMTP client just handed us into a row
//! of `mail.messages` that the web UI, IMAP and POP3 all read back.
//!
//! The module stores messages parsed apart rather than as raw MIME, so
//! delivering is the SAME work the IMAP sync does when it downloads a message
//! from a provider (`services::sync_service::store_message`): parse with
//! mail-parser, sanitise the HTML with ammonia, write the attachments to disk,
//! attach the message to a thread, insert. The sanitiser configuration is a
//! deliberate copy of the sync side's: a message must not become more dangerous
//! to display because it arrived through our own SMTP port instead of through
//! an account we poll.
//!
//! Resolving WHO the message is for is not done here: a client addresses a
//! person, and an address may be a mailbox, an alias, a catch-all or a
//! distribution list. `server::resolve` answers that question — including the
//! expansion limits it needs — and hands this module an already-resolved
//! [`LocalTarget`].

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use mail_parser::{HeaderValue, MessageParser, MimeHeaders};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::server::config::ServerConfig;
use crate::server::hygiene;

/// Where a local address lands: the Kubuno user, and the account row every
/// message and thread must hang off (`mail.messages.account_id` is NOT NULL).
///
/// `Hash` because this is what an expansion is deduplicated on: two aliases
/// leading to the same account must not file the same message twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalTarget {
    pub user_id:    Uuid,
    pub account_id: Uuid,
}

/// Where a message must be filed, as decided BEFORE it is stored.
///
/// The caller knows things this function cannot: whether the sending domain's
/// own DMARC policy said to quarantine, for one. Without this, a `quarantine`
/// verdict had nowhere to go and quietly degraded into a mark — a setting that
/// claimed to protect and did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Disposition {
    /// File normally; the recipient's own rules still apply.
    #[default]
    Inbox,
    /// File straight into Spam, and skip the Bayesian classifier: the decision
    /// has already been taken on stronger evidence than word statistics.
    Spam,
}

/// Stores one incoming message for one local recipient and returns its id.
///
/// `recipient` is the address that RESOLVED to `target` — the envelope
/// recipient itself, or the alias destination / list member it expanded to —
/// and `target` is what `server::resolve` decided. Resolution happens at RCPT
/// time, not here: refusing a recipient after DATA is refusing it too late.
///
/// `raw` is the DATA payload exactly as received (dot-unstuffed, with whatever
/// trace headers the server prepended). `attachments_dir` is the module's
/// configured directory, passed in rather than read from a global so this stays
/// callable from a test or from another entry point.
#[allow(clippy::too_many_arguments)] // one delivery, and every part of it is needed
pub async fn deliver_local(
    db: &PgPool,
    cfg: &ServerConfig,
    envelope_from: &str,
    recipient: &str,
    target: LocalTarget,
    raw: &[u8],
    attachments_dir: &str,
    disposition: Disposition,
    // Transport encryption of the hop that handed this message over, shown in
    // the message details ("tls" / "none"). `None` for an internal deposit,
    // which never crossed the network.
    security: Option<&str>,
    // DMARC verdict of this message ("pass" | "fail" | "none"). `None` when it
    // was not evaluated (internal deposit); the reader treats that as "not
    // authenticated" and withholds brand logos.
    auth_dmarc: Option<&str>,
) -> Result<Uuid> {
    // Input validation first — nothing reaches the database unchecked.
    if raw.is_empty() {
        anyhow::bail!("Message vide");
    }

    // Loop guard, before anything else: a message that already carries
    // HOPCOUNT_LIMIT `Received:` trace headers is not travelling, it is looping
    // between two servers (A→B→A). Refusing it here breaks the loop before it
    // can be stored and re-sent. Postfix's hopcount limit, re-implemented.
    let hops = hygiene::hop_count(raw);
    if hops >= hygiene::HOPCOUNT_LIMIT {
        tracing::error!(
            hops, limit = hygiene::HOPCOUNT_LIMIT, recipient = %recipient,
            "Boucle de courrier détectée (trop d'en-têtes Received) — dépôt refusé"
        );
        anyhow::bail!(
            "Boucle de courrier détectée : {hops} en-têtes Received (limite {})",
            hygiene::HOPCOUNT_LIMIT
        );
    }

    // Bounded address validation: a recipient that could not be a real address
    // (empty, too long, control characters, no `@`) never reaches the database.
    if !hygiene::valid_envelope_address(recipient) {
        tracing::error!(recipient = %recipient, "Adresse destinataire invalide — dépôt refusé");
        anyhow::bail!("Adresse destinataire invalide : {recipient}");
    }

    if !cfg.is_local_domain(recipient) {
        // Defence in depth: the SMTP front-end already refuses relaying, this
        // makes local delivery impossible to misuse from anywhere else.
        anyhow::bail!("Destinataire hors des domaines locaux");
    }

    // TODO(quota): `mail.mailboxes.quota_bytes` is read by the resolver and
    // carried on `resolve::LocalDelivery`, but it is NOT enforced here, because
    // enforcing it needs the size of what the mailbox already holds and
    // `mail.messages` has no size column — bodies are stored parsed apart, so
    // anything computed from them is an estimate, and an estimate means
    // refusing legitimate mail on a mailbox that is not actually full. When a
    // size becomes available, the refusal must be TEMPORARY (`452 4.2.2`, see
    // `resolve::Refusal::MailboxFull`): a quota is a passing state, and a 5xx
    // would bounce for good a message the next deletion makes deliverable.

    // Stamp this local-delivery stage's own trace header before parsing and
    // storing. When the message arrives over SMTP the network peer is already
    // recorded by the front-end's own Received line; this one marks that the
    // message passed through local delivery on this host — and, standing on its
    // own, keeps the trace honest when `deliver_local` is driven from another
    // entry point. Both inserted values are sanitised, so neither the peer label
    // nor the hostname can smuggle in a second header.
    let stamped = hygiene::prepend_received(raw, "local", &cfg.hostname);

    let parsed = MessageParser::default()
        .parse(&stamped)
        .ok_or_else(|| anyhow::anyhow!("Parse RFC 5322 échoué"))?;

    let message_id = parsed.message_id().map(str::to_string);

    // Full References chain (RFC 5322): threading walks the whole set so a
    // reply still lands in its conversation when the direct parent is missing.
    let references: Vec<String> = match parsed.header("References") {
        Some(HeaderValue::Text(t)) => vec![t.to_string()],
        Some(HeaderValue::TextList(l)) => l.iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    };
    let in_reply_to = match parsed.in_reply_to() {
        HeaderValue::Text(t) => Some(t.to_string()),
        HeaderValue::TextList(list) => list.first().map(|s| s.to_string()),
        _ => None,
    };

    // Non-standard headers go down mail-parser's raw branch, so they are looked
    // up by name rather than through the typed accessors.
    let raw_header = |name: &str| -> Option<String> {
        parsed
            .headers()
            .iter()
            .find(|h| h.name().eq_ignore_ascii_case(name))
            .and_then(|h| match &h.value {
                HeaderValue::Text(t) => Some(t.to_string()),
                HeaderValue::TextList(l) => l.first().map(|s| s.to_string()),
                HeaderValue::Address(a) => a.first().and_then(|x| x.address()).map(str::to_string),
                _ => None,
            })
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };

    let list_unsubscribe = raw_header("List-Unsubscribe");

    // Loop signal for the forwarder: a message that already carries our forward
    // marker has been forwarded once, so it must never be forwarded again.
    let already_forwarded = raw_header(crate::services::forwarding::FORWARD_MARKER).is_some();

    // RFC 3834 anti-loop signals for the vacation responder, read here so the
    // responder never has to re-parse: whether this message is itself automatic
    // (`Auto-Submitted`), bulk/list traffic (`Precedence`), or a mailing-list
    // post (any `List-*` header).
    let auto_submitted = raw_header("Auto-Submitted");
    let precedence = raw_header("Precedence");
    let has_list_header = parsed
        .headers()
        .iter()
        .any(|h| h.name().get(..5).is_some_and(|p| p.eq_ignore_ascii_case("list-")));

    let reply_to = parsed
        .reply_to()
        .and_then(|a| a.first())
        .and_then(|a| a.address())
        .map(str::to_string);

    // "Mailed by": the envelope sender's domain. Here we have the real SMTP
    // envelope, which is better evidence than any header.
    let domain_of = |addr: &str| {
        addr.rsplit('@')
            .next()
            .map(|d| d.trim_end_matches('>').trim().to_ascii_lowercase())
            .filter(|d| !d.is_empty())
    };
    let mailed_by = domain_of(envelope_from).or_else(|| raw_header("Return-Path").and_then(|rp| domain_of(&rp)));

    // "Signed by": the d= tag of the DKIM signature. The signature itself is
    // NOT verified here — the column records a claim, not a verdict.
    let signed_by = raw_header("DKIM-Signature").and_then(|dkim| {
        dkim.split(';')
            .map(str::trim)
            .find_map(|tag| tag.strip_prefix("d=").map(|d| d.trim().to_string()))
    });

    let subject = parsed.subject().unwrap_or("(sans sujet)").to_string();

    let (from_name, from_email) = parsed
        .from()
        .and_then(|addrs| addrs.first())
        .map(|addr| {
            (
                addr.name().map(str::to_string),
                addr.address().unwrap_or("").to_string(),
            )
        })
        .filter(|(_, email)| !email.is_empty())
        // A message with no usable From falls back to the SMTP envelope, which
        // always has one on a legitimate delivery.
        .unwrap_or_else(|| (None, fallback_sender(envelope_from)));

    let to_addresses = addr_list_json(parsed.to());
    let cc_addresses = addr_list_json(parsed.cc());

    let mut index_entries: Vec<(String, Option<String>)> =
        vec![(from_email.clone(), from_name.clone())];
    index_entries.extend(crate::services::address_index::from_json_list(&to_addresses));
    index_entries.extend(crate::services::address_index::from_json_list(&cc_addresses));

    let body_text = parsed.body_text(0).map(|s| s.into_owned());
    let body_html_raw = parsed.body_html(0).map(|s| s.into_owned());
    let body_html = body_html_raw.as_deref().map(sanitize_html);

    let snippet = body_text
        .as_deref()
        .or(body_html.as_deref())
        .map(|s| s.chars().take(200).collect::<String>());

    // schema.org rich cards (Gmail-style), same as the sync path: JSON-LD from
    // the RAW html (before sanitising) plus any text/calendar (ICS) invite part.
    let ics_parts: Vec<String> = parsed
        .attachments()
        .filter(|p| {
            let is_cal = p.content_type().is_some_and(|c| {
                c.ctype().eq_ignore_ascii_case("text")
                    && c.subtype().is_some_and(|s| s.eq_ignore_ascii_case("calendar"))
            });
            is_cal
                || p.attachment_name()
                    .is_some_and(|n| n.to_ascii_lowercase().ends_with(".ics"))
        })
        .filter_map(|p| String::from_utf8(p.contents().to_vec()).ok())
        .collect();
    let structured_data =
        crate::services::structured_data::extract(body_html_raw.as_deref(), &ics_parts);
    // Read the calendar meaning now: `structured_data` is moved into the INSERT.
    let invite_notice = structured_data
        .as_ref()
        .and_then(crate::services::structured_data::invite_notice);

    // Attachments: metadata now, files on disk only after the transaction
    // commits, so a rolled back delivery leaves no orphan files behind.
    let msg_id = Uuid::new_v4();
    let msg_dir = std::path::Path::new(attachments_dir).join(msg_id.to_string());
    let mut att_meta: Vec<Value> = Vec::new();
    let mut att_files: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();
    for (idx, part) in parsed.attachments().enumerate() {
        let bytes = part.contents();
        if bytes.is_empty() {
            continue;
        }
        let raw_name = part.attachment_name().unwrap_or("piece-jointe").to_string();
        // Keep the filename readable but safe for the filesystem.
        let safe: String = raw_name
            .chars()
            .map(|c| if c.is_control() || matches!(c, '/' | '\\' | '\0') { '_' } else { c })
            .take(180)
            .collect();
        let mime = part
            .content_type()
            .map(|ct| match ct.subtype() {
                Some(sub) => format!("{}/{}", ct.ctype(), sub),
                None => ct.ctype().to_string(),
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let path = msg_dir.join(format!("{idx}_{safe}"));
        att_meta.push(json!({
            "name": raw_name,
            "mime": mime,
            "size": bytes.len(),
            "storage_path": path.to_string_lossy(),
        }));
        att_files.push((path, bytes.to_vec()));
    }
    let has_attachments = !att_meta.is_empty();

    let sent_at = parsed
        .date()
        .and_then(|d| DateTime::from_timestamp(d.to_timestamp(), 0));
    let msg_at = sent_at.unwrap_or_else(Utc::now);

    // A blocked sender goes straight to spam, exactly as on the sync side.
    let blocked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mail.blocked_senders WHERE user_id = $1 AND email = LOWER($2))",
    )
    .bind(target.user_id)
    .bind(&from_email)
    .fetch_one(db)
    .await
    .unwrap_or_else(|e| {
        tracing::error!(error = %e, "Lecture des expéditeurs bloqués");
        false
    });
    let quarantined = disposition == Disposition::Spam;
    let folder = if blocked || quarantined { "spam" } else { "inbox" };

    // The operator's spam prefix, applied to what we STORE — never to the bytes
    // we keep on disk. Rewriting the raw subject would invalidate the message's
    // DKIM signature, which matters the moment the recipient forwards it.
    let subject = match cfg.spam_subject_prefix.trim() {
        prefix if folder == "spam" && !prefix.is_empty() && !subject.starts_with(prefix) => {
            format!("{prefix} {subject}")
        }
        _ => subject,
    };

    let category = crate::services::categorize::for_sender(&from_email);

    // ── Thread + message, in one transaction ────────────────────────────────
    // A message without its thread (or a thread whose counters were never
    // updated) is a conversation the UI cannot render, so the two writes stand
    // or fall together.
    let mut tx = db.begin().await.context("Ouverture de la transaction de dépôt")?;

    let thread_id = find_or_create_thread(
        &mut tx,
        target,
        &subject,
        in_reply_to.as_deref(),
        &references,
        msg_at,
        from_name.as_deref(),
        &from_email,
    )
    .await?;

    sqlx::query(
        r#"INSERT INTO mail.messages
           (id, thread_id, account_id, user_id, message_id, in_reply_to, imap_uid, imap_folder,
            from_name, from_email, to_addresses, cc_addresses, attachments,
            subject, body_text, body_html, is_read, folder, sent_at, list_unsubscribe,
            reply_to, mailed_by, signed_by, security, category, auth_dmarc, is_starred, received_at,
            structured_data)
           VALUES ($1,$2,$3,$4,$5,$6,NULL,'INBOX',$7,$8,$9,$10,$11,$12,$13,$14,FALSE,$15,$16,$17,
                   $18,$19,$20,$21,$22,$23,FALSE,NOW(),$24)"#,
    )
    .bind(msg_id)
    .bind(thread_id)
    .bind(target.account_id)
    .bind(target.user_id)
    .bind(message_id.as_deref())
    .bind(in_reply_to.as_deref())
    .bind(from_name.as_deref())
    .bind(from_email.as_str())
    .bind(to_addresses)
    .bind(cc_addresses)
    .bind(json!(att_meta))
    .bind(subject.as_str())
    .bind(body_text.as_deref())
    .bind(body_html.as_deref())
    .bind(folder)
    .bind(sent_at)
    .bind(list_unsubscribe.as_deref())
    .bind(reply_to.as_deref())
    .bind(mailed_by.as_deref())
    .bind(signed_by.as_deref())
    .bind(security)
    .bind(category)
    .bind(auth_dmarc)
    .bind(structured_data)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, recipient = %recipient, "Insertion du message entrant échouée");
        e
    })
    .context("Insertion du message entrant")?;

    // Thread roll-up: counters are RECOMPUTED, and the headline fields only
    // move when this message really is the newest of the conversation.
    sqlx::query(
        "UPDATE mail.threads t
         SET message_count     = (SELECT COUNT(*) FROM mail.messages m
                                  WHERE m.thread_id = t.id AND m.is_deleted = FALSE),
             unread_count      = (SELECT COUNT(*) FROM mail.messages m
                                  WHERE m.thread_id = t.id AND m.is_deleted = FALSE AND m.is_read = FALSE),
             has_attachments   = t.has_attachments OR $5,
             snippet           = CASE WHEN $6 >= t.last_message_at THEN COALESCE($2, t.snippet) ELSE t.snippet END,
             last_sender_name  = CASE WHEN $6 >= t.last_message_at THEN $3 ELSE t.last_sender_name END,
             last_sender_email = CASE WHEN $6 >= t.last_message_at THEN $4 ELSE t.last_sender_email END,
             category          = CASE WHEN t.category_pinned THEN t.category
                                      WHEN $6 >= t.last_message_at THEN $7
                                      ELSE COALESCE(t.category, $7) END,
             last_message_at   = GREATEST(t.last_message_at, $6)
         WHERE t.id = $1",
    )
    .bind(thread_id)
    .bind(snippet.as_deref())
    .bind(from_name.as_deref())
    .bind(from_email.as_str())
    .bind(has_attachments)
    .bind(msg_at)
    .bind(category)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, thread_id = %thread_id, "Mise à jour du fil échouée");
        e
    })
    .context("Mise à jour du fil")?;

    tx.commit().await.context("Validation du dépôt local")?;

    // ── After the commit: things whose failure must not lose the message ────
    if has_attachments {
        if let Err(e) = tokio::fs::create_dir_all(&msg_dir).await {
            tracing::error!(dir = %msg_dir.display(), error = %e, "Création répertoire pièces jointes échouée");
        } else {
            for (path, bytes) in &att_files {
                if let Err(e) = tokio::fs::write(path, bytes).await {
                    tracing::error!(path = %path.display(), error = %e, "Écriture pièce jointe échouée");
                }
            }
        }
    }

    crate::services::address_index::upsert(db, target.user_id, &index_entries, 1).await;

    // Bayesian spam scoring, for a message that is still in the inbox. Tracks
    // whether the message ended up filed as spam, so the vacation responder below
    // never answers junk.
    let mut filed_as_spam = folder == "spam";
    if folder == "inbox" {
        match crate::services::spam_classifier::classify_incoming(
            db,
            target.user_id,
            &subject,
            body_text.as_deref(),
            &from_email,
        )
        .await
        {
            Ok(verdict) => {
                if let Some(score) = verdict.score {
                    if let Err(e) = sqlx::query("UPDATE mail.messages SET spam_score = $1 WHERE id = $2")
                        .bind(score as f32)
                        .bind(msg_id)
                        .execute(db)
                        .await
                    {
                        tracing::error!(error = %e, "Enregistrement du score de spam échoué");
                    }
                }
                if verdict.move_to_spam {
                    if let Err(e) = sqlx::query("UPDATE mail.messages SET folder = 'spam' WHERE id = $1")
                        .bind(msg_id)
                        .execute(db)
                        .await
                    {
                        tracing::error!(error = %e, "Déplacement vers spam échoué");
                    } else {
                        filed_as_spam = true;
                        tracing::info!(msg = %msg_id, score = ?verdict.score, "Message entrant déplacé vers spam");
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "Classification spam du message entrant échouée"),
        }
    }

    tracing::info!(
        msg = %msg_id, recipient = %recipient, folder,
        "Message déposé localement"
    );

    // Push notification — a message that actually reached the inbox rings the
    // recipient's registered devices through the core. Best-effort, after the
    // commit, and only for genuine inbox arrivals: a spam-filed message must not
    // notify. A publish failure never fails a delivery that already succeeded.
    if !filed_as_spam {
        crate::events::notify_incoming_mail(
            &cfg.core_url,
            &cfg.internal_secret,
            target.user_id,
            thread_id,
            from_name.as_deref(),
            &from_email,
            &subject,
        )
        .await;

        // Calendar messages get their own notification on top: an invitation to
        // answer, someone's reply to an invitation we sent, or a cancellation.
        if let Some(notice) = &invite_notice {
            crate::events::notify_calendar_message(
                &cfg.core_url,
                &cfg.internal_secret,
                target.user_id,
                thread_id,
                notice,
            )
            .await;
        }
    }

    // Vacation auto-reply — the single choke point every local delivery passes
    // through. Best-effort and last: it must never fail or delay a delivery that
    // already succeeded. It applies the RFC 3834 guard, the active window, the
    // contacts scope and the per-sender rate-limit before sending anything.
    crate::services::vacation::maybe_autorespond(
        db,
        cfg,
        attachments_dir,
        target.user_id,
        target.account_id,
        recipient,
        &crate::services::vacation::Incoming {
            envelope_from,
            from_email: from_email.as_str(),
            message_id: message_id.as_deref(),
            references: &references,
            auto_submitted: auto_submitted.as_deref(),
            precedence: precedence.as_deref(),
            has_list_header,
            is_spam: filed_as_spam,
        },
    )
    .await;

    // Automatic forwarding — the same choke point as the vacation responder, and
    // just as best-effort. If the recipient configured active forwarding rules,
    // re-send a copy of the stored message (with the anti-loop marker) to each
    // destination, and archive the local copy when asked. The marker on the copy
    // is what bounds the deliver_local → forward → deliver_local recursion.
    crate::services::forwarding::maybe_forward(
        db,
        cfg,
        attachments_dir,
        target.user_id,
        target.account_id,
        recipient,
        &crate::services::forwarding::Incoming {
            envelope_from,
            from_email: from_email.as_str(),
            auto_submitted: auto_submitted.as_deref(),
            already_forwarded,
            is_spam: filed_as_spam,
        },
        &stamped,
        msg_id,
    )
    .await;

    Ok(msg_id)
}

/// HTML sanitiser for incoming bodies.
///
/// Same configuration as the IMAP sync path (`services::sync_service`): mail
/// HTML relies on presentational attributes no modern sanitiser allows by
/// default, and dropping them turns legitimate mail into unreadable soup —
/// while scripts, event handlers and unknown URL schemes stay out.
fn sanitize_html(html: &str) -> String {
    ammonia::Builder::default()
        // Keep <style> blocks and the structural tags mail relies on.
        .rm_clean_content_tags(&["style"])
        .add_tags(&["style", "head", "html", "body", "font", "center"])
        // Generic attributes present on almost every element of an HTML mail.
        .add_generic_attributes(&[
            "style", "class", "id", "dir", "lang",
            "align", "valign",
            "bgcolor", "background", "color",
            "width", "height",
            "role", "aria-label", "aria-hidden",
        ])
        // <a>: target and name (anchors). NOT `rel`: ammonia 4.x panics when it
        // is listed here while `link_rel` (default: noopener noreferrer) already
        // adds it to every link.
        .add_tag_attributes("a", &["target", "name"])
        // <img>: legacy HTML mail attributes + lazy loading.
        .add_tag_attributes("img", &["border", "hspace", "vspace", "loading"])
        // <font>: colour, face, size (old mailers / Outlook).
        .add_tag_attributes("font", &["color", "face", "size"])
        // <table> and friends: the usual HTML mail attributes.
        .add_tag_attributes("table", &["cellpadding", "cellspacing", "border", "bgcolor", "background", "summary"])
        .add_tag_attributes("tr", &["bgcolor", "valign", "height"])
        .add_tag_attributes("td", &["cellpadding", "cellspacing", "bgcolor", "background", "nowrap", "valign", "width", "height"])
        .add_tag_attributes("th", &["cellpadding", "cellspacing", "bgcolor", "background", "nowrap", "valign", "width", "height"])
        // <body>: legacy background colours.
        .add_tag_attributes("body", &["bgcolor", "background", "text", "link", "alink", "vlink"])
        // Allow data: (inline base64 images) and cid: (inline MIME parts).
        .add_url_schemes(&["data", "cid"])
        .clean(html)
        .to_string()
}

/// Attaches the message to a conversation, inside the caller's transaction.
#[allow(clippy::too_many_arguments)] // threading needs the full envelope context
async fn find_or_create_thread(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target: LocalTarget,
    subject: &str,
    in_reply_to: Option<&str>,
    references: &[String],
    last_at: DateTime<Utc>,
    sender_name: Option<&str>,
    sender_email: &str,
) -> Result<Uuid> {
    // Graph threading: any stored message whose Message-ID appears in the
    // References chain (or In-Reply-To) anchors this one to its thread.
    let mut candidates: Vec<String> = references.to_vec();
    if let Some(r) = in_reply_to {
        if !candidates.iter().any(|c| c == r) {
            candidates.push(r.to_string());
        }
    }
    if !candidates.is_empty() {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT t.id FROM mail.threads t
             JOIN mail.messages m ON m.thread_id = t.id
             WHERE t.account_id = $1 AND m.message_id = ANY($2)
             ORDER BY t.last_message_at DESC
             LIMIT 1",
        )
        .bind(target.account_id)
        .bind(&candidates)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Recherche du fil par références");
            e
        })
        .context("Recherche du fil par références")?;

        if let Some(id) = existing {
            return Ok(id);
        }
    }

    // Subject fallback — ONLY for actual replies/forwards, so that unrelated
    // messages sharing a subject do not get merged into one conversation.
    let normalized = normalize_subject(subject);
    let had_prefix = normalized != subject.to_lowercase().trim();
    if had_prefix {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM mail.threads
             WHERE account_id = $1
               AND LOWER(subject) = $2
               AND last_message_at > NOW() - INTERVAL '30 days'
             ORDER BY last_message_at DESC
             LIMIT 1",
        )
        .bind(target.account_id)
        .bind(&normalized)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Recherche du fil par sujet");
            e
        })
        .context("Recherche du fil par sujet")?;

        if let Some(id) = existing {
            return Ok(id);
        }
    }

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO mail.threads (id, account_id, user_id, subject, last_sender_name, last_sender_email, last_message_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(id)
    .bind(target.account_id)
    .bind(target.user_id)
    .bind(subject)
    .bind(sender_name)
    .bind(sender_email)
    .bind(last_at)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Création du fil échouée");
        e
    })
    .context("Création du fil")?;

    Ok(id)
}

fn normalize_subject(s: &str) -> String {
    let s = s.to_lowercase();
    let s = s
        .trim_start_matches("re: ")
        .trim_start_matches("fwd: ")
        .trim_start_matches("fw: ");
    s.trim().to_string()
}

/// Sender of last resort when the message carries no usable `From`.
fn fallback_sender(envelope_from: &str) -> String {
    let candidate = envelope_from.trim();
    if candidate.contains('@') && candidate.len() <= 320 {
        candidate.to_ascii_lowercase()
    } else {
        "unknown@unknown".to_string()
    }
}

fn addr_list_json(addrs: Option<&mail_parser::Address>) -> Value {
    match addrs {
        None => json!([]),
        Some(addr) => {
            let list: Vec<Value> = addr
                .clone()
                .into_list()
                .into_iter()
                .map(|a| {
                    json!({
                        "name":  a.name().map(str::to_string),
                        "email": a.address().unwrap_or("").to_string(),
                    })
                })
                .collect();
            Value::Array(list)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_stripped_but_mail_layout_survives() {
        // NB : ammonia assainit un FRAGMENT — <body> est absorbé par l'analyseur
        // HTML, d'où les attributs legacy portés ici par <table>/<td>.
        let dirty = r##"<table cellpadding="4" bgcolor="#ffffff"><tr><td align="center">
             <font color="red">Bonjour</font><script>alert(1)</script>
             <a href="https://exemple.org" target="_blank" onclick="steal()">lien</a>
             </td></tr></table>"##;
        let clean = sanitize_html(dirty);
        assert!(!clean.contains("script"), "aucun script ne doit survivre");
        assert!(!clean.contains("onclick"), "aucun gestionnaire d'événement");
        assert!(clean.contains("cellpadding"), "la mise en page e-mail est conservée");
        assert!(clean.contains("bgcolor"));
        assert!(clean.contains("<font color=\"red\">"), "les polices legacy survivent");
        assert!(clean.contains("https://exemple.org"));
    }

    #[test]
    fn inline_images_keep_their_scheme() {
        let clean = sanitize_html(r#"<img src="cid:part1" width="10"><img src="data:image/png;base64,AA">"#);
        assert!(clean.contains("cid:part1"));
        assert!(clean.contains("data:image/png"));
    }

    #[test]
    fn javascript_urls_are_dropped() {
        let clean = sanitize_html(r#"<a href="javascript:alert(1)">clic</a>"#);
        assert!(!clean.contains("javascript"));
    }

    #[test]
    fn reply_subjects_normalise_to_their_root() {
        assert_eq!(normalize_subject("Re: Facture"), "facture");
        assert_eq!(normalize_subject("Fwd: Facture"), "facture");
        assert_eq!(normalize_subject("Facture"), "facture");
    }

    #[test]
    fn envelope_sender_is_the_last_resort() {
        assert_eq!(fallback_sender(" Alice@Example.COM "), "alice@example.com");
        assert_eq!(fallback_sender(""), "unknown@unknown");
        assert_eq!(fallback_sender("mailer-daemon"), "unknown@unknown");
    }
}
