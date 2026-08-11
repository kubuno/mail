//! POP / IMAP access policy — the server-side half of the "Forwarding and
//! POP/IMAP" settings tab.
//!
//! This instance IS the IMAP/POP3 server (`crate::server`), so a user's choices
//! there are not hints to some upstream provider: they are the rules this
//! server enforces on its own sockets. This module persists those choices and
//! exposes the pure decisions the protocol handlers make on every login and
//! every fetch — kept free of I/O so they can be unit-tested without a database.
//!
//! ── The defaults are compatibility, not preference ──────────────────────────
//! The live instance already answered IMAP and POP logins for any credential,
//! with no per-user gate. A user who never opened this tab has NO row, and
//! [`PopImapSettings::default`] must therefore reproduce that exact behaviour or
//! a running mail client breaks at its next poll. Every default below is chosen
//! to match what the server did before this table existed — see the migration
//! `000037` header for the field-by-field rationale.

use anyhow::{Context, Result};
use sqlx::PgPool;
use uuid::Uuid;

// ── Enumerations (parsed / validated at the edge, stored as text) ────────────

/// Which messages a POP maildrop exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopMode {
    /// Every inbox message.
    All,
    /// Only messages received after the cursor was set.
    FromNow,
}

impl PopMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PopMode::All => "all",
            PopMode::FromNow => "from_now",
        }
    }

    /// Parses the wire value; unknown strings are rejected rather than coerced,
    /// so a typo in a client request never silently changes the policy.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "all" => Some(PopMode::All),
            "from_now" => Some(PopMode::FromNow),
            _ => None,
        }
    }
}

/// What happens to a message once a POP session has fetched it and quit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopPostAction {
    /// Leave it exactly as it was.
    Keep,
    /// Mark it read (the historical POP behaviour).
    MarkRead,
    /// Move it out of the inbox into the archive.
    Archive,
    /// Move it to Trash.
    Delete,
}

impl PopPostAction {
    pub fn as_str(self) -> &'static str {
        match self {
            PopPostAction::Keep => "keep",
            PopPostAction::MarkRead => "mark_read",
            PopPostAction::Archive => "archive",
            PopPostAction::Delete => "delete",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "keep" => Some(PopPostAction::Keep),
            "mark_read" => Some(PopPostAction::MarkRead),
            "archive" => Some(PopPostAction::Archive),
            "delete" => Some(PopPostAction::Delete),
            _ => None,
        }
    }
}

/// How an IMAP `\Deleted` flag is committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImapExpungeMode {
    /// Commit immediately when the flag is set (Gmail's auto-expunge).
    Auto,
    /// Standard IMAP: wait for the client's EXPUNGE.
    Wait,
}

impl ImapExpungeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ImapExpungeMode::Auto => "auto",
            ImapExpungeMode::Wait => "wait",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(ImapExpungeMode::Auto),
            "wait" => Some(ImapExpungeMode::Wait),
            _ => None,
        }
    }
}

/// Where an expunged message goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImapPurgeMode {
    /// Out of the folder, kept in the archive.
    Archive,
    /// To Trash (the historical EXPUNGE behaviour).
    Trash,
    /// Removed for good.
    Delete,
}

impl ImapPurgeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ImapPurgeMode::Archive => "archive",
            ImapPurgeMode::Trash => "trash",
            ImapPurgeMode::Delete => "delete",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "archive" => Some(ImapPurgeMode::Archive),
            "trash" => Some(ImapPurgeMode::Trash),
            "delete" => Some(ImapPurgeMode::Delete),
            _ => None,
        }
    }
}

// ── The persisted policy ─────────────────────────────────────────────────────

/// One user's POP/IMAP policy, as the server needs it. Cheap to copy so the
/// protocol handlers can snapshot it at login and consult it per command.
#[derive(Debug, Clone, Copy)]
pub struct PopImapSettings {
    pub imap_enabled:      bool,
    pub imap_expunge_mode: ImapExpungeMode,
    pub imap_purge_mode:   ImapPurgeMode,
    /// 0 = unlimited.
    pub imap_folder_limit: i64,
    pub pop_enabled:       bool,
    pub pop_mode:          PopMode,
    pub pop_from_uid:      i64,
    pub pop_post_action:   PopPostAction,
}

impl Default for PopImapSettings {
    /// The no-row default: EXACTLY what the server did before this table existed.
    fn default() -> Self {
        PopImapSettings {
            imap_enabled:      true,
            imap_expunge_mode: ImapExpungeMode::Wait,
            imap_purge_mode:   ImapPurgeMode::Trash,
            imap_folder_limit: 0,
            pop_enabled:       true,
            pop_mode:          PopMode::All,
            pop_from_uid:      0,
            pop_post_action:   PopPostAction::MarkRead,
        }
    }
}

