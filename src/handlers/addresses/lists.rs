//! `/admin/mailing-lists` — an address that expands to a membership.
//!
//! Separate from an alias because the question "who is allowed to post here"
//! only exists for a list, and answering it wrong is how an internal list
//! becomes a spam relay. Hence `post_policy` is required to be meaningful:
//! `allowed` with an empty `allowed_senders` would accept nobody while reading
//! like a restriction, so it is refused at write time rather than discovered
//! when a message silently bounces.
//!
//! A list and its members are two tables, so every write that touches both runs
//! in one transaction: a list whose membership was half-applied delivers to the
//! wrong people, which is the one failure mode a mailing list must not have.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{
    db_error, local_domains, parse_address, parse_address_set, parse_destinations,
    require_address_free, require_admin, require_local_domain, translate_conflict, ListQuery,
    MAX_DESTINATIONS,
};
use crate::{errors::MailError, middleware::AuthUser, state::AppState};

const COLUMNS: &str = "id, address, domain, name, post_policy, allowed_senders, is_active, \
                       comment, created_at, updated_at";

/// The four policies the schema's CHECK constraint allows.
const POLICIES: [&str; 4] = ["anyone", "members", "internal", "allowed"];

#[derive(Debug, sqlx::FromRow, Serialize)]
pub struct ListRow {
    pub id: Uuid,
    pub address: String,
    pub domain: String,
    pub name: String,
    /// `anyone` | `members` | `internal` | `allowed`.
    pub post_policy: String,
    pub allowed_senders: Vec<String>,
    pub is_active: bool,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ListView {
    #[serde(flatten)]
    pub list: ListRow,
    pub members: Vec<String>,
    pub member_count: i64,
    /// `null` when the core was unreachable.
    pub domain_served: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ListPage {
    pub items: Vec<ListView>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub served_domains: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct CreateListDto {
    pub address: String,
    pub name: String,
    /// Defaults to `internal` — the schema default, and the only one that is not
    /// a decision an operator should make by omission.
    pub post_policy: Option<String>,
    pub allowed_senders: Option<Vec<String>>,
    pub members: Option<Vec<String>>,
    pub is_active: Option<bool>,
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateListDto {
    pub address: Option<String>,
    pub name: Option<String>,
    pub post_policy: Option<String>,
    pub allowed_senders: Option<Vec<String>>,
    /// Replaces the whole membership when present.
    pub members: Option<Vec<String>>,
    pub is_active: Option<bool>,
    /// An empty string clears the comment.
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MembersDto {
    pub addresses: Vec<String>,
}

// ── Read ─────────────────────────────────────────────────────────────────────

pub async fn list_mailing_lists(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListPage>, MailError> {
    require_admin(&user)?;

    let (limit, offset) = q.page();
    let domain = q.domain_filter();
    let pattern = q.pattern();

    let filter = r#"
        WHERE ($1::text IS NULL OR domain = $1)
          AND ($2::boolean IS NULL OR is_active = $2)
          AND ($3::text IS NULL
               OR address LIKE $3 ESCAPE '\'
               OR LOWER(name) LIKE $3 ESCAPE '\'
               OR LOWER(COALESCE(comment, '')) LIKE $3 ESCAPE '\')
    "#;

    // Audited: `filter` above is a literal; the domain, the active flag and
    // the search pattern are bound parameters.
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM mail.mailing_lists {filter}"
    )))
        .bind(&domain)
        .bind(q.active)
        .bind(&pattern)
        .fetch_one(&state.db)
        .await
        .map_err(db_error("comptage des listes"))?;

    // Audited: the only interpolations are the `COLUMNS` constant and the
    // `filter` literal above; every caller value is a bound parameter.
    let rows = sqlx::query_as::<_, ListRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM mail.mailing_lists {filter} ORDER BY address LIMIT $4 OFFSET $5"
    )))
    .bind(&domain)
    .bind(q.active)
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(db_error("liste des listes de diffusion"))?;

    // One query for every membership on the page rather than one per row.
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let memberships: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT list_id, address FROM mail.mailing_list_members \
         WHERE list_id = ANY($1) ORDER BY address",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await
    .map_err(db_error("membres des listes"))?;

    let served = local_domains(&state).await.ok();
    let items = rows
        .into_iter()
        .map(|r| {
            let members: Vec<String> = memberships
                .iter()
                .filter(|(list_id, _)| *list_id == r.id)
                .map(|(_, address)| address.clone())
                .collect();
            view(r, members, served.as_deref())
        })
        .collect();

    Ok(Json(ListPage {
        items,
        total,
        limit,
        offset,
        served_domains: served,
    }))
}

pub async fn get_mailing_list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ListView>, MailError> {
    require_admin(&user)?;
    let row = fetch_one(&state.db, id).await?;
    let members = fetch_members(&state.db, id).await?;
    let served = local_domains(&state).await.ok();
    Ok(Json(view(row, members, served.as_deref())))
}

// ── Write ────────────────────────────────────────────────────────────────────

pub async fn create_mailing_list(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<CreateListDto>,
) -> Result<Json<ListView>, MailError> {
    require_admin(&user)?;

    let parsed = parse_address(&dto.address)?;
    if parsed.is_catch_all {
        return Err(MailError::Validation(
            "Une liste de diffusion ne peut pas être un attrape-tout : « @domaine » ne se crée qu'en alias".into(),
        ));
    }

    let name = clean_name(&dto.name)?;
    let served = local_domains(&state).await?;
    require_local_domain(&served, &parsed.domain)?;

    let allowed = parse_address_set(dto.allowed_senders.as_deref().unwrap_or_default())?;
    let policy = normalize_policy(dto.post_policy.as_deref(), &allowed)?;
    let members = parse_members(dto.members.as_deref().unwrap_or_default(), &parsed.address)?;

    require_address_free(&state.db, &parsed.address, None).await?;

    // The list and its membership are one object; they are written as one.
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(db_error("création d'une liste : ouverture de transaction"))?;

    // Audited: `COLUMNS` is a constant; every caller value is bound.
    let row = sqlx::query_as::<_, ListRow>(sqlx::AssertSqlSafe(format!(
        r#"INSERT INTO mail.mailing_lists
             (address, domain, name, post_policy, allowed_senders, is_active, comment)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           RETURNING {COLUMNS}"#
    )))
    .bind(&parsed.address)
    .bind(&parsed.domain)
    .bind(&name)
    .bind(&policy)
    .bind(&allowed)
    .bind(dto.is_active.unwrap_or(true))
    .bind(clean_text(dto.comment.as_deref()))
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| translate_conflict(e, &parsed.address, &parsed.domain, "création d'une liste"))?;

