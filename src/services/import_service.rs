//! Mailbox migration: copies an existing IMAP mailbox from a third-party
//! provider into a mailbox this instance hosts.
//!
//! The module is a stateless worker. It owns no table and remembers nothing
//! between calls: the core drives the migration, hands us an opaque cursor and
//! a time budget, and stores back whatever cursor we return. One HTTP call =
//! one bounded chunk of work, so a migration of a hundred thousand messages is
//! just many short calls that can be paused, resumed or replayed.
//!
//! Ingestion goes through `sync_service::store_message`, exactly like a regular
//! IMAP sync — same parsing, threading, attachment handling and, crucially, the
//! same `(account_id, imap_folder, imap_uid)` dedup, which is what makes
//! replaying a chunk harmless.

use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    config::settings::MailSettings,
    models::EmailAccount,
    services::{
        imap_service::{self, ImapAuth, ImapConfig, ImapSession},
        sync_service,
    },
};

/// Bumped whenever the cursor layout changes: an older cursor is then simply
/// rebuilt from scratch instead of being misread.
const CURSOR_VERSION: u32 = 1;

/// Hard ceiling on one FETCH batch, whatever the configured sync batch size:
/// a whole batch is held in memory at once.
const MAX_BATCH: usize = 200;

/// The third-party mailbox to read from.
///
/// Deliberately NOT `Debug`: it holds a password, and a stray `{:?}` in a log
/// line is all it takes to leak it.
pub struct SourceSpec {
    pub host:     String,
    pub port:     u16,
    pub security: String,
    pub username: String,
    pub password: String,
}

/// One mailbox found on the source server, as reported to the admin screen.
#[derive(Serialize)]
pub struct FolderProbe {
    /// Raw name, as the server spells it (this is what must be sent back in
    /// `exclude_folders`, though the display name is accepted too).
    pub name:         String,
    /// Same name, modified-UTF-7 decoded — readable by a human.
    pub display_name: String,
    /// Local bucket it will feed: inbox, sent, drafts, spam, trash, archive, custom.
    pub kind:         String,
    /// Messages the server reports in that mailbox (EXISTS).
    pub messages:     u32,
}

/// Result of one bounded chunk of copying.
pub struct ChunkOutcome {
    pub done:   bool,
    pub cursor: serde_json::Value,
    pub copied: u32,
    pub total:  u32,
}

/// Per-folder progress inside the cursor.
#[derive(Serialize, Deserialize)]
struct FolderCursor {
    name: String,
    kind: String,
    uids: Vec<u32>,
    pos:  usize,
}

/// The opaque cursor the core stores between two calls. Its shape is ours
/// alone; the core must treat it as a blob and hand it back untouched.
#[derive(Serialize, Deserialize)]
struct Cursor {
    v:       u32,
    folders: Vec<FolderCursor>,
    /// Index of the folder currently being copied.
    fi:      usize,
    copied:  u32,
    total:   u32,
}

/// Lists the source mailboxes and how many messages each holds.
///
/// Used by the admin screen before starting a migration, to let the operator
/// pick what to leave behind.
pub async fn probe(src: &SourceSpec) -> Result<Vec<FolderProbe>> {
    let mut session = imap_service::connect(&imap_config(src))
        .await
        .map_err(|e| anyhow::anyhow!("Connexion à la boîte source impossible : {e}"))?;

    let result = probe_folders(&mut session).await;

    // Hand the session back on every path, so a probe never leaks a connection
    // on the source server.
    imap_service::logout(session).await;
    result
}

async fn probe_folders(session: &mut ImapSession) -> Result<Vec<FolderProbe>> {
    let mailboxes = imap_service::list_mailboxes(session)
        .await
        .map_err(|e| anyhow::anyhow!("Lecture des dossiers de la boîte source impossible : {e}"))?;

    let mut out = Vec::with_capacity(mailboxes.len());
    for mb in mailboxes {
        // Virtual "all messages" mailboxes duplicate the whole account.
        if mb.kind == "__skip__" {
            continue;
        }
        // A mailbox we cannot SELECT cannot be migrated either; report it with
        // a zero count rather than hiding it from the operator.
        let messages = match imap_service::select_folder(session, &mb.name).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(folder = %mb.name, error = %e, "Migration : dossier source non sélectionnable");
                0
            }
        };
        out.push(FolderProbe {
            display_name: imap_service::decode_imap_utf7(&mb.name),
            name:         mb.name,
            kind:         mb.kind.to_string(),
            messages,
        });
    }
    Ok(out)
}

