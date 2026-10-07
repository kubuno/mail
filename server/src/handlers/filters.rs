use axum::{
    extract::{Path, State},
    Json,
};
use kubuno_db::{Backend, DbPool, DbQueryBuilder};
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    models::{BlockSenderDto, BlockedSender, CreateFilterDto, EmailFilter},
    state::AppState,
};

/// Ids per `IN (…)` statement when applying a filter to existing mail: well
/// under every engine's bind limit (SQLite 32766, MySQL/PostgreSQL 65535).
const IN_CHUNK: usize = 500;

fn like(term: &str) -> String {
    format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

/// Accent/case-insensitive "contains" of `col` against `term` (see
/// `crate::db::ci_like`). `col` is a literal written in this file.
fn push_contains(qb: &mut DbQueryBuilder, col: &str, term: &str) {
    let backend = qb.backend();
    let n = qb.bind_only(like(term));
    qb.push(crate::db::ci_like(backend, col, n));
    if backend == Backend::Sqlite {
        // SQLite's LIKE has no default escape character.
        qb.push(" ESCAPE '\\'");
    }
}

/// A JSON address column as searchable text.
fn json_text(backend: Backend, col: &'static str) -> String {
    match backend {
        Backend::Postgres => format!("{col}::text"),
        Backend::MySql => format!("CAST({col} AS CHAR)"),
        Backend::Sqlite => col.to_string(),
    }
}

pub async fn list_filters(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let filters = crate::db::query_as::<EmailFilter>(
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

    // The key is generated here: MySQL has no RETURNING to hand it back.
    let id = kubuno_db::new_id();
    crate::db::query(
        r#"INSERT INTO mail.filters
           (id, user_id, account_id, from_contains, to_contains, subject_contains, query_contains,
            act_archive, act_mark_read, act_star, act_important, act_trash, act_spam, act_label_id)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)"#,
    )
    .bind(id)
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
    .execute(&state.db)
    .await?;

    // Appliquer aussi aux messages DÉJÀ reçus (option « appliquer aux existants »).
    if dto_existing.apply_existing.unwrap_or(false) {
        apply_to_existing(&state, user.id, &dto_existing).await;
    }
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn apply_to_existing(state: &AppState, user_id: Uuid, dto: &CreateFilterDto) {
    let backend = state.db.backend();
    // 1. Trouver les messages correspondants (insensible casse/accents).
    let mut qb = DbQueryBuilder::new(
        backend,
        "SELECT m.id, m.thread_id FROM mail.messages m JOIN mail.threads t ON t.id = m.thread_id WHERE t.user_id = ",
    );
    qb.push_bind(user_id).push(" AND m.is_deleted = FALSE");
    if let Some(c) = dto.from_contains.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND (");
        push_contains(&mut qb, "m.from_email", c);
        qb.push(" OR ");
        push_contains(&mut qb, "COALESCE(m.from_name,'')", c);
        qb.push(")");
    }
    if let Some(c) = dto.to_contains.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND ");
        push_contains(&mut qb, &json_text(backend, "m.to_addresses"), c);
    }
    if let Some(c) = dto.subject_contains.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND ");
        push_contains(&mut qb, "m.subject", c);
    }
    if let Some(c) = dto.query_contains.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND (");
        push_contains(&mut qb, "m.subject", c);
        qb.push(" OR ");
        push_contains(&mut qb, "COALESCE(m.body_text,'')", c);
        qb.push(")");
    }
    let rows: Vec<(Uuid, Uuid)> = match qb.fetch_all_as(&state.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "apply_to_existing: recherche des messages");
            return;
        }
    };
    if rows.is_empty() { return; }
    let msg_ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    let mut thread_ids: Vec<Uuid> = rows.iter().map(|r| r.1).collect();
    thread_ids.sort(); thread_ids.dedup();

    if dto.act_star.unwrap_or(false) {
        update_in(&state.db, "UPDATE mail.threads SET is_starred = TRUE WHERE id", &thread_ids).await;
    }
    if dto.act_important.unwrap_or(false) {
        update_in(&state.db, "UPDATE mail.threads SET is_important = TRUE WHERE id", &thread_ids).await;
    }
    if dto.act_mark_read.unwrap_or(false) {
        update_in(&state.db, "UPDATE mail.messages SET is_read = TRUE WHERE id", &msg_ids).await;
    }
    let fold = if dto.act_trash.unwrap_or(false) { Some("trash") } else if dto.act_spam.unwrap_or(false) { Some("spam") } else if dto.act_archive.unwrap_or(false) { Some("archive") } else { None };
    if let Some(f) = fold {
        // The folder is one of the three literals just above, never caller text.
        let head = match f {
            "trash" => "UPDATE mail.messages SET folder = 'trash' WHERE id",
            "spam" => "UPDATE mail.messages SET folder = 'spam' WHERE id",
            _ => "UPDATE mail.messages SET folder = 'archive' WHERE id",
        };
        update_in(&state.db, head, &msg_ids).await;
    }
    if let Some(lid) = dto.act_label_id {
        // One multi-row INSERT-ignore per chunk (replaces `SELECT unnest($1::uuid[])`).
        for chunk in thread_ids.chunks(IN_CHUNK) {
            let mut qb = DbQueryBuilder::new(backend, "INSERT ");
            qb.push(backend.insert_ignore_prefix());
            qb.push("INTO mail.thread_labels (thread_id, label_id) VALUES ");
            for (i, tid) in chunk.iter().enumerate() {
                if i > 0 {
                    qb.push(", ");
                }
                qb.push("(").push_bind(*tid).push(", ").push_bind(lid).push(")");
            }
            qb.push(backend.on_conflict_do_nothing(&["thread_id", "label_id"]));
            if let Err(e) = qb.execute(&state.db).await {
                tracing::error!(error = %e, "apply_to_existing: libellé");
            }
        }
    }
    // Recalcul des non-lus des fils touchés.
    update_in(
        &state.db,
        "UPDATE mail.threads SET unread_count = (SELECT COUNT(*) FROM mail.messages m \
         WHERE m.thread_id = mail.threads.id AND m.is_read = FALSE AND m.is_deleted = FALSE) \
         WHERE id",
        &thread_ids,
    )
    .await;
}

