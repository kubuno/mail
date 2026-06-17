//! Spam classifier endpoints: model stats, settings, and full retraining.

use axum::{extract::State, Json};
use uuid::Uuid;

use crate::{errors::MailError, middleware::AuthUser, services::spam_classifier, state::AppState};

/// Cap per class when rebuilding the model from the existing corpus, to bound
/// runtime on large mailboxes. The most recent messages are the most relevant.
const REBUILD_CAP: i64 = 3000;

/// GET /spam/stats — corpus size, distinct tokens, and auto-classify settings.
pub async fn stats(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let row: Option<(i32, i32, bool, f32)> = sqlx::query_as(
        "SELECT spam_messages, ham_messages, auto_classify, threshold
         FROM mail.spam_stats WHERE user_id = $1",
    )
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?;

    let tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mail.spam_tokens WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let (spam, ham, auto, threshold) = row.unwrap_or((0, 0, true, 0.95));

    Ok(Json(serde_json::json!({
        "spam_messages":  spam,
        "ham_messages":   ham,
        "distinct_tokens": tokens,
        "auto_classify":  auto,
        "threshold":      threshold,
    })))
}

/// PATCH /spam/settings — update auto-classify flag and/or threshold.
pub async fn update_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, MailError> {
    let auto = body["auto_classify"].as_bool();
    let threshold = body["threshold"].as_f64().map(|t| t.clamp(0.5, 0.999) as f32);

    // Ensure a row exists, then apply only the provided fields.
    sqlx::query(
        "INSERT INTO mail.spam_stats (user_id) VALUES ($1) ON CONFLICT (user_id) DO NOTHING",
    )
    .bind(user.id)
    .execute(&state.db)
    .await?;

    if let Some(a) = auto {
        sqlx::query("UPDATE mail.spam_stats SET auto_classify = $1, updated_at = NOW() WHERE user_id = $2")
            .bind(a)
            .bind(user.id)
            .execute(&state.db)
            .await?;
    }
    if let Some(t) = threshold {
        sqlx::query("UPDATE mail.spam_stats SET threshold = $1, updated_at = NOW() WHERE user_id = $2")
            .bind(t)
            .bind(user.id)
            .execute(&state.db)
            .await?;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /spam/train — rebuild the model from scratch using the existing corpus:
/// messages currently in `spam` are spam examples, read messages in `inbox` are
/// ham examples. Idempotent (settings are preserved, counts are reset first).
pub async fn train(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    // Reset the model but keep the user's auto_classify / threshold settings.
    sqlx::query("DELETE FROM mail.spam_tokens WHERE user_id = $1")
        .bind(user.id)
        .execute(&state.db)
        .await?;
    sqlx::query(
        "INSERT INTO mail.spam_stats (user_id, spam_messages, ham_messages)
         VALUES ($1, 0, 0)
         ON CONFLICT (user_id) DO UPDATE SET spam_messages = 0, ham_messages = 0, updated_at = NOW()",
    )
    .bind(user.id)
    .execute(&state.db)
    .await?;
    sqlx::query("UPDATE mail.messages SET spam_trained = NULL, spam_score = NULL WHERE user_id = $1")
        .bind(user.id)
        .execute(&state.db)
        .await?;

    let spam = train_class(&state, user.id, "spam", true).await?;
    let ham = train_class(&state, user.id, "inbox", false).await?;

    if spam >= REBUILD_CAP || ham >= REBUILD_CAP {
        tracing::info!(spam, ham, cap = REBUILD_CAP, "Réentraînement spam plafonné");
    }

    Ok(Json(serde_json::json!({
        "spam_messages": spam,
        "ham_messages":  ham,
        "capped":        spam >= REBUILD_CAP || ham >= REBUILD_CAP,
    })))
}

/// Train every (capped) message of a folder as the given class. Returns count.
async fn train_class(
    state: &AppState,
    user_id: Uuid,
    folder: &str,
    is_spam: bool,
) -> Result<i64, MailError> {
    // Ham examples are limited to messages the user has actually read, which is
    // a reasonable proxy for "wanted mail".
    let read_clause = if is_spam { "" } else { " AND is_read = TRUE" };
    let sql = format!(
        "SELECT id, subject, body_text, from_email
         FROM mail.messages
         WHERE user_id = $1 AND folder = $2 AND is_deleted = FALSE{read_clause}
         ORDER BY received_at DESC
         LIMIT $3",
    );
    let msgs: Vec<(Uuid, String, Option<String>, String)> = sqlx::query_as(&sql)
        .bind(user_id)
        .bind(folder)
        .bind(REBUILD_CAP)
        .fetch_all(&state.db)
        .await?;

    let mut n = 0i64;
    for (id, subject, body, from_email) in msgs {
        match spam_classifier::learn_message(
            &state.db, user_id, &subject, body.as_deref(), &from_email, is_spam, None,
        )
        .await
        {
            Ok(guard) => {
                let _ = sqlx::query("UPDATE mail.messages SET spam_trained = $1 WHERE id = $2")
                    .bind(guard)
                    .bind(id)
                    .execute(&state.db)
                    .await;
                n += 1;
            }
            Err(e) => tracing::warn!(error = %e, "Entraînement spam (rebuild) échoué"),
        }
    }
    Ok(n)
}