/// Copies as much as fits in `budget_secs`, then returns where it stopped.
///
/// Persists nothing of its own: the returned cursor is the whole state. Calling
/// it again with that cursor resumes; calling it again with the PREVIOUS cursor
/// re-does the same work harmlessly (see the dedup note on `store_message`).
#[allow(clippy::too_many_arguments)] // source + destination + filters + budget + cursor
pub async fn run_chunk(
    db: &PgPool,
    mail_cfg: &MailSettings,
    src: &SourceSpec,
    target_user_id: Uuid,
    since: Option<chrono::NaiveDate>,
    exclude: &[String],
    budget_secs: u64,
    cursor: serde_json::Value,
) -> Result<ChunkOutcome> {
    let account = load_target_account(db, target_user_id).await?;

    let started = Instant::now();
    let budget  = Duration::from_secs(budget_secs.clamp(1, 120));

    let mut session = imap_service::connect(&imap_config(src))
        .await
        .map_err(|e| anyhow::anyhow!("Connexion à la boîte source impossible : {e}"))?;

    let outcome = copy_chunk(
        db, mail_cfg, &account, &mut session, since, exclude, started, budget, cursor,
    )
    .await;

    // Always log out — error path included — or a failed migration would pile
    // up sessions on the source server until it refuses new ones.
    imap_service::logout(session).await;
    outcome
}

#[allow(clippy::too_many_arguments)] // same reason as run_chunk, plus the session
async fn copy_chunk(
    db: &PgPool,
    mail_cfg: &MailSettings,
    account: &EmailAccount,
    session: &mut ImapSession,
    since: Option<chrono::NaiveDate>,
    exclude: &[String],
    started: Instant,
    budget: Duration,
    cursor_in: serde_json::Value,
) -> Result<ChunkOutcome> {
    // A cursor we cannot read (first call, or a layout from an older version)
    // means: start over. Re-copying is safe, so this can never corrupt anything.
    let mut cur = match serde_json::from_value::<Cursor>(cursor_in) {
        Ok(c) if c.v == CURSOR_VERSION => c,
        _ => build_cursor(session, since, exclude).await?,
    };

    let batch = (mail_cfg.max_fetch_per_sync.max(1) as usize).min(MAX_BATCH);
    // The mailbox currently SELECTed, to avoid a round trip per batch.
    let mut selected: Option<String> = None;

    loop {
        if started.elapsed() >= budget {
            break;
        }

        // Take everything we need out of the cursor before any await, so the
        // borrow is over by the time we write the new position back.
        let fi = cur.fi;
        let Some(folder) = cur.folders.get(fi) else {
            break;
        };
        if folder.pos >= folder.uids.len() {
            cur.fi = fi + 1;
            continue;
        }
        let end  = (folder.pos + batch).min(folder.uids.len());
        let name = folder.name.clone();
        let kind = folder.kind.clone();
        let uids = folder.uids[folder.pos..end].to_vec();

        if selected.as_deref() != Some(name.as_str()) {
            match imap_service::select_folder(session, &name).await {
                Ok(_) => selected = Some(name.clone()),
                Err(e) => {
                    tracing::warn!(folder = %name, error = %e, "Migration : SELECT échoué — dossier ignoré");
                    selected = None;
                    cur.fi = fi + 1;
                    continue;
                }
            }
        }

        let raws = match imap_service::fetch_uids(session, &uids).await {
            Ok(r) => r,
            Err(e) => {
                // Skip the batch rather than the folder: the next one may well
                // go through. `pos` still advances, otherwise the migration
                // would retry the same failing batch for ever.
                tracing::warn!(folder = %name, error = %e, "Migration : FETCH échoué — lot ignoré");
                if let Some(f) = cur.folders.get_mut(fi) {
                    f.pos = end;
                }
                continue;
            }
        };

        let local = local_folder(&kind);
        for raw in raws {
            // Second date gate, needed when the server refused the SINCE search
            // and we fell back to the full UID list (see `uids_to_copy`).
            if let Some(limit) = since {
                if is_older_than(&raw.body, limit) {
                    continue;
                }
            }
            match sync_service::store_message(
                db,
                account,
                &raw.body,
                raw.uid,
                &name,
                local,
                raw.seen,
                raw.flagged,
                &mail_cfg.attachments_dir,
                // Historical import: never re-fire an RSVP for old mail.
                None,
            )
            .await
            {
                Ok(()) => cur.copied = cur.copied.saturating_add(1),
                // One unparsable message must never stall a migration: log the
                // UID (never the body, which is the user's mail) and move on.
                Err(e) => tracing::warn!(
                    folder = %name, uid = raw.uid, error = %e,
                    "Migration : message ignoré (stockage échoué)"
                ),
            }
        }

        if let Some(f) = cur.folders.get_mut(fi) {
            f.pos = end;
        }
    }

    let done   = cur.fi >= cur.folders.len();
    let copied = cur.copied;
    let total  = cur.total;
    let cursor = serde_json::to_value(&cur).map_err(|e| {
        tracing::error!(error = %e, "Migration : sérialisation du curseur échouée");
        anyhow::anyhow!("Sérialisation de l'état de migration impossible.")
    })?;

    Ok(ChunkOutcome { done, cursor, copied, total })
}

