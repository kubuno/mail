//! E-mail template endpoints — the user's reusable canned drafts.
//!
//! Every route is scoped to the authenticated user: a template is only ever
//! visible or mutable by its owner, and an id that is not the user's yields
//! `NotFound` (never someone else's row). Names are unique per user, so a
//! duplicate name is a `Conflict`.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{errors::MailError, middleware::AuthUser, services::templates, state::AppState};

// ── Wire shapes (camelCase, round-tripped by the compose menu) ──────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateDto {
    pub id:         Uuid,
    pub name:       String,
    pub subject:    String,
    pub body_html:  String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<templates::EmailTemplate> for TemplateDto {
    fn from(t: templates::EmailTemplate) -> Self {
        TemplateDto {
            id:         t.id,
            name:       t.name,
            subject:    t.subject,
            body_html:  t.body_html,
            created_at: t.created_at,
            updated_at: t.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertTemplateDto {
    pub name:      String,
    #[serde(default)]
    pub subject:   String,
    #[serde(default)]
    pub body_html: String,
}

// ── Endpoints ───────────────────────────────────────────────────────────────

/// GET /templates — the user's templates, alphabetical.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<TemplateDto>>, MailError> {
    let rows = templates::list(&state.db, user.id)
        .await
        .map_err(MailError::Internal)?;
    Ok(Json(rows.into_iter().map(TemplateDto::from).collect()))
}

/// POST /templates — create a template. Rejects an empty name and a name
/// already used by this user.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<UpsertTemplateDto>,
) -> Result<Json<TemplateDto>, MailError> {
    let name = templates::clean_name(&body.name);
    templates::validate_name(&name).map_err(MailError::Validation)?;
    let subject = templates::clean_subject(&body.subject);

    if templates::name_taken(&state.db, user.id, &name, None)
        .await
        .map_err(MailError::Internal)?
    {
        return Err(MailError::Conflict("Un modèle porte déjà ce nom.".into()));
    }

    let row = templates::insert(&state.db, user.id, &name, &subject, &body.body_html)
        .await
        .map_err(MailError::Internal)?;
    Ok(Json(TemplateDto::from(row)))
}

/// PUT /templates/:id — rename / update subject and body. `NotFound` if the id
/// is not the user's; `Conflict` if the new name collides with another template.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpsertTemplateDto>,
) -> Result<Json<TemplateDto>, MailError> {
    let name = templates::clean_name(&body.name);
    templates::validate_name(&name).map_err(MailError::Validation)?;
    let subject = templates::clean_subject(&body.subject);

    if templates::name_taken(&state.db, user.id, &name, Some(id))
        .await
        .map_err(MailError::Internal)?
    {
        return Err(MailError::Conflict("Un modèle porte déjà ce nom.".into()));
    }

    let row = templates::update(&state.db, user.id, id, &name, &subject, &body.body_html)
        .await
        .map_err(MailError::Internal)?
        .ok_or_else(|| MailError::NotFound("Modèle introuvable".into()))?;
    Ok(Json(TemplateDto::from(row)))
}

/// DELETE /templates/:id — remove a template.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let affected = templates::delete(&state.db, user.id, id)
        .await
        .map_err(MailError::Internal)?;
    if affected == 0 {
        return Err(MailError::NotFound("Modèle introuvable".into()));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}
