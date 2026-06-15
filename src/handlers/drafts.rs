use axum::{
    extract::{Path, State},
    Json,
};
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    models::{Draft, SaveDraftDto},
    state::AppState,
};

pub async fn list_drafts(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let drafts = sqlx::query_as::<_, Draft>(
        r#"SELECT id, account_id, user_id, to_addresses, cc_addresses, bcc_addresses,
                  subject, body_html, reply_to_id, attachments, created_at, updated_at
           FROM mail.drafts WHERE user_id = $1 ORDER BY updated_at DESC"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(serde_json::json!({ "drafts": drafts })))
}

/// Brouillons programmés (« Planifié »).
pub async fn scheduled_drafts(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let rows: Vec<(Uuid, serde_json::Value, String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        r#"SELECT id, to_addresses, subject, body_html, scheduled_at
           FROM mail.drafts
           WHERE user_id = $1 AND scheduled_at IS NOT NULL
           ORDER BY scheduled_at ASC"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;

    let scheduled: Vec<serde_json::Value> = rows.into_iter().map(|(id, to, subject, body_html, at)| {
        serde_json::json!({
            "id": id, "to_addresses": to, "subject": subject,
            "body_html": body_html, "scheduled_at": at,
        })
    }).collect();
    Ok(Json(serde_json::json!({ "scheduled": scheduled })))
}

pub async fn save_draft(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<SaveDraftDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    let id = Uuid::new_v4();
    let to  = serde_json::to_value(&dto.to_addresses).unwrap_or(serde_json::json!([]));
    let cc  = serde_json::to_value(&dto.cc_addresses).unwrap_or(serde_json::json!([]));
    let bcc = serde_json::to_value(&dto.bcc_addresses).unwrap_or(serde_json::json!([]));
    let subject   = dto.subject.unwrap_or_default();
    let body_html = dto.body_html.unwrap_or_default();

    sqlx::query(
        r#"INSERT INTO mail.drafts (id, account_id, user_id, to_addresses, cc_addresses, bcc_addresses, subject, body_html, reply_to_id)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
    )
    .bind(id)
    .bind(dto.account_id)
    .bind(user.id)
    .bind(to)
    .bind(cc)
    .bind(bcc)
    .bind(&subject)
    .bind(&body_html)
    .bind(dto.reply_to_id)
    .execute(&state.db)
    .await?;

    Ok(Json(serde_json::json!({ "id": id })))
}

pub async fn update_draft(
    State(state): State<AppState>,
    user: AuthUser,
    Path(draft_id): Path<Uuid>,
    Json(dto): Json<SaveDraftDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    let to  = serde_json::to_value(&dto.to_addresses).unwrap_or(serde_json::json!([]));
    let cc  = serde_json::to_value(&dto.cc_addresses).unwrap_or(serde_json::json!([]));
    let bcc = serde_json::to_value(&dto.bcc_addresses).unwrap_or(serde_json::json!([]));
    let subject   = dto.subject.unwrap_or_default();
    let body_html = dto.body_html.unwrap_or_default();

    let result = sqlx::query(
        r#"UPDATE mail.drafts SET to_addresses=$1, cc_addresses=$2, bcc_addresses=$3,
                  subject=$4, body_html=$5, reply_to_id=$6
           WHERE id=$7 AND user_id=$8"#,
    )
    .bind(to)
    .bind(cc)
    .bind(bcc)
    .bind(&subject)
    .bind(&body_html)
    .bind(dto.reply_to_id)
    .bind(draft_id)
    .bind(user.id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Brouillon {draft_id}")));
    }
    Ok(Json(serde_json::json!({ "message": "Brouillon enregistré" })))
}

pub async fn delete_draft(
    State(state): State<AppState>,
    user: AuthUser,
    Path(draft_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let result = sqlx::query("DELETE FROM mail.drafts WHERE id = $1 AND user_id = $2")
        .bind(draft_id)
        .bind(user.id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Brouillon {draft_id}")));
    }
    Ok(Json(serde_json::json!({ "message": "Brouillon supprimé" })))
}
