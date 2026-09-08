use axum::{
    extract::{Path, State},
    Json,
};
use base64::Engine as _;
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    models::{AttachmentInput, EmailAccount, EmailAddress, SendMailDto},
    state::AppState,
};

use super::send::send_message_inner;

#[derive(serde::Deserialize)]
pub struct InviteReplyDto {
    /// accepted | tentative | declined
    pub response: String,
    /// Optional note for the organizer — typically the reason for a decline.
    /// Travels as the iCalendar COMMENT of the reply.
    #[serde(default)]
    pub comment: Option<String>,
    /// An alternative time the invitee suggests. Turns the message into an iTIP
    /// COUNTER: same event, proposed at this time instead.
    #[serde(default)]
    pub proposed_start: Option<String>,
    #[serde(default)]
    pub proposed_end: Option<String>,
}

/// Reply to a calendar invitation the way Gmail does: email the organizer a
/// `text/calendar; method=REPLY` with our PARTSTAT, and remember the answer on
/// the message so the card shows it on reload. Adding the event to the calendar
/// is done client-side (via the calendar module), so this endpoint only sends
/// the iMIP reply and records the choice.
pub async fn invite_reply(
    State(state): State<AppState>,
    user: AuthUser,
    Path(msg_id): Path<Uuid>,
    Json(dto): Json<InviteReplyDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    let rsvp = crate::services::imip::Rsvp::parse(dto.response.trim())
        .ok_or_else(|| MailError::Validation("Réponse d'invitation invalide".into()))?;

    // Load just what we need: the message's account, subject and structured data.
    let row = sqlx::query_as::<_, (Uuid, String, Option<serde_json::Value>)>(
        "SELECT account_id, subject, structured_data FROM mail.messages \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Message {msg_id}")))?;
    let (account_id, subject, structured_data) = row;

    // Find the invitation node (an ICS-sourced Event) and its reply fields.
    let node = structured_data
        .as_ref()
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find(|n| {
                n.get("_source").and_then(|s| s.as_str()) == Some("ics")
                    && n.get("_invite").and_then(|b| b.as_bool()) == Some(true)
            })
        })
        .ok_or_else(|| MailError::Validation("Ce message n'est pas une invitation".into()))?;

    let organizer_email = node.get("organizerEmail").and_then(|v| v.as_str());
    let request_ics = node.get("_ics").and_then(|v| v.as_str()).unwrap_or("");
    let summary = node
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(subject.as_str())
        .to_string();

    // Always remember the choice, even when no organizer address is available to
    // reply to (some invites omit a usable ORGANIZER mailto).
    sqlx::query("UPDATE mail.messages SET invite_response = $1 WHERE id = $2 AND user_id = $3")
        .bind(rsvp.as_db())
        .bind(msg_id)
        .bind(user.id)
        .execute(&state.db)
        .await?;

    let Some(organizer_email) = organizer_email.filter(|e| !e.is_empty()) else {
        return Ok(Json(serde_json::json!({ "invite_response": rsvp.as_db(), "sent": false })));
    };

    // Our own address is the ATTENDEE and the From of the reply.
    let account = sqlx::query_as::<_, EmailAccount>(
        r#"SELECT id, user_id, name, email_address, kind, mailbox_id,
                  incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  smtp_host, smtp_port, smtp_security, smtp_username, auth_kind,
                  is_default, is_active, last_sync_at, last_error,
                  created_at, updated_at
           FROM mail.accounts WHERE id = $1 AND user_id = $2"#,
    )
    .bind(account_id)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    let proposal = dto
        .proposed_start
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|start| crate::services::imip::Proposal {
            start: start.to_string(),
            end: dto.proposed_end.clone().filter(|e| !e.trim().is_empty()),
        });
    let reply_ics = crate::services::imip::build_reply(
        request_ics,
        &account.email_address,
        Some(&account.name),
        rsvp,
        dto.comment.as_deref(),
        proposal.as_ref(),
    );

    let verb = match rsvp {
        crate::services::imip::Rsvp::Accepted => "Accepté",
        crate::services::imip::Rsvp::Tentative => "Provisoire",
        crate::services::imip::Rsvp::Declined => "Refusé",
    };
    // A counter-proposal is announced as such: the organizer is being asked to
    // move the event, not merely told we decline.
    let heading = if proposal.is_some() {
        format!("Nouvel horaire proposé : {}", html_escape(&summary))
    } else {
        format!("{verb} : {}", html_escape(&summary))
    };
    let note = dto
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| format!("<p>{}</p>", html_escape(c)))
        .unwrap_or_default();
    let body_html = format!("<p>{heading}</p>{note}");
    let ics_b64 = base64::engine::general_purpose::STANDARD.encode(reply_ics.as_bytes());
    let send_dto = SendMailDto {
        account_id,
        to_addresses: vec![EmailAddress {
            name: None,
            email: organizer_email.to_string(),
        }],
        cc_addresses: None,
        bcc_addresses: None,
        subject: if proposal.is_some() {
            format!("Nouvel horaire proposé : {summary}")
        } else {
            format!("{verb} : {summary}")
        },
        body_html,
        reply_to_id: None,
        draft_id: None,
        scheduled_at: None,
        attachments: Some(vec![AttachmentInput {
            filename: "invite.ics".into(),
            mime: "text/calendar; method=REPLY; charset=utf-8".into(),
            content: ics_b64,
        }]),
        sign: None,
        encrypt: None,
        label_ids: None,
    };

    if let Err(e) = send_message_inner(&state, user.id, &send_dto, None).await {
        tracing::error!(error = %e, msg_id = %msg_id, "Envoi de la réponse d'invitation échoué");
        // The RSVP is already recorded; report that the reply mail didn't go out.
        return Ok(Json(serde_json::json!({ "invite_response": rsvp.as_db(), "sent": false })));
    }

    Ok(Json(serde_json::json!({ "invite_response": rsvp.as_db(), "sent": true })))
}

/// Minimal HTML escape for the one-line reply body.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
