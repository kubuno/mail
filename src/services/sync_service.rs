use anyhow::Result;
use chrono::Utc;
use mail_parser::{HeaderValue, MessageParser, MimeHeaders};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    config::settings::MailSettings,
    models::{EmailAccount, EmailFilter},
    services::{
        crypto::MailCrypto,
        imap_service::{self, ImapConfig},
    },
};

pub async fn sync_account(db: &PgPool, account: &EmailAccount, crypto: &MailCrypto, mail_cfg: &MailSettings) -> Result<()> {
    let (imap_pass_enc, imap_nonce): (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT imap_password, imap_password_nonce FROM mail.accounts WHERE id = $1"
    )
    .bind(account.id)
    .fetch_one(db)
    .await?;

    let imap_pass = crypto.decrypt(&imap_pass_enc, &imap_nonce)?;

    let cfg = ImapConfig {
        host:     account.imap_host.clone(),
        port:     account.imap_port as u16,
        security: account.imap_security.clone(),
        username: account.imap_username.clone(),
        password: imap_pass,
    };

    let mut session = imap_service::connect(&cfg)
        .await
        .map_err(|e| anyhow::anyhow!("IMAP connect: {e}"))?;

    for (folder, folder_name) in &[
        ("INBOX", "inbox"),
        ("Sent",  "sent"),
        ("Spam",  "spam"),
        ("Trash", "trash"),
    ] {
        if let Err(e) = sync_folder(db, account, &mut session, folder, folder_name, mail_cfg).await {
            tracing::warn!(
                account_id = %account.id,
                folder,
                error = %e,
                "Sync dossier échoué"
            );
        }
    }

    imap_service::logout(session).await;

    sqlx::query("UPDATE mail.accounts SET last_sync_at = $1, last_error = NULL WHERE id = $2")
        .bind(Utc::now())
        .bind(account.id)
        .execute(db)
        .await?;

    Ok(())
}

async fn sync_folder(
    db: &PgPool,
    account: &EmailAccount,
    session: &mut imap_service::ImapSession,
    imap_folder: &str,
    folder_name: &str,
    mail_cfg: &MailSettings,
) -> Result<()> {
    let last_uid: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(imap_uid) FROM mail.messages WHERE account_id = $1 AND imap_folder = $2"
    )
    .bind(account.id)
    .bind(imap_folder)
    .fetch_one(db)
    .await
    .unwrap_or(None);

    let since_uid = last_uid.map(|u| u as u32);
    // Honour the configured cap (`mail.max_fetch_per_sync`) instead of a hard-coded 100.
    let max_fetch = mail_cfg.max_fetch_per_sync.max(1);
    let raw_messages = imap_service::fetch_recent(session, imap_folder, max_fetch, since_uid).await?;

    for raw in raw_messages {
        if let Err(e) = store_message(db, account, &raw.body, raw.uid, imap_folder, folder_name, &mail_cfg.attachments_dir).await {
            tracing::warn!(uid = raw.uid, folder = imap_folder, error = %e, "Stockage message échoué");
        }
    }

    Ok(())
}

