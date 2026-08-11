// ── OAuth2 account connection (Gmail / Microsoft) ────────────────────────────
// Authorization-code flow: `start` returns the provider consent URL, the
// provider redirects the browser back to `callback` (a top-level GET, so the
// SameSite=Lax session cookie is sent and the core proxy authenticates it),
// which exchanges the code, resolves the address, and creates or converts the
// account. Tokens are AES-256-GCM encrypted at rest; they never appear in
// logs or JSON responses.

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Html,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    services::{crypto::MailCrypto, oauth::{self, Provider}},
    state::AppState,
};

/// GET /oauth/providers — which OAuth providers the admin configured.
/// Never exposes the client ids/secrets themselves.
pub async fn providers(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Json<serde_json::Value> {
    let mail = &state.settings.mail;
    Json(serde_json::json!({
        "google":    Provider::Google.client(mail).is_some(),
        "microsoft": Provider::Microsoft.client(mail).is_some(),
    }))
}

fn parse_provider(s: &str) -> Result<Provider, MailError> {
    Provider::parse(s).ok_or_else(|| MailError::NotFound(format!("Fournisseur OAuth {s}")))
}

/// Base URL used to build the redirect URIs: explicit `mail.public_base_url`
/// when configured, otherwise derived from the request. The browser's Origin
/// header reflects exactly the URL the user is on and survives both proxies
/// (nginx + core), unlike Host which the core proxy client rewrites to the
/// module's internal address.
fn base_url(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(b) = state
        .settings
        .mail
        .public_base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return b.trim_end_matches('/').to_string();
    }
    if let Some(o) = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .filter(|o| o.starts_with("http"))
    {
        return o.trim_end_matches('/').to_string();
    }
    if let Some(origin) = headers
        .get("referer")
        .and_then(|v| v.to_str().ok())
        .and_then(|r| reqwest::Url::parse(r).ok())
        .map(|u| u.origin().ascii_serialization())
        .filter(|o| o.starts_with("http"))
    {
        return origin;
    }
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:8080");
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or(if host.starts_with("localhost") || host.starts_with("127.") { "http" } else { "https" });
    format!("{scheme}://{host}")
}

fn redirect_uri(state: &AppState, headers: &HeaderMap, provider: Provider) -> String {
    format!(
        "{}/api/v1/mail/oauth/{}/callback",
        base_url(state, headers),
        provider.as_str()
    )
}

async fn purge_stale_states(state: &AppState) {
    let _ = sqlx::query("DELETE FROM mail.oauth_states WHERE created_at < NOW() - INTERVAL '10 minutes'")
        .execute(&state.db)
        .await;
}

/// POST /oauth/:provider/start — returns the provider's consent URL.
pub async fn start(
    State(state): State<AppState>,
    user: AuthUser,
    Path(provider): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, MailError> {
    let provider = parse_provider(&provider)?;
    let (client_id, _) = provider.client(&state.settings.mail).ok_or_else(|| {
        MailError::Conflict(format!(
            "Connexion {} non configurée sur ce serveur (client OAuth absent)",
            provider.as_str()
        ))
    })?;

    purge_stale_states(&state).await;

    // Random, single-use CSRF state bound to the session user.
    let csrf: String = {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        hex::encode(bytes)
    };
    // The exact redirect URI must be reused for the code exchange at callback
    // time (providers reject any mismatch), so it travels with the state.
    let redirect = redirect_uri(&state, &headers, provider);
    sqlx::query(
        "INSERT INTO mail.oauth_states (state, user_id, provider, redirect_uri) VALUES ($1, $2, $3, $4)",
    )
    .bind(&csrf)
    .bind(user.id)
    .bind(provider.as_str())
    .bind(&redirect)
    .execute(&state.db)
    .await?;
    let mut params: Vec<(&str, &str)> = vec![
        ("client_id",     client_id.as_str()),
        ("redirect_uri",  redirect.as_str()),
        ("response_type", "code"),
        ("scope",         provider.scopes()),
        ("state",         csrf.as_str()),
    ];
    if provider == Provider::Google {
        // Required to obtain a refresh token from Google.
        params.push(("access_type", "offline"));
        params.push(("prompt",      "consent"));
    } else {
        params.push(("response_mode", "query"));
    }

    let auth_url = reqwest::Url::parse_with_params(provider.authorize_endpoint(), &params)
        .map_err(|e| MailError::Internal(anyhow::anyhow!("URL d'autorisation invalide: {e}")))?;

    Ok(Json(serde_json::json!({ "auth_url": auth_url.as_str() })))
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code:  Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// Browser redirect to the mail settings page. Served as a tiny HTML page
/// (meta-refresh + location.replace) instead of a 3xx because the core proxy's
/// HTTP client follows redirects itself — a Location header would never reach
/// the browser. All embedded values are fixed slugs (no user input).
fn settings_redirect(provider: Provider, status: &str, reason: Option<&str>) -> Html<String> {
    let mut url = format!("/mail/settings?oauth={}&status={}", provider.as_str(), status);
    if let Some(r) = reason {
        url.push_str("&reason=");
        url.push_str(r);
    }
    Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"refresh\" content=\"0;url={url}\">\
         <script>location.replace(\"{url}\")</script>\
         </head><body></body></html>"
    ))
}

