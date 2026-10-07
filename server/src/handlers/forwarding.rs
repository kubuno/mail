//! Forwarding endpoints: read and save the current user's automatic-forwarding
//! rules (a list of destination addresses, plus whether to keep the local copy).
//!
//! The wire shape mirrors the client's forwarding preferences (camelCase), so
//! the settings tab round-trips it without a mapping layer. The POP/IMAP part of
//! that tab stays client-side; only the forwarding rules reach the server.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::{
    errors::MailError, middleware::AuthUser, server::config, services::forwarding, state::AppState,
};

/// One destination address, as the client edits it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardAddressDto {
    pub email:   String,
    pub enabled: bool,
}

/// The forwarding configuration the client sends and receives.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardingDto {
    #[serde(default)]
    pub forward_addresses: Vec<ForwardAddressDto>,
    /// Keep Kubuno's copy in the inbox when forwarding (vs. archive it).
    #[serde(default = "default_true")]
    pub forward_keep: bool,
    /// Whether the instance allows automatic forwarding at all. Read-only: the
    /// client uses it to explain why the section is unavailable instead of
    /// letting a user configure a rule that will never fire. Ignored on write.
    #[serde(default = "default_true")]
    pub forwarding_allowed: bool,
}

fn default_true() -> bool {
    true
}

/// The instance's automatic-forwarding switch, or `true` when the core cannot be
/// asked.
///
/// Failing OPEN is deliberate here and only here: an unreachable core must not
/// silently look like "the administrator forbade forwarding", because the user
/// would then be told a policy exists that may not. The delivery side
/// ([`forwarding::maybe_forward`]) reads the same switch from the configuration
/// it already holds, so a rule saved during an outage still does not fire while
/// the setting is off.
async fn forwarding_allowed(state: &AppState) -> bool {
    let http = reqwest::Client::new();
    match config::fetch(&http, &state.settings).await {
        Some(cfg) => cfg.allow_auto_forwarding,
        None => {
            tracing::warn!("Transfert : politique d'instance illisible — édition autorisée par défaut");
            true
        }
    }
}

/// RFC-lite address check — good enough to reject obvious typos before storing,
/// matching the client-side validation.
fn is_email(v: &str) -> bool {
    let v = v.trim();
    match v.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && !v.chars().any(char::is_whitespace)
        }
        None => false,
    }
}

/// GET /forwarding — the current user's forwarding rules, or empty defaults.
pub async fn get_forwarding(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<ForwardingDto>, MailError> {
    let rules = forwarding::load(&state.db, user.id).await.map_err(MailError::Internal)?;

    // `forward_keep` is uniform across a user's addresses in the UI; report the
    // first rule's value (defaulting to "keep" when there is none).
    let forward_keep = rules.first().map(|r| r.keep_copy).unwrap_or(true);

    let dto = ForwardingDto {
        forward_addresses: rules
            .into_iter()
            .map(|r| ForwardAddressDto { email: r.forward_to, enabled: r.enabled })
            .collect(),
        forward_keep,
        forwarding_allowed: forwarding_allowed(&state).await,
    };
    Ok(Json(dto))
}

/// PUT /forwarding — replace the current user's forwarding rules.
pub async fn put_forwarding(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<ForwardingDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Validate every address before touching the database, and reject duplicates
    // (they would collapse on the primary key and silently drop a row).
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut rules: Vec<(String, bool, bool)> = Vec::with_capacity(body.forward_addresses.len());
    for addr in &body.forward_addresses {
        let email = addr.email.trim().to_ascii_lowercase();
        if !is_email(&email) {
            return Err(MailError::Validation(format!("Adresse de transfert invalide : {}", addr.email)));
        }
        if !seen.insert(email.clone()) {
            return Err(MailError::Validation(format!("Adresse de transfert en double : {email}")));
        }
        rules.push((email, addr.enabled, body.forward_keep));
    }

    // The instance's switch. Only an ENABLED rule is refused: a user must always
    // be able to switch their forwarding off, or to delete a rule saved before
    // the administrator closed the door — and only the network call is paid for
    // when there is actually something to enable.
    if rules.iter().any(|(_, enabled, _)| *enabled) && !forwarding_allowed(&state).await {
        return Err(MailError::Validation(
            "Le transfert automatique du courrier est désactivé par l'administrateur de l'instance.".into(),
        ));
    }

    forwarding::replace(&state.db, user.id, &rules)
        .await
        .map_err(MailError::Internal)?;

    Ok(Json(serde_json::json!({ "ok": true })))
}
