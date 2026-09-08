use axum::{
    extract::{Query, State},
    Json,
};

use crate::{errors::MailError, middleware::AuthUser, state::AppState};

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
