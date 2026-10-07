//! Recipient-group endpoints — the user's personal distribution lists.
//!
//! Every route is scoped to the authenticated user: a group is only ever visible
//! or mutable by its owner, and an id that is not the user's yields `NotFound`.
//! Names are unique per user (duplicate → `Conflict`). Members are validated,
//! normalized and de-duplicated server-side before any write (an invalid address
//! → `Validation`).

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    services::recipient_groups::{self, Member},
    state::AppState,
};

// ── Wire shapes (camelCase, round-tripped by the compose menu) ──────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupDto {
    pub id:         Uuid,
    pub name:       String,
    pub members:    Vec<Member>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<recipient_groups::RecipientGroup> for GroupDto {
    fn from(g: recipient_groups::RecipientGroup) -> Self {
        GroupDto {
            id:         g.id,
            name:       g.name,
            members:    g.members,
            created_at: g.created_at,
            updated_at: g.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertGroupDto {
    pub name:    String,
    #[serde(default)]
    pub members: Vec<Member>,
}

// ── Endpoints ───────────────────────────────────────────────────────────────

/// GET /recipient-groups — the user's groups, alphabetical.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<GroupDto>>, MailError> {
    let rows = recipient_groups::list(&state.db, user.id)
        .await
        .map_err(MailError::Internal)?;
    Ok(Json(rows.into_iter().map(GroupDto::from).collect()))
}

/// POST /recipient-groups — create a group. Rejects an empty name, a duplicate
/// name (`Conflict`) and any invalid member address (`Validation`).
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<UpsertGroupDto>,
) -> Result<Json<GroupDto>, MailError> {
    let name = recipient_groups::clean_name(&body.name);
    recipient_groups::validate_name(&name).map_err(MailError::Validation)?;
    let members = recipient_groups::normalize_members(&body.members).map_err(MailError::Validation)?;

    if recipient_groups::name_taken(&state.db, user.id, &name, None)
        .await
        .map_err(MailError::Internal)?
    {
        return Err(MailError::Conflict("Un groupe porte déjà ce nom.".into()));
    }

    let row = recipient_groups::insert(&state.db, user.id, &name, &members)
        .await
        .map_err(MailError::Internal)?;
    Ok(Json(GroupDto::from(row)))
}

/// PUT /recipient-groups/:id — rename / replace members. `NotFound` if the id is
/// not the user's; `Conflict` if the new name collides with another group.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpsertGroupDto>,
) -> Result<Json<GroupDto>, MailError> {
    let name = recipient_groups::clean_name(&body.name);
    recipient_groups::validate_name(&name).map_err(MailError::Validation)?;
    let members = recipient_groups::normalize_members(&body.members).map_err(MailError::Validation)?;

    if recipient_groups::name_taken(&state.db, user.id, &name, Some(id))
        .await
        .map_err(MailError::Internal)?
    {
        return Err(MailError::Conflict("Un groupe porte déjà ce nom.".into()));
    }

    let row = recipient_groups::update(&state.db, user.id, id, &name, &members)
        .await
        .map_err(MailError::Internal)?
        .ok_or_else(|| MailError::NotFound("Groupe introuvable".into()))?;
    Ok(Json(GroupDto::from(row)))
}

/// DELETE /recipient-groups/:id — remove a group.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let affected = recipient_groups::delete(&state.db, user.id, id)
        .await
        .map_err(MailError::Internal)?;
    if affected == 0 {
        return Err(MailError::NotFound("Groupe introuvable".into()));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}
