//! What the served protocols read: a user's messages, and the RFC 5322 bytes
//! for one of them.
//!
//! The module stores messages parsed apart (envelope in columns, bodies in
//! `body_text`/`body_html`, attachments as files) and never keeps the original
//! MIME. Serving IMAP or POP3 therefore means REBUILDING a message, not
//! replaying one — see `render`. It is faithful in content, not byte for byte:
//! a client that re-downloads a message gets our rendering of it, not the
//! original bytes, so signatures over the raw body will not verify.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// One message as the protocols see it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredMessage {
    pub id:         Uuid,
    pub local_uid:  i64,
    pub message_id: Option<String>,
    pub from_name:  Option<String>,
    pub from_email: String,
    pub to_addresses: Value,
    pub cc_addresses: Value,
    pub subject:    String,
    pub body_text:  Option<String>,
    pub body_html:  Option<String>,
    pub attachments: Value,
    pub is_read:    bool,
    pub is_starred: bool,
    pub folder:     String,
    pub sent_at:    Option<DateTime<Utc>>,
    pub received_at: DateTime<Utc>,
    /// CONDSTORE modification sequence (RFC 7162): a monotonic counter the
    /// database bumps on every change to the row. Serving it lets a client sync
    /// flags incrementally; comparing it drives CHANGEDSINCE / UNCHANGEDSINCE.
    pub modseq:     i64,
}

/// Folders a client can select. The provider's own folders are not exposed:
/// they are a property of an upstream account, not of this mailbox.
pub const FOLDERS: [&str; 5] = ["INBOX", "Sent", "Drafts", "Spam", "Trash"];

/// Maps an IMAP/POP3 mailbox name onto the local folder bucket.
pub fn folder_of(mailbox: &str) -> Option<&'static str> {
    match mailbox.trim().trim_matches('"').to_ascii_uppercase().as_str() {
        "INBOX"  => Some("inbox"),
        "SENT"   => Some("sent"),
        "DRAFTS" => Some("drafts"),
        "SPAM" | "JUNK" => Some("spam"),
        "TRASH"  => Some("trash"),
        _ => None,
    }
}

/// Messages of one folder, oldest first — the order both protocols number
/// their sequence numbers in.
pub async fn list(db: &PgPool, user_id: Uuid, folder: &str) -> Result<Vec<StoredMessage>> {
    sqlx::query_as::<_, StoredMessage>(
        r#"SELECT id, local_uid, message_id, from_name, from_email,
                  to_addresses, cc_addresses, subject, body_text, body_html,
                  attachments, is_read, is_starred, folder, sent_at, received_at, modseq
           FROM mail.messages
           WHERE user_id = $1 AND folder = $2 AND is_deleted = FALSE
             AND local_uid IS NOT NULL
           ORDER BY local_uid"#,
    )
    .bind(user_id)
    .bind(folder)
    .fetch_all(db)
    .await
    .context("Lecture des messages de la boîte")
}

/// Marks a message read/unread and keeps its thread's counter in step.
pub async fn set_read(db: &PgPool, user_id: Uuid, message_id: Uuid, read: bool) -> Result<()> {
    let mut tx = db.begin().await.context("Transaction état de lecture")?;
    sqlx::query("UPDATE mail.messages SET is_read = $1 WHERE id = $2 AND user_id = $3")
        .bind(read)
        .bind(message_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .context("Mise à jour de l'état de lecture")?;
    sqlx::query(
        "UPDATE mail.threads t SET unread_count = (
             SELECT COUNT(*) FROM mail.messages m
             WHERE m.thread_id = t.id AND m.is_deleted = FALSE AND m.is_read = FALSE)
         WHERE t.id = (SELECT thread_id FROM mail.messages WHERE id = $1)",
    )
    .bind(message_id)
    .execute(&mut *tx)
    .await
    .context("Recalcul des non-lus du fil")?;
    tx.commit().await.context("Validation de l'état de lecture")
}

/// Flags a message deleted (what IMAP EXPUNGE and POP3 DELE mean here). The row
/// is kept: the web UI's trash is the same store.
pub async fn set_deleted(db: &PgPool, user_id: Uuid, message_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE mail.messages SET is_deleted = TRUE, folder = 'trash' WHERE id = $1 AND user_id = $2",
    )
    .bind(message_id)
    .bind(user_id)
    .execute(db)
    .await
    .context("Suppression du message")?;
    Ok(())
}