impl PopImapSettings {
    /// Whether a message with this `local_uid` belongs in a POP maildrop under
    /// the current mode. In `from_now`, only messages strictly after the cursor
    /// are visible; `all` shows everything.
    pub fn pop_shows(&self, local_uid: i64) -> bool {
        match self.pop_mode {
            PopMode::All => true,
            PopMode::FromNow => local_uid > self.pop_from_uid,
        }
    }

    /// Caps a folder listing to the most recent `imap_folder_limit` messages
    /// (highest UIDs). The input is assumed oldest-first (as `store::list`
    /// returns it); the tail is what a limited folder exposes. 0 = no cap.
    pub fn cap_folder<T>(&self, mut messages: Vec<T>) -> Vec<T> {
        if self.imap_folder_limit > 0 {
            let limit = self.imap_folder_limit as usize;
            if messages.len() > limit {
                messages.drain(0..messages.len() - limit);
            }
        }
        messages
    }
}

// ── Persistence ──────────────────────────────────────────────────────────────

/// The row as it comes back from Postgres, before validation into the typed
/// [`PopImapSettings`]: (imap_enabled, imap_expunge_mode, imap_purge_mode,
/// imap_folder_limit, pop_enabled, pop_mode, pop_from_uid, pop_post_action).
type SettingsRow = (bool, String, String, i32, bool, String, i64, String);

