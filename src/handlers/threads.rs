use axum::{
    extract::{Path, Query, State},
    Json,
};
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    models::{EmailMessage, SnoozeDto, Thread, ThreadListQuery},
    state::AppState,
};
use chrono::{DateTime, Utc};

/// Row shape for Bayesian spam training over a thread's messages.
type SpamTrainRow = (Uuid, String, Option<String>, String, Option<i16>);
/// Row shape for the subscriptions aggregation query.
type SubscriptionRow = (String, Option<String>, Option<String>, i64, DateTime<Utc>);

pub async fn list_threads(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<ThreadListQuery>,
) -> Result<Json<serde_json::Value>, MailError> {
    let limit = q.limit.unwrap_or(50).min(100);
    let folder = q.folder.as_deref().unwrap_or("inbox").to_string();

    let threads: Vec<Thread> = if let Some(raw) = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        // RECHERCHE : requête dynamique parsant les opérateurs Gmail (from:/to:/
        // subject:/has:attachment/newer_than:/in:/is:unread|starred/-exclusion/mots).
        // Cherche dans TOUS les dossiers par défaut (sauf `in:` explicite).
        let c = parse_search(raw);
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "SELECT DISTINCT t.id, t.account_id, t.user_id, t.subject, \
                    t.message_count, t.unread_count, t.has_attachments, \
                    t.is_starred, t.is_important, t.snippet, t.last_sender_name, t.last_sender_email, \
                    t.last_message_at, t.created_at \
             FROM mail.threads t JOIN mail.messages m ON m.thread_id = t.id \
             WHERE t.user_id = ",
        );
        qb.push_bind(user.id);
        qb.push(" AND m.is_deleted = FALSE");
        if let Some(acc) = q.account_id {
            qb.push(" AND t.account_id = ").push_bind(acc);
        }
        if let Some(folder) = c.folder.as_deref() {
            if folder != "all" { qb.push(" AND m.folder = ").push_bind(folder.to_string()); }
        }
        if c.unread          { qb.push(" AND m.is_read = FALSE"); }
        if c.starred         { qb.push(" AND t.is_starred = TRUE"); }
        if c.has_attachment  { qb.push(" AND t.has_attachments = TRUE"); }
        if let Some(days) = c.newer_than_days {
            qb.push(" AND m.received_at > NOW() - make_interval(days => ").push_bind(days).push(")");
        }
        // Comparaisons INSENSIBLES à la casse ET aux accents : unaccent(col) ILIKE unaccent(motif)
        // → « OVH »=« ovh », « ete »=« été ».
        for term in &c.from {
            qb.push(" AND (unaccent(m.from_email) ILIKE unaccent(").push_bind(like(term))
              .push(") OR unaccent(COALESCE(m.from_name,'')) ILIKE unaccent(").push_bind(like(term)).push("))");
        }
        for term in &c.to {
            qb.push(" AND unaccent(m.to_addresses::text) ILIKE unaccent(").push_bind(like(term)).push(")");
        }
        for term in &c.subject {
            qb.push(" AND unaccent(m.subject) ILIKE unaccent(").push_bind(like(term)).push(")");
        }
        for term in &c.words {
            qb.push(" AND (unaccent(m.subject) ILIKE unaccent(").push_bind(like(term))
              .push(") OR unaccent(COALESCE(m.body_text,'')) ILIKE unaccent(").push_bind(like(term))
              .push(") OR unaccent(m.from_email) ILIKE unaccent(").push_bind(like(term))
              .push(") OR unaccent(COALESCE(m.from_name,'')) ILIKE unaccent(").push_bind(like(term)).push("))");
        }
        for term in &c.exclude {
            qb.push(" AND NOT (unaccent(m.subject) ILIKE unaccent(").push_bind(like(term))
              .push(") OR unaccent(COALESCE(m.body_text,'')) ILIKE unaccent(").push_bind(like(term)).push("))");
        }
        if let Some(before) = q.before {
            qb.push(" AND t.last_message_at < ").push_bind(before);
        }
        qb.push(" ORDER BY t.last_message_at DESC LIMIT ").push_bind(limit);
        qb.build_query_as::<Thread>().fetch_all(&state.db).await?
    } else if q.important == Some(true) {
        sqlx::query_as::<_, Thread>(
            r#"SELECT t.id, t.account_id, t.user_id, t.subject,
                      t.message_count, t.unread_count, t.has_attachments,
                      t.is_starred, t.is_important, t.snippet,
                      t.last_sender_name, t.last_sender_email,
                      t.last_message_at, t.created_at
               FROM mail.threads t
               WHERE t.user_id = $1
                 AND t.is_important = TRUE
                 AND ($2::uuid IS NULL OR t.account_id = $2)
                 AND ($3::timestamptz IS NULL OR t.last_message_at < $3)
               ORDER BY t.last_message_at DESC
               LIMIT $4"#,
        )
        .bind(user.id)
        .bind(q.account_id)
        .bind(q.before)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else if q.snoozed == Some(true) {
        // En attente : fils dont le réveil est dans le futur.
        sqlx::query_as::<_, Thread>(
            r#"SELECT t.id, t.account_id, t.user_id, t.subject,
                      t.message_count, t.unread_count, t.has_attachments,
                      t.is_starred, t.is_important, t.snippet,
                      t.last_sender_name, t.last_sender_email,
                      t.last_message_at, t.created_at
               FROM mail.threads t
               WHERE t.user_id = $1
                 AND t.snoozed_until > NOW()
                 AND ($2::uuid IS NULL OR t.account_id = $2)
                 AND ($3::timestamptz IS NULL OR t.snoozed_until < $3)
               ORDER BY t.snoozed_until ASC
               LIMIT $4"#,
        )
        .bind(user.id)
        .bind(q.account_id)
        .bind(q.before)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else if let Some(label_id) = q.label_id {
        sqlx::query_as::<_, Thread>(
            r#"SELECT t.id, t.account_id, t.user_id, t.subject,
                      t.message_count, t.unread_count, t.has_attachments,
                      t.is_starred, t.is_important, t.snippet,
                      t.last_sender_name, t.last_sender_email,
                      t.last_message_at, t.created_at
               FROM mail.threads t
               JOIN mail.thread_labels tl ON tl.thread_id = t.id
               WHERE t.user_id = $1
                 AND tl.label_id = $2
                 AND ($3::timestamptz IS NULL OR t.last_message_at < $3)
               ORDER BY t.last_message_at DESC
               LIMIT $4"#,
        )
        .bind(user.id)
        .bind(label_id)
        .bind(q.before)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else if q.starred == Some(true) {
        sqlx::query_as::<_, Thread>(
            r#"SELECT t.id, t.account_id, t.user_id, t.subject,
                      t.message_count, t.unread_count, t.has_attachments,
                      t.is_starred, t.is_important, t.snippet,
                      t.last_sender_name, t.last_sender_email,
                      t.last_message_at, t.created_at
               FROM mail.threads t
               WHERE t.user_id = $1
                 AND t.is_starred = TRUE
                 AND ($2::uuid IS NULL OR t.account_id = $2)
                 AND ($3::timestamptz IS NULL OR t.last_message_at < $3)
               ORDER BY t.last_message_at DESC
               LIMIT $4"#,
        )
        .bind(user.id)
        .bind(q.account_id)
        .bind(q.before)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<_, Thread>(
            r#"SELECT DISTINCT ON (t.id, t.last_message_at) t.id, t.account_id, t.user_id, t.subject,
                      t.message_count, t.unread_count, t.has_attachments,
                      t.is_starred, t.is_important, t.snippet,
                      t.last_sender_name, t.last_sender_email,
                      t.last_message_at, t.created_at
               FROM mail.threads t
               JOIN mail.messages m ON m.thread_id = t.id
               WHERE t.user_id = $1
                 AND ($2 = 'all' OR m.folder = $2)
                 AND m.is_deleted = FALSE
                 -- les fils en attente (snooze) disparaissent de la boîte jusqu'à leur réveil
                 AND ($2 <> 'inbox' OR t.snoozed_until IS NULL OR t.snoozed_until <= NOW())
                 -- les fils ignorés (mute) ne reviennent jamais dans la boîte
                 AND ($2 <> 'inbox' OR NOT t.is_muted)
                 AND ($3::uuid IS NULL OR t.account_id = $3)
                 AND ($4::timestamptz IS NULL OR t.last_message_at < $4)
               ORDER BY t.last_message_at DESC
               LIMIT $5"#,
        )
        .bind(user.id)
        .bind(&folder)
        .bind(q.account_id)
        .bind(q.before)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    };

    let has_more = threads.len() as i64 == limit;
    let cursor   = threads.last().map(|t| t.last_message_at);

    Ok(Json(serde_json::json!({
        "threads":  threads,
        "has_more": has_more,
        "cursor":   cursor,
    })))
}

pub async fn get_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let thread = sqlx::query_as::<_, Thread>(
        r#"SELECT id, account_id, user_id, subject,
                  message_count, unread_count, has_attachments,
                  is_starred, is_important, snippet,
                  last_sender_name, last_sender_email,
                  last_message_at, created_at
           FROM mail.threads WHERE id = $1 AND user_id = $2"#,
    )
    .bind(thread_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Thread {thread_id}")))?;

    let messages = sqlx::query_as::<_, EmailMessage>(
        r#"SELECT id, thread_id, account_id, user_id, message_id, in_reply_to,
                  imap_uid, imap_folder, from_name, from_email,
                  to_addresses, cc_addresses, bcc_addresses, reply_to,
                  subject, body_text, body_html, attachments,
                  is_read, is_starred, is_deleted, folder, label_ids,
                  sent_at, received_at, created_at, spam_score, list_unsubscribe
           FROM mail.messages
           WHERE thread_id = $1 AND is_deleted = FALSE
           ORDER BY received_at ASC"#,
    )
    .bind(thread_id)
    .fetch_all(&state.db)
    .await?;

    // Marquer comme lus
    sqlx::query(
        "UPDATE mail.messages SET is_read = TRUE WHERE thread_id = $1 AND user_id = $2 AND is_read = FALSE"
    )
    .bind(thread_id)
    .bind(user.id)
    .execute(&state.db)
    .await?;

    sqlx::query("UPDATE mail.threads SET unread_count = 0 WHERE id = $1")
        .bind(thread_id)
        .execute(&state.db)
        .await?;

    Ok(Json(serde_json::json!({ "thread": thread, "messages": messages })))
}

pub async fn star_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let row = sqlx::query_scalar::<_, bool>(
        "UPDATE mail.threads SET is_starred = NOT is_starred WHERE id = $1 AND user_id = $2 RETURNING is_starred"
    )
    .bind(thread_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Thread {thread_id}")))?;

    Ok(Json(serde_json::json!({ "is_starred": row })))
}

pub async fn move_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, MailError> {
    let folder = body["folder"]
        .as_str()
        .ok_or_else(|| MailError::Validation("folder requis".into()))?
        .to_string();

    // « archive » = hors boîte de réception mais conservé (visible dans « Tous les messages »).
    if !["inbox", "sent", "spam", "trash", "archive"].contains(&folder.as_str()) {
        return Err(MailError::Validation(format!("Dossier invalide: {folder}")));
    }

    sqlx::query("UPDATE mail.messages SET folder = $1 WHERE thread_id = $2 AND user_id = $3")
        .bind(&folder)
        .bind(thread_id)
        .bind(user.id)
        .execute(&state.db)
        .await?;

    // Feedback bayésien : marquer comme spam (folder='spam') ou « pas spam »
    // (folder='inbox') entraîne le classifieur sur les messages du fil.
    if folder == "spam" || folder == "inbox" {
        let is_spam = folder == "spam";
        let msgs: Vec<SpamTrainRow> = sqlx::query_as(
            "SELECT id, subject, body_text, from_email, spam_trained
             FROM mail.messages WHERE thread_id = $1 AND user_id = $2",
        )
        .bind(thread_id)
        .bind(user.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        for (id, subject, body, from_email, prev) in msgs {
            match crate::services::spam_classifier::learn_message(
                &state.db, user.id, &subject, body.as_deref(), &from_email, is_spam, prev,
            ).await {
                Ok(guard) => {
                    let _ = sqlx::query("UPDATE mail.messages SET spam_trained = $1, spam_score = NULL WHERE id = $2")
                        .bind(guard).bind(id).execute(&state.db).await;
                }
                Err(e) => tracing::warn!(error = %e, "Entraînement spam (feedback) échoué"),
            }
        }
    }

    Ok(Json(serde_json::json!({ "message": "Thread déplacé" })))
}

pub async fn delete_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
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
        "UPDATE mail.messages SET is_deleted = TRUE, folder = 'trash' WHERE thread_id = $1 AND user_id = $2"
    )
    .bind(thread_id)
    .bind(user.id)
    .execute(&state.db)
    .await?;

    Ok(Json(serde_json::json!({ "message": "Thread supprimé" })))
}

// ── Important / En attente (snooze) / Abonnements ────────────────────────────
pub async fn important_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let row = sqlx::query_scalar::<_, bool>(
        "UPDATE mail.threads SET is_important = NOT is_important WHERE id = $1 AND user_id = $2 RETURNING is_important",
    )
    .bind(thread_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Thread {thread_id}")))?;
    Ok(Json(serde_json::json!({ "is_important": row })))
}

pub async fn mute_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let row = sqlx::query_scalar::<_, bool>(
        "UPDATE mail.threads SET is_muted = NOT is_muted WHERE id = $1 AND user_id = $2 RETURNING is_muted",
    )
    .bind(thread_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Thread {thread_id}")))?;
    Ok(Json(serde_json::json!({ "is_muted": row })))
}

pub async fn snooze_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
    Json(dto): Json<SnoozeDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    let res = sqlx::query(
        "UPDATE mail.threads SET snoozed_until = $1 WHERE id = $2 AND user_id = $3",
    )
    .bind(dto.until)
    .bind(thread_id)
    .bind(user.id)
    .execute(&state.db)
    .await?;
    if res.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Thread {thread_id}")));
    }
    Ok(Json(serde_json::json!({ "snoozed_until": dto.until })))
}

