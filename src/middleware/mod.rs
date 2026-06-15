use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};
use uuid::Uuid;

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