/// GET /oauth/:provider/callback — provider redirect target (browser
/// navigation, authenticated by the session cookie through the core proxy).
/// Always ends with a redirect to the mail settings page; failures carry a
/// short machine reason, never provider details.
pub async fn callback(
    State(state): State<AppState>,
    user: AuthUser,
    Path(provider): Path<String>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Result<Html<String>, MailError> {
    let provider = parse_provider(&provider)?;

    purge_stale_states(&state).await;

    // User denied the consent screen (or provider-side error).
    if q.error.is_some() || q.code.is_none() {
        return Ok(settings_redirect(provider, "error", Some("denied")));
    }
    let code = q.code.unwrap_or_default();

    // CSRF state: must exist, be recent (purge above), belong to this session
    // user and this provider. Single use: consumed by the DELETE.
    let Some(csrf) = q.state.filter(|s| !s.is_empty()) else {
        return Ok(settings_redirect(provider, "error", Some("state")));
    };
    let row: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
        "DELETE FROM mail.oauth_states WHERE state = $1 RETURNING user_id, provider, redirect_uri",
    )
    .bind(&csrf)
    .fetch_optional(&state.db)
    .await?;
    let stored_redirect = match row {
        Some((uid, prov, redirect)) if uid == user.id && prov == provider.as_str() => redirect,
        _ => return Ok(settings_redirect(provider, "error", Some("state"))),
    };

    let Some((client_id, client_secret)) = provider.client(&state.settings.mail) else {
        return Ok(settings_redirect(provider, "error", Some("config")));
    };

    // Reuse the redirect URI sent at `start` (required by the providers);
    // recompute only as a defensive fallback.
    let redirect = stored_redirect
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| redirect_uri(&state, &headers, provider));
    let tokens = match oauth::exchange_code(provider, &client_id, &client_secret, &code, &redirect).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(provider = provider.as_str(), error = %e, "Échange de code OAuth échoué");
            return Ok(settings_redirect(provider, "error", Some("exchange")));
        }
    };

    // A refresh token is required for background sync; Google only returns one
    // with access_type=offline&prompt=consent (both sent by `start`).
    let Some(refresh_token) = tokens.refresh_token.as_deref().filter(|t| !t.is_empty()) else {
        tracing::error!(provider = provider.as_str(), "Jeton de rafraîchissement absent de la réponse OAuth");
        return Ok(settings_redirect(provider, "error", Some("no_refresh")));
    };

    let email = match oauth::fetch_userinfo_email(provider, &tokens.access_token).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(provider = provider.as_str(), error = %e, "Lecture du profil OpenID échouée");
            return Ok(settings_redirect(provider, "error", Some("userinfo")));
        }
    };

    if let Err(e) = upsert_oauth_account(&state, user.id, provider, &email, refresh_token, &tokens).await {
        tracing::error!(provider = provider.as_str(), error = %e, "Enregistrement du compte OAuth échoué");
        return Ok(settings_redirect(provider, "error", Some("save")));
    }

    Ok(settings_redirect(provider, "ok", None))
}

