use anyhow::Result;
use chrono::Utc;
use mail_parser::{HeaderValue, MessageParser, MimeHeaders};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    config::settings::{CoreSettings, MailSettings},
    models::{EmailAccount, EmailFilter},
    services::{
        crypto::MailCrypto,
        imap_service::{self, ImapConfig},
    },
};

pub async fn sync_account(db: &PgPool, account: &EmailAccount, crypto: &MailCrypto, mail_cfg: &MailSettings, core_cfg: &CoreSettings) -> Result<()> {
    // A local account has no external server to poll: the instance itself
    // delivers into it. Attempting an IMAP connection to its empty host would
    // only log errors in a loop. The worker's selection already excludes these;
    // this guard makes a stray direct call (manual sync) a no-op too.
    if account.kind == "local" {
        return Ok(());
    }

    // OAuth accounts (Gmail, Microsoft) authenticate with XOAUTH2 and an access
    // token; password accounts keep the historical LOGIN path untouched.
    let auth = if account.auth_kind.starts_with("oauth") {
        let token = crate::services::oauth::valid_access_token(db, crypto, mail_cfg, account.id).await?;
        imap_service::ImapAuth::Xoauth2(token)
    } else {
        let (imap_pass_enc, imap_nonce): (Vec<u8>, Vec<u8>) = sqlx::query_as(
            "SELECT imap_password, imap_password_nonce FROM mail.accounts WHERE id = $1"
        )
        .bind(account.id)
        .fetch_one(db)
        .await?;

        let imap_pass =
            crate::services::app_password_normalize(&account.imap_host, &crypto.decrypt(&imap_pass_enc, &imap_nonce)?);
        imap_service::ImapAuth::Password(imap_pass)
    };

    let cfg = ImapConfig {
        host:     account.imap_host.clone(),
        port:     account.imap_port as u16,
        security: account.imap_security.clone(),
        username: account.imap_username.clone(),
        auth,
    };

    let mut session = imap_service::connect(&cfg)
        .await
        .map_err(|e| anyhow::anyhow!("IMAP connect: {e}"))?;

    // Every mailbox the account owns — the five well-known ones AND the user's
    // own folders — instead of the four hard-coded names we used to sync.
    let mailboxes = match imap_service::list_mailboxes(&mut session).await {
        Ok(m) if !m.is_empty() => m,
        Ok(_) => fallback_mailboxes(),
        Err(e) => {
            tracing::warn!(account_id = %account.id, error = %e, "LIST dossiers échoué — repli sur les dossiers standard");
            fallback_mailboxes()
        }
    };

    // The inbox first, then the other well-known folders, then the user's own:
    // when the time budget runs out mid-run, what matters most is already in.
    let mut ordered = mailboxes;
    ordered.sort_by_key(|m| match m.kind {
        "inbox"  => 0,
        "sent"   => 1,
        "drafts" => 2,
        "custom" => 3,
        "archive" => 4,
        "spam"   => 5,
        _        => 6,
    });

    let started  = std::time::Instant::now();
    let deadline = std::time::Duration::from_secs(mail_cfg.sync_deadline_secs.max(30));

    for mb in &ordered {
        if started.elapsed() >= deadline {
            tracing::info!(account_id = %account.id, "Budget de synchronisation atteint — la suite au prochain passage");
            break;
        }
        if let Err(e) = sync_folder(db, account, &mut session, mb, mail_cfg, core_cfg, started, deadline).await {
            tracing::warn!(
                account_id = %account.id,
                folder = %mb.name,
                error = %e,
                "Sync dossier échoué"
            );
            let _ = sqlx::query(
                "UPDATE mail.folder_sync SET last_error = $1 WHERE account_id = $2 AND imap_folder = $3",
            )
            .bind(e.to_string())
            .bind(account.id)
            .bind(&mb.name)
            .execute(db)
            .await;
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

/// Servers that refuse LIST still get the classic five folders.
fn fallback_mailboxes() -> Vec<imap_service::MailboxInfo> {
    ["INBOX", "Sent", "Drafts", "Spam", "Trash"]
        .iter()
        .map(|n| imap_service::MailboxInfo {
            name: n.to_string(),
            kind: match *n {
                "INBOX" => "inbox",
                "Sent"  => "sent",
                "Drafts" => "drafts",
                "Spam"  => "spam",
                _       => "trash",
            },
        })
        .collect()
}

/// Synchronises one mailbox, in full.
///
/// Two passes over `mail.folder_sync`'s cursors: forward from `uid_high` for
/// what arrived since last time, then backwards from `uid_low` to bring down
/// the history. Both walk in batches of `max_fetch_per_sync`, newest batch
/// first, and every batch commits its own cursor — so a run cut short by the
/// time budget loses nothing and the next one picks up exactly where it
/// stopped. Repeated runs converge on the whole mailbox being stored, without
/// any single run holding more than one batch in memory.
#[allow(clippy::too_many_arguments)]
async fn sync_folder(
    db: &PgPool,
    account: &EmailAccount,
    session: &mut imap_service::ImapSession,
    mailbox: &imap_service::MailboxInfo,
    mail_cfg: &MailSettings,
    core_cfg: &CoreSettings,
    started: std::time::Instant,
    deadline: std::time::Duration,
) -> Result<()> {
    let imap_folder = mailbox.name.as_str();
    let folder_name = mailbox.kind;
    let batch = mail_cfg.max_fetch_per_sync.max(1) as usize;

    let exists = imap_service::select_folder(session, imap_folder).await?;

    // Self-heal: a mailbox first taken for a user folder (server silent on
    // special-use, or its name only readable once decoded) and now recognised
    // as a system one moves its messages over. Only rows still marked 'custom'
    // are touched — a message the classifier or a filter moved elsewhere keeps
    // where it was put.
    if folder_name != "custom" {
        match sqlx::query(
            "UPDATE mail.messages SET folder = $3 \
             WHERE account_id = $1 AND imap_folder = $2 AND folder = 'custom'",
        )
        .bind(account.id)
        .bind(imap_folder)
        .bind(folder_name)
        .execute(db)
        .await
        {
            Ok(r) if r.rows_affected() > 0 => tracing::info!(
                folder = imap_folder, kind = folder_name, moved = r.rows_affected(),
                "Dossier reclassé"
            ),
            Ok(_)  => {}
            Err(e) => tracing::error!(error = %e, folder = imap_folder, "Reclassement du dossier échoué"),
        }
    }

    let state: Option<(Option<i64>, Option<i64>, bool)> = sqlx::query_as(
        "SELECT uid_low, uid_high, backfill_done FROM mail.folder_sync \
         WHERE account_id = $1 AND imap_folder = $2",
    )
    .bind(account.id)
    .bind(imap_folder)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, folder = imap_folder, "Lecture état de synchronisation");
        e
    })?;

    let (mut uid_low, mut uid_high, mut backfill_done) = state.unwrap_or((None, None, false));

    if exists == 0 {
        save_folder_state(db, account.id, imap_folder, folder_name, uid_low, uid_high, true, 0).await;
        return Ok(());
    }

    let mut stored = 0usize;

    // ── Forward: everything newer than the highest UID we already hold ────────
    let forward_range = match uid_high {
        Some(h) => format!("{}:*", h + 1),
        None    => "1:*".to_string(),
    };
    let mut fresh = imap_service::uid_list(session, &forward_range).await?;
    fresh.retain(|u| uid_high.is_none_or(|h| *u as i64 > h));

    for chunk in fresh.rchunks(batch) {
        stored += store_batch(db, account, session, chunk, imap_folder, folder_name, mail_cfg, core_cfg).await;
        uid_high = Some(uid_high.unwrap_or(0).max(chunk.iter().copied().max().unwrap_or(0) as i64));
        if uid_low.is_none() {
            uid_low = chunk.iter().copied().min().map(|u| u as i64);
        }
        save_folder_state(db, account.id, imap_folder, folder_name, uid_low, uid_high, backfill_done, stored).await;
        if started.elapsed() >= deadline {
            return Ok(());
        }
    }

    // ── Backfill: walk down from the lowest UID we hold, batch by batch ───────
    if !backfill_done {
        let low = uid_low.unwrap_or(0);
        if low <= 1 {
            backfill_done = true;
        } else {
            let older = imap_service::uid_list(session, &format!("1:{}", low - 1)).await?;
            if older.is_empty() {
                backfill_done = true;
            } else {
                let mut exhausted = true;
                for chunk in older.rchunks(batch) {
                    stored += store_batch(db, account, session, chunk, imap_folder, folder_name, mail_cfg, core_cfg).await;
                    uid_low = chunk.iter().copied().min().map(|u| u as i64).min(uid_low);
                    save_folder_state(db, account.id, imap_folder, folder_name, uid_low, uid_high, false, stored).await;
                    if started.elapsed() >= deadline {
                        exhausted = false;
                        break;
                    }
                }
                backfill_done = exhausted;
            }
        }
    }

    save_folder_state(db, account.id, imap_folder, folder_name, uid_low, uid_high, backfill_done, stored).await;
    if stored > 0 {
        tracing::info!(
            account_id = %account.id, folder = imap_folder, stored, backfill_done,
            "Dossier synchronisé"
        );
    }
    Ok(())
}

