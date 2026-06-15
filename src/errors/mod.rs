use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("Non authentifié")]
    Unauthorized,

    #[error("Accès refusé")]
    Forbidden,

    #[error("Ressource introuvable: {0}")]
    NotFound(String),

    #[error("Données invalides: {0}")]
    Validation(String),

    #[error("Conflit: {0}")]
    Conflict(String),

    #[error("Erreur IMAP: {0}")]
    Imap(String),

    #[error("Erreur SMTP: {0}")]
    Smtp(String),

    #[error("Erreur de déchiffrement")]
    Crypto,

    #[error("Erreur base de données")]
    Database(#[from] sqlx::Error),

    #[error("Erreur interne")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for MailError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            MailError::Unauthorized        => (StatusCode::UNAUTHORIZED,            "UNAUTHORIZED",   self.to_string()),
            MailError::Forbidden           => (StatusCode::FORBIDDEN,               "FORBIDDEN",      self.to_string()),
            MailError::NotFound(m)         => (StatusCode::NOT_FOUND,               "NOT_FOUND",      m.clone()),
            MailError::Validation(m)       => (StatusCode::UNPROCESSABLE_ENTITY,    "VALIDATION",     m.clone()),
            MailError::Conflict(m)         => (StatusCode::CONFLICT,                "CONFLICT",       m.clone()),
            MailError::Imap(m)             => (StatusCode::BAD_GATEWAY,             "IMAP_ERROR",     m.clone()),
            MailError::Smtp(m)             => (StatusCode::BAD_GATEWAY,             "SMTP_ERROR",     m.clone()),
            MailError::Crypto              => (StatusCode::INTERNAL_SERVER_ERROR,   "CRYPTO_ERROR",   self.to_string()),
            MailError::Database(e)         => {
                tracing::error!(error = %e, "Erreur base de données");
                (StatusCode::INTERNAL_SERVER_ERROR, "DATABASE_ERROR", "Erreur interne".to_string())
            }
            MailError::Internal(e)         => {
                tracing::error!(error = %e, "Erreur interne");
                (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", "Erreur interne".to_string())
            }
        };

        (status, Json(json!({ "error": code, "message": message }))).into_response()
    }
}
