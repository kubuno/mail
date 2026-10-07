//! Vacation responder endpoints: read and save the current user's out-of-office
//! auto-reply configuration.
//!
//! The wire shape is the client's `VacationResponder` verbatim (camelCase), so
//! the settings tab round-trips it without a mapping layer. Dates travel as
//! `YYYY-MM-DD` strings, with `""` meaning "no bound" — the empty end-date the
//! UI already supports.

use axum::{extract::State, Json};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::{
    errors::MailError, middleware::AuthUser, services::html_sanitize, services::vacation,
    state::AppState,
};

/// The responder as the client edits it. `start_date` / `end_date` are
/// `YYYY-MM-DD` or `""`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VacationDto {
    pub enabled:       bool,
    #[serde(default)]
    pub start_date:    String,
    #[serde(default)]
    pub end_date:      String,
    #[serde(default)]
    pub subject:       String,
    #[serde(default)]
    pub message_html:  String,
    #[serde(default)]
    pub contacts_only: bool,
}

/// GET /vacation — the current user's responder, or empty defaults when unset.
pub async fn get_vacation(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<VacationDto>, MailError> {
    let config = vacation::load(&state.db, user.id).await.map_err(MailError::Internal)?;

    let dto = match config {
        Some(c) => VacationDto {
            enabled:       c.enabled,
            start_date:    c.start_date.map(|d| d.to_string()).unwrap_or_default(),
            end_date:      c.end_date.map(|d| d.to_string()).unwrap_or_default(),
            subject:       c.subject,
            message_html:  c.message_html,
            contacts_only: c.contacts_only,
        },
        None => VacationDto {
            enabled:       false,
            start_date:    String::new(),
            end_date:      String::new(),
            subject:       String::new(),
            message_html:  String::new(),
            contacts_only: false,
        },
    };
    Ok(Json(dto))
}

/// PUT /vacation — insert or replace the current user's responder.
pub async fn put_vacation(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<VacationDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Validate the dates before touching the database.
    let start = parse_optional_date(&body.start_date)?;
    let end = parse_optional_date(&body.end_date)?;

    // A responder that is turned ON needs a body to send and a start day to
    // begin — an empty active responder would answer nothing yet claim to be on.
    if body.enabled {
        if start.is_none() {
            return Err(MailError::Validation(
                "La réponse automatique nécessite un premier jour.".into(),
            ));
        }
        if body.message_html.trim().is_empty() {
            return Err(MailError::Validation(
                "La réponse automatique nécessite un message.".into(),
            ));
        }
    }
    if let (Some(s), Some(e)) = (start, end) {
        if e < s {
            return Err(MailError::Validation(
                "Le dernier jour ne peut pas précéder le premier.".into(),
            ));
        }
    }

    // Sanitise the body once, on the way in: it is sent verbatim to strangers, so
    // scripts and event handlers must never survive to their inbox.
    let clean_html = html_sanitize::sanitize_email_html(&body.message_html);

    vacation::upsert(
        &state.db,
        user.id,
        body.enabled,
        start,
        end,
        body.subject.trim(),
        &clean_html,
        body.contacts_only,
    )
    .await
    .map_err(MailError::Internal)?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Parses a `YYYY-MM-DD` field; `""` (or whitespace) means "no bound".
fn parse_optional_date(s: &str) -> Result<Option<NaiveDate>, MailError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(Some)
        .map_err(|_| MailError::Validation(format!("Date invalide : {s}")))
}