/// Fetches one batch of UIDs and stores each message. Returns how many were
/// handled; a single bad message never aborts the batch.
///
/// The announced sizes are asked for FIRST (one extra round trip): a message
/// over `imap_service::MAX_MESSAGE_BYTES` is then skipped without a single byte
/// of its body crossing the wire, and the rest of the batch is split into
/// groups small enough to hold in memory at once. Skipping is deliberate and
/// final — the folder cursor moves past those UIDs — because retrying a message
/// we will never accept would stall the mailbox forever.
#[allow(clippy::too_many_arguments)]
async fn store_batch(
    db: &PgPool,
    account: &EmailAccount,
    session: &mut imap_service::ImapSession,
    uids: &[u32],
    imap_folder: &str,
    folder_name: &str,
    mail_cfg: &MailSettings,
    core_cfg: &CoreSettings,
) -> usize {
    let sizes = match imap_service::uid_sizes(session, uids).await {
        Ok(s) => s,
        Err(e) => {
            // No sizes means no pre-filter, not no limits: the planner then
            // budgets every UID at the worst case and the post-fetch guard in
            // `imap_service` still drops whatever comes back too big.
            tracing::warn!(folder = imap_folder, error = %e, "RFC822.SIZE indisponible — repli sur le pire cas");
            std::collections::HashMap::new()
        }
    };
    let plan = imap_service::plan_fetches(uids, &sizes);
    for &(uid, size) in &plan.oversized {
        tracing::warn!(
            uid, size, folder = imap_folder, limit = imap_service::MAX_MESSAGE_BYTES,
            "Message hors limite de taille — non téléchargé"
        );
    }

    let mut n = 0;
    for group in &plan.groups {
        let raws = match imap_service::fetch_uids(session, group).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(folder = imap_folder, error = %e, "FETCH lot échoué");
                continue;
            }
        };
        for raw in raws {
            match store_message(
                db, account, &raw.body, raw.uid, imap_folder, folder_name,
                raw.seen, raw.flagged, &mail_cfg.attachments_dir, Some(core_cfg),
            )
            .await
            {
                Ok(()) => n += 1,
                Err(e) => tracing::warn!(uid = raw.uid, folder = imap_folder, error = %e, "Stockage message échoué"),
            }
        }
    }
    n
}

