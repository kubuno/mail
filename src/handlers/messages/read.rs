use axum::{
    extract::{Path, Query, State},
    Json,
};
use uuid::Uuid;

use crate::{errors::MailError, middleware::AuthUser, models::EmailMessage, state::AppState};

use super::pgp::decode_pgp_in_place;

#[derive(serde::Deserialize)]
pub struct GetMessageQuery {
    /// Mark the message read as a side effect of this GET. Defaults to `true`
    /// to preserve the web client's behaviour; a mobile client that prefetches
    /// message bodies passes `false` so opening a row does not silently mark it
    /// read (see also `GET /threads/:id`, which never marks read).
    pub mark_read: Option<bool>,
    /// Act on another user's mailbox (account delegation); see `resolve_acting_user`.
    pub on_behalf_of: Option<Uuid>,
}

pub async fn get_message(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<GetMessageQuery>,
    Path(msg_id): Path<Uuid>,
) -> Result<Json<EmailMessage>, MailError> {
    // Delegation: read the grantor's message when authorised.
    let acting_id = crate::services::delegation::resolve_acting_user(&state.db, &user, q.on_behalf_of).await?;
    let user = AuthUser { id: acting_id, ..user };

    let msg = sqlx::query_as::<_, EmailMessage>(
        r#"SELECT id, thread_id, account_id, user_id, message_id, in_reply_to,
                  imap_uid, imap_folder, from_name, from_email,
                  to_addresses, cc_addresses, bcc_addresses, reply_to,
                  subject, body_text, body_html, attachments,
                  is_read, is_starred, is_deleted, folder, label_ids,
                  sent_at, received_at, created_at, spam_score, list_unsubscribe,
                  mailed_by, signed_by, security, auth_dmarc, structured_data, invite_response
           FROM mail.messages WHERE id = $1 AND user_id = $2"#,
    )
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Message {msg_id}")))?;
    let mut msg = msg;

    // OpenPGP: if this was stored as PGP/MIME, decrypt / verify it now with the
    // reader's own key and fill in the trust verdict. A no-op for ordinary mail.
    decode_pgp_in_place(&state, user.id, &mut msg).await;

    if q.mark_read.unwrap_or(true) && !msg.is_read {
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

    strip_storage_paths(&mut msg);
    Ok(Json(msg))
}

/// Removes the server-side `storage_path` from a message's attachments before
/// it leaves the module. The client downloads by `(message id, index)` through
/// the API and never needs — nor should learn — where the file physically
/// lives on the server.
pub(crate) fn strip_storage_paths(msg: &mut EmailMessage) {
    if let Some(arr) = msg.attachments.as_array_mut() {
        for att in arr {
            if let Some(obj) = att.as_object_mut() {
                obj.remove("storage_path");
            }
        }
    }
}
