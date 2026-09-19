//! Outbound relay (smarthost) configuration (admin).
//!
//! An instance that cannot deliver direct-to-MX — a residential line blocks
//! port 25 outbound — hands every remote message to ONE SMTP host that can
//! (the VPS's Postfix over a private tunnel). This is where an administrator
//! turns that on and points it at the host. See migration 000026 and the
//! runtime side in `server::relay`.
//!
//! The relay password is WRITE-ONLY: encrypted at rest with the module's
//! `MailCrypto` (like account and DKIM keys), never returned by the API, never
//! logged. `GET` reports only whether one is set.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::{
    errors::MailError, middleware::AuthUser, server::relay::RelaySecurity,
    services::crypto::MailCrypto, state::AppState,
};

/// Inter-MTA default; a submission relay usually wants 587 or 465, set explicitly.
const RELAY_DEFAULT_PORT: i32 = 25;

/// Only an administrator configures the instance-wide relay.
fn require_admin(user: &AuthUser) -> Result<(), MailError> {
    if user.role == "admin" {
        Ok(())
    } else {
        Err(MailError::Forbidden)
    }
}

/// The relay state, WITHOUT the password: whether one is set, never its value.
#[derive(Serialize, sqlx::FromRow)]
pub struct RelayView {
    pub enabled:      bool,
    pub host:         String,
    pub port:         i32,
    pub security:     String,
    pub username:     String,
    pub has_password: bool,
}

pub async fn get_relay(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<RelayView>, MailError> {
    require_admin(&user)?;
    let view = read_relay(&state).await?;
    Ok(Json(view))
}

/// Reads the singleton relay row, minus the password. The migration seeds the
/// row, but a defensive default keeps the read total on a database that somehow
/// lost it.
async fn read_relay(state: &AppState) -> Result<RelayView, MailError> {
    let view = sqlx::query_as::<_, RelayView>(
        "SELECT enabled, host, port, security, username, \
                (password_enc IS NOT NULL AND password_nonce IS NOT NULL) AS has_password \
         FROM mail.outbound_relay WHERE id = TRUE",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Relais sortant : lecture impossible");
        MailError::Database(e)
    })?;

    Ok(view.unwrap_or(RelayView {
        enabled:      false,
        host:         String::new(),
        port:         RELAY_DEFAULT_PORT,
        security:     RelaySecurity::None.as_str().to_string(),
        username:     String::new(),
        has_password: false,
    }))
}

#[derive(Deserialize)]
pub struct RelayDto {
    pub enabled: bool,
    #[serde(default)]
    pub host: String,
    pub port: Option<i32>,
    #[serde(default)]
    pub security: String,
    #[serde(default)]
    pub username: Option<String>,
    /// Write-only. Non-empty ⇒ encrypt and store. Absent/empty ⇒ keep existing.
    #[serde(default)]
    pub password: Option<String>,
    /// Explicitly wipe the stored password (distinct from "leave it as is").
    #[serde(default)]
    pub clear_password: bool,
}

/// What happens to the encrypted password column on this write.
enum PasswordChange {
    /// Leave the stored password untouched (the default when none is supplied).
    Keep,
    /// Wipe it.
    Clear,
    /// Replace it with freshly-encrypted bytes.
    Set(Vec<u8>, Vec<u8>),
}