/// Persists the folder cursors. Called after every batch: the cursor must
/// survive a run that is cut short.
#[allow(clippy::too_many_arguments)]
async fn save_folder_state(
    db: &PgPool,
    account_id: Uuid,
    imap_folder: &str,
    folder: &str,
    uid_low: Option<i64>,
    uid_high: Option<i64>,
    backfill_done: bool,
    stored: usize,
) {
    if let Err(e) = sqlx::query(
        r#"INSERT INTO mail.folder_sync
             (account_id, imap_folder, folder, uid_low, uid_high, backfill_done, messages_synced, last_error, last_sync_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,NULL,NOW())
           ON CONFLICT (account_id, imap_folder) DO UPDATE SET
             folder          = EXCLUDED.folder,
             uid_low         = LEAST(COALESCE(mail.folder_sync.uid_low, EXCLUDED.uid_low), EXCLUDED.uid_low),
             uid_high        = GREATEST(COALESCE(mail.folder_sync.uid_high, EXCLUDED.uid_high), EXCLUDED.uid_high),
             backfill_done   = EXCLUDED.backfill_done,
             messages_synced = mail.folder_sync.messages_synced + EXCLUDED.messages_synced,
             last_error      = NULL,
             last_sync_at    = NOW()"#,
    )
    .bind(account_id)
    .bind(imap_folder)
    .bind(folder)
    .bind(uid_low)
    .bind(uid_high)
    .bind(backfill_done)
    .bind(stored as i32)
    .execute(db)
    .await
    {
        tracing::error!(error = %e, folder = imap_folder, "Écriture état de synchronisation échouée");
    }
}

