use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, Response, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    models::{EmailAccount, EmailMessage, SendMailDto},
    services::{
        crypto::MailCrypto,
        smtp_service::{self, SmtpConfig},
    },
    state::AppState,
};

pub async fn get_message(
    State(state): State<AppState>,
    user: AuthUser,
    Path(msg_id): Path<Uuid>,
) -> Result<Json<EmailMessage>, MailError> {
    let msg = sqlx::query_as::<_, EmailMessage>(
        r#"SELECT id, thread_id, account_id, user_id, message_id, in_reply_to,
                  imap_uid, imap_folder, from_name, from_email,
                  to_addresses, cc_addresses, bcc_addresses, reply_to,
                  subject, body_text, body_html, attachments,
                  is_read, is_starred, is_deleted, folder, label_ids,
                  sent_at, received_at, created_at, spam_score, list_unsubscribe
           FROM mail.messages WHERE id = $1 AND user_id = $2"#,
    )
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Message {msg_id}")))?;

    if !msg.is_read {
        let _ = sqlx::query("UPDATE mail.messages SET is_read = TRUE WHERE id = $1")
            .bind(msg_id)
            .execute(&state.db)
            .await;

        let _ = sqlx::query(
            "UPDATE mail.threads SET unread_count = GREATEST(0, unread_count - 1) WHERE id = $1"
        )
        .bind(msg.thread_id)
        .execute(&state.db)
        .await;
    }

    Ok(Json(msg))
}