    replace_members(&mut tx, row.id, &members).await?;

    tx.commit()
        .await
        .map_err(db_error("création d'une liste : validation"))?;

    Ok(Json(view(row, members, Some(&served))))
}

pub async fn update_mailing_list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(dto): Json<UpdateListDto>,
) -> Result<Json<ListView>, MailError> {
    require_admin(&user)?;

    let current = fetch_one(&state.db, id).await?;
    let served = local_domains(&state).await?;

    let (address, domain) = match dto.address.as_deref() {
        Some(raw) => {
            let parsed = parse_address(raw)?;
            if parsed.is_catch_all {
                return Err(MailError::Validation(
                    "Une liste de diffusion ne peut pas être un attrape-tout".into(),
                ));
            }
            require_local_domain(&served, &parsed.domain)?;
            require_address_free(&state.db, &parsed.address, Some(id)).await?;
            (parsed.address, parsed.domain)
        }
        None => (current.address.clone(), current.domain.clone()),
    };

    let name = match dto.name.as_deref() {
        Some(raw) => clean_name(raw)?,
        None => current.name.clone(),
    };

    let allowed = match dto.allowed_senders.as_deref() {
        Some(list) => parse_address_set(list)?,
        None => current.allowed_senders.clone(),
    };
    // The policy is checked against the allow-list as it will BE: switching to
    // `allowed` while clearing the senders in the same request must not slip
    // through as "restricted to nobody".
    let policy = match dto.post_policy.as_deref() {
        Some(raw) => normalize_policy(Some(raw), &allowed)?,
        None => normalize_policy(Some(&current.post_policy), &allowed)?,
    };

    // Membership is re-validated against the address as it will be, so renaming
    // a list onto one of its own members is refused like any other self-loop.
    let members = match dto.members.as_deref() {
        Some(list) => parse_members(list, &address)?,
        None => parse_members(&fetch_members(&state.db, id).await?, &address)?,
    };

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(db_error("mise à jour d'une liste : ouverture de transaction"))?;

    // Audited: `COLUMNS` is a constant; every caller value is bound.
    let row = sqlx::query_as::<_, ListRow>(sqlx::AssertSqlSafe(format!(
        r#"UPDATE mail.mailing_lists SET
             address         = $2,
             domain          = $3,
             name            = $4,
             post_policy     = $5,
             allowed_senders = $6,
             is_active       = COALESCE($7, is_active),
             comment         = CASE WHEN $8::text IS NULL THEN comment
                                    WHEN $8 = '' THEN NULL ELSE $8 END
           WHERE id = $1
           RETURNING {COLUMNS}"#
    )))
    .bind(id)
    .bind(&address)
    .bind(&domain)
    .bind(&name)
    .bind(&policy)
    .bind(&allowed)
    .bind(dto.is_active)
    .bind(dto.comment.as_deref().map(str::trim))
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| translate_conflict(e, &address, &domain, "mise à jour d'une liste"))?;

    replace_members(&mut tx, id, &members).await?;

    tx.commit()
        .await
        .map_err(db_error("mise à jour d'une liste : validation"))?;

    Ok(Json(view(row, members, Some(&served))))
}