/// Ceilings on what ONE ingested message may contribute in attachments.
///
/// Both the IMAP sync and the mailbox importer decode every attachment of a
/// message into memory before the row is inserted — files can only be written
/// once the INSERT succeeded, otherwise a losing `ON CONFLICT DO NOTHING`
/// leaves orphans on disk that every later sync re-creates. That ordering is
/// worth keeping, so the buffer is bounded instead: without these, a crafted
/// message with thousands of tiny parts (a MIME bomb) or a handful of huge ones
/// sizes the process, not the sender.
///
/// The byte budget matches `imap_service::MAX_MESSAGE_BYTES`: an attachment set
/// can never legitimately outweigh the message that carried it. It still bites
/// on the importer path, which ingests local files rather than IMAP bodies.
const MAX_ATTACHMENTS_PER_MESSAGE: usize = 100;
const MAX_ATTACHMENT_BYTES_PER_MESSAGE: usize = imap_service::MAX_MESSAGE_BYTES;

/// Whether one more attachment of `next_len` bytes still fits in a message that
/// already holds `kept` attachments totalling `kept_bytes`.
fn attachment_fits(kept: usize, kept_bytes: usize, next_len: usize) -> bool {
    kept < MAX_ATTACHMENTS_PER_MESSAGE
        && kept_bytes.saturating_add(next_len) <= MAX_ATTACHMENT_BYTES_PER_MESSAGE
}

