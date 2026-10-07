//! Account delegation endpoints — Gmail-style "grant access to your account".
//!
//! A grantor invites a delegate (`POST /delegations`), lists what they granted
//! (`GET /delegations`) and revokes (`DELETE /delegations/:id`). A delegate lists
//! the accounts they can access (`GET /delegations/incoming`) and accepts or
//! declines an invitation (`POST /delegations/:id/accept|decline`).
//!
//! Every route is scoped to the caller (`X-Kubuno-User-Id`): a grantor route
//! filters by `grantor_user_id`, a delegate route by `delegate_user_id`. The
//! delegate of a new grant is resolved through the core's INTERNAL directory —
//! the module cannot read `core.users` — and no response ever distinguishes
//! "unknown address" from "cannot be granted", so the routes leak nothing about
//! who does or does not have an account here.
//!
//! The actual enforcement of a delegated read/send lives in
//! `services::delegation::resolve_acting_user`, applied by the threads / messages
//! routes; these endpoints only manage the delegation records themselves.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    services::delegation,
    state::AppState,
};

#[derive(Debug, Deserialize)]
pub struct GrantDto {
    #[serde(default)]
    pub email: String,
}

/// GET /delegations — the delegations I have granted (as grantor), any status.
pub async fn list_granted(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<delegation::Delegation>>, MailError> {
    let rows = delegation::list_granted(&state.db, user.id)
        .await
        .map_err(MailError::Internal)?;
    Ok(Json(rows))
}

/// GET /delegations/incoming — delegations addressed to me: pending invitations
/// (to accept/decline) and accepted delegations (accounts I may act on).
pub async fn list_incoming(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<delegation::Delegation>>, MailError> {
    let rows = delegation::list_incoming(&state.db, user.id)
        .await
        .map_err(MailError::Internal)?;
    Ok(Json(rows))
}

/// POST /delegations { email } — grant a user of this instance access to my
/// mailbox. The invitation starts `pending`; the delegate must accept it.
pub async fn grant(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<GrantDto>,
) -> Result<Json<delegation::Delegation>, MailError> {
    let email = delegation::normalize_email(&body.email);
    if !delegation::is_valid_email(&email) {
        return Err(MailError::Validation("Adresse e-mail invalide.".into()));
    }

    // Resolve the address to an account of THIS instance, through the core's
    // internal directory. A generic error on no match — never "unknown user".
    let delegate = resolve_user_by_email(&state, &email)
        .await?
        .ok_or_else(|| {
            MailError::Validation(
                "Impossible d'accorder l'accès à cette adresse.".into(),
            )
        })?;

    // Refuse self-delegation up front (same generic surface as a stranger).
    if delegate.id == user.id {
        return Err(MailError::Validation(
            "Vous ne pouvez pas vous accorder l'accès à votre propre compte.".into(),
        ));
    }

    match delegation::grant(&state.db, user.id, &user.email, delegate.id, &delegate.email)
        .await
        .map_err(MailError::Internal)?
    {
        Some(row) => {
            tracing::info!(grantor = %user.id, delegate = %delegate.id, "Délégation créée (pending)");
            Ok(Json(row))
        }
        None => Err(MailError::Conflict(
            "Cet utilisateur a déjà accès à votre compte (ou une invitation est en attente).".into(),
        )),
    }
}

/// DELETE /delegations/:id — I (the grantor) revoke a delegation I granted.
pub async fn revoke(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, MailError> {
    let affected = delegation::revoke_as_grantor(&state.db, user.id, id)
        .await
        .map_err(MailError::Internal)?;
    if affected == 0 {
        return Err(MailError::NotFound("Délégation introuvable".into()));
    }
    tracing::info!(grantor = %user.id, delegation = %id, "Délégation révoquée");
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /delegations/:id/accept — I (the delegate) accept an invitation.
pub async fn accept(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, MailError> {
    let affected = delegation::accept_as_delegate(&state.db, user.id, id)
        .await
        .map_err(MailError::Internal)?;
    if affected == 0 {
        // Unknown id, not mine, or not pending — one generic answer.
        return Err(MailError::NotFound("Invitation introuvable".into()));
    }
    tracing::info!(delegate = %user.id, delegation = %id, "Délégation acceptée");
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /delegations/:id/decline — I (the delegate) decline an invitation or
/// step away from an accepted delegation.
pub async fn decline(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, MailError> {
    let affected = delegation::decline_as_delegate(&state.db, user.id, id)
        .await
        .map_err(MailError::Internal)?;
    if affected == 0 {
        return Err(MailError::NotFound("Invitation introuvable".into()));
    }
    tracing::info!(delegate = %user.id, delegation = %id, "Délégation refusée");
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Directory resolution (email → account) ──────────────────────────────────

/// A directory account, reduced to what a grant needs.
struct DirectoryUser {
    id:    Uuid,
    email: String,
}

/// Resolves an email to an active account of this instance via the core's
/// internal directory (`/internal/directory/users?q=…`), matching the address
/// exactly (case-insensitive). Returns `None` when nothing matches.
///
/// The core is asked with the internal secret (a module holds no user token).
/// A core that cannot be reached is an INTERNAL error (503-class), distinct from
/// "no match" (a validation refusal) — so "I could not ask" never masquerades as
/// "that address is not here".
async fn resolve_user_by_email(
    state: &AppState,
    email: &str,
) -> Result<Option<DirectoryUser>, MailError> {
    let url = format!("{}/internal/directory/users", state.settings.core.url);
    let response = reqwest::Client::new()
        .get(&url)
        .query(&[("q", email), ("limit", "25")])
        .header(
            "X-Internal-Secret",
            state.settings.core.internal_secret.as_str(),
        )
        .send()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Délégation : annuaire du core injoignable");
            MailError::Internal(anyhow::anyhow!(
                "Annuaire des comptes injoignable : réessayez plus tard"
            ))
        })?;

    if !response.status().is_success() {
        let status = response.status();
        tracing::error!(%status, "Délégation : le core a refusé la recherche d'annuaire");
        return Err(MailError::Internal(anyhow::anyhow!(
            "Annuaire des comptes indisponible (code {status})"
        )));
    }

    let body: Value = response.json().await.map_err(|e| {
        tracing::error!(error = %e, "Délégation : réponse d'annuaire illisible");
        MailError::Internal(anyhow::anyhow!("Réponse de l'annuaire des comptes illisible"))
    })?;

    // The directory `q` is a substring match; we require an EXACT address match
    // so "al" never resolves to "alice@…".
    let target = email.trim().to_ascii_lowercase();
    let found = body
        .get("users")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|u| {
            let em = u.get("email").and_then(Value::as_str)?;
            if em.trim().to_ascii_lowercase() == target {
                let id = u.get("id").and_then(Value::as_str)?.parse::<Uuid>().ok()?;
                Some(DirectoryUser { id, email: em.trim().to_string() })
            } else {
                None
            }
        });

    Ok(found)
}
