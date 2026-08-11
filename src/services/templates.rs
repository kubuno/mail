//! Personal e-mail templates — reusable canned subject/body drafts.
//!
//! A user saves a named template once and inserts it into any compose window
//! later. This module owns the persisted rows and the pure validation logic; the
//! HTTP surface lives in `handlers::templates`.
//!
//! The input-shaping decisions (`clean_name`, `validate_name`) are pure
//! functions taking their inputs directly, so they are unit-tested without a
//! database.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Longest accepted template name — matches the `VARCHAR(255)` column.
pub const NAME_MAX_LEN: usize = 255;

/// Longest accepted subject — matches the `VARCHAR(998)` column (RFC 5322 line).
pub const SUBJECT_MAX_LEN: usize = 998;

// ── Pure logic (unit-tested, no database) ───────────────────────────────────

/// Trims a template name for storage and comparison.
pub fn clean_name(raw: &str) -> String {
    raw.trim().to_string()
}

/// Validates a cleaned name: non-empty and within the column length. Returns a
/// user-facing message on failure, `Ok(())` otherwise.
pub fn validate_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err("Le nom du modèle est requis.".into());
    }
    if name.chars().count() > NAME_MAX_LEN {
        return Err("Le nom du modèle est trop long.".into());
    }
    Ok(())
}

/// Trims a subject and clamps it to the column length. A subject is optional, so
/// this never rejects — it only shapes.
pub fn clean_subject(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.chars().count() > SUBJECT_MAX_LEN {
        trimmed.chars().take(SUBJECT_MAX_LEN).collect()
    } else {
        trimmed.to_string()
    }
}

// ── Persisted rows ──────────────────────────────────────────────────────────

/// The public view of a template.
#[derive(Debug, Clone)]
pub struct EmailTemplate {
    pub id:         Uuid,
    pub name:       String,
    pub subject:    String,
    pub body_html:  String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

type Row = (Uuid, String, String, String, DateTime<Utc>, DateTime<Utc>);

fn to_public(r: Row) -> EmailTemplate {
    EmailTemplate {
        id:         r.0,
        name:       r.1,
        subject:    r.2,
        body_html:  r.3,
        created_at: r.4,
        updated_at: r.5,
    }
}

// ── Database access (always scoped by user_id) ──────────────────────────────

/// Every template of `user_id`, alphabetical by name (case-insensitive).
pub async fn list(db: &PgPool, user_id: Uuid) -> Result<Vec<EmailTemplate>> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, subject, body_html, created_at, updated_at
         FROM mail.email_templates
         WHERE user_id = $1
         ORDER BY lower(name) ASC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .context("Lecture des modèles d'e-mail")?;
    Ok(rows.into_iter().map(to_public).collect())
}

/// Does this user already have a template with this name (case-insensitive)?
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
             SELECT 1 FROM mail.email_templates
             WHERE user_id = $1 AND lower(name) = lower($2)
               AND ($3::uuid IS NULL OR id <> $3)
         )",
    )
    .bind(user_id)
    .bind(name)
    .bind(exclude_id)
    .fetch_one(db)
    .await
    .context("Vérification d'un nom de modèle existant")?;
    Ok(taken)
}

/// Inserts a new template, returning the public row.
pub async fn insert(
    db: &PgPool,
    user_id: Uuid,
    name: &str,
    subject: &str,
    body_html: &str,
) -> Result<EmailTemplate> {
    let row: Row = sqlx::query_as(
        "INSERT INTO mail.email_templates (user_id, name, subject, body_html)
         VALUES ($1, $2, $3, $4)
         RETURNING id, name, subject, body_html, created_at, updated_at",
    )
    .bind(user_id)
    .bind(name)
    .bind(subject)
    .bind(body_html)
    .fetch_one(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Insertion d'un modèle d'e-mail échouée");
        e
    })
    .context("Insertion d'un modèle d'e-mail")?;
    Ok(to_public(row))
}

/// Updates a template's name/subject/body, scoped to its owner. Returns the
/// updated public row, or `None` when the id is not this user's.
pub async fn update(
    db: &PgPool,
    user_id: Uuid,
    id: Uuid,
    name: &str,
    subject: &str,
    body_html: &str,
) -> Result<Option<EmailTemplate>> {
    let row: Option<Row> = sqlx::query_as(
        "UPDATE mail.email_templates
         SET name = $3, subject = $4, body_html = $5
         WHERE id = $1 AND user_id = $2
         RETURNING id, name, subject, body_html, created_at, updated_at",
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(subject)
    .bind(body_html)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Mise à jour d'un modèle d'e-mail échouée");
        e
    })
    .context("Mise à jour d'un modèle d'e-mail")?;
    Ok(row.map(to_public))
}

/// Deletes a template; returns how many rows went (0 = not this user's).
pub async fn delete(db: &PgPool, user_id: Uuid, id: Uuid) -> Result<u64> {
    let res = sqlx::query("DELETE FROM mail.email_templates WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Suppression d'un modèle d'e-mail échouée");
            e
        })
        .context("Suppression d'un modèle d'e-mail")?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_name_trims() {
        assert_eq!(clean_name("  Devis  "), "Devis");
        assert_eq!(clean_name("\tRelance\n"), "Relance");
    }

    #[test]
    fn empty_name_is_rejected() {
        assert!(validate_name("").is_err());
        assert!(validate_name(&clean_name("   ")).is_err());
    }

    #[test]
    fn ordinary_name_is_accepted() {
        assert!(validate_name("Devis standard").is_ok());
    }

    #[test]
    fn overlong_name_is_rejected() {
        let long = "a".repeat(NAME_MAX_LEN + 1);
        assert!(validate_name(&long).is_err());
        // Exactly at the limit is fine.
        assert!(validate_name(&"a".repeat(NAME_MAX_LEN)).is_ok());
    }

    #[test]
    fn subject_is_trimmed_and_clamped() {
        assert_eq!(clean_subject("  Bonjour  "), "Bonjour");
        let long = "x".repeat(SUBJECT_MAX_LEN + 50);
        assert_eq!(clean_subject(&long).chars().count(), SUBJECT_MAX_LEN);
    }
}
