use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts, Request, State},
    http::{request::Parts, HeaderMap, StatusCode},
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
/// nothing may then be considered internal, so everything is refused. The
/// comparison is constant-time.
pub async fn require_internal_secret(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, MailError> {
    let expected = state.settings.core.internal_secret.as_str();
    if expected.is_empty() {
        tracing::error!("mail: core.internal_secret is empty, internal request refused");
        return Err(MailError::Unauthorized);
    }

    let provided = req
        .headers()
        .get("x-internal-secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !internal_secret_matches(expected, provided) {
        return Err(MailError::Unauthorized);
    }
    Ok(next.run(req).await)
}

/// This module's id, the audience of the identity tokens the core mints for it.
const MODULE_ID: &str = "mail";

/// Who the core says is calling, from the signed `X-Kubuno-Auth` token the core
/// mints with this module's internal secret (see `kubuno-modauth`).
///
/// The plain `X-Kubuno-User-*` headers are never read: any process reaching this
/// module's loopback port could set them and impersonate any user. A token is
/// only accepted when it was signed with this module's secret, for this module,
/// and has not expired. An **empty** configured secret refuses everything: an
/// HMAC keyed with nothing would let anyone mint a valid token.
pub fn authenticate(secret: &str, headers: &HeaderMap) -> Option<kubuno_modauth::ModuleUser> {
    if secret.is_empty() {
        tracing::error!(
            "mail: core.internal_secret is empty, request refused. Set KUBUNO_INTERNAL_SECRET."
        );
        return None;
    }
    let token = headers.get(kubuno_modauth::TOKEN_HEADER)?.to_str().ok()?;
    kubuno_modauth::verify(secret.as_bytes(), token, MODULE_ID).ok()
}

/// Whether `provided` is the configured internal secret: never when that secret
/// is empty, and compared in constant time.
pub fn internal_secret_matches(expected: &str, provided: &str) -> bool {
    !expected.is_empty() && constant_time_eq(provided.as_bytes(), expected.as_bytes())
}

/// Byte comparison whose duration does not depend on where the first difference
/// is. The length check leaks the length, which is not a secret.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The authenticated user, from the signed identity token the core proxy mints
/// (see [`authenticate`]). As an `Option<AuthUser>` (public routes such as the
/// OAuth callback), a request without a valid token yields `None`.
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
    AppState: FromRef<S>,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);
        let user = authenticate(&app.settings.core.internal_secret, &parts.headers)
            .ok_or((StatusCode::UNAUTHORIZED, "Non authentifié"))?;
        Ok(AuthUser { id: user.id, email: user.email, role: user.role })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    const SECRET: &str = "module-secret-for-tests";

    fn user() -> kubuno_modauth::ModuleUser {
        kubuno_modauth::ModuleUser {
            id: Uuid::new_v4(),
            role: "user".into(),
            email: "ada@example.org".into(),
        }
    }

    fn with_token(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(kubuno_modauth::TOKEN_HEADER, HeaderValue::from_str(token).expect("header"));
        h
    }

    #[test]
    fn forged_plain_headers_are_not_an_identity() {
        let mut h = HeaderMap::new();
        h.insert("x-kubuno-user-id", HeaderValue::from_static("0e835e1a-64fb-47c4-9775-5786236fce19"));
        h.insert("x-kubuno-user-role", HeaderValue::from_static("admin"));
        h.insert("x-kubuno-user-email", HeaderValue::from_static("admin@example.org"));
        assert!(authenticate(SECRET, &h).is_none());
    }

    #[test]
    fn a_token_signed_for_this_module_is_accepted() {
        let u = user();
        let token = kubuno_modauth::sign(SECRET.as_bytes(), &u, MODULE_ID);
        let got = authenticate(SECRET, &with_token(&token)).expect("valid token");
        assert_eq!(got.id, u.id);
        assert_eq!(got.email, u.email);
        assert_eq!(got.role, u.role);
    }

    #[test]
    fn a_token_for_another_module_or_key_is_refused() {
        let u = user();
        let other_aud = kubuno_modauth::sign(SECRET.as_bytes(), &u, "some-other-module");
        assert!(authenticate(SECRET, &with_token(&other_aud)).is_none());
        let other_key = kubuno_modauth::sign(b"another-secret", &u, MODULE_ID);
        assert!(authenticate(SECRET, &with_token(&other_key)).is_none());
        assert!(authenticate(SECRET, &with_token("v1.garbage.garbage")).is_none());
    }

    #[test]
    fn an_empty_secret_refuses_everything() {
        let token = kubuno_modauth::sign(b"", &user(), MODULE_ID);
        assert!(authenticate("", &with_token(&token)).is_none());
    }

    #[test]
    fn the_internal_secret_guard() {
        assert!(internal_secret_matches("abc", "abc"));
        assert!(!internal_secret_matches("abc", "abd"));
        assert!(!internal_secret_matches("abc", ""));
        assert!(!internal_secret_matches("", ""));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
