//! POP / IMAP access-policy endpoints: read and save the current user's
//! per-protocol settings (enable/disable, POP fetch behaviour, IMAP expunge and
//! folder rules). The instance's own IMAP/POP3 server reads these at login and
//! honours them — see `crate::services::pop_imap` and `crate::server`.
//!
//! The wire shape is the POP/IMAP subset of the client's forwarding preferences
//! (camelCase, `popState` / `popOnFetch` / `imapExpunge` …), so the settings tab
//! round-trips it without a mapping layer.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::{
    errors::MailError,
    middleware::AuthUser,
    services::pop_imap::{self, ImapExpungeMode, ImapPurgeMode, PopImapSettings, PopMode, PopPostAction},
    state::AppState,
};

/// The policy as the client edits it. `popState` folds `pop_enabled` and
/// `pop_mode` into one control ("disabled" / "all" / "from_now"), matching
/// Gmail's radio group.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PopImapDto {
    pub imap_enabled:      bool,
    /// "auto" | "wait"
    pub imap_expunge:      String,
    /// "archive" | "trash" | "delete"
    pub imap_purge:        String,
    /// "0" (unlimited) or a positive count, kept as a string to match the UI's
    /// Dropdown values.
    pub imap_folder_limit: String,
    /// "disabled" | "all" | "from_now"
    pub pop_state:         String,
    /// "keep" | "mark_read" | "archive" | "delete"
    pub pop_on_fetch:      String,
}

impl PopImapDto {
    fn from_settings(s: &PopImapSettings) -> Self {
        let pop_state = if !s.pop_enabled {
            "disabled".to_string()
        } else {
            s.pop_mode.as_str().to_string()
        };
        PopImapDto {
            imap_enabled:      s.imap_enabled,
            imap_expunge:      s.imap_expunge_mode.as_str().to_string(),
            imap_purge:        s.imap_purge_mode.as_str().to_string(),
            imap_folder_limit: s.imap_folder_limit.to_string(),
            pop_state,
            pop_on_fetch:      s.pop_post_action.as_str().to_string(),
        }
    }
}

/// GET /pop-imap — the current user's policy, or the compatibility defaults.
pub async fn get_pop_imap(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<PopImapDto>, MailError> {
    let settings = pop_imap::load(&state.db, user.id).await.map_err(MailError::Internal)?;
    Ok(Json(PopImapDto::from_settings(&settings)))
}

/// PUT /pop-imap — validate and replace the current user's policy.
pub async fn put_pop_imap(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<PopImapDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Validate every enum before touching the database; an unknown value is a
    // client bug, not a silent coercion.
    let imap_expunge_mode = ImapExpungeMode::parse(&body.imap_expunge)
        .ok_or_else(|| MailError::Validation(format!("Mode d'expunge invalide : {}", body.imap_expunge)))?;
    let imap_purge_mode = ImapPurgeMode::parse(&body.imap_purge)
        .ok_or_else(|| MailError::Validation(format!("Mode de purge invalide : {}", body.imap_purge)))?;
    let imap_folder_limit: i64 = body
        .imap_folder_limit
        .trim()
        .parse()
        .ok()
        .filter(|n| *n >= 0)
        .ok_or_else(|| MailError::Validation(format!("Limite de dossier invalide : {}", body.imap_folder_limit)))?;
    let pop_post_action = PopPostAction::parse(&body.pop_on_fetch)
        .ok_or_else(|| MailError::Validation(format!("Action POP invalide : {}", body.pop_on_fetch)))?;

    let (pop_enabled, pop_mode) = match body.pop_state.as_str() {
        "disabled" => (false, PopMode::All),
        "all" => (true, PopMode::All),
        "from_now" => (true, PopMode::FromNow),
        other => {
            return Err(MailError::Validation(format!("État POP invalide : {other}")));
        }
    };

    // The "from now on" cursor is snapped to the current top of the inbox the
    // moment the user FIRST chooses that mode; re-saving while already in
    // from_now keeps the original cursor, so previously hidden mail does not
    // suddenly reappear. Load the previous policy to decide.
    let previous = pop_imap::load(&state.db, user.id).await.map_err(MailError::Internal)?;
    let pop_from_uid = if pop_mode == PopMode::FromNow {
        if previous.pop_mode == PopMode::FromNow && previous.pop_enabled {
            previous.pop_from_uid
        } else {
            pop_imap::current_inbox_cursor(&state.db, user.id).await.map_err(MailError::Internal)?
        }
    } else {
        // Keep the last cursor around (harmless) rather than resetting it.
        previous.pop_from_uid
    };

    let settings = PopImapSettings {
        imap_enabled: body.imap_enabled,
        imap_expunge_mode,
        imap_purge_mode,
        imap_folder_limit,
        pop_enabled,
        pop_mode,
        pop_from_uid,
        pop_post_action,
    };

    pop_imap::upsert(&state.db, user.id, &settings).await.map_err(MailError::Internal)?;

    Ok(Json(serde_json::json!({ "ok": true })))
}
