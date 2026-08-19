use axum::{
    async_trait,
    extract::{FromRequestParts, Request, State},
    http::{request::Parts, StatusCode},
    middleware::Next,
    response::Response,
};
use uuid::Uuid;

use crate::{errors::MailError, state::AppState};

/// Rejects any `/internal/*` request that does not carry the shared secret the
/// core and this module agree on. Applied as a layer on the internal sub-router
/// (same shape as the other Kubuno modules) rather than as an extractor: the
/// guard must run before the handler body is even read.
///
/// An unset secret means the module was started outside the core supervisor —
/// nothing may then be considered internal, so everything is refused.
pub async fn require_internal_secret(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, MailError> {
    let expected = state.settings.core.internal_secret.as_str();
    if expected.is_empty() {
        tracing::error!("Secret interne non configuré — requête interne refusée");
        return Err(MailError::Unauthorized);
    }

    let provided = req
        .headers()
        .get("x-internal-secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if provided.is_empty() || !secrets_match(provided, expected) {
        return Err(MailError::Unauthorized);
    }
    Ok(next.run(req).await)
}

/// Constant-time byte comparison. A plain `==` bails out on the first differing
/// byte, which turns the response time into an oracle for guessing the secret
/// one byte at a time. Only the length is allowed to leak (comparing padded
/// buffers would leak it through timing anyway).
fn secrets_match(provided: &str, expected: &str) -> bool {
    let (a, b) = (provided.as_bytes(), expected.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Utilisateur authentifié extrait des headers X-Kubuno-* injectés par le core proxy
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id:    Uuid,
    pub email: String,
    pub role:  String,
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let id_str = parts
            .headers
            .get("X-Kubuno-User-Id")
            .and_then(|v| v.to_str().ok())
            .ok_or((StatusCode::UNAUTHORIZED, "Non authentifié"))?;

        let id = Uuid::parse_str(id_str)
            .map_err(|_| (StatusCode::UNAUTHORIZED, "User-Id invalide"))?;

        let email = parts
            .headers
            .get("X-Kubuno-User-Email")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let role = parts
            .headers
            .get("X-Kubuno-User-Role")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("user")
            .to_string();

        Ok(AuthUser { id, email, role })
    }
}