/// Reads a user's policy, or the compatibility default when they never saved
/// one. A malformed stored enum (only reachable by a hand-edited row, since the
/// writer validates) falls back to that field's default rather than failing the
/// login — a mail client must not be locked out by a bad settings row.
pub async fn load(db: &PgPool, user_id: Uuid) -> Result<PopImapSettings> {
    let row: Option<SettingsRow> = sqlx::query_as(
        "SELECT imap_enabled, imap_expunge_mode, imap_purge_mode, imap_folder_limit,
                pop_enabled, pop_mode, pop_from_uid, pop_post_action
         FROM mail.pop_imap_settings WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .context("Lecture des préférences POP/IMAP")?;

    let default = PopImapSettings::default();
    Ok(match row {
        None => default,
        Some((
            imap_enabled,
            expunge_mode,
            purge_mode,
            folder_limit,
            pop_enabled,
            pop_mode,
            pop_from_uid,
            pop_post_action,
        )) => PopImapSettings {
            imap_enabled,
            imap_expunge_mode: ImapExpungeMode::parse(&expunge_mode)
                .unwrap_or(default.imap_expunge_mode),
            imap_purge_mode: ImapPurgeMode::parse(&purge_mode).unwrap_or(default.imap_purge_mode),
            imap_folder_limit: i64::from(folder_limit),
            pop_enabled,
            pop_mode: PopMode::parse(&pop_mode).unwrap_or(default.pop_mode),
            pop_from_uid,
            pop_post_action: PopPostAction::parse(&pop_post_action)
                .unwrap_or(default.pop_post_action),
        },
    })
}

/// Inserts or replaces a user's policy. The caller has already validated the
/// enums (they arrive typed); `imap_auto_expunge` is stored consistent with the
/// mode.
pub async fn upsert(db: &PgPool, user_id: Uuid, s: &PopImapSettings) -> Result<()> {
    let auto_expunge = s.imap_expunge_mode == ImapExpungeMode::Auto;
    sqlx::query(
        r#"INSERT INTO mail.pop_imap_settings
             (user_id, imap_enabled, imap_expunge_mode, imap_auto_expunge, imap_purge_mode,
              imap_folder_limit, pop_enabled, pop_mode, pop_from_uid, pop_post_action, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())
           ON CONFLICT (user_id) DO UPDATE SET
             imap_enabled      = EXCLUDED.imap_enabled,
             imap_expunge_mode = EXCLUDED.imap_expunge_mode,
             imap_auto_expunge = EXCLUDED.imap_auto_expunge,
             imap_purge_mode   = EXCLUDED.imap_purge_mode,
             imap_folder_limit = EXCLUDED.imap_folder_limit,
             pop_enabled       = EXCLUDED.pop_enabled,
             pop_mode          = EXCLUDED.pop_mode,
             pop_from_uid      = EXCLUDED.pop_from_uid,
             pop_post_action   = EXCLUDED.pop_post_action,
             updated_at        = NOW()"#,
    )
    .bind(user_id)
    .bind(s.imap_enabled)
    .bind(s.imap_expunge_mode.as_str())
    .bind(auto_expunge)
    .bind(s.imap_purge_mode.as_str())
    .bind(s.imap_folder_limit as i32)
    .bind(s.pop_enabled)
    .bind(s.pop_mode.as_str())
    .bind(s.pop_from_uid)
    .bind(s.pop_post_action.as_str())
    .execute(db)
    .await
    .context("Enregistrement des préférences POP/IMAP")?;
    Ok(())
}

/// The current top of a user's inbox: the greatest `local_uid` among their
/// messages. This is the cursor snapped when a user switches POP to "from now
/// on" — everything at or below it is considered already-arrived and hidden.
/// A user with no mail yet gets 0, so the very next message is visible.
pub async fn current_inbox_cursor(db: &PgPool, user_id: Uuid) -> Result<i64> {
    let cursor: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(local_uid) FROM mail.messages
         WHERE user_id = $1 AND local_uid IS NOT NULL",
    )
    .bind(user_id)
    .fetch_one(db)
    .await
    .context("Lecture du curseur POP 'à partir de maintenant'")?;
    Ok(cursor.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_round_trips_and_rejects_unknown() {
        for m in [PopMode::All, PopMode::FromNow] {
            assert_eq!(PopMode::parse(m.as_str()), Some(m));
        }
        for a in [
            PopPostAction::Keep,
            PopPostAction::MarkRead,
            PopPostAction::Archive,
            PopPostAction::Delete,
        ] {
            assert_eq!(PopPostAction::parse(a.as_str()), Some(a));
        }
        for e in [ImapExpungeMode::Auto, ImapExpungeMode::Wait] {
            assert_eq!(ImapExpungeMode::parse(e.as_str()), Some(e));
        }
        for p in [ImapPurgeMode::Archive, ImapPurgeMode::Trash, ImapPurgeMode::Delete] {
            assert_eq!(ImapPurgeMode::parse(p.as_str()), Some(p));
        }
        assert_eq!(PopMode::parse("nonsense"), None);
        assert_eq!(PopPostAction::parse(""), None);
        assert_eq!(ImapExpungeMode::parse("client"), None);
        assert_eq!(ImapPurgeMode::parse("keep"), None);
    }

    #[test]
    fn default_preserves_current_server_behaviour() {
        let d = PopImapSettings::default();
        // Access is open on both protocols — the compatibility imperative.
        assert!(d.imap_enabled);
        assert!(d.pop_enabled);
        // Every inbox message is visible, RETR marks read, standard expunge to
        // Trash, no folder cap.
        assert_eq!(d.pop_mode, PopMode::All);
        assert_eq!(d.pop_post_action, PopPostAction::MarkRead);
        assert_eq!(d.imap_expunge_mode, ImapExpungeMode::Wait);
        assert_eq!(d.imap_purge_mode, ImapPurgeMode::Trash);
        assert_eq!(d.imap_folder_limit, 0);
    }

    #[test]
    fn pop_all_shows_everything() {
        let s = PopImapSettings { pop_mode: PopMode::All, pop_from_uid: 100, ..Default::default() };
        assert!(s.pop_shows(1));
        assert!(s.pop_shows(100));
        assert!(s.pop_shows(101));
    }

    #[test]
    fn pop_from_now_hides_up_to_and_including_the_cursor() {
        let s =
            PopImapSettings { pop_mode: PopMode::FromNow, pop_from_uid: 100, ..Default::default() };
        assert!(!s.pop_shows(1), "un message d'avant le curseur est masqué");
        assert!(!s.pop_shows(100), "le curseur lui-même (déjà arrivé) est masqué");
        assert!(s.pop_shows(101), "un message postérieur au curseur est visible");
    }

    #[test]
    fn folder_cap_keeps_the_most_recent_and_no_cap_when_zero() {
        // Oldest-first input; cap keeps the tail (highest UIDs).
        let s = PopImapSettings { imap_folder_limit: 3, ..Default::default() };
        assert_eq!(s.cap_folder(vec![1, 2, 3, 4, 5]), vec![3, 4, 5]);
        // Fewer than the limit → untouched.
        assert_eq!(s.cap_folder(vec![1, 2]), vec![1, 2]);
        // 0 = unlimited.
        let unlimited = PopImapSettings { imap_folder_limit: 0, ..Default::default() };
        assert_eq!(unlimited.cap_folder(vec![1, 2, 3, 4, 5]), vec![1, 2, 3, 4, 5]);
    }
}