/// Moves a message OUT of the inbox into the archive (IMAP "Archive" purge, and
/// the POP `archive` post-action). Kept out of Trash: archiving is not
/// deleting, so `is_deleted` stays false and the message remains in "All mail".
pub async fn archive(db: &PgPool, user_id: Uuid, message_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE mail.messages SET folder = 'archive', is_deleted = FALSE \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(message_id)
    .bind(user_id)
    .execute(db)
    .await
    .context("Archivage du message")?;
    Ok(())
}

pub async fn set_starred(db: &PgPool, user_id: Uuid, message_id: Uuid, starred: bool) -> Result<()> {
    sqlx::query("UPDATE mail.messages SET is_starred = $1 WHERE id = $2 AND user_id = $3")
        .bind(starred)
        .bind(message_id)
        .bind(user_id)
        .execute(db)
        .await
        .context("Mise à jour du suivi")?;
    Ok(())
}

/// Moves a message to another folder (IMAP MOVE, RFC 6851). The message keeps its
/// `local_uid` — which is globally unique — so the UID reported in the
/// destination is unchanged. Returns that UID, for the MOVE/UIDPLUS response.
pub async fn move_to(db: &PgPool, user_id: Uuid, message_id: Uuid, folder: &str) -> Result<Option<i64>> {
    let uid: Option<i64> = sqlx::query_scalar(
        "UPDATE mail.messages SET folder = $3, is_deleted = FALSE \
         WHERE id = $1 AND user_id = $2 RETURNING local_uid",
    )
    .bind(message_id)
    .bind(user_id)
    .bind(folder)
    .fetch_optional(db)
    .await
    .context("Déplacement du message")?;
    Ok(uid)
}

/// Copies a message into another folder (IMAP COPY). A new row is inserted with a
/// fresh `local_uid` (the destination's new UID, returned for COPYUID). The copy
/// shares the same thread and attachment metadata; attachment files are not
/// duplicated on disk (the storage_path still points at the originals).
pub async fn copy_to(db: &PgPool, user_id: Uuid, message_id: Uuid, folder: &str) -> Result<Option<i64>> {
    let uid: Option<i64> = sqlx::query_scalar(
        r#"INSERT INTO mail.messages
             (thread_id, account_id, user_id, message_id, in_reply_to, imap_uid, imap_folder,
              from_name, from_email, to_addresses, cc_addresses, bcc_addresses, reply_to,
              subject, body_text, body_html, attachments, is_read, is_starred, is_deleted,
              folder, label_ids, sent_at, received_at, category)
           SELECT thread_id, account_id, user_id, message_id, in_reply_to, imap_uid, imap_folder,
              from_name, from_email, to_addresses, cc_addresses, bcc_addresses, reply_to,
              subject, body_text, body_html, attachments, is_read, is_starred, FALSE,
              $3, label_ids, sent_at, received_at, category
           FROM mail.messages WHERE id = $1 AND user_id = $2
           RETURNING local_uid"#,
    )
    .bind(message_id)
    .bind(user_id)
    .bind(folder)
    .fetch_optional(db)
    .await
    .context("Copie du message")?;
    Ok(uid)
}

// ── CONDSTORE / QRESYNC (RFC 7162) ───────────────────────────────────────────

/// The folder's HIGHESTMODSEQ: the greatest modification sequence a client could
/// have seen for it. That is the max over the messages currently in the folder
/// AND over its tombstones — a message that VANISHED still advanced the sequence
/// as it left, and a reconnecting client must be told about it. Never zero for a
/// folder that has ever changed; a folder that never changed reports the RFC's
/// floor of 1.
pub async fn highest_modseq(db: &PgPool, user_id: Uuid, folder: &str) -> Result<i64> {
    let value: i64 = sqlx::query_scalar(
        r#"SELECT GREATEST(
                    COALESCE((SELECT MAX(modseq) FROM mail.messages
                              WHERE user_id = $1 AND folder = $2 AND local_uid IS NOT NULL), 0),
                    COALESCE((SELECT MAX(modseq) FROM mail.message_tombstones
                              WHERE user_id = $1 AND folder = $2), 0)
                  )"#,
    )
    .bind(user_id)
    .bind(folder)
    .fetch_one(db)
    .await
    .context("Lecture du HIGHESTMODSEQ")?;
    // RFC 7162: a mailbox that supports CONDSTORE always has a HIGHESTMODSEQ of
    // at least 1, even before anything has changed.
    Ok(value.max(1))
}