/// First call of a migration: decide, once, what is to be copied.
///
/// The whole UID plan is computed here and then carried in the cursor, so the
/// following chunks cost one SELECT and one FETCH each — and so the total shown
/// to the operator stops moving once the migration has started.
async fn build_cursor(
    session: &mut ImapSession,
    since: Option<chrono::NaiveDate>,
    exclude: &[String],
) -> Result<Cursor> {
    let mut mailboxes = imap_service::list_mailboxes(session)
        .await
        .map_err(|e| anyhow::anyhow!("Lecture des dossiers de la boîte source impossible : {e}"))?;

    // Inbox first, then the other well-known folders: if a migration is stopped
    // half-way, what matters most is already in.
    mailboxes.sort_by_key(|m| match m.kind {
        "inbox"   => 0,
        "sent"    => 1,
        "drafts"  => 2,
        "custom"  => 3,
        "archive" => 4,
        "spam"    => 5,
        _         => 6,
    });

    let mut folders: Vec<FolderCursor> = Vec::new();
    let mut total: u32 = 0;

    for mb in mailboxes {
        if mb.kind == "__skip__" || is_excluded(&mb.name, exclude) {
            continue;
        }
        if let Err(e) = imap_service::select_folder(session, &mb.name).await {
            tracing::warn!(folder = %mb.name, error = %e, "Migration : dossier source ignoré (SELECT échoué)");
            continue;
        }
        let uids = match uids_to_copy(session, since).await {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!(folder = %mb.name, error = %e, "Migration : dossier source ignoré (liste des UID indisponible)");
                continue;
            }
        };
        total = total.saturating_add(uids.len() as u32);
        folders.push(FolderCursor {
            name: mb.name,
            kind: mb.kind.to_string(),
            uids,
            pos:  0,
        });
    }

    Ok(Cursor { v: CURSOR_VERSION, folders, fi: 0, copied: 0, total })
}

/// UIDs of the selected mailbox that fall inside the migration window.
async fn uids_to_copy(session: &mut ImapSession, since: Option<chrono::NaiveDate>) -> Result<Vec<u32>> {
    if let Some(day) = since {
        // IMAP SEARCH keys given in sequence are ANDed (RFC 3501 §6.4.4), so
        // `UID 1:* SINCE <dd-Mon-yyyy>` narrows the very command `uid_list`
        // already issues — no new IMAP verb and no new helper to get wrong.
        let range = format!("1:* SINCE {}", day.format("%d-%b-%Y"));
        match imap_service::uid_list(session, &range).await {
            Ok(uids) => return Ok(uids),
            Err(e) => {
                // Trade-off: a server that refuses the compound criterion makes
                // us list everything and drop what is too old only after the
                // fetch, from the `Date:` header. Costs bandwidth, but it never
                // loses a message — and correctness wins here.
                tracing::warn!(error = %e, "Migration : UID SEARCH SINCE refusé — repli sur un filtrage après récupération");
            }
        }
    }
    imap_service::uid_list(session, "1:*").await
}

