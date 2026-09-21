//! Portable replacements for the PostgreSQL-only delta plumbing of the mail
//! store — the pieces that used to live in `plpgsql` triggers and native
//! `SEQUENCE`s and must now be driven from Rust so the same code runs on
//! PostgreSQL, MySQL/MariaDB and SQLite.
//!
//! Three protocol-level mechanisms are involved (none of them is the web
//! delta-sync of `notes`/`wiki`; mail has no `change_seq` feed):
//!
//! * **`modseq` (CONDSTORE, RFC 7162).** Every write to a message stamps a
//!   fresh, ever-increasing `modseq`; a folder's `HIGHESTMODSEQ` is the max over
//!   its live messages and its tombstones. The old `message_modseq_seq` +
//!   `BEFORE` trigger become [`next_modseq`], taken in the same transaction as
//!   the write and bound explicitly on the row.
//! * **`local_uid`.** A monotonic per-message identity (the IMAP UID space),
//!   previously `message_local_uid_seq`. Now [`next_local_uid`].
//! * **QRESYNC vanished tombstones.** When a message leaves a folder (moved out
//!   or deleted), `message_tombstones` records the departure so a reconnecting
//!   client learns which UIDs are gone since its last `modseq`. The old
//!   `AFTER UPDATE` trigger becomes [`record_departure`] / [`clear_tombstone`],
//!   called explicitly at the write sites.
//!
//! All literal table / domain names live here — `&'static str`, never request
//! data — so the write sites read uniformly and a rename happens in one place.

use kubuno_db::{params, DbPool, DbTx};
use uuid::Uuid;

/// One shared counter table per schema; `next_seq` keys it by domain. Replaces
/// the two native sequences (`message_modseq_seq`, `message_local_uid_seq`).
pub const CHANGE_COUNTER: &str = "mail.change_counter";

/// Logical counter domains (the row keys in `change_counter`).
pub const MODSEQ_DOMAIN: &str = "modseq";
pub const LOCAL_UID_DOMAIN: &str = "local_uid";

pub const MESSAGE_TOMBSTONES: &str = "mail.message_tombstones";

/// The internal channel the IMAP sessions LISTEN on for IDLE. Unlike the core
/// event bus, this is module-internal: a write to `mail.messages` pings every
/// IMAP session so it can push the change to its client.
///
/// ⚠️ Portable only on PostgreSQL: MySQL and SQLite have no `LISTEN`, so on
/// those engines IDLE degrades to the client's poll interval (the IMAP session
/// has no other session's write to observe in-process). Documented, not hidden.
pub const IMAP_CHANGE_CHANNEL: &str = "mail_changes";

/// The next monotonic `modseq`, taken inside `tx` so it is ordered against every
/// other change and commits atomically with the row it stamps.
pub async fn next_modseq(tx: &mut DbTx) -> Result<i64, sqlx::Error> {
    kubuno_db::journal::next_seq(tx, CHANGE_COUNTER, MODSEQ_DOMAIN).await
}

/// The next `modseq` on a pool, wrapping its own transaction. Prefer
/// [`next_modseq`] when the write already runs in a transaction.
pub async fn next_modseq_on_pool(pool: &DbPool) -> Result<i64, sqlx::Error> {
    kubuno_db::journal::next_seq_on_pool(pool, CHANGE_COUNTER, MODSEQ_DOMAIN).await
}

/// The next `local_uid` (IMAP UID space), taken inside `tx`.
pub async fn next_local_uid(tx: &mut DbTx) -> Result<i64, sqlx::Error> {
    kubuno_db::journal::next_seq(tx, CHANGE_COUNTER, LOCAL_UID_DOMAIN).await
}

/// The next `local_uid` on a pool, wrapping its own transaction.
pub async fn next_local_uid_on_pool(pool: &DbPool) -> Result<i64, sqlx::Error> {
    kubuno_db::journal::next_seq_on_pool(pool, CHANGE_COUNTER, LOCAL_UID_DOMAIN).await
}

/// Records a QRESYNC departure: a message left `folder` at `modseq`. Idempotent
/// on `(user_id, folder, local_uid)` — a repeated departure only refreshes the
/// `modseq`, matching the old `ON CONFLICT DO UPDATE` trigger.
pub async fn record_departure(
    tx: &mut DbTx,
    user_id: Uuid,
    folder: &str,
    local_uid: i64,
    modseq: i64,
) -> Result<(), sqlx::Error> {
    // A repeated departure refreshes the modseq (the old ON CONFLICT DO UPDATE).
    let upsert = format!(
        "INSERT INTO {MESSAGE_TOMBSTONES} (user_id, folder, local_uid, modseq) VALUES ($1, $2, $3, $4){}",
        tx.backend().upsert(
            MESSAGE_TOMBSTONES,
            &["user_id", "folder", "local_uid"],
            &[kubuno_db::dialect::Assign::Incoming("modseq")],
        ),
    );
    tx.execute(&upsert, params![user_id, folder.to_string(), local_uid, modseq])
        .await
        .map(|_| ())
}

/// Clears any tombstone for `(user_id, folder, local_uid)` — a message arriving
/// in a folder is no longer "vanished" from it.
pub async fn clear_tombstone(
    tx: &mut DbTx,
    user_id: Uuid,
    folder: &str,
    local_uid: i64,
) -> Result<(), sqlx::Error> {
    let sql = format!(
        "DELETE FROM {MESSAGE_TOMBSTONES} \
         WHERE user_id = $1 AND folder = $2 AND local_uid = $3"
    );
    tx.execute(&sql, params![user_id, folder.to_string(), local_uid])
        .await
        .map(|_| ())
}

/// Pings the IMAP IDLE channel for `(user_id, folder)`. Best-effort: the caller
/// logs and carries on. A no-op-as-far-as-clients-go on MySQL/SQLite (see
/// [`IMAP_CHANGE_CHANNEL`]).
pub async fn notify_imap_change(pool: &DbPool, user_id: Uuid, folder: &str) {
    let payload = serde_json::json!({ "user_id": user_id, "folder": folder }).to_string();
    if let Err(e) =
        kubuno_db::events::notify(pool, crate::SCHEMA, IMAP_CHANGE_CHANNEL, &payload).await
    {
        tracing::warn!(error = %e, "IMAP change notify failed");
    }
}
