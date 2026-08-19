//! Retention: the background purge of Spam and Trash.
//!
//! Junk and deleted mail are the two folders nobody empties. Left alone they
//! grow without bound — and, worse, they keep content a user believed gone.
//! Every hosted mail service therefore ages them out; this is that job.
//!
//! Three properties it must have, and how each is obtained:
//!
//!   * **Never a surprise.** Both delays default to "jamais" (`0`). Nothing is
//!     deleted until an administrator sets a number, so upgrading the module
//!     cannot cost anybody a message.
//!   * **Only what this instance owns.** Messages of an EXTERNAL account are a
//!     local copy of what still sits on the provider — deleting one would just
//!     make the sync worker download it again on the next pass. The purge is
//!     restricted to `mail.accounts.kind = 'local'`, the mailboxes this
//!     instance actually hosts.
//!   * **No orphans.** A deleted message's attachment files are removed from
//!     disk, its thread's counters are recomputed, and a thread left with no
//!     message at all is dropped. A purge that only deleted rows would leave
//!     the attachments directory growing for ever.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::config::{self, ServerConfig};
use crate::config::settings::Settings;

/// How often the purge looks for expired messages. A retention delay is
/// expressed in days; checking hourly is precise enough and costs one indexed
/// query per folder.
const POLL_INTERVAL: Duration = Duration::from_secs(3_600);

/// Messages removed per folder per cycle. Bounded so a first run on a large
/// instance spreads its work instead of locking a table for minutes; the next
/// cycle picks up where this one stopped.
const BATCH: i64 = 500;

/// Runs forever. Idles until an administrator sets a retention delay.
pub async fn run(db: PgPool, settings: Settings, http: reqwest::Client) {
    tracing::info!("Rétention : worker démarré");
    loop {
        // Same read as every other background task: the console is the single
        // source of truth, and an unreachable core simply means "do nothing this
        // cycle" — never "delete with a guessed configuration".
        if let Some(cfg) = config::fetch(&http, &settings).await {
            purge_cycle(&db, &cfg, &settings.mail.attachments_dir).await;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One pass over both folders.
async fn purge_cycle(db: &PgPool, cfg: &ServerConfig, attachments_dir: &str) {
    for (folder, days) in [
        ("spam", cfg.spam_retention_days),
        ("trash", cfg.trash_retention_days),
    ] {
        if days <= 0 {
            continue;
        }
        match purge_folder(db, folder, days, attachments_dir).await {
            Ok(0) => {}
            Ok(removed) => tracing::info!(folder, days, removed, "Rétention : messages purgés"),
            Err(e) => tracing::error!(error = %e, folder, "Rétention : purge impossible"),
        }
    }
}

/// Removes up to [`BATCH`] messages older than `days` from `folder` and returns
/// how many went. Returns `Ok(0)` when there is nothing to do.
async fn purge_folder(
    db: &PgPool,
    folder: &str,
    days: i64,
    attachments_dir: &str,
) -> anyhow::Result<usize> {
    // Guard the cast: the setting is already clamped when it is read, and this
    // makes the invariant local — a day count that cannot be expressed must
    // delete nothing rather than wrap into something small.
    let days: i32 = match i32::try_from(days) {
        Ok(d) if d > 0 => d,
        _ => return Ok(0),
    };

    let expired = sqlx::query_as::<_, (Uuid, Uuid, Value)>(
        r#"SELECT m.id, m.thread_id, m.attachments
             FROM mail.messages m
             JOIN mail.accounts a ON a.id = m.account_id
            WHERE m.folder = $1
              AND a.kind = 'local'
              AND m.received_at < NOW() - ($2::int * INTERVAL '1 day')
            ORDER BY m.received_at
            LIMIT $3"#,
    )
    .bind(folder)
    .bind(days)
    .bind(BATCH)
    .fetch_all(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, folder, "Rétention : lecture des messages expirés");
        e
    })?;

    if expired.is_empty() {
        return Ok(0);
    }

    let ids: Vec<Uuid> = expired.iter().map(|(id, _, _)| *id).collect();
    let mut threads: Vec<Uuid> = expired.iter().map(|(_, thread, _)| *thread).collect();
    threads.sort_unstable();
    threads.dedup();

    // The rows go first, in one transaction with the thread bookkeeping: a
    // half-purged thread whose counters say otherwise is a conversation the
    // interface renders wrong. The files come after the commit — an orphan file
    // is a disk-space problem, an orphan row is a correctness one.
    let mut tx = db.begin().await.map_err(|e| {
        tracing::error!(error = %e, "Rétention : ouverture de la transaction de purge");
        e
    })?;

    sqlx::query("DELETE FROM mail.messages WHERE id = ANY($1)")
        .bind(&ids)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, folder, "Rétention : suppression des messages");
            e
        })?;

    // Recount what is left of each touched thread. Doing it from the messages
    // themselves rather than by decrementing keeps the counters true even if a
    // previous run was interrupted.
    sqlx::query(
        r#"UPDATE mail.threads t
              SET message_count = c.total,
                  unread_count  = c.unread
             FROM (SELECT thread_id,
                          COUNT(*)                                AS total,
                          COUNT(*) FILTER (WHERE NOT is_read)     AS unread
                     FROM mail.messages
                    WHERE thread_id = ANY($1)
                    GROUP BY thread_id) AS c
            WHERE t.id = c.thread_id"#,
    )
    .bind(&threads)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Rétention : recomptage des conversations");
        e
    })?;

    // A thread with no message left is not a conversation any more.
    sqlx::query(
        r#"DELETE FROM mail.threads t
            WHERE t.id = ANY($1)
              AND NOT EXISTS (SELECT 1 FROM mail.messages m WHERE m.thread_id = t.id)"#,
    )
    .bind(&threads)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Rétention : suppression des conversations vides");
        e
    })?;

    tx.commit().await.map_err(|e| {
        tracing::error!(error = %e, "Rétention : validation de la purge");
        e
    })?;

    // Files last, best effort: a file we fail to unlink is logged and left, and
    // the message row is already gone either way.
    for (_, _, attachments) in &expired {
        for path in attachment_paths(attachments, attachments_dir) {
            if let Err(e) = tokio::fs::remove_file(&path).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(error = %e, path = %path, "Rétention : pièce jointe non supprimée");
                }
            }
        }
    }

    Ok(expired.len())
}

