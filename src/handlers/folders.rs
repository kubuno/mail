//! The account's own IMAP folders — the ones that are not inbox/sent/drafts/
//! spam/trash. They are synced like the rest; this exposes the list so the
//! sidebar can offer them.

use axum::{extract::State, Json};

use crate::{errors::MailError, middleware::AuthUser, state::AppState};

/// One custom folder and how much mail it holds.
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct CustomFolder {
    /// Name as the provider spells it — also the value to filter listings on.
    pub name:   String,
    /// Same name, readable: IMAP encodes accents in modified UTF-7.
    #[sqlx(default)]
    pub display: String,
    pub total:  i64,
    pub unread: i64,
}

/// Custom folders that actually contain mail, most populated first.
pub async fn list_custom_folders(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<CustomFolder>>, MailError> {
    let mut rows = sqlx::query_as::<_, CustomFolder>(
        r#"SELECT imap_folder AS name,
                  COUNT(*)                                    AS total,
                  COUNT(*) FILTER (WHERE is_read = FALSE)     AS unread
           FROM mail.messages
           WHERE user_id = $1 AND folder = 'custom' AND is_deleted = FALSE
           GROUP BY imap_folder
           ORDER BY imap_folder"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "list_custom_folders");
        MailError::Database(e)
    })?;

    for f in &mut rows {
        // Drop the "INBOX." prefix most servers put in front of user folders:
        // it is plumbing, not part of the name the user gave.
        let decoded = crate::services::imap_service::decode_imap_utf7(&f.name);
        f.display = decoded.strip_prefix("INBOX.").unwrap_or(&decoded).to_string();
    }
    rows.sort_by_key(|f| f.display.to_lowercase());

    Ok(Json(rows))
}