/// Shared with the migration importer (`services::import_service`): copying a
/// mailbox in from another provider must go through the very same ingestion
/// pipeline as a normal sync — parsing, threading, attachments, filters,
/// spam scoring and the `(account_id, imap_folder, imap_uid)` dedup — so an
/// imported mailbox is indistinguishable from a synced one.
#[allow(clippy::too_many_arguments)] // one message = envelope + flags + placement
pub(crate) async fn store_message(
    db: &PgPool,
    account: &EmailAccount,
    raw: &[u8],
    uid: u32,
    imap_folder: &str,
    folder_name: &str,
    seen: bool,
    flagged: bool,
    attachments_dir: &str,
    // When set (live IMAP sync), an iMIP REPLY found in this message is
    // forwarded to Calendar. `None` for historical ingestion (import/migration),
    // which must never re-fire an RSVP for mail received long ago.
    core_cfg: Option<&CoreSettings>,
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

    // Sent folder: skip messages we already stored ourselves right after the
    // SMTP send (services::sent_copy) — the server copy would be a duplicate.
    if folder_name == "sent" {
        if let Some(mid) = message_id.as_deref().filter(|s| !s.is_empty()) {
            let dup: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM mail.messages \
                 WHERE account_id = $1 AND folder = 'sent' \
                   AND TRIM(BOTH '<>' FROM COALESCE(message_id,'')) = TRIM(BOTH '<>' FROM $2))",
            )
            .bind(account.id)
            .bind(mid)
            .fetch_one(db)
            .await?;
            if dup {
                // Record the UID mapping is unnecessary; simply skip.
                return Ok(());
            }
        }
    }
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

    // Non-standard headers: look them up ourselves rather than through
    // `header(name)`. Unknown names go down mail-parser's raw branch and the
    // typed lookup came back empty, which is why List-Unsubscribe was never
    // stored and the whole "unsubscribe" feature stayed inert.
    let raw_header = |name: &str| -> Option<String> {
        parsed
            .headers()
            .iter()
            .find(|h| h.name().eq_ignore_ascii_case(name))
            .and_then(|h| match &h.value {
                HeaderValue::Text(t)     => Some(t.to_string()),
                HeaderValue::TextList(l) => l.first().map(|s| s.to_string()),
                HeaderValue::Address(a)  => a.first().and_then(|x| x.address()).map(str::to_string),
                _                        => None,
            })
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };

    let list_unsubscribe = raw_header("List-Unsubscribe");

    // Reply-To, when it differs from the From address.
    let reply_to = parsed
        .reply_to()
        .and_then(|a| a.first())
        .and_then(|a| a.address())
        .map(str::to_string);

    // "Mailed by": the envelope sender's domain — Return-Path first, then the
    // domain SPF authenticated, and finally the From domain.
    let domain_of = |addr: &str| addr.rsplit('@').next().map(|d| d.trim_end_matches('>').to_string());
    let mailed_by = raw_header("Return-Path")
        .and_then(|rp| domain_of(&rp))
        .or_else(|| {
            raw_header("Received-SPF").and_then(|spf| {
                spf.split("domain of ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|d| domain_of(d).or_else(|| Some(d.to_string())))
            })
        });

    // "Signed by": the d= tag of the DKIM signature.
    let signed_by = raw_header("DKIM-Signature").and_then(|dkim| {
        dkim.split(';')
            .map(str::trim)
            .find_map(|tag| tag.strip_prefix("d=").map(|d| d.trim().to_string()))
    });

    // "Security": the last hop is encrypted when the topmost Received line was
    // handed over through ESMTPS / a TLS version.
    let security = raw_header("Received")
        .filter(|r| {
            let r = r.to_ascii_lowercase();
            r.contains("esmtps") || r.contains("tls") || r.contains("using tlsv")
        })
        .map(|_| "tls".to_string());

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
    let body_html = body_html_raw
        .as_deref()
        .map(crate::services::html_sanitize::sanitize_email_html);

    let snippet = body_text
        .as_deref()
        .or(body_html.as_deref())
        .map(|s| s.chars().take(200).collect::<String>());

    // schema.org rich cards (Gmail-style): JSON-LD from the RAW html (before the
    // sanitizer strips <script>) plus any text/calendar (ICS) invite part.
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
    // Capture an iMIP REPLY's fields before `structured_data` is moved into the
    // INSERT; forwarded to Calendar after the row lands (live sync only).
    let invite_reply = core_cfg
        .and(structured_data.as_ref())
        .and_then(crate::services::structured_data::invite_reply_details);

    // Incoming attachments: collect metadata + bytes now, but only write the files
    // to disk AFTER the INSERT succeeds (ON CONFLICT DO NOTHING → no orphan files
    // duplicated on every sync). Served later by download_attachment (fs read).
    // The parts are BORROWED from `parsed`, which outlives the write below:
    // copying them would double the resident size of every attachment for no
    // reason.
    let msg_id = Uuid::new_v4();
    let msg_dir = std::path::Path::new(attachments_dir).join(msg_id.to_string());
    let mut att_meta:  Vec<serde_json::Value> = Vec::new();
    let mut att_files: Vec<(std::path::PathBuf, &[u8])> = Vec::new();
    let mut att_bytes   = 0usize;
    let mut att_skipped = 0usize;
    for (idx, part) in parsed.attachments().enumerate() {
        let bytes = part.contents();
        if bytes.is_empty() { continue }
        if !attachment_fits(att_files.len(), att_bytes, bytes.len()) {
            // The message itself is never dropped over its attachments: losing
            // the mail would be a worse outcome than losing a part of it. The
            // excess parts are left out of the metadata too, so the reader is
            // not offered a download that has no file behind it.
            att_skipped += 1;
            continue;
        }
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
        att_bytes = att_bytes.saturating_add(bytes.len());
        att_files.push((path, bytes));
    }
    if att_skipped > 0 {
        tracing::warn!(
            uid, folder = imap_folder, skipped = att_skipped,
            kept = att_files.len(), kept_bytes = att_bytes,
            "Pièces jointes hors limites — message stocké sans les excédentaires"
        );
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

    // Inbox category, decided ONCE here and read back by every listing.
    let category = crate::services::categorize::for_sender(&f_from);

    // OpenPGP: an encrypted or signed message must be decrypted / verified AT READ
    // TIME with the reader's key, but the store never keeps original MIME — so for
    // PGP messages only we preserve the raw RFC 5322 bytes. NULL for ordinary mail.
    let pgp_raw: Option<&[u8]> =
        crate::services::pgp_mime::looks_like_pgp(raw).then_some(raw);

    let inserted = sqlx::query(
        r#"INSERT INTO mail.messages
           (id, thread_id, account_id, user_id, message_id, in_reply_to, imap_uid, imap_folder,
            from_name, from_email, to_addresses, cc_addresses, attachments,
            subject, body_text, body_html, is_read, folder, sent_at, list_unsubscribe,
            reply_to, mailed_by, signed_by, security, category, is_starred, received_at, pgp_raw,
            structured_data)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                   $21,$22,$23,$24,$25,$26,COALESCE($19, NOW()),$27,$28)
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
    // Read state comes from the server's \Seen flag: backfilling a mailbox must
    // not resurrect years of mail as unread.
    .bind(seen)
    .bind(folder_name)
    .bind(sent_at)
    .bind(list_unsubscribe)
    .bind(reply_to)
    .bind(mailed_by)
    .bind(signed_by)
    .bind(security)
    .bind(category)
    .bind(flagged)
    .bind(pgp_raw)
    .bind(structured_data)
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

    // Autocrypt (Level 1): a message advertises the sender's public key in an
    // `Autocrypt:` header. Learn it passively (source='autocrypt') so we can
    // encrypt back later — only on a genuinely new message.
    if inserted.rows_affected() > 0 {
        let autocrypt_headers: Vec<String> = parsed
            .headers()
            .iter()
            .filter(|h| h.name().eq_ignore_ascii_case("Autocrypt"))
            .filter_map(|h| match &h.value {
                HeaderValue::Text(t) => Some(t.to_string()),
                HeaderValue::TextList(l) => l.first().map(|s| s.to_string()),
                _ => None,
            })
            .collect();
        if !autocrypt_headers.is_empty() {
            process_autocrypt(db, account.user_id, &f_from, autocrypt_headers).await;
        }
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

    // Thread roll-up. Counts are RECOMPUTED rather than incremented: backfill
    // stores messages out of order and a filter may already have marked one
    // read, so a running total would drift. The sender, snippet and date only
    // move when this message really is the newest of the thread — otherwise
    // downloading a 2019 message would relabel the conversation with it.
    let msg_at = sent_at.unwrap_or_else(Utc::now);
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
             -- Category follows the newest message, unless the user pinned the
             -- thread to a tab by dropping it there.
             category          = CASE WHEN t.category_pinned THEN t.category
                                      WHEN $6 >= t.last_message_at THEN $7
                                      ELSE COALESCE(t.category, $7) END,
             last_message_at   = GREATEST(t.last_message_at, $6)
         WHERE t.id = $1",
    )
    .bind(thread_id)
    .bind(snippet)
    .bind(from_name_clone)
    .bind(from_email_clone)
    .bind(has_attachments)
    .bind(msg_at)
    .bind(category)
    .execute(db)
    .await?;

    // An RSVP that just synced into the organizer's (external) account is
    // forwarded to Calendar. Only for a genuinely new message, and only in live
    // sync (`core_cfg` is `None` for import/migration). `account.user_id` owns
    // the mailbox that received the reply, i.e. the organizer.
    if inserted.rows_affected() > 0 {
        if let (Some(core), Some(reply)) = (core_cfg, &invite_reply) {
            crate::events::notify_invite_reply(
                &core.url,
                &core.internal_secret,
                account.user_id,
                &reply.uid,
                &reply.from,
                &reply.partstat,
                reply.sequence,
                None,
            )
            .await;
        }
    }

    Ok(())
}

/// Learn the sender's OpenPGP key from an `Autocrypt:` header (Level 1) and store
/// it as a correspondent key (source='autocrypt'). Never overwrites a manually
/// imported or WKD-fetched key — those are higher trust. Per the spec, a message
/// carrying more than one Autocrypt header, or whose `addr` does not match the
/// From address, is ignored.
///
/// Recency limitation: without a per-contact Autocrypt timestamp column, an
/// out-of-order backfill could refresh an autocrypt-sourced key with an older
/// message's key. The source gate confines this to keys we already learned
/// passively; user/WKD keys are unaffected.
async fn process_autocrypt(db: &PgPool, user_id: Uuid, from_email: &str, headers: Vec<String>) {
    // Exactly one Autocrypt header is honoured; zero or several ⇒ ignore.
    let [value] = headers.as_slice() else { return };
    let Some(header) = crate::services::autocrypt::parse(value) else { return };
    // The advertised address must be the message's From (Autocrypt §2.1).
    if !header.addr.eq_ignore_ascii_case(from_email) {
        return;
    }
    let Ok((armored, fingerprint, _emails)) =
        crate::services::pgp::import_public_bytes(&header.keydata)
    else {
        return;
    };
    let res = sqlx::query(
        r#"INSERT INTO mail.pgp_contacts (user_id, email, fingerprint, public_key, source)
           VALUES ($1, $2, $3, $4, 'autocrypt')
           ON CONFLICT (user_id, lower(email))
           DO UPDATE SET fingerprint = EXCLUDED.fingerprint,
                         public_key  = EXCLUDED.public_key,
                         source      = 'autocrypt'
           WHERE mail.pgp_contacts.source IN ('autocrypt', 'attached')"#,
    )
    .bind(user_id)
    .bind(header.addr.to_lowercase())
    .bind(&fingerprint)
    .bind(&armored)
    .execute(db)
    .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, "Stockage clé Autocrypt échoué");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the acceptance loop of `store_message` over a synthetic part list and
    /// reports what would actually have been buffered.
    fn ingest(parts: &[usize]) -> (usize, usize) {
        let mut kept = 0usize;
        let mut bytes = 0usize;
        for &len in parts {
            if attachment_fits(kept, bytes, len) {
                kept += 1;
                bytes += len;
            }
        }
        (kept, bytes)
    }

    #[test]
    fn tronque_un_message_portant_500_pieces_jointes() {
        let (kept, _) = ingest(&[1024; 500]);
        assert_eq!(
            kept, MAX_ATTACHMENTS_PER_MESSAGE,
            "le nombre de pièces jointes retenues doit être plafonné"
        );
    }

    #[test]
    fn plafonne_le_volume_cumule_des_pieces_jointes() {
        let chunk = 8 * 1024 * 1024;
        assert!(
            attachment_fits(2, 2 * chunk, chunk),
            "24 Mio cumulés doivent encore tenir sous le plafond"
        );
        assert!(
            !attachment_fits(3, 3 * chunk, chunk),
            "au-delà du volume cumulé, la pièce jointe doit être écartée"
        );
        let (kept, bytes) = ingest(&[chunk; 10]);
        assert_eq!(kept, 3, "seules les pièces jointes tenant dans le budget sont gardées");
        assert!(
            bytes <= MAX_ATTACHMENT_BYTES_PER_MESSAGE,
            "le volume bufferisé ne doit jamais dépasser le budget"
        );
    }

    #[test]
    fn une_piece_jointe_geante_ne_bloque_pas_les_suivantes() {
        let (kept, bytes) = ingest(&[MAX_ATTACHMENT_BYTES_PER_MESSAGE + 1, 1_024, 2_048]);
        assert_eq!(kept, 2, "la pièce jointe hors limite est sautée, pas les autres");
        assert_eq!(bytes, 3_072, "seules les pièces jointes acceptées comptent");
    }
}
