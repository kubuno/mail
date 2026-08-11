// ── OAuth2 (XOAUTH2) support for Gmail and Microsoft accounts ────────────────
// Google (and Microsoft for new tenants) refuse account passwords on IMAP/SMTP:
// only app passwords or OAuth2 pass. This module holds the provider metadata,
// the authorization-code exchange, and the access-token refresh logic.
//
// SECURITY: tokens and authorization codes must NEVER be logged or returned in
// JSON. Errors are summarized (status code + generic message) before surfacing.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{config::settings::MailSettings, services::crypto::MailCrypto};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Google,
    Microsoft,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "google"    => Some(Provider::Google),
            "microsoft" => Some(Provider::Microsoft),
            _           => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Google    => "google",
            Provider::Microsoft => "microsoft",
        }
    }

    /// Value stored in mail.accounts.auth_kind.
    pub fn auth_kind(&self) -> &'static str {
        match self {
            Provider::Google    => "oauth_google",
            Provider::Microsoft => "oauth_microsoft",
        }
    }

    pub fn from_auth_kind(kind: &str) -> Option<Self> {
        match kind {
            "oauth_google"    => Some(Provider::Google),
            "oauth_microsoft" => Some(Provider::Microsoft),
            _                 => None,
        }
    }

    pub fn authorize_endpoint(&self) -> &'static str {
        match self {
            Provider::Google    => "https://accounts.google.com/o/oauth2/v2/auth",
            Provider::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
        }
    }

    pub fn token_endpoint(&self) -> &'static str {
        match self {
            Provider::Google    => "https://oauth2.googleapis.com/token",
            Provider::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        }
    }

    /// OpenID userinfo endpoint, used to learn the account's email address.
    pub fn userinfo_endpoint(&self) -> &'static str {
        match self {
            Provider::Google    => "https://openidconnect.googleapis.com/v1/userinfo",
            Provider::Microsoft => "https://graph.microsoft.com/oidc/userinfo",
        }
    }

    pub fn scopes(&self) -> &'static str {
        match self {
            Provider::Google    => "https://mail.google.com/ openid email",
            Provider::Microsoft => "offline_access openid email \
                https://outlook.office.com/IMAP.AccessAsUser.All \
                https://outlook.office.com/SMTP.Send",
        }
    }

    /// IMAP/SMTP server presets applied when an account is created via OAuth.
    /// (host, port, security) — IMAP then SMTP.
    pub fn imap_preset(&self) -> (&'static str, i32, &'static str) {
        match self {
            Provider::Google    => ("imap.gmail.com",        993, "ssl"),
            Provider::Microsoft => ("outlook.office365.com", 993, "ssl"),
        }
    }

    pub fn smtp_preset(&self) -> (&'static str, i32, &'static str) {
        match self {
            Provider::Google    => ("smtp.gmail.com",       587, "starttls"),
            Provider::Microsoft => ("smtp.office365.com",   587, "starttls"),
        }
    }

    /// Client id/secret from the module settings; None when not configured.
    pub fn client(&self, mail: &MailSettings) -> Option<(String, String)> {
        let (id, secret) = match self {
            Provider::Google    => (&mail.google_client_id,    &mail.google_client_secret),
            Provider::Microsoft => (&mail.microsoft_client_id, &mail.microsoft_client_secret),
        };
        match (id.as_deref(), secret.as_deref()) {
            (Some(i), Some(s)) if !i.trim().is_empty() && !s.trim().is_empty() => {
                Some((i.trim().to_string(), s.trim().to_string()))
            }
            _ => None,
        }
    }
}

// ── Token endpoint responses ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token:  String,
    refresh_token: Option<String>,
    /// Lifetime in seconds (both providers return it).
    expires_in:    Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error:             Option<String>,
    #[allow(dead_code)]
    error_description: Option<String>,
}

pub struct TokenSet {
    pub access_token:  String,
    pub refresh_token: Option<String>,
    pub expires_at:    DateTime<Utc>,
}

fn expires_at(expires_in: Option<i64>) -> DateTime<Utc> {
    // Default to 30 minutes when the provider omits expires_in (defensive).
    Utc::now() + Duration::seconds(expires_in.unwrap_or(1800).clamp(60, 86_400))
}

/// Exchanges an authorization code for tokens (step 2 of the code flow).
pub async fn exchange_code(
    provider:      Provider,
    client_id:     &str,
    client_secret: &str,
    code:          &str,
    redirect_uri:  &str,
) -> Result<TokenSet> {
    let params = [
        ("client_id",     client_id),
        ("client_secret", client_secret),
        ("code",          code),
        ("redirect_uri",  redirect_uri),
        ("grant_type",    "authorization_code"),
    ];
    let resp = reqwest::Client::new()
        .post(provider.token_endpoint())
        .form(&params)
        .send()
        .await
        .context("Requête token OAuth")?;

    let status = resp.status();
    if !status.is_success() {
        // Summarize only — never log the code or the raw body (may echo secrets).
        let kind = resp
            .json::<TokenErrorResponse>()
            .await
            .ok()
            .and_then(|e| e.error)
            .unwrap_or_else(|| "unknown".into());
        tracing::error!(provider = provider.as_str(), %status, error = %kind, "Échange de code OAuth refusé");
        return Err(anyhow!("Échange de code refusé par le fournisseur ({kind})"));
    }

    let tok: TokenResponse = resp.json().await.context("Réponse token OAuth invalide")?;
    Ok(TokenSet {
        expires_at:    expires_at(tok.expires_in),
        access_token:  tok.access_token,
        refresh_token: tok.refresh_token,
    })
}

