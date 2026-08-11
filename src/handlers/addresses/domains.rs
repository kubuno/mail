//! `/admin/domains` — the per-domain view: what a domain holds, and the policy
//! it applies to new mailboxes.
//!
//! ⚠️ This is NOT where a domain becomes local. **The instance decides**: a
//! domain is declared in the console and proven by DNS (`core.domains`), and
//! this module serves the verified ones — plus the `server_domains` stop-gap
//! list, for names DNS can never prove (`kubuno.local` on a laboratory machine).
//! `mail.domain_policies` only holds what a domain may additionally SAY about
//! itself. Two competing answers to "is this domain ours" is exactly the split
//! that delivers mail nowhere, and it is the split this view now reports on.
//!
//! The overview is therefore a UNION of four sources — the served domains, the
//! domains the instance declares (verified or not), the domains with a policy
//! row, and the domains that actually hold an object — with `is_served` telling
//! the truth about each. Two fields say *why*:
//!
//!   * `source` — `instance` (verified here), `extra` (only in the stop-gap
//!     list, served with no proof behind it), `both`, or `null` when not served.
//!   * `instance_state` — `verified`, `pending` (declared, DNS record not
//!     published yet) or `absent` (the instance has never heard of this name).
//!
//! `pending` and `absent` call for opposite advice — publish the record, versus
//! declare the domain — which is the whole reason both are reported.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{db_error, local_domains, normalize_domain, require_admin, server_config};
use crate::{
    errors::MailError,
    middleware::AuthUser,
    server::config::InstanceDomain,
    state::AppState,
};

