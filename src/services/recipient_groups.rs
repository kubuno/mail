//! Personal recipient groups — private, per-user distribution lists.
//!
//! A user names a set of recipients ("Family", "Team") and addresses the whole
//! group at once when composing. These are NOT shared server aliases: they are a
//! personal convenience that expands into individual recipients client-side.
//! This module owns the persisted rows and the pure member-shaping logic; the
//! HTTP surface lives in `handlers::recipient_groups`.
//!
//! `normalize_members` is the heart of the module and is pure: it validates,
//! normalizes (trim + lower-case the address, trim the name) and de-duplicates
//! the submitted members, so an invalid or duplicate address never reaches the
//! database. It reuses the shared e-mail validator (`send_as::is_valid_email`),
//! the same one delegation and send-as use.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::services::send_as::{is_valid_email, normalize_email};

/// Longest accepted group name — matches the `VARCHAR(255)` column.
pub const NAME_MAX_LEN: usize = 255;

/// A single member of a group. `name` is optional (empty display name is stored
/// as `None`). This type is both the wire shape (camelCase JSON) and the jsonb
/// row shape, so the stored array round-trips through the API unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name:  Option<String>,
}

// ── Pure logic (unit-tested, no database) ───────────────────────────────────

/// Trims a group name for storage and comparison.
pub fn clean_name(raw: &str) -> String {
    raw.trim().to_string()
}

/// Validates a cleaned name: non-empty and within the column length. Returns a
/// user-facing message on failure.
pub fn validate_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err("Le nom du groupe est requis.".into());
    }
    if name.chars().count() > NAME_MAX_LEN {
        return Err("Le nom du groupe est trop long.".into());
    }
    Ok(())
}

/// Validates, normalizes and de-duplicates the submitted members.
///
/// Each member's address is trimmed + lower-cased and checked with the shared
/// validator; the first invalid one aborts with a user-facing message naming it.
/// Duplicates (by normalized address) collapse to their first occurrence, so the
/// earliest display name wins. Order is otherwise preserved.
///
/// Pure by design: takes the raw members directly, touches no clock and no
/// database, so every branch is unit-tested.
pub fn normalize_members(raw: &[Member]) -> std::result::Result<Vec<Member>, String> {
    let mut seen: Vec<String> = Vec::with_capacity(raw.len());
    let mut out: Vec<Member> = Vec::with_capacity(raw.len());
    for m in raw {
        let email = normalize_email(&m.email);
        if !is_valid_email(&email) {
            return Err(format!("Adresse e-mail invalide : « {} ».", m.email.trim()));
        }
        // De-duplicate on the normalized address; keep the first display name.
        if seen.iter().any(|e| e == &email) {
            continue;
        }
        let name = m
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        seen.push(email.clone());
        out.push(Member { email, name });
    }
    Ok(out)
}

// ── Persisted rows ──────────────────────────────────────────────────────────

/// The public view of a recipient group.
#[derive(Debug, Clone)]
pub struct RecipientGroup {
    pub id:         Uuid,
    pub name:       String,
    pub members:    Vec<Member>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// `members` is read back as jsonb via sqlx's Json adapter.
type Row = (
    Uuid,
    String,
    sqlx::types::Json<Vec<Member>>,
    DateTime<Utc>,
    DateTime<Utc>,
);

fn to_public(r: Row) -> RecipientGroup {
    RecipientGroup {
        id:         r.0,
        name:       r.1,
        members:    r.2 .0,
        created_at: r.3,
        updated_at: r.4,
    }
}

// ── Database access (always scoped by user_id) ──────────────────────────────

/// Every group of `user_id`, alphabetical by name (case-insensitive).
pub async fn list(db: &PgPool, user_id: Uuid) -> Result<Vec<RecipientGroup>> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, members, created_at, updated_at
         FROM mail.recipient_groups
         WHERE user_id = $1
         ORDER BY lower(name) ASC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .context("Lecture des groupes de destinataires")?;
    Ok(rows.into_iter().map(to_public).collect())
}

