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
    // Toggle then read back in one transaction (MySQL has no RETURNING): the
    // value returned is the one this toggle wrote.
    let mut tx = state.db.begin().await?;
    let toggled = crate::db::query(
        "UPDATE mail.messages SET is_starred = NOT is_starred WHERE id = $1 AND user_id = $2",
    )
    .bind(msg_id)
    .bind(user.id)
    .execute(&mut tx)
    .await?;
    if toggled.rows_affected() == 0 {
        tx.rollback().await?;
        return Err(MailError::NotFound(format!("Message {msg_id}")));
    }
    let row = crate::db::query_scalar::<bool>(
        "SELECT is_starred FROM mail.messages WHERE id = $1 AND user_id = $2",
    )
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&mut tx)
    .await?;
    tx.commit().await?;
    let row = row.ok_or_else(|| MailError::NotFound(format!("Message {msg_id}")))?;

    Ok(Json(serde_json::json!({ "is_starred": row })))
}

pub async fn mark_read(
    State(state): State<AppState>,
    user: AuthUser,
    Path(msg_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, MailError> {
    let is_read = body["is_read"].as_bool().unwrap_or(true);

    let updated = crate::db::query(
        "UPDATE mail.messages SET is_read = $1 WHERE id = $2 AND user_id = $3",
    )
    .bind(is_read)
    .bind(msg_id)
    .bind(user.id)
    .execute(&state.db)
    .await?;
    let thread_id: Option<Uuid> = if updated.rows_affected() == 0 {
        None
    } else {
        crate::db::query_scalar("SELECT thread_id FROM mail.messages WHERE id = $1 AND user_id = $2")
            .bind(msg_id)
            .bind(user.id)
            .fetch_optional(&state.db)
            .await?
    };

    if thread_id.is_none() {
        return Err(MailError::NotFound(format!("Message {msg_id}")));
    }

    let unread: i64 = crate::db::query_scalar(format!(
        "SELECT {} FROM mail.messages WHERE thread_id = $1 AND is_read = FALSE AND is_deleted = FALSE",
        state.db.backend().count_bigint("*"),
    ))
    .bind(thread_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    crate::db::query("UPDATE mail.threads SET unread_count = $1 WHERE id = $2")
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
    let result = crate::db::query(
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