/// Deleting a list removes the address and its membership (the members table
/// cascades). Nothing already delivered to the members moves or disappears.
pub async fn delete_mailing_list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    require_admin(&user)?;

    let row = fetch_one(&state.db, id).await?;
    let members = fetch_members(&state.db, id).await?;

    let deleted = sqlx::query("DELETE FROM mail.mailing_lists WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(db_error("suppression d'une liste"))?;
    if deleted.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Liste {id}")));
    }

    Ok(Json(serde_json::json!({
        "deleted": true,
        "address": row.address,
        "members_removed": members.len(),
        "message": format!(
            "La liste « {} » et ses {} membre(s) sont supprimés. Les messages déjà distribués aux \
             membres restent dans leurs boîtes : supprimer une liste ne supprime aucun message.",
            row.address,
            members.len()
        ),
    })))
}

// ── Membership ───────────────────────────────────────────────────────────────

/// Replaces the whole membership. Idempotent, which is what a panel editing a
/// set of addresses wants.
pub async fn set_members(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(dto): Json<MembersDto>,
) -> Result<Json<ListView>, MailError> {
    require_admin(&user)?;
    let row = fetch_one(&state.db, id).await?;
    let members = parse_members(&dto.addresses, &row.address)?;
    commit_members(&state, row, members).await
}

pub async fn add_members(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(dto): Json<MembersDto>,
) -> Result<Json<ListView>, MailError> {
    require_admin(&user)?;
    let row = fetch_one(&state.db, id).await?;

    let added = parse_members(&dto.addresses, &row.address)?;
    let mut members = fetch_members(&state.db, id).await?;
    for address in added {
        if !members.contains(&address) {
            members.push(address);
        }
    }
    if members.len() > MAX_DESTINATIONS {
        return Err(MailError::Validation(format!(
            "La liste dépasserait {MAX_DESTINATIONS} membres"
        )));
    }
    members.sort();
    commit_members(&state, row, members).await
}

pub async fn remove_members(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(dto): Json<MembersDto>,
) -> Result<Json<ListView>, MailError> {
    require_admin(&user)?;
    let row = fetch_one(&state.db, id).await?;

    // Removals are normalised the same way additions are, so an address typed in
    // a different case still matches the stored one.
    let removed = parse_address_set(&dto.addresses)?;
    let members: Vec<String> = fetch_members(&state.db, id)
        .await?
        .into_iter()
        .filter(|m| !removed.contains(m))
        .collect();

    commit_members(&state, row, members).await
}

async fn commit_members(
    state: &AppState,
    row: ListRow,
    members: Vec<String>,
) -> Result<Json<ListView>, MailError> {
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(db_error("membres : ouverture de transaction"))?;
    replace_members(&mut tx, row.id, &members).await?;
    tx.commit()
        .await
        .map_err(db_error("membres : validation"))?;

    let served = local_domains(state).await.ok();
    Ok(Json(view(row, members, served.as_deref())))
}

/// Rewrites the membership inside the caller's transaction. Delete-then-insert
/// rather than a diff: the set is small, and one statement pair is easier to
/// reason about than three.
async fn replace_members(
    tx: &mut Transaction<'_, Postgres>,
    list_id: Uuid,
    members: &[String],
) -> Result<(), MailError> {
    sqlx::query("DELETE FROM mail.mailing_list_members WHERE list_id = $1")
        .bind(list_id)
        .execute(&mut **tx)
        .await
        .map_err(db_error("remplacement des membres : purge"))?;

    if members.is_empty() {
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO mail.mailing_list_members (list_id, address) \
         SELECT $1, address FROM unnest($2::text[]) AS address \
         ON CONFLICT (list_id, address) DO NOTHING",
    )
    .bind(list_id)
    .bind(members)
    .execute(&mut **tx)
    .await
    .map_err(db_error("remplacement des membres : insertion"))?;

    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn fetch_one(db: &PgPool, id: Uuid) -> Result<ListRow, MailError> {
    // Audited: `COLUMNS` is a constant; every caller value is bound.
    sqlx::query_as::<_, ListRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM mail.mailing_lists WHERE id = $1"
    )))
        .bind(id)
        .fetch_optional(db)
        .await
        .map_err(db_error("lecture d'une liste"))?
        .ok_or_else(|| MailError::NotFound(format!("Liste {id}")))
}