/// Does this user already have a group with this name (case-insensitive)?
/// `exclude_id`, when set, is skipped — used on rename so a row does not clash
/// with itself.
pub async fn name_taken(
    db: &PgPool,
    user_id: Uuid,
    name: &str,
    exclude_id: Option<Uuid>,
) -> Result<bool> {
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM mail.recipient_groups
             WHERE user_id = $1 AND lower(name) = lower($2)
               AND ($3::uuid IS NULL OR id <> $3)
         )",
    )
    .bind(user_id)
    .bind(name)
    .bind(exclude_id)
    .fetch_one(db)
    .await
    .context("Vérification d'un nom de groupe existant")?;
    Ok(taken)
}

/// Inserts a new group with its (already normalized) members, returning the
/// public row.
pub async fn insert(
    db: &PgPool,
    user_id: Uuid,
    name: &str,
    members: &[Member],
) -> Result<RecipientGroup> {
    let row: Row = sqlx::query_as(
        "INSERT INTO mail.recipient_groups (user_id, name, members)
         VALUES ($1, $2, $3)
         RETURNING id, name, members, created_at, updated_at",
    )
    .bind(user_id)
    .bind(name)
    .bind(sqlx::types::Json(members))
    .fetch_one(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Insertion d'un groupe de destinataires échouée");
        e
    })
    .context("Insertion d'un groupe de destinataires")?;
    Ok(to_public(row))
}

/// Updates a group's name/members, scoped to its owner. Returns the updated
/// public row, or `None` when the id is not this user's.
pub async fn update(
    db: &PgPool,
    user_id: Uuid,
    id: Uuid,
    name: &str,
    members: &[Member],
) -> Result<Option<RecipientGroup>> {
    let row: Option<Row> = sqlx::query_as(
        "UPDATE mail.recipient_groups
         SET name = $3, members = $4
         WHERE id = $1 AND user_id = $2
         RETURNING id, name, members, created_at, updated_at",
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(sqlx::types::Json(members))
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Mise à jour d'un groupe de destinataires échouée");
        e
    })
    .context("Mise à jour d'un groupe de destinataires")?;
    Ok(row.map(to_public))
}

/// Deletes a group; returns how many rows went (0 = not this user's).
pub async fn delete(db: &PgPool, user_id: Uuid, id: Uuid) -> Result<u64> {
    let res = sqlx::query("DELETE FROM mail.recipient_groups WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Suppression d'un groupe de destinataires échouée");
            e
        })
        .context("Suppression d'un groupe de destinataires")?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(email: &str, name: Option<&str>) -> Member {
        Member { email: email.into(), name: name.map(str::to_string) }
    }

    // ── Name ─────────────────────────────────────────────────────────────────
    #[test]
    fn empty_name_is_rejected() {
        assert!(validate_name(&clean_name("   ")).is_err());
        assert!(validate_name("").is_err());
    }

    #[test]
    fn ordinary_name_is_accepted() {
        assert_eq!(clean_name("  Famille "), "Famille");
        assert!(validate_name("Famille").is_ok());
    }

    // ── Members ──────────────────────────────────────────────────────────────
    #[test]
    fn members_are_normalized() {
        let out = normalize_members(&[m("  Alice@Example.COM ", Some("  Alice  "))]).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].email, "alice@example.com");
        assert_eq!(out[0].name.as_deref(), Some("Alice"));
    }

    #[test]
    fn blank_display_name_becomes_none() {
        let out = normalize_members(&[m("bob@example.com", Some("   "))]).unwrap();
        assert_eq!(out[0].name, None);
        let out2 = normalize_members(&[m("bob@example.com", None)]).unwrap();
        assert_eq!(out2[0].name, None);
    }

    #[test]
    fn duplicates_collapse_keeping_first_name() {
        let out = normalize_members(&[
            m("dup@example.com", Some("First")),
            m("DUP@example.com", Some("Second")),
            m("other@example.com", None),
        ])
        .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].email, "dup@example.com");
        assert_eq!(out[0].name.as_deref(), Some("First"));
        assert_eq!(out[1].email, "other@example.com");
    }

    #[test]
    fn an_invalid_email_aborts_the_whole_group() {
        let err = normalize_members(&[
            m("ok@example.com", None),
            m("not-an-email", None),
        ])
        .unwrap_err();
        assert!(err.contains("not-an-email"));
    }

    #[test]
    fn empty_members_list_is_allowed() {
        assert_eq!(normalize_members(&[]).unwrap(), Vec::<Member>::new());
    }
}