/// The attachment files of one message, kept only when they really live under
/// the module's attachments directory.
///
/// The paths come from our own delivery code, so this is defence in depth — but
/// it is the difference between a purge and an arbitrary file deletion, and a
/// background task running with the module's rights is exactly where that
/// distinction has to be enforced.
fn attachment_paths(attachments: &Value, attachments_dir: &str) -> Vec<String> {
    let root = Path::new(attachments_dir.trim_end_matches('/'));
    if attachments_dir.trim().is_empty() {
        return Vec::new();
    }
    attachments
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("storage_path").and_then(Value::as_str))
                .filter(|path| {
                    let candidate = Path::new(path);
                    candidate.starts_with(root) && !candidate.components().any(|c| c.as_os_str() == "..")
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_paths_inside_the_attachments_directory_are_removed() {
        let attachments = json!([
            { "name": "a.pdf", "storage_path": "/var/lib/kubuno/mail/attachments/1/0_a.pdf" },
            { "name": "b.pdf", "storage_path": "/etc/shadow" },
            { "name": "c.pdf", "storage_path": "/var/lib/kubuno/mail/attachments/../../../etc/passwd" },
            { "name": "sans chemin" },
        ]);
        let kept = attachment_paths(&attachments, "/var/lib/kubuno/mail/attachments/");
        assert_eq!(kept, vec!["/var/lib/kubuno/mail/attachments/1/0_a.pdf"]);
    }

    #[test]
    fn a_message_without_attachments_yields_nothing() {
        assert!(attachment_paths(&json!([]), "/var/lib/kubuno/mail/attachments").is_empty());
        assert!(attachment_paths(&json!(null), "/var/lib/kubuno/mail/attachments").is_empty());
        // An unset attachments directory disables the file removal entirely
        // rather than resolving paths against the process's working directory.
        assert!(attachment_paths(&json!([{ "storage_path": "/x/y" }]), "").is_empty());
    }
}