async fn store_message(
    db: &PgPool,
    account: &EmailAccount,
    raw: &[u8],
    uid: u32,
    imap_folder: &str,
    folder_name: &str,
    attachments_dir: &str,
) -> Result<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mail.messages WHERE account_id = $1 AND imap_folder = $2 AND imap_uid = $3)"
    )
    .bind(account.id)
    .bind(imap_folder)
    .bind(uid as i64)
    .fetch_one(db)
    .await?;

    if exists {
        return Ok(());
    }

    let parser = MessageParser::default();
    let parsed = parser.parse(raw).ok_or_else(|| anyhow::anyhow!("Parse RFC 5322 échoué"))?;

    let message_id  = parsed.message_id().map(str::to_string);
    // Full References chain (RFC 5322): every ancestor id. Threading walks this
    // whole set so a reply still lands in its thread even when the DIRECT parent
    // was never synced (deep chains, partial mailboxes).
    let references: Vec<String> = match parsed.header("References") {
        Some(HeaderValue::Text(t))     => vec![t.to_string()],
        Some(HeaderValue::TextList(l)) => l.iter().map(|s| s.to_string()).collect(),
        _                              => Vec::new(),
    };
    let in_reply_to = match parsed.in_reply_to() {
        HeaderValue::Text(t)        => Some(t.to_string()),
        HeaderValue::TextList(list) => list.first().map(|s| s.to_string()),
        _                           => None,
    };

    // En-tête List-Unsubscribe (RFC 2369) → permet la gestion des abonnements.
    let list_unsubscribe = match parsed.header("List-Unsubscribe") {
        Some(HeaderValue::Text(t))     => Some(t.to_string()),
        Some(HeaderValue::TextList(l)) => l.first().map(|s| s.to_string()),
        _                              => None,
    };

    let subject = parsed.subject().unwrap_or("(sans sujet)").to_string();

    let (from_name, from_email) = parsed
        .from()
        .and_then(|addrs| addrs.first())
        .map(|addr| (addr.name().map(str::to_string), addr.address().unwrap_or("").to_string()))
        .unwrap_or((None, "unknown@unknown".to_string()));
    let from_name_clone  = from_name.clone();
    let from_email_clone = from_email.clone();

    let to_addresses = addr_list_json(parsed.to());
    let cc_addresses = addr_list_json(parsed.cc());

    // Recipient-autocomplete index entries (sender + recipients), applied further
    // down only when the message is genuinely new.
    let mut index_entries: Vec<(String, Option<String>)> =
        vec![(from_email.clone(), from_name.clone())];
    index_entries.extend(crate::services::address_index::from_json_list(&to_addresses));
    index_entries.extend(crate::services::address_index::from_json_list(&cc_addresses));

    let body_text     = parsed.body_text(0).map(|s| s.into_owned());
    let body_html_raw = parsed.body_html(0).map(|s| s.into_owned());
    let body_html = body_html_raw.as_deref().map(|h| {
        ammonia::Builder::default()
            // Préserver les blocs <style> et les balises structurelles
            .rm_clean_content_tags(&["style"])
            .add_tags(&["style", "head", "html", "body", "font", "center"])
            // Attributs génériques présents sur presque tous les éléments HTML email
            .add_generic_attributes(&[
                "style", "class", "id", "dir", "lang",
                "align", "valign",
                "bgcolor", "background", "color",
                "width", "height",
                "role", "aria-label", "aria-hidden",
            ])
            // <a> : autoriser target et name (ancres). PAS `rel` : ammonia 4.x panique
            // si `rel` est listé ici alors que `link_rel` (défaut: noopener noreferrer)
            // l'ajoute déjà automatiquement aux liens.
            .add_tag_attributes("a", &["target", "name"])
            // <img> : attributs legacy HTML emails + lazy loading
            .add_tag_attributes("img", &["border", "hspace", "vspace", "loading"])
            // <font> : couleur, police, taille (emails anciens / Outlook)
            .add_tag_attributes("font", &["color", "face", "size"])
            // <table> : attributs courants HTML email
            .add_tag_attributes("table", &["cellpadding", "cellspacing", "border", "bgcolor", "background", "summary"])
            .add_tag_attributes("tr",    &["bgcolor", "valign", "height"])
            .add_tag_attributes("td",    &["cellpadding", "cellspacing", "bgcolor", "background", "nowrap", "valign", "width", "height"])
            .add_tag_attributes("th",    &["cellpadding", "cellspacing", "bgcolor", "background", "nowrap", "valign", "width", "height"])
            // <body> : couleurs de fond legacy
            .add_tag_attributes("body",  &["bgcolor", "background", "text", "link", "alink", "vlink"])
            // Autoriser data: (images base64 inline) et cid: (pièces jointes inline MIME)
            .add_url_schemes(&["data", "cid"])
            .clean(h)
            .to_string()
    });

    let snippet = body_text
        .as_deref()
        .or(body_html.as_deref())
        .map(|s| s.chars().take(200).collect::<String>());

    // Incoming attachments: collect metadata + bytes now, but only write the files
    // to disk AFTER the INSERT succeeds (ON CONFLICT DO NOTHING → no orphan files
    // duplicated on every sync). Served later by download_attachment (fs read).
    let msg_id = Uuid::new_v4();
    let msg_dir = std::path::Path::new(attachments_dir).join(msg_id.to_string());
    let mut att_meta:  Vec<serde_json::Value> = Vec::new();
    let mut att_files: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();
    for (idx, part) in parsed.attachments().enumerate() {
        let bytes = part.contents();
        if bytes.is_empty() { continue }
        let raw_name = part.attachment_name().unwrap_or("piece-jointe").to_string();
        // Keep the filename readable but safe for the filesystem (no separators/control chars).
        let safe: String = raw_name.chars()
            .map(|c| if c.is_control() || matches!(c, '/' | '\\' | '\0') { '_' } else { c })
            .take(180)
            .collect();
        let mime = part.content_type()
            .map(|ct| match ct.subtype() {
                Some(sub) => format!("{}/{}", ct.ctype(), sub),
                None      => ct.ctype().to_string(),
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

    let sent_at = parsed.date().and_then(|d| {
        chrono::DateTime::from_timestamp(d.to_timestamp(), 0)
    });

    let thread_id = find_or_create_thread(
        db,
        account,
        &subject,
        in_reply_to.as_deref(),
        &references,
        sent_at.unwrap_or_else(Utc::now),
        from_name.as_deref(),
        from_email.as_str(),
    )
    .await?;

    // Valeurs conservées pour l'évaluation des filtres (les autres sont déplacées dans les binds).
    let f_from    = from_email_clone.clone();
    let f_name    = from_name_clone.clone();
    let f_subject = subject.clone();
    let f_body    = body_text.clone();
    let f_to      = to_addresses.to_string();

    let inserted = sqlx::query(
        r#"INSERT INTO mail.messages
           (id, thread_id, account_id, user_id, message_id, in_reply_to, imap_uid, imap_folder,
            from_name, from_email, to_addresses, cc_addresses, attachments,
            subject, body_text, body_html, is_read, folder, sent_at, list_unsubscribe, received_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,NOW())
           ON CONFLICT (account_id, imap_folder, imap_uid) DO NOTHING"#,
    )
    .bind(msg_id)
    .bind(thread_id)
    .bind(account.id)
    .bind(account.user_id)
    .bind(message_id)
    .bind(in_reply_to)
    .bind(uid as i64)
    .bind(imap_folder)
    .bind(from_name)
    .bind(from_email)
    .bind(to_addresses)
    .bind(cc_addresses)
    .bind(json!(att_meta))
    .bind(&subject)
    .bind(body_text)
    .bind(body_html)
    .bind(false)
    .bind(folder_name)
    .bind(sent_at)
    .bind(list_unsubscribe)
    .execute(db)
    .await?;

    // Feed the recipient-autocomplete index (weight 1 for synced mail).
    if inserted.rows_affected() > 0 {
        crate::services::address_index::upsert(db, account.user_id, &index_entries, 1).await;
    }

    // Write attachment files + flag the thread — only for genuinely new messages.
    if inserted.rows_affected() > 0 && has_attachments {
        if let Err(e) = tokio::fs::create_dir_all(&msg_dir).await {
            tracing::error!(dir = %msg_dir.display(), error = %e, "Création répertoire pièces jointes échouée");
        } else {
            for (path, bytes) in &att_files {
                if let Err(e) = tokio::fs::write(path, bytes).await {
                    tracing::error!(path = %path.display(), error = %e, "Écriture pièce jointe échouée");
                }
            }
        }
        if let Err(e) = sqlx::query("UPDATE mail.threads SET has_attachments = TRUE WHERE id = $1")
            .bind(thread_id).execute(db).await
        {
            tracing::error!(thread_id = %thread_id, error = %e, "MAJ has_attachments échouée");
        }
    }

    // Expéditeur bloqué → spam direct (avant les filtres).
    if inserted.rows_affected() > 0 {
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM mail.blocked_senders WHERE user_id = $1 AND email = LOWER($2))",
        )
        .bind(account.user_id).bind(&f_from)
        .fetch_one(db).await.unwrap_or(false);
        if blocked {
            let _ = sqlx::query("UPDATE mail.messages SET folder = 'spam' WHERE id = $1").bind(msg_id).execute(db).await;
        }
    }

    // Filtres automatiques : uniquement sur un message RÉELLEMENT nouveau.
    if inserted.rows_affected() > 0 {
        let _ = apply_filters(db, account.user_id, account.id, thread_id, msg_id,
                              &f_from, f_name.as_deref(), &f_subject, f_body.as_deref(), &f_to).await;
    }

    // Classifieur bayésien anti-spam.
    if inserted.rows_affected() > 0 {
        if folder_name == "spam" {
            // Message déjà classé spam côté serveur (dossier IMAP « Spam ») :
            // exemple d'entraînement fiable, on l'apprend comme spam.
            match crate::services::spam_classifier::learn_message(
                db, account.user_id, &f_subject, f_body.as_deref(), &f_from, true, None,
            ).await {
                Ok(guard) => {
                    let _ = sqlx::query("UPDATE mail.messages SET spam_trained = $1 WHERE id = $2")
                        .bind(guard).bind(msg_id).execute(db).await;
                }
                Err(e) => tracing::warn!(error = %e, "Entraînement spam (dossier IMAP) échoué"),
            }
        } else if folder_name == "inbox" {
            // Le message a pu être déplacé entre-temps (expéditeur bloqué / filtre).
            // On ne score que s'il est TOUJOURS dans la boîte de réception.
            let still_inbox: bool = sqlx::query_scalar(
                "SELECT folder = 'inbox' FROM mail.messages WHERE id = $1",
            )
            .bind(msg_id).fetch_one(db).await.unwrap_or(false);
            if still_inbox {
                match crate::services::spam_classifier::classify_incoming(
                    db, account.user_id, &f_subject, f_body.as_deref(), &f_from,
                ).await {
                    Ok(v) => {
                        if let Some(score) = v.score {
                            let _ = sqlx::query("UPDATE mail.messages SET spam_score = $1 WHERE id = $2")
                                .bind(score as f32).bind(msg_id).execute(db).await;
                        }
                        if v.move_to_spam {
                            let _ = sqlx::query("UPDATE mail.messages SET folder = 'spam' WHERE id = $1")
                                .bind(msg_id).execute(db).await;
                            tracing::info!(msg = %msg_id, score = ?v.score, "Message déplacé vers spam (bayésien)");
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "Classification spam échouée"),
                }
            }
        }
    }

    sqlx::query(
        "UPDATE mail.threads
         SET message_count     = message_count + 1,
             unread_count      = unread_count + 1,
             snippet           = COALESCE($2, snippet),
             last_sender_name  = $3,
             last_sender_email = $4,
             last_message_at   = GREATEST(last_message_at, NOW())
         WHERE id = $1",
    )
    .bind(thread_id)
    .bind(snippet)
    .bind(from_name_clone)
    .bind(from_email_clone)
    .execute(db)
    .await?;

    Ok(())
}

#[allow(clippy::too_many_arguments)] // threading needs the full envelope context
async fn find_or_create_thread(
    db: &PgPool,
    account: &EmailAccount,
    subject: &str,
    in_reply_to: Option<&str>,
    references: &[String],
    last_at: chrono::DateTime<Utc>,
    sender_name: Option<&str>,
    sender_email: &str,
) -> Result<Uuid> {
    // Graph threading: any stored message whose Message-ID appears in the
    // References chain (or In-Reply-To) anchors this message to its thread —
    // the most recently active thread wins if several match.
    let mut candidates: Vec<String> = references.to_vec();
    if let Some(r) = in_reply_to {
        if !candidates.iter().any(|c| c == r) { candidates.push(r.to_string()) }
    }
    if !candidates.is_empty() {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT t.id FROM mail.threads t
             JOIN mail.messages m ON m.thread_id = t.id
             WHERE t.account_id = $1 AND m.message_id = ANY($2)
             ORDER BY t.last_message_at DESC
             LIMIT 1"
        )
        .bind(account.id)
        .bind(&candidates)
        .fetch_optional(db)
        .await?;

        if let Some(id) = existing {
            return Ok(id);
        }
    }

    // Subject fallback — ONLY for actual replies/forwards (Re:/Fwd: prefix).
    // A fresh subject with no prefix always starts its own thread; this stops
    // unrelated messages that merely share a subject («Facture», «Bonjour»…)
    // from being merged into one conversation.
    let normalized = normalize_subject(subject);
    let had_prefix = normalized != subject.to_lowercase().trim();
    if had_prefix {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM mail.threads
             WHERE account_id = $1
               AND LOWER(subject) = $2
               AND last_message_at > NOW() - INTERVAL '30 days'
             ORDER BY last_message_at DESC
             LIMIT 1"
        )
        .bind(account.id)
        .bind(&normalized)
        .fetch_optional(db)
        .await?;

        if let Some(id) = existing {
            return Ok(id);
        }
    }

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO mail.threads (id, account_id, user_id, subject, last_sender_name, last_sender_email, last_message_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7)"
    )
    .bind(id)
    .bind(account.id)
    .bind(account.user_id)
    .bind(subject)
    .bind(sender_name)
    .bind(sender_email)
    .bind(last_at)
    .execute(db)
    .await?;

    Ok(id)
}