// Marquer TOUT un fil comme lu / non lu (action groupée façon Gmail).
pub async fn read_thread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(thread_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, MailError> {
    let is_read = body["is_read"].as_bool().unwrap_or(true);
    sqlx::query("UPDATE mail.messages SET is_read = $1 WHERE thread_id = $2 AND user_id = $3")
        .bind(is_read).bind(thread_id).bind(user.id)
        .execute(&state.db).await?;
    let unread: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.messages WHERE thread_id = $1 AND is_read = FALSE AND is_deleted = FALSE",
    )
    .bind(thread_id).fetch_one(&state.db).await.unwrap_or(0);
    sqlx::query("UPDATE mail.threads SET unread_count = $1 WHERE id = $2")
        .bind(unread as i32).bind(thread_id).execute(&state.db).await?;
    Ok(Json(serde_json::json!({ "unread_count": unread })))
}

pub async fn subscriptions(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let rows: Vec<SubscriptionRow> = sqlx::query_as(
        r#"SELECT m.from_email,
                  MAX(m.from_name)                                                      AS from_name,
                  (ARRAY_AGG(m.list_unsubscribe ORDER BY m.received_at DESC))[1]         AS list_unsubscribe,
                  COUNT(*)                                                              AS cnt,
                  MAX(m.received_at)                                                    AS last_at
           FROM mail.messages m
           JOIN mail.threads t ON t.id = m.thread_id
           WHERE t.user_id = $1 AND m.list_unsubscribe IS NOT NULL AND m.is_deleted = FALSE
           GROUP BY m.from_email
           ORDER BY cnt DESC
           LIMIT 500"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let subs: Vec<serde_json::Value> = rows.into_iter().map(|(email, name, unsub, cnt, last)| {
        serde_json::json!({
            "from_email":       email,
            "from_name":        name,
            "list_unsubscribe": unsub,
            "count":            cnt,
            "last_at":          last,
        })
    }).collect();
    Ok(Json(serde_json::json!({ "subscriptions": subs })))
}