/// True when the message's `Date:` header is strictly before `limit`.
///
/// Messages with no parsable date are kept: copying one message too many is
/// recoverable, silently dropping mail is not.
fn is_older_than(raw: &[u8], limit: chrono::NaiveDate) -> bool {
    let parser = mail_parser::MessageParser::default();
    let Some(parsed) = parser.parse(raw) else {
        return false;
    };
    let Some(date) = parsed.date() else {
        return false;
    };
    match chrono::DateTime::<chrono::Utc>::from_timestamp(date.to_timestamp(), 0) {
        Some(dt) => dt.date_naive() < limit,
        None     => false,
    }
}

/// The hosted mailbox the messages land in: the destination user's default
/// local account.
///
/// Columns are enumerated rather than `SELECT *` — the table also holds the
/// encrypted IMAP/SMTP passwords, which must never be read into memory here.
async fn load_target_account(db: &PgPool, user_id: Uuid) -> Result<EmailAccount> {
    let found = sqlx::query_as::<_, EmailAccount>(
        r#"SELECT id, user_id, name, email_address, kind, mailbox_id,
                  incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  smtp_host, smtp_port, smtp_security, smtp_username, auth_kind,
                  is_default, is_active, last_sync_at, last_error,
                  created_at, updated_at
           FROM mail.accounts
           WHERE user_id = $1 AND kind = 'local'
           ORDER BY is_default DESC, created_at ASC
           LIMIT 1"#,
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!(user_id = %user_id, error = %e, "Migration : lecture du compte de destination échouée");
        anyhow::anyhow!("Lecture du compte de destination impossible.")
    })?;

    found.ok_or_else(|| {
        anyhow::anyhow!("Aucune boîte aux lettres hébergée pour ce compte de destination.")
    })
}

/// Source connection settings.
///
/// `security` is passed through as-is: `connect` treats "ssl" as implicit TLS
/// and anything else as a plain TCP session.
fn imap_config(src: &SourceSpec) -> ImapConfig {
    ImapConfig {
        host:     src.host.clone(),
        port:     src.port,
        security: src.security.clone(),
        username: src.username.clone(),
        // Providers that hand out app passwords display them in spaced groups;
        // an operator pasting one gets the same normalisation as everywhere else.
        auth: ImapAuth::Password(crate::services::app_password_normalize(&src.host, &src.password)),
    }
}

/// Local bucket a source mailbox feeds. Anything unknown becomes a user folder
/// rather than being guessed into a system one.
fn local_folder(kind: &str) -> &'static str {
    match kind {
        "inbox"   => "inbox",
        "sent"    => "sent",
        "drafts"  => "drafts",
        "spam"    => "spam",
        "trash"   => "trash",
        "archive" => "archive",
        _         => "custom",
    }
}

/// A folder is excluded when the operator's entry equals either the raw IMAP
/// name or its decoded display name, case-insensitively and trimmed.
///
/// Whole names only, never prefixes: excluding "Spam" must not take a folder
/// named "Spam archive" with it.
fn is_excluded(name: &str, exclude: &[String]) -> bool {
    if exclude.is_empty() {
        return false;
    }
    let raw     = name.trim().to_lowercase();
    let display = imap_service::decode_imap_utf7(name).trim().to_lowercase();
    exclude.iter().any(|entry| {
        let entry = entry.trim().to_lowercase();
        !entry.is_empty() && (entry == raw || entry == display)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_on_raw_or_decoded_name() {
        let excl = vec!["éléments envoyés".to_string(), "SPAM".to_string()];
        // Prefixed by its parent: a different folder, kept.
        assert!(!is_excluded("INBOX.&AMk-l&AOk-ments envoy&AOk-s", &excl));
        assert!(is_excluded("&AMk-l&AOk-ments envoy&AOk-s", &excl));
        assert!(is_excluded("Spam", &excl));
        assert!(!is_excluded("Spam archive", &excl));
        assert!(!is_excluded("Projets", &excl));
    }

    #[test]
    fn unknown_kinds_land_in_custom() {
        assert_eq!(local_folder("inbox"), "inbox");
        assert_eq!(local_folder("archive"), "archive");
        assert_eq!(local_folder("whatever"), "custom");
    }

    #[test]
    fn formats_the_imap_search_date() {
        let day = chrono::NaiveDate::from_ymd_opt(2020, 1, 3).expect("date valide");
        assert_eq!(day.format("%d-%b-%Y").to_string(), "03-Jan-2020");
    }
}