/// Runs `<head> IN (…)` over `ids`, chunked. `head` is a literal of this file
/// ending with the filtered column. Best-effort, like the rest of
/// [`apply_to_existing`]: a failure is logged, not propagated.
async fn update_in(db: &DbPool, head: &'static str, ids: &[Uuid]) {
    for chunk in ids.chunks(IN_CHUNK) {
        let mut qb = DbQueryBuilder::new(db.backend(), head);
        qb.push_in(chunk.iter().copied());
        if let Err(e) = qb.execute(db).await {
            tracing::error!(error = %e, "apply_to_existing: mise à jour");
        }
    }
}

// ── Adresses bloquées ─────────────────────────────────────────────────────────
pub async fn list_blocked(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let blocked = crate::db::query_as::<BlockedSender>(
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
    let backend = state.db.backend();
    crate::db::query(format!(
        "INSERT {}INTO mail.blocked_senders (id, user_id, email) VALUES ($1, $2, $3){}",
        backend.insert_ignore_prefix(),
        backend.on_conflict_do_nothing(&["user_id", "email"]),
    ))
    .bind(kubuno_db::new_id())
    .bind(user.id)
    .bind(&email)
    .execute(&state.db)
    .await?;
    // Déplacer les messages existants de cet expéditeur vers le spam.
    if let Err(e) = crate::db::query(
        "UPDATE mail.messages SET folder = 'spam'
         WHERE LOWER(from_email) = $2
           AND thread_id IN (SELECT t.id FROM mail.threads t WHERE t.user_id = $1)",
    )
    .bind(user.id)
    .bind(&email)
    .execute(&state.db)
    .await
    {
        tracing::error!(error = %e, "block_sender: déplacement vers le spam");
    }
    Ok(Json(serde_json::json!({ "message": "Expéditeur bloqué", "email": email })))
}

pub async fn unblock_sender(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let res = crate::db::query("DELETE FROM mail.blocked_senders WHERE id = $1 AND user_id = $2")
        .bind(id).bind(user.id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Bloqué {id}")));
    }
    Ok(Json(serde_json::json!({ "message": "Débloqué" })))
}

// ── Expéditeurs dont les images distantes sont affichées ─────────────────────
// The user's own allowlist ("Always show images from X"), plus the instance-wide
// one the administrator maintains — the latter is read-only here and applies to
// everyone, the way a Workspace image allowlist set at the top level does.

pub async fn list_image_senders(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let senders = crate::db::query_as::<BlockedSender>(
        "SELECT id, email, created_at FROM mail.image_allowed_senders WHERE user_id = $1 ORDER BY email",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;

    // The instance policy is advisory to the client: unreadable means "no
    // instance allowlist", never "block everything" — a missing policy must not
    // silently widen NOR narrow what the user already allowed.
    let http = reqwest::Client::new();
    let instance = match crate::server::config::fetch(&http, &state.settings).await {
        Some(cfg) => cfg.image_allowlist,
        None => {
            tracing::warn!("Politique d'instance illisible — liste blanche d'images ignorée");
            Vec::new()
        }
    };

    Ok(Json(serde_json::json!({ "senders": senders, "instance": instance })))
}

pub async fn allow_image_sender(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<BlockSenderDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Either a full address or a whole domain ("@example.com").
    let email = dto.email.trim().to_lowercase();
    let is_domain = email.starts_with('@') && email.len() > 1 && email[1..].contains('.');
    if email.is_empty() || (!is_domain && !email.contains('@')) {
        return Err(MailError::Validation("Adresse ou domaine invalide".into()));
    }
    let backend = state.db.backend();
    crate::db::query(format!(
        "INSERT {}INTO mail.image_allowed_senders (id, user_id, email) VALUES ($1, $2, $3){}",
        backend.insert_ignore_prefix(),
        backend.on_conflict_do_nothing(&["user_id", "email"]),
    ))
    .bind(kubuno_db::new_id())
    .bind(user.id)
    .bind(&email)
    .execute(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "message": "Images autorisées", "email": email })))
}

pub async fn forget_image_sender(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let res = crate::db::query("DELETE FROM mail.image_allowed_senders WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Entrée {id}")));
    }
    Ok(Json(serde_json::json!({ "message": "Retiré" })))
}

pub async fn delete_filter(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let res = crate::db::query("DELETE FROM mail.filters WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Filtre {id}")));
    }
    Ok(Json(serde_json::json!({ "message": "Filtre supprimé" })))
}