pub async fn send_message(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<SendMailDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    if dto.to_addresses.is_empty() {
        return Err(MailError::Validation("Au moins un destinataire requis".into()));
    }
    if dto.subject.trim().is_empty() {
        return Err(MailError::Validation("Sujet requis".into()));
    }

    // Envoi PROGRAMMÉ : on stocke comme brouillon planifié ; le worker scheduler
    // l'enverra quand l'heure sera venue. (Voir workers::scheduler_worker.)
    if let Some(when) = dto.scheduled_at {
        sqlx::query(
            r#"INSERT INTO mail.drafts
               (id, account_id, user_id, to_addresses, cc_addresses, bcc_addresses, subject, body_html, reply_to_id, scheduled_at)
               VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        )
        .bind(dto.account_id)
        .bind(user.id)
        .bind(serde_json::to_value(&dto.to_addresses).unwrap_or_else(|_| serde_json::json!([])))
        .bind(serde_json::to_value(dto.cc_addresses.clone().unwrap_or_default()).unwrap_or_else(|_| serde_json::json!([])))
        .bind(serde_json::to_value(dto.bcc_addresses.clone().unwrap_or_default()).unwrap_or_else(|_| serde_json::json!([])))
        .bind(&dto.subject)
        .bind(ammonia::clean(&dto.body_html))
        .bind(dto.reply_to_id)
        .bind(when)
        .execute(&state.db)
        .await?;
        if let Some(draft_id) = dto.draft_id {
            let _ = sqlx::query("DELETE FROM mail.drafts WHERE id = $1 AND user_id = $2")
                .bind(draft_id).bind(user.id).execute(&state.db).await;
        }
        return Ok(Json(serde_json::json!({ "message": "Envoi programmé", "scheduled_at": when })));
    }

    let account = sqlx::query_as::<_, EmailAccount>(
        r#"SELECT id, user_id, name, email_address,
                  incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  smtp_host, smtp_port, smtp_security, smtp_username,
                  is_default, is_active, last_sync_at, last_error,
                  created_at, updated_at
           FROM mail.accounts WHERE id = $1 AND user_id = $2"#,
    )
    .bind(dto.account_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Compte {}", dto.account_id)))?;

    // Récupérer le mot de passe SMTP chiffré
    let (smtp_pass_enc, smtp_nonce): (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT smtp_password, smtp_password_nonce FROM mail.accounts WHERE id = $1"
    )
    .bind(account.id)
    .fetch_one(&state.db)
    .await?;

    let crypto = MailCrypto::new(&state.settings.mail.encryption_key)
        .map_err(|_| MailError::Crypto)?;
    let smtp_pass = crypto.decrypt(&smtp_pass_enc, &smtp_nonce)
        .map_err(|_| MailError::Crypto)?;

    let smtp_cfg = SmtpConfig {
        host:       account.smtp_host.clone(),
        port:       account.smtp_port as u16,
        security:   account.smtp_security.clone(),
        username:   account.smtp_username.clone(),
        password:   smtp_pass,
        from_name:  account.name.clone(),
        from_email: account.email_address.clone(),
    };

    let body_html = ammonia::clean(&dto.body_html);

    smtp_service::send_message(&smtp_cfg, &dto, &dto.subject, &body_html)
        .await
        .map_err(|e| MailError::Smtp(e.to_string()))?;

    if let Some(draft_id) = dto.draft_id {
        let _ = sqlx::query("DELETE FROM mail.drafts WHERE id = $1 AND user_id = $2")
            .bind(draft_id)
            .bind(user.id)
            .execute(&state.db)
            .await;
    }

    // Feed the recipient-autocomplete index — people the user WRITES TO rank
    // highest (weight 3 vs 1 for synced mail).
    let mut sent_to: Vec<(String, Option<String>)> = Vec::new();
    for a in dto.to_addresses.iter()
        .chain(dto.cc_addresses.as_deref().unwrap_or(&[]))
        .chain(dto.bcc_addresses.as_deref().unwrap_or(&[]))
    {
        sent_to.push((a.email.clone(), a.name.clone()));
    }
    crate::services::address_index::upsert(&state.db, user.id, &sent_to, 3).await;

    Ok(Json(serde_json::json!({ "message": "Message envoyé" })))
}

#[derive(serde::Deserialize)]
pub struct SuggestQuery {
    pub q: String,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct AddressSuggestion {
    pub email: String,
    pub name:  Option<String>,
}

/// Recipient autocompletion: search the per-user address index (kept up to date
/// by the sync worker and outgoing sends — no scan of mail.messages).
pub async fn suggest_addresses(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<SuggestQuery>,
) -> Result<Json<Vec<AddressSuggestion>>, MailError> {
    let term = q.q.trim().to_lowercase();
    if term.is_empty() {
        return Ok(Json(vec![]));
    }
    let rows = sqlx::query_as::<_, AddressSuggestion>(
        r#"SELECT email, name FROM mail.address_index
           WHERE user_id = $1
             AND (email LIKE $2 || '%' OR email LIKE '%' || $2 || '%'
                  OR LOWER(COALESCE(name, '')) LIKE '%' || $2 || '%')
           ORDER BY (email LIKE $2 || '%') DESC, use_count DESC, last_used_at DESC
           LIMIT 8"#,
    )
    .bind(user.id)
    .bind(&term)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

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

pub async fn download_attachment(
    State(state): State<AppState>,
    user: AuthUser,
    Path((msg_id, index)): Path<(Uuid, usize)>,
) -> Result<Response<Body>, MailError> {
    let row = sqlx::query_as::<_, (serde_json::Value,)>(
        "SELECT attachments FROM mail.messages WHERE id = $1 AND user_id = $2",
    )
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Message {msg_id}")))?;

    let attachments: Vec<serde_json::Value> = serde_json::from_value(row.0).unwrap_or_default();
    let att = attachments
        .get(index)
        .ok_or_else(|| MailError::NotFound(format!("Pièce jointe {index}")))?;

    let storage_path = att
        .get("storage_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| MailError::NotFound("storage_path manquant".into()))?;

    let mime_type = att
        .get("mime")
        .and_then(|v| v.as_str())
        .unwrap_or("application/octet-stream")
        .to_string();

    let name = att
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("attachment")
        .to_string();

    let data = tokio::fs::read(storage_path)
        .await
        .map_err(|e| MailError::Internal(anyhow::anyhow!("Lecture fichier: {e}")))?;

    let disposition = format!("inline; filename=\"{}\"", name.replace('"', "\\\""));

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from(data))
        .map_err(|e| MailError::Internal(anyhow::anyhow!("Build response: {e}")))?;

    Ok(response)
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