#[derive(Debug, Serialize)]
pub struct DomainView {
    pub domain: String,
    /// True when this instance accepts mail for the domain — i.e. it is a
    /// verified domain of the instance, or an entry of the stop-gap list.
    pub is_served: bool,
    /// Why it is served: `instance`, `extra`, `both`. `null` when it is not.
    pub source: Option<&'static str>,
    /// What the instance's registry says: `verified`, `pending` (declared, DNS
    /// proof missing) or `absent` (never declared).
    pub instance_state: &'static str,
    /// `primary`, `secondary` or `alias` — only when the instance declares it.
    pub instance_kind: Option<String>,
    /// For an alias, the domain whose addresses it lends its name to.
    pub instance_parent: Option<String>,
    /// True when `mail.domain_policies` holds a row. A domain can be served with
    /// no policy (defaults apply) and can have a policy while unserved (inert).
    pub has_policy: bool,
    /// Applied to a mailbox created without an explicit quota. 0 = unlimited.
    pub default_quota_bytes: i64,
    /// 0 = unlimited. Refused at creation time, never enforced retroactively.
    pub max_mailboxes: i32,
    pub comment: Option<String>,
    pub mailbox_count: i64,
    pub alias_count: i64,
    pub mailing_list_count: i64,
    pub has_catch_all: bool,
    /// The catch-all alias's id, so the panel can link straight to it.
    pub catch_all_id: Option<Uuid>,
    /// Present only when a policy row exists.
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct DomainsResponse {
    pub items: Vec<DomainView>,
    /// Everything actually served — the union — so the panel can show it next
    /// to the derived view.
    pub served_domains: Vec<String>,
    /// Every domain the instance declares, verified or not, in the console's
    /// order (primary first). The panel's link back to Instance ▸ Domaines.
    pub instance_domains: Vec<InstanceDomain>,
    /// The `server_domains` stop-gap list, verbatim.
    pub extra_domains: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct DomainPolicyDto {
    pub default_quota_bytes: Option<i64>,
    pub max_mailboxes: Option<i32>,
    /// An empty string clears the comment.
    pub comment: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct PolicyRow {
    domain: String,
    default_quota_bytes: i64,
    max_mailboxes: i32,
    comment: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

pub async fn list_domains(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<DomainsResponse>, MailError> {
    require_admin(&user)?;

    // A write may not proceed without knowing the served domains, and neither
    // may this view: its whole point is to say which domains are inert.
    let cfg = server_config(&state).await?;
    let served = cfg.domains.clone();

    let policies: Vec<PolicyRow> = sqlx::query_as(
        "SELECT domain, default_quota_bytes, max_mailboxes, comment, created_at, updated_at \
         FROM mail.domain_policies",
    )
    .fetch_all(&state.db)
    .await
    .map_err(db_error("liste des politiques de domaine"))?;

    let mailbox_counts: Vec<(String, i64)> =
        sqlx::query_as("SELECT domain, COUNT(*) FROM mail.mailboxes GROUP BY domain")
            .fetch_all(&state.db)
            .await
            .map_err(db_error("comptage des boîtes par domaine"))?;

    let alias_counts: Vec<(String, i64)> =
        sqlx::query_as("SELECT domain, COUNT(*) FROM mail.aliases GROUP BY domain")
            .fetch_all(&state.db)
            .await
            .map_err(db_error("comptage des alias par domaine"))?;

    let list_counts: Vec<(String, i64)> =
        sqlx::query_as("SELECT domain, COUNT(*) FROM mail.mailing_lists GROUP BY domain")
            .fetch_all(&state.db)
            .await
            .map_err(db_error("comptage des listes par domaine"))?;

    let catch_alls: Vec<(String, Uuid)> =
        sqlx::query_as("SELECT domain, id FROM mail.aliases WHERE is_catch_all")
            .fetch_all(&state.db)
            .await
            .map_err(db_error("attrape-tout par domaine"))?;

    // Union of every source, so a domain that exists only in one of them is
    // still listed — that is how an inert domain becomes visible. The instance's
    // declared-but-unverified domains are in there too: an operator who has just
    // added a domain in the console must see it here, marked `pending`, rather
    // than wonder why the panel ignores it.
    let mut domains: Vec<String> = served.clone();
    for d in cfg
        .instance_domains
        .iter()
        .map(|d| d.name.clone())
        .chain(policies.iter().map(|p| p.domain.clone()))
        .chain(mailbox_counts.iter().map(|(d, _)| d.clone()))
        .chain(alias_counts.iter().map(|(d, _)| d.clone()))
        .chain(list_counts.iter().map(|(d, _)| d.clone()))
    {
        if !domains.contains(&d) {
            domains.push(d);
        }
    }
    domains.sort();

    let count_of = |list: &[(String, i64)], domain: &str| -> i64 {
        list.iter()
            .find(|(d, _)| d == domain)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    };

    let items = domains
        .into_iter()
        .map(|domain| {
            let policy = policies.iter().find(|p| p.domain == domain);
            let catch_all = catch_alls.iter().find(|(d, _)| *d == domain).map(|(_, id)| *id);
            let declared = cfg.instance_domains.iter().find(|d| d.name == domain);
            DomainView {
                is_served: served.contains(&domain),
                source: cfg.domain_source(&domain).map(|s| s.as_str()),
                instance_state: cfg.instance_state(&domain),
                instance_kind: declared.map(|d| d.kind.clone()),
                instance_parent: declared.and_then(|d| d.parent.clone()),
                has_policy: policy.is_some(),
                default_quota_bytes: policy.map(|p| p.default_quota_bytes).unwrap_or(0),
                max_mailboxes: policy.map(|p| p.max_mailboxes).unwrap_or(0),
                comment: policy.and_then(|p| p.comment.clone()),
                mailbox_count: count_of(&mailbox_counts, &domain),
                alias_count: count_of(&alias_counts, &domain),
                mailing_list_count: count_of(&list_counts, &domain),
                has_catch_all: catch_all.is_some(),
                catch_all_id: catch_all,
                created_at: policy.map(|p| p.created_at),
                updated_at: policy.map(|p| p.updated_at),
                domain,
            }
        })
        .collect();

    Ok(Json(DomainsResponse {
        items,
        served_domains: served,
        instance_domains: cfg.instance_domains,
        extra_domains: cfg.extra_domains,
    }))
}

/// Creates or replaces the policy of one domain.
///
/// A policy may be written for a domain that is not (or not yet) served — an
/// operator prepares a domain before pointing its MX at us, and refusing that
/// would make the panel useless during a migration. The response says plainly
/// whether the domain is served, so nobody mistakes a saved policy for a working
/// domain.
pub async fn upsert_domain_policy(
    State(state): State<AppState>,
    user: AuthUser,
    Path(raw_domain): Path<String>,
    Json(dto): Json<DomainPolicyDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    require_admin(&user)?;

    let domain = normalize_domain(&raw_domain)?;

    let quota = dto.default_quota_bytes.unwrap_or(0);
    if quota < 0 {
        return Err(MailError::Validation(
            "Le quota par défaut ne peut pas être négatif".into(),
        ));
    }
    let max_mailboxes = dto.max_mailboxes.unwrap_or(0);
    if max_mailboxes < 0 {
        return Err(MailError::Validation(
            "Le plafond de boîtes ne peut pas être négatif".into(),
        ));
    }

    let row: PolicyRow = sqlx::query_as(
        r#"INSERT INTO mail.domain_policies (domain, default_quota_bytes, max_mailboxes, comment)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (domain) DO UPDATE
             SET default_quota_bytes = EXCLUDED.default_quota_bytes,
                 max_mailboxes       = EXCLUDED.max_mailboxes,
                 comment             = EXCLUDED.comment
           RETURNING domain, default_quota_bytes, max_mailboxes, comment, created_at, updated_at"#,
    )
    .bind(&domain)
    .bind(quota)
    .bind(max_mailboxes)
    .bind(
        dto.comment
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
    )
    .fetch_one(&state.db)
    .await
    .map_err(db_error("enregistrement d'une politique de domaine"))?;

    // Best effort: an unreachable core must not fail a write that is already
    // committed, so the warning is simply omitted rather than invented.
    let served = local_domains(&state).await.ok();
    let is_served = served.as_ref().map(|list| list.contains(&domain));

    let mut body = serde_json::json!({
        "domain": row.domain,
        "default_quota_bytes": row.default_quota_bytes,
        "max_mailboxes": row.max_mailboxes,
        "comment": row.comment,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "is_served": is_served,
    });

    if is_served == Some(false) {
        body["warning"] = serde_json::json!(format!(
            "Le domaine « {domain} » n'est pas servi par cette instance : cette politique est \
             enregistrée mais inerte. Déclarez-le dans Instance ▸ Domaines et vérifiez-le, ou \
             ajoutez-le à la liste d'appoint des réglages du serveur de messagerie."
        ));
    }

    Ok(Json(body))
}

/// Removes the policy. The domain itself is untouched — it is served or not by
/// `server_domains`, and its mailboxes, aliases and lists stay exactly as they
/// are. Only the defaults applied to FUTURE mailboxes go away.
pub async fn delete_domain_policy(
    State(state): State<AppState>,
    user: AuthUser,
    Path(raw_domain): Path<String>,
) -> Result<Json<serde_json::Value>, MailError> {
    require_admin(&user)?;

    let domain = normalize_domain(&raw_domain)?;
    let deleted = sqlx::query("DELETE FROM mail.domain_policies WHERE domain = $1")
        .bind(&domain)
        .execute(&state.db)
        .await
        .map_err(db_error("suppression d'une politique de domaine"))?;

    if deleted.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Politique du domaine {domain}")));
    }

    Ok(Json(serde_json::json!({
        "deleted": true,
        "domain": domain,
        "message": format!(
            "La politique du domaine « {domain} » est supprimée. Les boîtes, alias et listes de ce \
             domaine sont CONSERVÉS, ainsi que leurs quotas actuels : seules les valeurs par défaut \
             appliquées aux futures boîtes disparaissent."
        ),
    })))
}