fn normalize_subject(s: &str) -> String {
    let s = s.to_lowercase();
    let s = s.trim_start_matches("re: ")
             .trim_start_matches("fwd: ")
             .trim_start_matches("fw: ");
    s.trim().to_string()
}

fn addr_list_json(addrs: Option<&mail_parser::Address>) -> serde_json::Value {
    match addrs {
        None => json!([]),
        Some(addr) => {
            let list: Vec<serde_json::Value> = addr
                .clone()
                .into_list()
                .into_iter()
                .map(|a| json!({
                    "name":  a.name().map(str::to_string),
                    "email": a.address().unwrap_or("").to_string(),
                }))
                .collect();
            serde_json::Value::Array(list)
        }
    }
}

// ── Application des filtres automatiques à un message entrant ─────────────────
#[allow(clippy::too_many_arguments)]
async fn apply_filters(
    db: &PgPool,
    user_id: uuid::Uuid,
    account_id: uuid::Uuid,
    thread_id: uuid::Uuid,
    msg_id: uuid::Uuid,
    from_email: &str,
    from_name: Option<&str>,
    subject: &str,
    body: Option<&str>,
    to_blob: &str,
) -> Result<()> {
    let filters = sqlx::query_as::<_, EmailFilter>(
        r#"SELECT id, user_id, account_id, from_contains, to_contains, subject_contains, query_contains,
                  act_archive, act_mark_read, act_star, act_important, act_trash, act_spam, act_label_id,
                  position, created_at
           FROM mail.filters
           WHERE user_id = $1 AND (account_id IS NULL OR account_id = $2)
           ORDER BY position, created_at"#,
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    if filters.is_empty() {
        return Ok(());
    }

    let contains = |hay: &str, needle: &str| hay.to_lowercase().contains(&needle.to_lowercase());
    let from_blob = format!("{} {}", from_email, from_name.unwrap_or(""));

    for f in filters {
        let mut m = true;
        if let Some(c) = f.from_contains.as_deref()    { m &= contains(&from_blob, c); }
        if let Some(c) = f.to_contains.as_deref()       { m &= contains(to_blob, c); }
        if let Some(c) = f.subject_contains.as_deref()  { m &= contains(subject, c); }
        if let Some(c) = f.query_contains.as_deref()    {
            m &= contains(subject, c) || body.map(|b| contains(b, c)).unwrap_or(false);
        }
        if !m { continue; }

        if f.act_star {
            let _ = sqlx::query("UPDATE mail.threads SET is_starred = TRUE WHERE id = $1").bind(thread_id).execute(db).await;
        }
        if f.act_important {
            let _ = sqlx::query("UPDATE mail.threads SET is_important = TRUE WHERE id = $1").bind(thread_id).execute(db).await;
        }
        if f.act_mark_read {
            let _ = sqlx::query("UPDATE mail.messages SET is_read = TRUE WHERE id = $1").bind(msg_id).execute(db).await;
        }
        let new_folder = if f.act_trash { Some("trash") } else if f.act_spam { Some("spam") } else if f.act_archive { Some("archive") } else { None };
        if let Some(fold) = new_folder {
            let _ = sqlx::query("UPDATE mail.messages SET folder = $1 WHERE id = $2").bind(fold).bind(msg_id).execute(db).await;
        }
        if let Some(lid) = f.act_label_id {
            let _ = sqlx::query("INSERT INTO mail.thread_labels (thread_id, label_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                .bind(thread_id).bind(lid).execute(db).await;
        }
    }

    // Recalcule les non-lus du fil (un filtre a pu marquer lu).
    let unread: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.messages WHERE thread_id = $1 AND is_read = FALSE AND is_deleted = FALSE",
    )
    .bind(thread_id).fetch_one(db).await.unwrap_or(0);
    let _ = sqlx::query("UPDATE mail.threads SET unread_count = $1 WHERE id = $2")
        .bind(unread as i32).bind(thread_id).execute(db).await;
    Ok(())
}
