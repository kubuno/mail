use axum::{
    extract::{Path, State},
    Json,
};
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    models::{BlockSenderDto, BlockedSender, CreateFilterDto, EmailFilter},
    state::AppState,
};

fn like(term: &str) -> String {
    format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

pub async fn list_filters(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let filters = sqlx::query_as::<_, EmailFilter>(
        r#"SELECT id, user_id, account_id, from_contains, to_contains, subject_contains, query_contains,
                  act_archive, act_mark_read, act_star, act_important, act_trash, act_spam, act_label_id,
                  position, created_at
           FROM mail.filters WHERE user_id = $1 ORDER BY position, created_at"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "filters": filters })))
}

pub async fn create_filter(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<CreateFilterDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Au moins une condition.
    let has_cond = dto.from_contains.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        || dto.to_contains.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        || dto.subject_contains.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        || dto.query_contains.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
    if !has_cond {
        return Err(MailError::Validation("Au moins une condition requise".into()));
    }
    let norm = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    let dto_existing = dto.clone();

    let id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO mail.filters
           (user_id, account_id, from_contains, to_contains, subject_contains, query_contains,
            act_archive, act_mark_read, act_star, act_important, act_trash, act_spam, act_label_id)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) RETURNING id"#,
    )
    .bind(user.id)
    .bind(dto.account_id)
    .bind(norm(dto.from_contains))
    .bind(norm(dto.to_contains))
    .bind(norm(dto.subject_contains))
    .bind(norm(dto.query_contains))
    .bind(dto.act_archive.unwrap_or(false))
    .bind(dto.act_mark_read.unwrap_or(false))
    .bind(dto.act_star.unwrap_or(false))
    .bind(dto.act_important.unwrap_or(false))
    .bind(dto.act_trash.unwrap_or(false))
    .bind(dto.act_spam.unwrap_or(false))
    .bind(dto.act_label_id)
    .fetch_one(&state.db)
    .await?;

    // Appliquer aussi aux messages DÉJÀ reçus (option « appliquer aux existants »).
    if dto_existing.apply_existing.unwrap_or(false) {
        apply_to_existing(&state, user.id, &dto_existing).await;
    }
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn apply_to_existing(state: &AppState, user_id: Uuid, dto: &CreateFilterDto) {
    // 1. Trouver les messages correspondants (insensible casse/accents).
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT m.id, m.thread_id FROM mail.messages m JOIN mail.threads t ON t.id = m.thread_id WHERE t.user_id = ",
    );
    qb.push_bind(user_id).push(" AND m.is_deleted = FALSE");
    if let Some(c) = dto.from_contains.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND (unaccent(m.from_email) ILIKE unaccent(").push_bind(like(c))
          .push(") OR unaccent(COALESCE(m.from_name,'')) ILIKE unaccent(").push_bind(like(c)).push("))");
    }
    if let Some(c) = dto.to_contains.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND unaccent(m.to_addresses::text) ILIKE unaccent(").push_bind(like(c)).push(")");
    }
    if let Some(c) = dto.subject_contains.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND unaccent(m.subject) ILIKE unaccent(").push_bind(like(c)).push(")");
    }
    if let Some(c) = dto.query_contains.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND (unaccent(m.subject) ILIKE unaccent(").push_bind(like(c))
          .push(") OR unaccent(COALESCE(m.body_text,'')) ILIKE unaccent(").push_bind(like(c)).push("))");
    }
    let rows: Vec<(Uuid, Uuid)> = match qb.build_query_as().fetch_all(&state.db).await {
        Ok(r) => r,
        Err(_) => return,
    };
    if rows.is_empty() { return; }
    let msg_ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    let mut thread_ids: Vec<Uuid> = rows.iter().map(|r| r.1).collect();
    thread_ids.sort(); thread_ids.dedup();

    if dto.act_star.unwrap_or(false) {
        let _ = sqlx::query("UPDATE mail.threads SET is_starred = TRUE WHERE id = ANY($1)").bind(&thread_ids).execute(&state.db).await;
    }
    if dto.act_important.unwrap_or(false) {
        let _ = sqlx::query("UPDATE mail.threads SET is_important = TRUE WHERE id = ANY($1)").bind(&thread_ids).execute(&state.db).await;
    }
    if dto.act_mark_read.unwrap_or(false) {
        let _ = sqlx::query("UPDATE mail.messages SET is_read = TRUE WHERE id = ANY($1)").bind(&msg_ids).execute(&state.db).await;
    }
    let fold = if dto.act_trash.unwrap_or(false) { Some("trash") } else if dto.act_spam.unwrap_or(false) { Some("spam") } else if dto.act_archive.unwrap_or(false) { Some("archive") } else { None };
    if let Some(f) = fold {
        let _ = sqlx::query("UPDATE mail.messages SET folder = $1 WHERE id = ANY($2)").bind(f).bind(&msg_ids).execute(&state.db).await;
    }
    if let Some(lid) = dto.act_label_id {
        let _ = sqlx::query("INSERT INTO mail.thread_labels (thread_id, label_id) SELECT unnest($1::uuid[]), $2 ON CONFLICT DO NOTHING")
            .bind(&thread_ids).bind(lid).execute(&state.db).await;
    }
    // Recalcul des non-lus des fils touchés.
    let _ = sqlx::query(
        "UPDATE mail.threads t SET unread_count = (SELECT COUNT(*) FROM mail.messages m WHERE m.thread_id = t.id AND m.is_read = FALSE AND m.is_deleted = FALSE) WHERE t.id = ANY($1)",
    ).bind(&thread_ids).execute(&state.db).await;
}

// ── Adresses bloquées ─────────────────────────────────────────────────────────
pub async fn list_blocked(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let blocked = sqlx::query_as::<_, BlockedSender>(
        "SELECT id, email, created_at FROM mail.blocked_senders WHERE user_id = $1 ORDER BY email",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "blocked": blocked })))
}

pub async fn block_sender(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<BlockSenderDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    let email = dto.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(MailError::Validation("Adresse e-mail invalide".into()));
    }
    sqlx::query("INSERT INTO mail.blocked_senders (user_id, email) VALUES ($1, $2) ON CONFLICT (user_id, email) DO NOTHING")
        .bind(user.id).bind(&email)
        .execute(&state.db)
        .await?;
    // Déplacer les messages existants de cet expéditeur vers le spam.
    let _ = sqlx::query(
        "UPDATE mail.messages m SET folder = 'spam'
         FROM mail.threads t WHERE m.thread_id = t.id AND t.user_id = $1 AND LOWER(m.from_email) = $2",
    ).bind(user.id).bind(&email).execute(&state.db).await;
    Ok(Json(serde_json::json!({ "message": "Expéditeur bloqué", "email": email })))
}

pub async fn unblock_sender(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let res = sqlx::query("DELETE FROM mail.blocked_senders WHERE id = $1 AND user_id = $2")
        .bind(id).bind(user.id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Bloqué {id}")));
    }
    Ok(Json(serde_json::json!({ "message": "Débloqué" })))
}

pub async fn delete_filter(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let res = sqlx::query("DELETE FROM mail.filters WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Filtre {id}")));
    }
    Ok(Json(serde_json::json!({ "message": "Filtre supprimé" })))
}
