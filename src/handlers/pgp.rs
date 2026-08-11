//! OpenPGP key-management HTTP surface (Wave 1 of the GPG feature).
//!
//! The user's own identities live in `mail.pgp_keys` with the secret key
//! AES-256-GCM encrypted at rest (via `MailCrypto`, exactly like account
//! passwords); correspondents' public keys live in `mail.pgp_contacts`. The
//! actual OpenPGP operations are in `services::pgp`. Every route refuses unless
//! the instance-wide `gpg_enabled` switch is on.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    services::{crypto::MailCrypto, pgp},
    state::AppState,
};

/// Whether the instance admin enabled OpenPGP — lets the client show or hide the
/// whole "Chiffrement" tab without a 403 round-trip.
pub async fn status(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let cfg = crate::handlers::addresses::server_config(&state).await?;
    Ok(Json(serde_json::json!({ "enabled": cfg.gpg_enabled })))
}

/// The instance admin must have enabled OpenPGP for any of this to work.
async fn require_gpg(state: &AppState) -> Result<(), MailError> {
    let cfg = crate::handlers::addresses::server_config(state).await?;
    if !cfg.gpg_enabled {
        return Err(MailError::Forbidden);
    }
    Ok(())
}

fn crypto(state: &AppState) -> Result<MailCrypto, MailError> {
    MailCrypto::new(&state.settings.mail.encryption_key).map_err(|_| MailError::Crypto)
}

// ── Own keys ──────────────────────────────────────────────────────────────────

/// The public view of an identity — never the secret material.
pub async fn list_keys(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    require_gpg(&state).await?;
    let rows = sqlx::query_as::<_, (Uuid, Option<String>, String, String, bool, chrono::DateTime<chrono::Utc>)>(
        r#"SELECT id, email, fingerprint, public_key, is_default, created_at
           FROM mail.pgp_keys WHERE user_id = $1 ORDER BY created_at"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;

    let keys: Vec<_> = rows
        .into_iter()
        .map(|(id, email, fingerprint, public_key, is_default, created_at)| {
            serde_json::json!({
                "id": id, "email": email, "fingerprint": fingerprint,
                "public_key": public_key, "is_default": is_default, "created_at": created_at,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "keys": keys })))
}

#[derive(Deserialize)]
pub struct GenerateKeyDto {
    #[serde(default)]
    pub name: String,
    pub email: String,
}

pub async fn generate_key(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<GenerateKeyDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    require_gpg(&state).await?;
    let email = dto.email.trim();
    if email.is_empty() || !email.contains('@') {
        return Err(MailError::Validation("Adresse email invalide".into()));
    }
    // "Name <addr>" is the standard User ID form; a bare address is also valid.
    let uid = if dto.name.trim().is_empty() {
        email.to_string()
    } else {
        format!("{} <{}>", dto.name.trim(), email)
    };
    let material = pgp::generate(&uid).map_err(|_| MailError::Internal(anyhow::anyhow!("keygen")))?;
    store_key(&state, user.id, &material).await
}

#[derive(Deserialize)]
pub struct ImportKeyDto {
    pub secret_armored: String,
    #[serde(default)]
    pub passphrase: Option<String>,
}

pub async fn import_key(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<ImportKeyDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    require_gpg(&state).await?;
    let material = pgp::import_secret(&dto.secret_armored, dto.passphrase.as_deref())
        .map_err(|_| MailError::Validation("Clé secrète OpenPGP invalide ou passphrase incorrecte".into()))?;
    store_key(&state, user.id, &material).await
}

/// Persist a freshly generated / imported identity: the secret key is encrypted
/// at rest before it ever touches storage. The first identity becomes default.
async fn store_key(
    state: &AppState,
    user_id: Uuid,
    material: &pgp::KeyMaterial,
) -> Result<Json<serde_json::Value>, MailError> {
    let (enc, nonce) = crypto(state)?
        .encrypt(&material.secret_armored)
        .map_err(|_| MailError::Crypto)?;
    let email = material.emails.first().cloned();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mail.pgp_keys WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(&state.db)
        .await?;
    let is_default = count == 0;

    let id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO mail.pgp_keys
              (user_id, email, fingerprint, public_key, private_key, private_key_nonce, is_default)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT (user_id, fingerprint) DO UPDATE
              SET email = EXCLUDED.email, public_key = EXCLUDED.public_key,
                  private_key = EXCLUDED.private_key, private_key_nonce = EXCLUDED.private_key_nonce
           RETURNING id"#,
    )
    .bind(user_id)
    .bind(&email)
    .bind(&material.fingerprint)
    .bind(&material.public_armored)
    .bind(&enc)
    .bind(&nonce)
    .bind(is_default)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(serde_json::json!({
        "id": id, "email": email, "fingerprint": material.fingerprint,
        "public_key": material.public_armored, "is_default": is_default,
    })))
}

pub async fn delete_key(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    require_gpg(&state).await?;
    let done = sqlx::query("DELETE FROM mail.pgp_keys WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    if done.rows_affected() == 0 {
        return Err(MailError::NotFound("clé".into()));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Correspondents' public keys ───────────────────────────────────────────────

pub async fn list_contacts(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    require_gpg(&state).await?;
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, String, chrono::DateTime<chrono::Utc>)>(
        r#"SELECT id, email, fingerprint, public_key, source, created_at
           FROM mail.pgp_contacts WHERE user_id = $1 ORDER BY email"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;

    let contacts: Vec<_> = rows
        .into_iter()
        .map(|(id, email, fingerprint, public_key, source, created_at)| {
            serde_json::json!({
                "id": id, "email": email, "fingerprint": fingerprint,
                "public_key": public_key, "source": source, "created_at": created_at,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "contacts": contacts })))
}

#[derive(Deserialize)]
pub struct AddContactDto {
    /// Optional: the correspondent address. If absent, the key's own User ID
    /// address is used.
    #[serde(default)]
    pub email: Option<String>,
    pub public_armored: String,
}

pub async fn add_contact(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<AddContactDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    require_gpg(&state).await?;
    let (re_armored, fingerprint, emails) = pgp::import_public(&dto.public_armored)
        .map_err(|_| MailError::Validation("Clé publique OpenPGP invalide".into()))?;
    let email = dto
        .email
        .map(|e| e.trim().to_lowercase())
        .filter(|e| e.contains('@'))
        .or_else(|| emails.first().cloned())
        .ok_or_else(|| MailError::Validation("Adresse du contact manquante".into()))?;

    let id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO mail.pgp_contacts (user_id, email, fingerprint, public_key, source)
           VALUES ($1, $2, $3, $4, 'manual')
           ON CONFLICT (user_id, lower(email)) DO UPDATE
              SET fingerprint = EXCLUDED.fingerprint, public_key = EXCLUDED.public_key,
                  source = 'manual'
           RETURNING id"#,
    )
    .bind(user.id)
    .bind(&email)
    .bind(&fingerprint)
    .bind(&re_armored)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(serde_json::json!({
        "id": id, "email": email, "fingerprint": fingerprint,
    })))
}

pub async fn delete_contact(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    require_gpg(&state).await?;
    let done = sqlx::query("DELETE FROM mail.pgp_contacts WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    if done.rows_affected() == 0 {
        return Err(MailError::NotFound("contact".into()));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}