/// UIDs that left a folder since `modseq`, for QRESYNC's `VANISHED (EARLIER)`.
/// Read straight from the tombstones the database records on every move/delete,
/// oldest UID first.
pub async fn vanished_since(
    db: &PgPool,
    user_id: Uuid,
    folder: &str,
    modseq: i64,
) -> Result<Vec<i64>> {
    let uids: Vec<i64> = sqlx::query_scalar(
        r#"SELECT local_uid FROM mail.message_tombstones
           WHERE user_id = $1 AND folder = $2 AND modseq > $3
           ORDER BY local_uid"#,
    )
    .bind(user_id)
    .bind(folder)
    .bind(modseq)
    .fetch_all(db)
    .await
    .context("Lecture des tombstones VANISHED")?;
    Ok(uids)
}

// ── APPEND (RFC 3501 + APPENDUID, RFC 4315) ──────────────────────────────────

/// Inserts a client-uploaded message into `folder` and returns its new
/// `local_uid` (the APPENDUID a client caches to find the message again).
///
/// The raw RFC 5322 bytes are parsed apart the way a synced message is — the
/// store never keeps original MIME — but without the sync path's threading and
/// categorisation: an appended message (a draft, a Sent copy) gets its own fresh
/// thread. The whole insert is one transaction so a message never exists without
/// the thread it belongs to. `\Seen` / `\Flagged` from the command map onto
/// `is_read` / `is_starred`.
pub async fn append(
    db: &PgPool,
    user_id: Uuid,
    folder: &str,
    raw: &[u8],
    seen: bool,
    flagged: bool,
) -> Result<i64> {
    use mail_parser::MessageParser;

    // Every message row hangs off an account (NOT NULL). A mailbox that a client
    // can log into may still have no account configured, in which case there is
    // nowhere to file the upload — surfaced to the client as a plain failure.
    let account_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM mail.accounts WHERE user_id = $1 ORDER BY is_default DESC, created_at ASC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .context("Recherche d'un compte pour APPEND")?
    .ok_or_else(|| anyhow::anyhow!("Aucun compte pour héberger le message APPEND"))?;

    let parsed = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| anyhow::anyhow!("Parse RFC 5322 du message APPEND échoué"))?;

    let message_id = parsed.message_id().map(str::to_string);
    let subject = parsed.subject().unwrap_or("(sans sujet)").to_string();
    let (from_name, from_email) = parsed
        .from()
        .and_then(|addrs| addrs.first())
        .map(|addr| (addr.name().map(str::to_string), addr.address().unwrap_or("").to_string()))
        .unwrap_or((None, "unknown@unknown".to_string()));
    let to_addresses = address_json(parsed.to());
    let cc_addresses = address_json(parsed.cc());

    let body_text = parsed.body_text(0).map(|s| s.into_owned());
    let body_html = parsed
        .body_html(0)
        .map(|h| ammonia::clean(&h)); // client-uploaded HTML is sanitised before it is ever served.

    let sent_at = parsed
        .date()
        .and_then(|d| DateTime::from_timestamp(d.to_timestamp(), 0));

    let mut tx = db.begin().await.context("Transaction APPEND")?;

    // A minimal thread of its own: APPEND is a deposit (draft / Sent copy), not a
    // reply that must join an existing conversation.
    let thread_id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO mail.threads
             (account_id, user_id, subject, message_count, unread_count, last_message_at)
           VALUES ($1, $2, $3, 1, $4, NOW())
           RETURNING id"#,
    )
    .bind(account_id)
    .bind(user_id)
    .bind(&subject)
    .bind(i32::from(!seen))
    .fetch_one(&mut *tx)
    .await
    .context("Création du fil pour APPEND")?;

    // local_uid and modseq are filled by their sequence / trigger defaults.
    let local_uid: i64 = sqlx::query_scalar(
        r#"INSERT INTO mail.messages
             (thread_id, account_id, user_id, message_id, imap_uid, imap_folder,
              from_name, from_email, to_addresses, cc_addresses,
              subject, body_text, body_html, is_read, is_starred, folder, sent_at, received_at)
           VALUES ($1, $2, $3, $4, NULL, $5,
                   $6, $7, $8, $9,
                   $10, $11, $12, $13, $14, $5, $15, COALESCE($15, NOW()))
           RETURNING local_uid"#,
    )
    .bind(thread_id)
    .bind(account_id)
    .bind(user_id)
    .bind(&message_id)
    .bind(folder)
    .bind(&from_name)
    .bind(&from_email)
    .bind(&to_addresses)
    .bind(&cc_addresses)
    .bind(&subject)
    .bind(&body_text)
    .bind(&body_html)
    .bind(seen)
    .bind(flagged)
    .bind(sent_at)
    .fetch_one(&mut *tx)
    .await
    .context("Insertion du message APPEND")?;

    tx.commit().await.context("Validation de l'APPEND")?;
    Ok(local_uid)
}

