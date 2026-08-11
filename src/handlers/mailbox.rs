//! Mailbox credentials — what a user gives their mail client to reach the
//! SMTP/IMAP/POP3 services this instance offers.
//!
//! The password is generated here, shown once, and stored only as an Argon2
//! hash. A client keeps such a secret for years and hands it to whatever host
//! it is pointed at, which is exactly why it must not be the Kubuno password:
//! revoking one costs the user their mail client and nothing else.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{errors::MailError, middleware::AuthUser, server::auth, state::AppState};

/// A credential as the UI lists it — never the secret itself.
#[derive(Serialize, sqlx::FromRow)]
pub struct CredentialRow {
    pub id:           Uuid,
    pub username:     String,
    pub label:        Option<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at:   DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct CreateCredentialDto {
    /// One of the user's own addresses; this is the login the client sends.
    pub username: String,
    pub label:    Option<String>,
}

#[derive(Serialize)]
pub struct CreatedCredential {
    pub id:       Uuid,
    pub username: String,
    /// Shown once. It is not stored and cannot be shown again.
    pub password: String,
}

pub async fn list_credentials(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<CredentialRow>>, MailError> {
    let rows = sqlx::query_as::<_, CredentialRow>(
        "SELECT id, username, label, last_used_at, created_at \
         FROM mail.mailbox_credentials WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "list_credentials");
        MailError::Database(e)
    })?;

    Ok(Json(rows))
}

pub async fn create_credential(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<CreateCredentialDto>,
) -> Result<Json<CreatedCredential>, MailError> {
    let username = dto.username.trim().to_ascii_lowercase();
    if !username.contains('@') || username.len() > 320 {
        return Err(MailError::Validation("Identifiant : adresse e-mail attendue".into()));
    }

    // The address must be one this user actually owns, otherwise anyone could
    // claim a colleague's address and receive their mail.
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mail.accounts \
         WHERE user_id = $1 AND LOWER(email_address) = $2)",
    )
    .bind(user.id)
    .bind(&username)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "create_credential: vérification de propriété");
        MailError::Database(e)
    })?;

    if !owned {
        return Err(MailError::Validation(
            "Cette adresse n'est pas l'une de vos adresses configurées".into(),
        ));
    }

    let password = generate_password();
    let id = auth::upsert_credential(&state.db, user.id, &username, &password, dto.label.as_deref())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "create_credential");
            MailError::Validation(e.to_string())
        })?;

    Ok(Json(CreatedCredential { id, username, password }))
}

pub async fn delete_credential(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let deleted = sqlx::query("DELETE FROM mail.mailbox_credentials WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "delete_credential");
            MailError::Database(e)
        })?;

    if deleted.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Identifiant {id}")));
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// 24 characters from an unambiguous alphabet — no O/0, no l/1/I. People do
/// retype these into a phone, and a lookalike character reads as a wrong
/// password with no way to tell.
fn generate_password() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..24)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_passwords_are_long_and_unambiguous() {
        let pw = generate_password();
        assert_eq!(pw.chars().count(), 24);
        assert!(!pw.contains(['0', 'O', 'l', '1', 'I']));
    }

    #[test]
    fn two_passwords_differ() {
        assert_ne!(generate_password(), generate_password());
    }
}