/// Creates the account with the provider's server presets, or converts an
/// existing account with the same address (the repair path for accounts whose
/// password no longer works, e.g. Gmail).
async fn upsert_oauth_account(
    state: &AppState,
    user_id: Uuid,
    provider: Provider,
    email: &str,
    refresh_token: &str,
    tokens: &oauth::TokenSet,
) -> anyhow::Result<()> {
    let crypto = MailCrypto::new(&state.settings.mail.encryption_key)?;
    let (refresh_enc, refresh_nonce) = crypto.encrypt(refresh_token)?;
    let (access_enc, access_nonce)   = crypto.encrypt(&tokens.access_token)?;

    let (imap_host, imap_port, imap_sec) = provider.imap_preset();
    let (smtp_host, smtp_port, smtp_sec) = provider.smtp_preset();

    let mut tx = state.db.begin().await?;

    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM mail.accounts WHERE user_id = $1 AND LOWER(email_address) = LOWER($2) LIMIT 1",
    )
    .bind(user_id)
    .bind(email)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(account_id) = existing {
        // Conversion to OAuth: keep the synced content (same mailbox), switch
        // the servers to the provider presets and store the tokens.
        sqlx::query(
            r#"UPDATE mail.accounts SET
                   auth_kind = $1,
                   imap_host = $2, imap_port = $3, imap_security = $4, imap_username = $5,
                   smtp_host = $6, smtp_port = $7, smtp_security = $8, smtp_username = $5,
                   incoming_protocol = 'imap',
                   oauth_refresh_token = $9, oauth_refresh_nonce = $10,
                   oauth_access_token = $11, oauth_access_nonce = $12, oauth_expires_at = $13,
                   is_active = TRUE, last_error = NULL
               WHERE id = $14"#,
        )
        .bind(provider.auth_kind())
        .bind(imap_host)
        .bind(imap_port)
        .bind(imap_sec)
        .bind(email)
        .bind(smtp_host)
        .bind(smtp_port)
        .bind(smtp_sec)
        .bind(refresh_enc.as_slice())
        .bind(refresh_nonce.as_slice())
        .bind(access_enc.as_slice())
        .bind(access_nonce.as_slice())
        .bind(tokens.expires_at)
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    } else {
        // Fresh account. The NOT NULL password columns hold an encrypted empty
        // string — never used for oauth_* auth kinds.
        let (empty_enc, empty_nonce)   = crypto.encrypt("")?;
        let (empty_enc2, empty_nonce2) = crypto.encrypt("")?;
        let has_accounts: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM mail.accounts WHERE user_id = $1)",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;

        let id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO mail.accounts
               (id, user_id, name, email_address, incoming_protocol,
                imap_host, imap_port, imap_security, imap_username, imap_password, imap_password_nonce,
                smtp_host, smtp_port, smtp_security, smtp_username, smtp_password, smtp_password_nonce,
                is_default, auth_kind,
                oauth_refresh_token, oauth_refresh_nonce,
                oauth_access_token, oauth_access_nonce, oauth_expires_at)
               VALUES ($1,$2,$3,$4,'imap',$5,$6,$7,$8,$9,$10,$11,$12,$13,$8,$14,$15,$16,$17,$18,$19,$20,$21,$22)"#,
        )
        .bind(id)
        .bind(user_id)
        .bind(email)          // display name defaults to the address
        .bind(email)
        .bind(imap_host)
        .bind(imap_port)
        .bind(imap_sec)
        .bind(email)          // imap/smtp username = address ($8)
        .bind(empty_enc.as_slice())
        .bind(empty_nonce.as_slice())
        .bind(smtp_host)
        .bind(smtp_port)
        .bind(smtp_sec)
        .bind(empty_enc2.as_slice())
        .bind(empty_nonce2.as_slice())
        .bind(!has_accounts)  // first account becomes the default
        .bind(provider.auth_kind())
        .bind(refresh_enc.as_slice())
        .bind(refresh_nonce.as_slice())
        .bind(access_enc.as_slice())
        .bind(access_nonce.as_slice())
        .bind(tokens.expires_at)
        .execute(&mut *tx)
        .await?;

        // Same system labels as password-account creation.
        for (name, folder) in &[
            ("Boîte de réception", "INBOX"),
            ("Envoyés",            "Sent"),
            ("Brouillons",         "Drafts"),
            ("Spam",               "Junk"),
            ("Corbeille",          "Trash"),
        ] {
            sqlx::query(
                "INSERT INTO mail.labels (account_id, user_id, name, imap_folder, is_system) VALUES ($1,$2,$3,$4,TRUE)",
            )
            .bind(id)
            .bind(user_id)
            .bind(name)
            .bind(folder)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(())
}
