//! Sender avatar endpoint: serves the logo a domain publishes about itself.
//!
//! The lookup and the network call live on the SERVER on purpose. The browser
//! must never fetch a third-party URL per message read (it would leak the
//! reader's activity and fight the page's CSP), and one shared cache spares
//! every user of the instance the same lookup.

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use crate::{errors::MailError, middleware::AuthUser, services::avatars, state::AppState};

#[derive(Deserialize)]
pub struct AvatarQuery {
    /// Sender address or bare domain — only the domain part is ever used.
    pub email: String,
    /// DMARC verdict of the message being displayed. Only `pass` unlocks the
    /// brand's own logo; anything else (fail, none, absent) gets at most a
    /// neutral mark, so a spoofed sender cannot borrow a brand's identity.
    #[serde(default)]
    pub dmarc: Option<String>,
}

/// `GET /avatar?email=…` → the domain's logo, or 404 when it publishes none.
///
/// Authentication is required: this is a cache of outbound lookups, not an open
/// proxy anyone may drive.
pub async fn sender_avatar(
    State(state): State<AppState>,
    _user: AuthUser,
    Query(q): Query<AvatarQuery>,
) -> Result<Response, MailError> {
    // A sender hosted BY THIS INSTANCE is a known Kubuno user: show their own
    // profile picture rather than a domain logo or an initial. The core owns
    // that image, so the browser is pointed straight at it.
    let address = q.email.trim().to_ascii_lowercase();
    let owner: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM mail.mailboxes WHERE LOWER(address) = $1 AND is_active",
    )
    .bind(&address)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    if let Some(user_id) = owner {
        // JSON rather than a redirect: the core proxies this route and FOLLOWS
        // redirects itself, so a `Location` would be resolved against the module
        // (which serves no /users route) and turn into a 404. Handing the URL
        // back lets the browser fetch the core's own avatar directly.
        return Ok(Json(serde_json::json!({
            "avatar_url": format!("/api/v1/users/{user_id}/avatar"),
        }))
        .into_response());
    }

    let domain = q.email.rsplit('@').next().unwrap_or_default().to_string();

    let http = reqwest::Client::new();
    let authenticated = q.dmarc.as_deref() == Some("pass");
    let found = avatars::for_domain(&state.db, &http, &domain, authenticated).await.map_err(|e| {
        tracing::error!(error = %e, domain, "Résolution de l'avatar d'expéditeur échouée");
        MailError::Internal(e)
    })?;

    let Some(avatar) = found else {
        // A plain 404 lets the UI fall back to the coloured initial, and the
        // browser will not ask again for a while.
        return Ok((
            StatusCode::NOT_FOUND,
            [(header::CACHE_CONTROL, "public, max-age=86400")],
        )
            .into_response());
    };

    Ok((
        [
            (header::CONTENT_TYPE, avatar.mime.as_str()),
            (header::CACHE_CONTROL, "public, max-age=86400"),
            // The bytes come from a third party: never let them be sniffed into
            // something executable.
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        avatar.bytes,
    )
        .into_response())
}