// ── Parsing de la requête de recherche (style Gmail) ─────────────────────────
struct SearchCriteria {
    from:            Vec<String>,
    to:              Vec<String>,
    subject:         Vec<String>,
    words:           Vec<String>,
    exclude:         Vec<String>,
    folder:          Option<String>,
    newer_than_days: Option<i32>,
    unread:          bool,
    starred:         bool,
    has_attachment:  bool,
}

fn parse_search(raw: &str) -> SearchCriteria {
    let mut c = SearchCriteria {
        from: vec![], to: vec![], subject: vec![], words: vec![], exclude: vec![],
        folder: None, newer_than_days: None, unread: false, starred: false, has_attachment: false,
    };
    for tok in raw.split_whitespace() {
        if let Some(v) = tok.strip_prefix("from:") {
            if !v.is_empty() { c.from.push(v.to_string()); }
        } else if let Some(v) = tok.strip_prefix("to:") {
            if !v.is_empty() { c.to.push(v.to_string()); }
        } else if let Some(v) = tok.strip_prefix("subject:") {
            if !v.is_empty() { c.subject.push(v.to_string()); }
        } else if let Some(v) = tok.strip_prefix("in:") {
            if !v.is_empty() { c.folder = Some(v.to_string()); }
        } else if tok == "has:attachment" {
            c.has_attachment = true;
        } else if tok == "is:unread" {
            c.unread = true;
        } else if tok == "is:starred" {
            c.starred = true;
        } else if let Some(v) = tok.strip_prefix("newer_than:") {
            c.newer_than_days = Some(range_to_days(v));
        } else if tok.starts_with("size:") || tok.starts_with("date:") || tok.starts_with("older_than:") {
            // non géré (taille/date exacte) — ignoré silencieusement
        } else if let Some(w) = tok.strip_prefix('-') {
            if !w.is_empty() { c.exclude.push(w.to_string()); }
        } else {
            c.words.push(tok.to_string());
        }
    }
    c
}

