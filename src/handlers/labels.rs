use axum::{
    extract::{Path, State},
    Json,
};
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    models::{CreateLabelDto, Label},
    state::AppState,
};

pub async fn list_labels(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let labels = sqlx::query_as::<_, Label>(
        "SELECT id, account_id, user_id, name, color, imap_folder, is_system, position, created_at
         FROM mail.labels WHERE user_id = $1 ORDER BY is_system DESC, position, name",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(serde_json::json!({ "labels": labels })))
}

pub async fn create_label(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<CreateLabelDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    if dto.name.trim().is_empty() {
        return Err(MailError::Validation("Nom requis".into()));
    }

    let account_exists: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM mail.accounts WHERE id = $1 AND user_id = $2)"
    )
    .bind(dto.account_id)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    if !account_exists {
        return Err(MailError::NotFound(format!("Compte {}", dto.account_id)));
    }

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO mail.labels (id, account_id, user_id, name, color) VALUES ($1,$2,$3,$4,$5)"
    )
    .bind(id)
    .bind(dto.account_id)
    .bind(user.id)
    .bind(&dto.name)
    .bind(&dto.color)
    .execute(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("unique") {
            MailError::Conflict(format!("Label '{}' existe déjà", dto.name))
        } else {
            MailError::Database(e)
        }
    })?;

    Ok(Json(serde_json::json!({ "id": id })))
}

pub async fn delete_label(
    State(state): State<AppState>,
    user: AuthUser,
    Path(label_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let is_system: Option<bool> = sqlx::query_scalar(
        "SELECT is_system FROM mail.labels WHERE id = $1 AND user_id = $2"
    )
    .bind(label_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?;

    match is_system {
        None       => return Err(MailError::NotFound(format!("Label {label_id}"))),
        Some(true) => return Err(MailError::Validation("Les labels système ne peuvent pas être supprimés".into())),
        Some(false) => {}
    }

    sqlx::query("DELETE FROM mail.labels WHERE id = $1")
        .bind(label_id)
        .execute(&state.db)
        .await?;

    Ok(Json(serde_json::json!({ "message": "Label supprimé" })))
}

pub async fn add_thread_label(
    State(state): State<AppState>,
    user: AuthUser,
    Path((thread_id, label_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, MailError> {
    let exists: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM mail.threads WHERE id = $1 AND user_id = $2)"
    )
    .bind(thread_id)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    if !exists {
        return Err(MailError::NotFound(format!("Thread {thread_id}")));
    }

    sqlx::query(
        "INSERT INTO mail.thread_labels (thread_id, label_id) VALUES ($1,$2) ON CONFLICT DO NOTHING"
    )
    .bind(thread_id)
    .bind(label_id)
    .execute(&state.db)
    .await?;

    Ok(Json(serde_json::json!({ "message": "Label ajouté" })))
}

pub async fn remove_thread_label(
    State(state): State<AppState>,
    user: AuthUser,
    Path((thread_id, label_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, MailError> {
    let exists: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM mail.threads WHERE id = $1 AND user_id = $2)"
    )
    .bind(thread_id)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    if !exists {
        return Err(MailError::NotFound(format!("Thread {thread_id}")));
    }

    sqlx::query("DELETE FROM mail.thread_labels WHERE thread_id = $1 AND label_id = $2")
        .bind(thread_id)
        .bind(label_id)
        .execute(&state.db)
        .await?;

    Ok(Json(serde_json::json!({ "message": "Label retiré" })))
}
