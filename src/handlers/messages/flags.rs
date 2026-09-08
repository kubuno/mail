use axum::{
    extract::{Path, State},
    Json,
};
use uuid::Uuid;

use crate::{errors::MailError, middleware::AuthUser, state::AppState};

pub async fn star_message(
    State(state): State<AppState>,
    user: AuthUser,
    Path(msg_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let row = sqlx::query_scalar::<_, bool>(
        "UPDATE mail.messages SET is_starred = NOT is_starred WHERE id = $1 AND user_id = $2 RETURNING is_starred"
    )
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Message {msg_id}")))?;

    Ok(Json(serde_json::json!({ "is_starred": row })))
}

pub async fn mark_read(
    State(state): State<AppState>,
    user: AuthUser,
    Path(msg_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, MailError> {
    let is_read = body["is_read"].as_bool().unwrap_or(true);

    let thread_id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE mail.messages SET is_read = $1 WHERE id = $2 AND user_id = $3 RETURNING thread_id"
    )
    .bind(is_read)
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?;

    if thread_id.is_none() {
        return Err(MailError::NotFound(format!("Message {msg_id}")));
    }

    let unread: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.messages WHERE thread_id = $1 AND is_read = FALSE AND is_deleted = FALSE"
    )
    .bind(thread_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    sqlx::query("UPDATE mail.threads SET unread_count = $1 WHERE id = $2")
        .bind(unread as i32)
        .bind(thread_id)
        .execute(&state.db)
        .await?;

    Ok(Json(serde_json::json!({ "is_read": is_read })))
}

pub async fn delete_message(
    State(state): State<AppState>,
    user: AuthUser,
    Path(msg_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let result = sqlx::query(
        "UPDATE mail.messages SET is_deleted = TRUE, folder = 'trash' WHERE id = $1 AND user_id = $2"
    )
    .bind(msg_id)
    .bind(user.id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Message {msg_id}")));
    }
    Ok(Json(serde_json::json!({ "message": "Message supprimé" })))
}
