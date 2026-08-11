//! `/admin/directory/users` — the accounts an administrator may attach a mailbox
//! to.
//!
//! Why the module proxies instead of the panel asking the core directly: the
//! panel is rendered inside the ADMIN console, whose caller is an administrator
//! with a token the core would accept — so a direct call would work. It would
//! also make the mailbox form depend on two origins, and would answer a
//! different set of accounts than the one this module validates against (the
//! public directory route narrows its result by the caller's sharing policy).
//! One door, one answer.
//!
//! The module has no user token — the proxy strips `Authorization` before a
//! module is reached, precisely so a module can never borrow a user's authority.
//! It authenticates with the internal secret, which proves only that the caller
//! is inside the instance. The route is therefore guarded here, by
//! `require_admin`, exactly like every other `/admin/*` route of this module:
//! the internal secret says WHERE the call comes from, never WHO asked.

use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::Value;

use super::require_admin;
use crate::{errors::MailError, middleware::AuthUser, state::AppState};

#[derive(Debug, Deserialize)]
pub struct DirectoryQuery {
    #[serde(default)]
    pub q:     Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Lists active accounts, for the owner picker of the mailbox form.
///
/// A core that cannot be reached yields a 503 with a sentence, never an empty
/// list: "no account matches" and "I could not ask" must not look the same to
/// an administrator about to create a mailbox for somebody.
pub async fn list_users(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<DirectoryQuery>,
) -> Result<Json<Value>, MailError> {
    require_admin(&user)?;

    let url = format!("{}/internal/directory/users", state.settings.core.url);
    // Built through `query`, never by string concatenation: the search term is
    // operator input, and a `&` in it would otherwise forge a second parameter.
    let mut params: Vec<(&str, String)> = Vec::new();
    if let Some(term) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        params.push(("q", term.to_string()));
    }
    if let Some(limit) = q.limit.filter(|n| (1..=200).contains(n)) {
        params.push(("limit", limit.to_string()));
    }

    let response = reqwest::Client::new()
        .get(&url)
        .query(&params)
        .header("X-Internal-Secret", state.settings.core.internal_secret.as_str())
        .send()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Annuaire : appel au core impossible");
            MailError::Internal(anyhow::anyhow!(
                "Annuaire des comptes injoignable : impossible de proposer un propriétaire"
            ))
        })?;

    if !response.status().is_success() {
        let status = response.status();
        tracing::error!(%status, "Annuaire : le core a refusé la demande");
        return Err(MailError::Internal(anyhow::anyhow!(
            "Annuaire des comptes indisponible (code {status})"
        )));
    }

    let body: Value = response.json().await.map_err(|e| {
        tracing::error!(error = %e, "Annuaire : réponse du core illisible");
        MailError::Internal(anyhow::anyhow!("Réponse de l'annuaire des comptes illisible"))
    })?;

    Ok(Json(body))
}