fn range_to_days(v: &str) -> i32 {
    match v {
        "1d" => 1, "3d" => 3, "1w" => 7, "2w" => 14, "1m" => 30, "6m" => 180, "1y" => 365,
        _ => 7,
    }
}

/// Échappe les jokers SQL LIKE et entoure de `%` pour une recherche « contient ».
fn like(term: &str) -> String {
    format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

// ── Compteurs pour la barre latérale (non-lus par dossier, brouillons, libellés) ──
pub async fn counts(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    // Non-lus par dossier
    let unread_rows: Vec<(String, i64)> = sqlx::query_as(
        r#"SELECT m.folder, COUNT(*)
           FROM mail.messages m
           JOIN mail.threads t ON t.id = m.thread_id
           WHERE t.user_id = $1 AND m.is_read = FALSE AND m.is_deleted = FALSE
           GROUP BY m.folder"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    let mut unread = serde_json::Map::new();
    for (folder, n) in unread_rows {
        unread.insert(folder, serde_json::json!(n));
    }

    // Total fils par dossier (pour les badges « tous »)
    let total_rows: Vec<(String, i64)> = sqlx::query_as(
        r#"SELECT m.folder, COUNT(DISTINCT t.id)
           FROM mail.messages m
           JOIN mail.threads t ON t.id = m.thread_id
           WHERE t.user_id = $1 AND m.is_deleted = FALSE
           GROUP BY m.folder"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    let mut total = serde_json::Map::new();
    for (folder, n) in total_rows {
        total.insert(folder, serde_json::json!(n));
    }

    let drafts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mail.drafts WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let starred: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.threads WHERE user_id = $1 AND is_starred = TRUE",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let important: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.threads WHERE user_id = $1 AND is_important = TRUE",
    )
    .bind(user.id).fetch_one(&state.db).await.unwrap_or(0);

    let snoozed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.threads WHERE user_id = $1 AND snoozed_until > NOW()",
    )
    .bind(user.id).fetch_one(&state.db).await.unwrap_or(0);

    let scheduled: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.drafts WHERE user_id = $1 AND scheduled_at IS NOT NULL",
    )
    .bind(user.id).fetch_one(&state.db).await.unwrap_or(0);

    let label_rows: Vec<(Uuid, i64)> = sqlx::query_as(
        r#"SELECT tl.label_id, COUNT(DISTINCT tl.thread_id)
           FROM mail.thread_labels tl
           JOIN mail.threads t ON t.id = tl.thread_id
           WHERE t.user_id = $1
           GROUP BY tl.label_id"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let mut labels = serde_json::Map::new();
    for (id, n) in label_rows {
        labels.insert(id.to_string(), serde_json::json!(n));
    }

    Ok(Json(serde_json::json!({
        "unread":    unread,
        "total":     total,
        "drafts":    drafts,
        "starred":   starred,
        "important": important,
        "snoozed":   snoozed,
        "scheduled": scheduled,
        "labels":    labels,
    })))
}