/// Renders a parsed address list into the `[{name, email}]` shape the store
/// keeps recipients in. A local twin of the sync path's helper, so `append` need
/// not reach into that module.
fn address_json(addrs: Option<&mail_parser::Address>) -> Value {
    match addrs {
        None => Value::Array(Vec::new()),
        Some(addr) => Value::Array(
            addr.clone()
                .into_list()
                .into_iter()
                .map(|a| {
                    serde_json::json!({
                        "name":  a.name().map(str::to_string),
                        "email": a.address().unwrap_or("").to_string(),
                    })
                })
                .collect(),
        ),
    }
}

/// The UIDVALIDITY announced for a folder. It must be stable for the life of the
/// mailbox — a client that sees it change throws away its whole cache. We derive
/// it deterministically from the folder name, so it never changes across
/// restarts. (Folders here are fixed system folders; there is no recreation that
/// would warrant bumping it.)
pub fn uidvalidity(folder: &str) -> u32 {
    // FNV-1a over the local folder name. Deterministic and stable.
    let mut hash: u32 = 0x811c_9dc5;
    for b in folder.as_bytes() {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    // Keep it non-zero (0 is not a valid UIDVALIDITY).
    hash | 1
}

// ── RFC 5322 rendering ───────────────────────────────────────────────────────

/// Rebuilds a full message: headers, then the body, then the attachments read
/// back from disk. `attachments_dir` is the module's configured directory; a
/// file that has gone missing is skipped rather than failing the whole fetch —
/// a client asking for its mail should get the mail, not an error.
pub fn render(message: &StoredMessage) -> String {
    let mut out = String::with_capacity(2048);

    let date = message.sent_at.unwrap_or(message.received_at);
    push_header(&mut out, "Date", &date.format("%a, %d %b %Y %H:%M:%S %z").to_string());
    push_header(&mut out, "From", &format_address(message.from_name.as_deref(), &message.from_email));
    let to = format_address_list(&message.to_addresses);
    if !to.is_empty() {
        push_header(&mut out, "To", &to);
    }
    let cc = format_address_list(&message.cc_addresses);
    if !cc.is_empty() {
        push_header(&mut out, "Cc", &cc);
    }
    push_header(&mut out, "Subject", &encode_header_value(&message.subject));
    if let Some(mid) = message.message_id.as_deref().filter(|m| !m.is_empty()) {
        let bracketed = if mid.starts_with('<') { mid.to_string() } else { format!("<{mid}>") };
        push_header(&mut out, "Message-ID", &bracketed);
    }
    out.push_str("MIME-Version: 1.0\r\n");

    let attachments = attachment_list(&message.attachments);
    let boundary = format!("=_kubuno_{}", message.local_uid);

    if attachments.is_empty() {
        push_body(&mut out, message, &boundary);
    } else {
        out.push_str(&format!("Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n"));
        out.push_str(&format!("--{boundary}\r\n"));
        let inner = format!("{boundary}_alt");
        push_body(&mut out, message, &inner);
        for att in &attachments {
            out.push_str(&format!("\r\n--{boundary}\r\n"));
            out.push_str(&format!("Content-Type: {}; name=\"{}\"\r\n", att.mime, att.name));
            out.push_str("Content-Transfer-Encoding: base64\r\n");
            out.push_str(&format!("Content-Disposition: attachment; filename=\"{}\"\r\n\r\n", att.name));
            match std::fs::read(&att.path) {
                Ok(bytes) => out.push_str(&base64_wrapped(&bytes)),
                Err(e) => {
                    tracing::warn!(path = %att.path, error = %e, "Pièce jointe illisible — partie vide servie");
                }
            }
        }
        out.push_str(&format!("\r\n--{boundary}--\r\n"));
    }
    out
}

/// Body part: both alternatives when the message has them, otherwise the one it
/// has. An empty message still gets a body, so clients do not choke on it.
fn push_body(out: &mut String, message: &StoredMessage, boundary: &str) {
    let text = message.body_text.as_deref().unwrap_or("");
    let html = message.body_html.as_deref().unwrap_or("");

    match (text.is_empty(), html.is_empty()) {
        (false, false) => {
            out.push_str(&format!("Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n"));
            out.push_str(&format!("--{boundary}\r\n"));
            out.push_str("Content-Type: text/plain; charset=utf-8\r\n");
            out.push_str("Content-Transfer-Encoding: 8bit\r\n\r\n");
            out.push_str(&dot_safe(text));
            out.push_str(&format!("\r\n--{boundary}\r\n"));
            out.push_str("Content-Type: text/html; charset=utf-8\r\n");
            out.push_str("Content-Transfer-Encoding: 8bit\r\n\r\n");
            out.push_str(&dot_safe(html));
            out.push_str(&format!("\r\n--{boundary}--\r\n"));
        }
        (true, false) => {
            out.push_str("Content-Type: text/html; charset=utf-8\r\n");
            out.push_str("Content-Transfer-Encoding: 8bit\r\n\r\n");
            out.push_str(&dot_safe(html));
        }
        _ => {
            out.push_str("Content-Type: text/plain; charset=utf-8\r\n");
            out.push_str("Content-Transfer-Encoding: 8bit\r\n\r\n");
            out.push_str(&dot_safe(text));
        }
    }
}

struct RenderedAttachment {
    name: String,
    mime: String,
    path: String,
}

fn attachment_list(value: &Value) -> Vec<RenderedAttachment> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|a| {
                    Some(RenderedAttachment {
                        name: a.get("name")?.as_str()?.replace('"', "'"),
                        mime: a
                            .get("mime")
                            .and_then(Value::as_str)
                            .unwrap_or("application/octet-stream")
                            .to_string(),
                        path: a.get("storage_path")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn push_header(out: &mut String, name: &str, value: &str) {
    // A header value may never carry a bare newline: that is how a crafted
    // subject turns into extra headers.
    let clean: String = value.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    out.push_str(name);
    out.push_str(": ");
    out.push_str(&clean);
    out.push_str("\r\n");
}

fn format_address(name: Option<&str>, email: &str) -> String {
    match name.filter(|n| !n.trim().is_empty()) {
        Some(n) => format!("{} <{}>", encode_header_value(n), email),
        None => email.to_string(),
    }
}

fn format_address_list(value: &Value) -> String {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|a| {
                    let email = a.get("email")?.as_str()?;
                    if email.is_empty() { return None }
                    Some(format_address(a.get("name").and_then(Value::as_str), email))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// RFC 2047 encoding, applied only when the value is not plain ASCII — an
/// unencoded accent in a Subject is what makes it arrive as mojibake.
fn encode_header_value(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    use base64::Engine;
    format!(
        "=?UTF-8?B?{}?=",
        base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
    )
}

/// Normalises line endings and protects a line that is a lone dot, which would
/// otherwise end the message early in SMTP and POP3.
fn dot_safe(body: &str) -> String {
    body.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|line| if line == "." { ".." } else { line })
        .collect::<Vec<_>>()
        .join("\r\n")
}

fn base64_wrapped(bytes: &[u8]) -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    encoded
        .as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join("\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message() -> StoredMessage {
        StoredMessage {
            id: Uuid::nil(),
            local_uid: 7,
            message_id: Some("abc@example.com".into()),
            from_name: Some("Renée Dupont".into()),
            from_email: "renee@example.com".into(),
            to_addresses: json!([{ "name": "Bob", "email": "bob@example.org" }]),
            cc_addresses: json!([]),
            subject: "Réunion".into(),
            body_text: Some("Bonjour\n.\nfin".into()),
            body_html: None,
            attachments: json!([]),
            is_read: false,
            is_starred: false,
            folder: "inbox".into(),
            sent_at: None,
            received_at: Utc::now(),
            modseq: 1,
        }
    }

    #[test]
    fn renders_headers_and_body() {
        let out = render(&message());
        assert!(out.contains("From: =?UTF-8?B?"), "nom non-ASCII encodé");
        assert!(out.contains("To: Bob <bob@example.org>"));
        assert!(out.contains("Subject: =?UTF-8?B?"));
        assert!(out.contains("Message-ID: <abc@example.com>"));
        assert!(out.contains("Content-Type: text/plain; charset=utf-8"));
    }

    #[test]
    fn a_lone_dot_line_is_escaped() {
        let out = render(&message());
        assert!(out.contains("\r\n..\r\n"), "un point seul terminerait le message");
    }

    #[test]
    fn header_injection_is_stripped() {
        let mut m = message();
        m.subject = "Salut\r\nBcc: victime@example.net".into();
        let out = render(&m);
        // What matters is that the crafted text cannot OPEN a header line: it
        // stays inside the Subject value, where it is inert.
        assert!(
            !out.lines().any(|l| l.starts_with("Bcc:")),
            "pas d'en-tête injecté"
        );
        assert!(out.contains("Subject: SalutBcc: victime@example.net"));
    }

    #[test]
    fn maps_mailbox_names() {
        assert_eq!(folder_of("INBOX"), Some("inbox"));
        assert_eq!(folder_of("\"Sent\""), Some("sent"));
        assert_eq!(folder_of("Junk"), Some("spam"));
        assert_eq!(folder_of("Projets"), None);
    }
}