async fn fetch_members(db: &PgPool, id: Uuid) -> Result<Vec<String>, MailError> {
    sqlx::query_scalar(
        "SELECT address FROM mail.mailing_list_members WHERE list_id = $1 ORDER BY address",
    )
    .bind(id)
    .fetch_all(db)
    .await
    .map_err(db_error("lecture des membres d'une liste"))
}

fn view(list: ListRow, members: Vec<String>, served: Option<&[String]>) -> ListView {
    ListView {
        domain_served: served.map(|l| l.iter().any(|d| d == &list.domain)),
        member_count: members.len() as i64,
        members,
        list,
    }
}

fn clean_text(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn clean_name(raw: &str) -> Result<String, MailError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(MailError::Validation("Le nom de la liste est requis".into()));
    }
    if name.chars().count() > 255 {
        return Err(MailError::Validation(
            "Le nom de la liste dépasse 255 caractères".into(),
        ));
    }
    Ok(name.to_string())
}

/// Validates the posting policy and refuses the one combination that reads as a
/// restriction but means "nobody".
fn normalize_policy(raw: Option<&str>, allowed: &[String]) -> Result<String, MailError> {
    let policy = raw.unwrap_or("internal").trim().to_ascii_lowercase();
    if !POLICIES.contains(&policy.as_str()) {
        return Err(MailError::Validation(format!(
            "Politique d'envoi inconnue : « {policy} » (valeurs : {})",
            POLICIES.join(", ")
        )));
    }
    if policy == "allowed" && allowed.is_empty() {
        return Err(MailError::Validation(
            "La politique « allowed » exige au moins un expéditeur autorisé, sinon personne ne peut écrire à la liste".into(),
        ));
    }
    Ok(policy)
}

/// Members are destinations: validated, de-duplicated, and never the list
/// itself. Unlike an alias, an EMPTY membership is legitimate — a list created
/// before anybody joined it is not a black hole, it is an empty room.
fn parse_members(raw: &[String], own: &str) -> Result<Vec<String>, MailError> {
    if raw.iter().all(|m| m.trim().is_empty()) {
        return Ok(Vec::new());
    }
    parse_destinations(raw, own)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_policy_is_internal() {
        assert_eq!(normalize_policy(None, &[]).ok().as_deref(), Some("internal"));
        assert_eq!(normalize_policy(Some(" MEMBERS "), &[]).ok().as_deref(), Some("members"));
    }

    #[test]
    fn an_unknown_policy_is_refused() {
        assert!(normalize_policy(Some("everyone"), &[]).is_err());
    }

    #[test]
    fn allowed_without_a_single_sender_is_refused() {
        assert!(normalize_policy(Some("allowed"), &[]).is_err());
        assert!(normalize_policy(Some("allowed"), &["boss@example.com".to_string()]).is_ok());
    }

    #[test]
    fn an_empty_membership_is_legitimate() {
        assert_eq!(parse_members(&[], "staff@example.com").expect("valide"), Vec::<String>::new());
        assert_eq!(
            parse_members(&["  ".into()], "staff@example.com").expect("valide"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn members_are_deduplicated_and_never_the_list_itself() {
        let out = parse_members(
            &["Alice@example.com".into(), "alice@EXAMPLE.com".into()],
            "staff@example.com",
        )
        .expect("valide");
        assert_eq!(out, vec!["alice@example.com"]);

        assert!(parse_members(&["Staff@Example.com".into()], "staff@example.com").is_err());
    }

    #[test]
    fn a_list_name_is_required_and_bounded() {
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"n".repeat(256)).is_err());
        assert_eq!(clean_name("  Équipe  ").ok().as_deref(), Some("Équipe"));
    }

    #[test]
    fn the_view_reports_the_membership_size() {
        let row = ListRow {
            id: Uuid::nil(),
            address: "staff@example.com".into(),
            domain: "example.com".into(),
            name: "Équipe".into(),
            post_policy: "internal".into(),
            allowed_senders: Vec::new(),
            is_active: true,
            comment: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let v = view(row, vec!["a@example.com".into(), "b@example.com".into()], None);
        assert_eq!(v.member_count, 2);
        assert_eq!(v.domain_served, None);
    }
}