/// Fetches the authenticated account's email address via the OpenID userinfo
/// endpoint.
pub async fn fetch_userinfo_email(provider: Provider, access_token: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct UserInfo {
        email: Option<String>,
    }

    let resp = reqwest::Client::new()
        .get(provider.userinfo_endpoint())
        .bearer_auth(access_token)
        .send()
        .await
        .context("Requête userinfo OAuth")?;

    let status = resp.status();
    if !status.is_success() {
        tracing::error!(provider = provider.as_str(), %status, "userinfo OAuth refusé");
        return Err(anyhow!("Le fournisseur a refusé la lecture du profil"));
    }

    let info: UserInfo = resp.json().await.context("Réponse userinfo invalide")?;
    info.email
        .filter(|e| e.contains('@'))
        .ok_or_else(|| anyhow!("Adresse e-mail absente du profil OpenID"))
}

// ── Access-token refresh ──────────────────────────────────────────────────────

/// Returns a currently valid access token for an OAuth account, refreshing it
/// via the stored refresh token when it expires within 60 s. On `invalid_grant`
/// (token revoked / expired), sets an explicit `last_error` on the account so
/// the UI can prompt for a reconnection.
pub async fn valid_access_token(
    db:      &PgPool,
    crypto:  &MailCrypto,
    mail:    &MailSettings,
    account_id: Uuid,
) -> Result<String> {
    type OauthRow = (
        String,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<DateTime<Utc>>,
    );
    let row: OauthRow =
        sqlx::query_as(
            "SELECT auth_kind, oauth_refresh_token, oauth_refresh_nonce, \
                    oauth_access_token, oauth_access_nonce, oauth_expires_at \
             FROM mail.accounts WHERE id = $1",
        )
        .bind(account_id)
        .fetch_one(db)
        .await
        .context("Lecture des jetons OAuth")?;

    let (auth_kind, refresh_enc, refresh_nonce, access_enc, access_nonce, exp) = row;
    let provider = Provider::from_auth_kind(&auth_kind)
        .ok_or_else(|| anyhow!("Compte non OAuth (auth_kind = {auth_kind})"))?;

    // Cached access token still valid (with a 60 s safety margin)?
    if let (Some(enc), Some(nonce), Some(exp)) = (&access_enc, &access_nonce, exp) {
        if exp - Duration::seconds(60) > Utc::now() {
            if let Ok(token) = crypto.decrypt(enc, nonce) {
                return Ok(token);
            }
        }
    }

    let (client_id, client_secret) = provider.client(mail).ok_or_else(|| {
        anyhow!("OAuth {} non configuré sur ce serveur", provider.as_str())
    })?;

    let (refresh_enc, refresh_nonce) = match (refresh_enc, refresh_nonce) {
        (Some(e), Some(n)) => (e, n),
        _ => return Err(anyhow!("Jeton de rafraîchissement absent — reconnexion requise")),
    };
    let refresh_token = crypto
        .decrypt(&refresh_enc, &refresh_nonce)
        .map_err(|_| anyhow!("Déchiffrement du jeton de rafraîchissement échoué"))?;

    let params = [
        ("client_id",     client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("refresh_token", refresh_token.as_str()),
        ("grant_type",    "refresh_token"),
    ];
    let resp = reqwest::Client::new()
        .post(provider.token_endpoint())
        .form(&params)
        .send()
        .await
        .context("Requête refresh OAuth")?;

    let status = resp.status();
    if !status.is_success() {
        let kind = resp
            .json::<TokenErrorResponse>()
            .await
            .ok()
            .and_then(|e| e.error)
            .unwrap_or_else(|| "unknown".into());
        tracing::error!(%account_id, provider = provider.as_str(), %status, error = %kind, "Refresh OAuth échoué");
        if kind == "invalid_grant" {
            // Token revoked or expired: surface a clear, actionable error.
            let msg = "Autorisation OAuth expirée ou révoquée — reconnexion requise dans les réglages du compte";
            let _ = sqlx::query("UPDATE mail.accounts SET last_error = $1 WHERE id = $2")
                .bind(msg)
                .bind(account_id)
                .execute(db)
                .await;
            return Err(anyhow!("{msg}"));
        }
        return Err(anyhow!("Rafraîchissement du jeton refusé ({kind})"));
    }

    let tok: TokenResponse = resp.json().await.context("Réponse refresh OAuth invalide")?;
    let new_exp = expires_at(tok.expires_in);

    let (acc_enc, acc_nonce) = crypto
        .encrypt(&tok.access_token)
        .map_err(|_| anyhow!("Chiffrement du jeton d'accès échoué"))?;
    sqlx::query(
        "UPDATE mail.accounts SET oauth_access_token = $1, oauth_access_nonce = $2, oauth_expires_at = $3 WHERE id = $4",
    )
    .bind(acc_enc.as_slice())
    .bind(acc_nonce.as_slice())
    .bind(new_exp)
    .bind(account_id)
    .execute(db)
    .await
    .context("Stockage du jeton d'accès")?;

    // Microsoft rotates refresh tokens: persist the new one when returned.
    if let Some(new_refresh) = tok.refresh_token.as_deref().filter(|t| !t.is_empty()) {
        if let Ok((r_enc, r_nonce)) = crypto.encrypt(new_refresh) {
            let _ = sqlx::query(
                "UPDATE mail.accounts SET oauth_refresh_token = $1, oauth_refresh_nonce = $2 WHERE id = $3",
            )
            .bind(r_enc.as_slice())
            .bind(r_nonce.as_slice())
            .bind(account_id)
            .execute(db)
            .await;
        }
    }

    Ok(tok.access_token)
}
