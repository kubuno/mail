//! `/admin/aliases` — an address that is not a place but a redirection.
//!
//! `address` is either a whole address (`contact@example.com`) or `@example.com`
//! — the catch-all, which matches anything in that domain that nothing else
//! matched. The database allows at most one catch-all per domain (a partial
//! unique index); two would make delivery depend on the order rows come back in.
//!
//! Destinations may be local or remote. A remote destination only leaves the
//! instance if outbound delivery is enabled — an alias forwarding outside is an
//! outgoing message and obeys the same switch. The panel says so rather than
//! letting an operator believe forwarding works when the outbound queue is off.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    db_error, local_domains, parse_address, parse_destinations, require_address_free, require_admin,
    require_local_domain, translate_conflict, ListQuery,
};
use crate::{errors::MailError, middleware::AuthUser, state::AppState};

const COLUMNS: &str = "id, address, domain, destinations, is_catch_all, is_active, comment, \
                       created_at, updated_at";

#[derive(Debug, sqlx::FromRow, Serialize)]
pub struct AliasRow {
    pub id: Uuid,
    pub address: String,
    pub domain: String,
    pub destinations: Vec<String>,
    pub is_catch_all: bool,
    pub is_active: bool,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct AliasView {
    #[serde(flatten)]
    pub alias: AliasRow,
    /// `null` when the core was unreachable — see `mailboxes::MailboxView`.
    pub domain_served: Option<bool>,
    /// Destinations outside every local domain. They are only actually
    /// forwarded when outbound delivery is enabled.
    pub remote_destinations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AliasPage {
    pub items: Vec<AliasView>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub served_domains: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAliasDto {
    /// `contact@example.com`, or `@example.com` for the domain's catch-all.
    pub address: String,
    pub destinations: Vec<String>,
    pub is_active: Option<bool>,
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAliasDto {
    pub address: Option<String>,
    /// Replaces the whole set. An empty set is refused: an alias that expands to
    /// nothing is a black hole, and `is_active: false` is how one says that out
    /// loud.
    pub destinations: Option<Vec<String>>,
    pub is_active: Option<bool>,
    /// An empty string clears the comment.
    pub comment: Option<String>,
}

// ── Read ─────────────────────────────────────────────────────────────────────

pub async fn list_aliases(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<AliasPage>, MailError> {
    require_admin(&user)?;

    let (limit, offset) = q.page();
    let domain = q.domain_filter();
    let pattern = q.pattern();

    let filter = r#"
        WHERE ($1::text IS NULL OR domain = $1)
          AND ($2::boolean IS NULL OR is_active = $2)
          AND ($3::text IS NULL
               OR address LIKE $3 ESCAPE '\'
               OR LOWER(COALESCE(comment, '')) LIKE $3 ESCAPE '\'
               OR EXISTS (SELECT 1 FROM unnest(destinations) d WHERE LOWER(d) LIKE $3 ESCAPE '\'))
    "#;

    let total: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM mail.aliases {filter}"))
        .bind(&domain)
        .bind(q.active)
        .bind(&pattern)
        .fetch_one(&state.db)
        .await
        .map_err(db_error("comptage des alias"))?;

    // Catch-alls first within a domain: they are the rule that applies last at
    // delivery, and the one an operator most often means to check.
    let rows = sqlx::query_as::<_, AliasRow>(&format!(
        "SELECT {COLUMNS} FROM mail.aliases {filter} \
         ORDER BY domain, is_catch_all DESC, address LIMIT $4 OFFSET $5"
    ))
    .bind(&domain)
    .bind(q.active)
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(db_error("liste des alias"))?;

    let served = local_domains(&state).await.ok();
    let items = rows
        .into_iter()
        .map(|r| decorate(r, served.as_deref()))
        .collect();

    Ok(Json(AliasPage {
        items,
        total,
        limit,
        offset,
        served_domains: served,
    }))
}

pub async fn get_alias(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<AliasView>, MailError> {
    require_admin(&user)?;
    let row = fetch_one(&state.db, id).await?;
    let served = local_domains(&state).await.ok();
    Ok(Json(decorate(row, served.as_deref())))
}

// ── Write ────────────────────────────────────────────────────────────────────

pub async fn create_alias(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<CreateAliasDto>,
) -> Result<Json<AliasView>, MailError> {
    require_admin(&user)?;

    let parsed = parse_address(&dto.address)?;
    let served = local_domains(&state).await?;
    require_local_domain(&served, &parsed.domain)?;

    // Refusing a destination equal to the alias is the cheapest place to stop
    // the loop somebody will eventually configure; the resolver's depth limit is
    // the backstop, not the answer.
    let destinations = parse_destinations(&dto.destinations, &parsed.address)?;
    reject_catch_all_self_loop(&parsed.address, parsed.is_catch_all, &destinations)?;

    require_address_free(&state.db, &parsed.address, None).await?;

    let row = sqlx::query_as::<_, AliasRow>(&format!(
        r#"INSERT INTO mail.aliases
             (address, domain, destinations, is_catch_all, is_active, comment)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING {COLUMNS}"#
    ))
    .bind(&parsed.address)
    .bind(&parsed.domain)
    .bind(&destinations)
    .bind(parsed.is_catch_all)
    .bind(dto.is_active.unwrap_or(true))
    .bind(clean_text(dto.comment.as_deref()))
    .fetch_one(&state.db)
    .await
    .map_err(|e| translate_conflict(e, &parsed.address, &parsed.domain, "création d'un alias"))?;

    Ok(Json(decorate(row, Some(&served))))
}

pub async fn update_alias(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(dto): Json<UpdateAliasDto>,
) -> Result<Json<AliasView>, MailError> {
    require_admin(&user)?;

    let current = fetch_one(&state.db, id).await?;
    let served = local_domains(&state).await?;

    let (address, domain, is_catch_all) = match dto.address.as_deref() {
        Some(raw) => {
            let parsed = parse_address(raw)?;
            require_local_domain(&served, &parsed.domain)?;
            require_address_free(&state.db, &parsed.address, Some(id)).await?;
            (parsed.address, parsed.domain, parsed.is_catch_all)
        }
        None => (
            current.address.clone(),
            current.domain.clone(),
            current.is_catch_all,
        ),
    };

    // Destinations are re-validated against the address as it will BE, not as it
    // was: renaming an alias onto one of its own destinations is the same loop.
    let destinations = match dto.destinations.as_deref() {
        Some(list) => parse_destinations(list, &address)?,
        None => parse_destinations(&current.destinations, &address)?,
    };
    reject_catch_all_self_loop(&address, is_catch_all, &destinations)?;

    let row = sqlx::query_as::<_, AliasRow>(&format!(
        r#"UPDATE mail.aliases SET
             address      = $2,
             domain       = $3,
             is_catch_all = $4,
             destinations = $5,
             is_active    = COALESCE($6, is_active),
             comment      = CASE WHEN $7::text IS NULL THEN comment
                                 WHEN $7 = '' THEN NULL ELSE $7 END
           WHERE id = $1
           RETURNING {COLUMNS}"#
    ))
    .bind(id)
    .bind(&address)
    .bind(&domain)
    .bind(is_catch_all)
    .bind(&destinations)
    .bind(dto.is_active)
    .bind(dto.comment.as_deref().map(str::trim))
    .fetch_one(&state.db)
    .await
    .map_err(|e| translate_conflict(e, &address, &domain, "mise à jour d'un alias"))?;

    Ok(Json(decorate(row, Some(&served))))
}

/// Deleting an alias removes a redirection. Nothing that was already delivered
/// through it moves or disappears: the messages live in the destination
/// mailboxes and are untouched.
pub async fn delete_alias(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    require_admin(&user)?;

    let row = fetch_one(&state.db, id).await?;
    let deleted = sqlx::query("DELETE FROM mail.aliases WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(db_error("suppression d'un alias"))?;
    if deleted.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Alias {id}")));
    }

    Ok(Json(serde_json::json!({
        "deleted": true,
        "address": row.address,
        "message": format!(
            "L'alias « {} » n'est plus distribué. Les messages déjà redirigés restent dans les \
             boîtes de destination : supprimer un alias ne supprime aucun message.",
            row.address
        ),
    })))
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn fetch_one(db: &PgPool, id: Uuid) -> Result<AliasRow, MailError> {
    sqlx::query_as::<_, AliasRow>(&format!("SELECT {COLUMNS} FROM mail.aliases WHERE id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await
        .map_err(db_error("lecture d'un alias"))?
        .ok_or_else(|| MailError::NotFound(format!("Alias {id}")))
}

fn clean_text(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn decorate(alias: AliasRow, served: Option<&[String]>) -> AliasView {
    let remote_destinations = match served {
        Some(list) => alias
            .destinations
            .iter()
            .filter(|d| {
                d.rsplit_once('@')
                    .map(|(_, dom)| !list.iter().any(|s| s == dom))
                    .unwrap_or(false)
            })
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    AliasView {
        domain_served: served.map(|list| list.iter().any(|d| d == &alias.domain)),
        remote_destinations,
        alias,
    }
}

/// A catch-all whose destination is in its own domain re-enters the catch-all
/// whenever that destination is not itself a mailbox, an alias or a list — the
/// loop the migration warns about, in its least obvious form. The exact
/// self-reference is caught by `parse_destinations`; this catches the shape it
/// cannot see.
fn reject_catch_all_self_loop(
    address: &str,
    is_catch_all: bool,
    destinations: &[String],
) -> Result<(), MailError> {
    if !is_catch_all {
        return Ok(());
    }
    let Some(domain) = address.strip_prefix('@') else {
        return Ok(());
    };
    for destination in destinations {
        if destination.rsplit_once('@').map(|(_, d)| d) == Some(domain) {
            return Err(MailError::Validation(format!(
                "« {destination} » ne peut pas être une destination de l'attrape-tout « {address} » : \
                 tant que cette adresse n'est ni une boîte, ni un alias, ni une liste, elle retombe \
                 sur l'attrape-tout et boucle"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(address: &str, destinations: &[&str], catch_all: bool) -> AliasRow {
        AliasRow {
            id: Uuid::nil(),
            address: address.into(),
            domain: address.rsplit_once('@').map(|(_, d)| d).unwrap_or("").into(),
            destinations: destinations.iter().map(|d| (*d).to_string()).collect(),
            is_catch_all: catch_all,
            is_active: true,
            comment: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn a_catch_all_pointing_into_its_own_domain_is_refused() {
        let err = reject_catch_all_self_loop(
            "@example.com",
            true,
            &["staff@example.com".to_string()],
        );
        assert!(err.is_err());
    }

    #[test]
    fn a_catch_all_pointing_elsewhere_is_fine() {
        assert!(reject_catch_all_self_loop(
            "@example.com",
            true,
            &["boss@ailleurs.org".to_string()]
        )
        .is_ok());
    }

    #[test]
    fn an_ordinary_alias_may_point_inside_its_own_domain() {
        assert!(reject_catch_all_self_loop(
            "contact@example.com",
            false,
            &["alice@example.com".to_string()]
        )
        .is_ok());
    }

    #[test]
    fn remote_destinations_are_those_outside_every_local_domain() {
        let served = vec!["example.com".to_string()];
        let view = decorate(
            row("contact@example.com", &["alice@example.com", "boss@ailleurs.org"], false),
            Some(&served),
        );
        assert_eq!(view.remote_destinations, vec!["boss@ailleurs.org"]);
        assert_eq!(view.domain_served, Some(true));
    }

    #[test]
    fn without_the_settings_nothing_is_claimed_about_remoteness() {
        let view = decorate(row("contact@example.com", &["boss@ailleurs.org"], false), None);
        assert!(view.remote_destinations.is_empty());
        assert_eq!(view.domain_served, None);
    }

    #[test]
    fn the_view_flattens_the_row_without_renaming_it() {
        let served = vec!["example.com".to_string()];
        let view = decorate(row("@example.com", &["boss@ailleurs.org"], true), Some(&served));
        let json = serde_json::to_value(&view).expect("sérialisable");
        assert_eq!(json.get("address").and_then(|v| v.as_str()), Some("@example.com"));
        assert_eq!(json.get("is_catch_all").and_then(|v| v.as_bool()), Some(true));
        assert!(json.get("remote_destinations").is_some());
    }
}