pub async fn put_relay(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<RelayDto>,
) -> Result<Json<RelayView>, MailError> {
    require_admin(&user)?;

    let host = dto.host.trim().to_string();
    let username = dto.username.unwrap_or_default().trim().to_string();
    let security = validate_relay(dto.enabled, &host, &dto.security, &username)?;
    let port = dto
        .port
        .filter(|p| (1..=65535).contains(p))
        .unwrap_or(RELAY_DEFAULT_PORT);

    // Decide the password column BEFORE touching the DB, so an encryption failure
    // is reported without a half-applied write.
    let change = if dto.clear_password {
        PasswordChange::Clear
    } else if let Some(secret) = dto.password.as_deref().filter(|s| !s.is_empty()) {
        let crypto = MailCrypto::new(&state.settings.mail.encryption_key).map_err(|_| MailError::Crypto)?;
        let (enc, nonce) = crypto.encrypt(secret).map_err(|_| MailError::Crypto)?;
        PasswordChange::Set(enc, nonce)
    } else {
        PasswordChange::Keep
    };

    // The singleton is seeded by the migration; ensure it exists all the same so
    // the UPDATE below (which, in the Keep case, never touches the secret
    // columns) always lands on a row.
    sqlx::query("INSERT INTO mail.outbound_relay (id, enabled) VALUES (TRUE, FALSE) ON CONFLICT (id) DO NOTHING")
        .execute(&state.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Relais sortant : initialisation de la ligne");
            MailError::Database(e)
        })?;

    // Append the password columns only when they change, so "keep existing"
    // stays a plain UPDATE that leaves the secret alone.
    let mut sql = String::from(
        "UPDATE mail.outbound_relay \
         SET enabled = $1, host = $2, port = $3, security = $4, username = $5, updated_at = NOW()",
    );
    match &change {
        PasswordChange::Keep => {}
        PasswordChange::Clear => sql.push_str(", password_enc = NULL, password_nonce = NULL"),
        PasswordChange::Set(_, _) => sql.push_str(", password_enc = $6, password_nonce = $7"),
    }
    sql.push_str(" WHERE id = TRUE");

    // Audited: `sql` is assembled from the literals just above — the branch is
    // chosen by `change`, never by caller text — and every value is bound.
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(dto.enabled)
        .bind(&host)
        .bind(port)
        .bind(security.as_str())
        .bind(&username);
    if let PasswordChange::Set(enc, nonce) = &change {
        query = query.bind(enc).bind(nonce);
    }
    query.execute(&state.db).await.map_err(|e| {
        tracing::error!(error = %e, "Relais sortant : enregistrement impossible");
        MailError::Database(e)
    })?;

    // Echo the fresh state — never the password.
    let view = read_relay(&state).await?;
    Ok(Json(view))
}

/// Validates a relay configuration and returns the parsed security level.
///
/// Two refusals:
///   * enabled with no host — the relay points nowhere;
///   * `security = none` with a username — a password would cross the wire in
///     the clear.
fn validate_relay(
    enabled: bool,
    host: &str,
    security_raw: &str,
    username: &str,
) -> Result<RelaySecurity, MailError> {
    let security = RelaySecurity::parse(security_raw).ok_or_else(|| {
        MailError::Validation("Sécurité de relais inconnue : none, starttls ou tls.".into())
    })?;
    if enabled && host.is_empty() {
        return Err(MailError::Validation("Un relais activé exige un hôte.".into()));
    }
    if security == RelaySecurity::None && !username.is_empty() {
        return Err(MailError::Validation(
            "Un mot de passe ne doit pas transiter en clair — utilisez STARTTLS ou TLS, \
             ou un relais sans authentification sur réseau de confiance."
                .into(),
        ));
    }
    Ok(security)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_enabled_relay_needs_a_host() {
        assert!(validate_relay(true, "", "starttls", "").is_err());
        // Disabled: the host may be blank (a config being drafted).
        assert!(validate_relay(false, "", "none", "").is_ok());
        assert!(validate_relay(true, "relay.example.com", "starttls", "user").is_ok());
    }

    #[test]
    fn a_password_may_not_cross_the_wire_in_the_clear() {
        // none + a username = a cleartext password: refused.
        assert!(validate_relay(true, "relay", "none", "user").is_err());
        // none WITHOUT authentication is fine (trusted-network relay).
        assert!(validate_relay(true, "relay", "none", "").is_ok());
        // With STARTTLS/TLS a username is fine.
        assert!(validate_relay(true, "relay", "starttls", "user").is_ok());
        assert!(validate_relay(true, "relay", "tls", "user").is_ok());
    }

    #[test]
    fn an_unknown_security_level_is_rejected() {
        assert!(validate_relay(true, "relay", "ssl", "").is_err());
        assert!(validate_relay(true, "relay", "", "").is_err());
    }
}
