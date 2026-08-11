//! Account delegation — the server-side authority behind Gmail-style "grant
//! access to your account".
//!
//! A GRANTOR invites a DELEGATE (another user of this instance) to read their
//! mailbox and send on their behalf, without ever sharing a password. Access is
//! conferred by exactly ONE thing: a delegation row whose `status = 'accepted'`.
//! Everything else — a pending invitation, a revoked delegation, or no row at
//! all — confers no access.
//!
//! The acceptance decision is a pure function ([`authorize_acting_as`]) taking an
//! already-fetched delegation, so every branch (accepted, pending, revoked,
//! absent, self) is unit-tested without a database. The single enforcement point
//! ([`resolve_acting_user`]) is reused by every delegated read/send route: given
//! the authenticated user and an optional `on_behalf_of`, it returns the user id
//! that scopes the request — the grantor when a delegation authorises it, the
//! caller themselves when no `on_behalf_of` is supplied, and a generic 403
//! otherwise.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{errors::MailError, middleware::AuthUser};

// ── Status constants ────────────────────────────────────────────────────────

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_ACCEPTED: &str = "accepted";
pub const STATUS_REVOKED: &str = "revoked";

// ── Pure logic (unit-tested, no database) ───────────────────────────────────

/// Re-exported so callers validate emails through the one implementation the
/// send-as flow already uses (single `@`, dotted domain, no whitespace…).
pub use crate::services::send_as::{is_valid_email, normalize_email};

/// The minimal projection [`authorize_acting_as`] needs to decide access. Kept
/// tiny and owned so the decision is pure and trivially constructed in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationAuth {
    pub delegate_user_id: Uuid,
    pub status:           String,
}

/// May `auth_user` act as `on_behalf_of`, given the delegation row that was
/// fetched for that (grantor, delegate) pair (or `None` if there is none)?
///
/// Access is granted ONLY when a row exists, it is `accepted`, and it names
/// exactly this delegate. Acting as oneself is refused here on purpose: a real
/// self-request omits `on_behalf_of` entirely (see [`resolve_acting_user`]), so
/// reaching this with `auth_user == on_behalf_of` means a caller tried to forge a
/// delegation to themselves.
pub fn authorize_acting_as(
    auth_user: Uuid,
    on_behalf_of: Uuid,
    found: Option<&DelegationAuth>,
) -> Result<Uuid, ActingDenied> {
    if auth_user == on_behalf_of {
        return Err(ActingDenied);
    }
    match found {
        Some(d) if d.status == STATUS_ACCEPTED && d.delegate_user_id == auth_user => Ok(on_behalf_of),
        _ => Err(ActingDenied),
    }
}

/// Opaque refusal — carries no reason, so no acting-as denial can leak WHY (no
/// row vs. pending vs. revoked vs. wrong delegate all look identical).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActingDenied;

/// A pending invitation is the only state a delegate may accept.
pub fn can_accept(status: &str) -> bool {
    status == STATUS_PENDING
}

/// A delegate may step away from a pending OR an accepted delegation (declining
/// an accepted one is how they remove their own access).
pub fn can_decline(status: &str) -> bool {
    status == STATUS_PENDING || status == STATUS_ACCEPTED
}

/// May a fresh grant be (re)created against an existing row for the same pair?
/// Only a previously revoked delegation is reusable — an active or still-pending
/// one is a duplicate and must be refused.
pub fn can_regrant(existing_status: &str) -> bool {
    existing_status == STATUS_REVOKED
}

// ── Persisted rows / wire view ──────────────────────────────────────────────

/// A delegation as returned to clients. Same shape serves both the grantor view
/// ("delegations I granted") and the delegate view ("accounts I can access").
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Delegation {
    pub id:               Uuid,
    pub grantor_user_id:  Uuid,
    pub grantor_email:    String,
    pub delegate_user_id: Uuid,
    pub delegate_email:   String,
    pub status:           String,
    pub can_send:         bool,
    pub created_at:       DateTime<Utc>,
    pub accepted_at:      Option<DateTime<Utc>>,
}

type Row = (
    Uuid,
    Uuid,
    String,
    Uuid,
    String,
    String,
    bool,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

fn to_delegation(r: Row) -> Delegation {
    Delegation {
        id:               r.0,
        grantor_user_id:  r.1,
        grantor_email:    r.2,
        delegate_user_id: r.3,
        delegate_email:   r.4,
        status:           r.5,
        can_send:         r.6,
        created_at:       r.7,
        accepted_at:      r.8,
    }
}

const SELECT_COLS: &str = "id, grantor_user_id, grantor_email, delegate_user_id, \
                           delegate_email, status, can_send, created_at, accepted_at";

// ── Database access (always scoped by grantor or delegate) ──────────────────

/// Delegations `grantor` has granted (any status), newest first.
pub async fn list_granted(db: &PgPool, grantor: Uuid) -> Result<Vec<Delegation>> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM mail.delegations \
         WHERE grantor_user_id = $1 ORDER BY created_at DESC"
    ))
    .bind(grantor)
    .fetch_all(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Délégation : lecture des délégations accordées échouée");
        e
    })
    .context("Lecture des délégations accordées")?;
    Ok(rows.into_iter().map(to_delegation).collect())
}

/// Live delegations addressed to `delegate` — both PENDING invitations (which
/// they may accept or decline) and ACCEPTED delegations (the accounts they may
/// act on). Revoked rows are excluded. The delegate's settings UI shows the
/// accept/decline controls for the pending ones; a future app-switcher filters
/// this to `accepted`.
pub async fn list_incoming(db: &PgPool, delegate: Uuid) -> Result<Vec<Delegation>> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM mail.delegations \
         WHERE delegate_user_id = $1 AND status IN ('pending', 'accepted') \
         ORDER BY status DESC, accepted_at DESC NULLS LAST, created_at DESC"
    ))
    .bind(delegate)
    .fetch_all(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Délégation : lecture des accès délégués échouée");
        e
    })
    .context("Lecture des accès délégués")?;
    Ok(rows.into_iter().map(to_delegation).collect())
}

/// The one row for a (grantor, delegate) pair, if any.
pub async fn find_pair(db: &PgPool, grantor: Uuid, delegate: Uuid) -> Result<Option<Delegation>> {
    let row: Option<Row> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM mail.delegations \
         WHERE grantor_user_id = $1 AND delegate_user_id = $2"
    ))
    .bind(grantor)
    .bind(delegate)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Délégation : lecture d'une paire échouée");
        e
    })
    .context("Lecture d'une délégation")?;
    Ok(row.map(to_delegation))
}

/// Creates (or re-arms a previously revoked) pending delegation for a pair, in a
/// single transaction so the check-then-write cannot race a concurrent grant.
///
/// Returns `Ok(None)` when an ACTIVE or still-PENDING delegation already exists
/// (a duplicate the caller must refuse), or `Ok(Some(row))` on success.
pub async fn grant(
    db: &PgPool,
    grantor: Uuid,
    grantor_email: &str,
    delegate: Uuid,
    delegate_email: &str,
) -> Result<Option<Delegation>> {
    let mut tx = db.begin().await.context("Ouverture transaction de délégation")?;

    // Lock the pair's row (if any) for the duration of the decision.
    let existing: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, status FROM mail.delegations \
         WHERE grantor_user_id = $1 AND delegate_user_id = $2 FOR UPDATE",
    )
    .bind(grantor)
    .bind(delegate)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Délégation : verrou de paire échoué");
        e
    })
    .context("Verrouillage d'une délégation")?;

    let row: Row = match existing {
        Some((_, status)) if !can_regrant(&status) => {
            // Active or pending already: nothing to do, signal a duplicate.
            tx.rollback().await.ok();
            return Ok(None);
        }
        Some((id, _)) => {
            // Revoked → re-arm as a fresh pending invitation.
            sqlx::query_as(&format!(
                "UPDATE mail.delegations \
                 SET status = 'pending', can_send = TRUE, accepted_at = NULL, \
                     grantor_email = $2, delegate_email = $3 \
                 WHERE id = $1 RETURNING {SELECT_COLS}"
            ))
            .bind(id)
            .bind(grantor_email)
            .bind(delegate_email)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Délégation : réarmement échoué");
                e
            })
            .context("Réarmement d'une délégation révoquée")?
        }
        None => sqlx::query_as(&format!(
            "INSERT INTO mail.delegations \
                 (grantor_user_id, grantor_email, delegate_user_id, delegate_email, status) \
             VALUES ($1, $2, $3, $4, 'pending') RETURNING {SELECT_COLS}"
        ))
        .bind(grantor)
        .bind(grantor_email)
        .bind(delegate)
        .bind(delegate_email)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Délégation : insertion échouée");
            e
        })
        .context("Insertion d'une délégation")?,
    };

    tx.commit().await.context("Validation transaction de délégation")?;
    Ok(Some(to_delegation(row)))
}

/// Revokes a delegation the GRANTOR owns. Returns the affected row count (0 = not
/// this grantor's / unknown id).
pub async fn revoke_as_grantor(db: &PgPool, grantor: Uuid, id: Uuid) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE mail.delegations SET status = 'revoked', accepted_at = NULL \
         WHERE id = $1 AND grantor_user_id = $2 AND status <> 'revoked'",
    )
    .bind(id)
    .bind(grantor)
    .execute(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Délégation : révocation échouée");
        e
    })
    .context("Révocation d'une délégation")?;
    Ok(res.rows_affected())
}

/// The delegate accepts a PENDING invitation addressed to them. Guarded by
/// `status = 'pending'` in SQL so a double-accept or a race is a no-op.
pub async fn accept_as_delegate(db: &PgPool, delegate: Uuid, id: Uuid) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE mail.delegations SET status = 'accepted', accepted_at = NOW() \
         WHERE id = $1 AND delegate_user_id = $2 AND status = 'pending'",
    )
    .bind(id)
    .bind(delegate)
    .execute(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Délégation : acceptation échouée");
        e
    })
    .context("Acceptation d'une délégation")?;
    Ok(res.rows_affected())
}

/// The delegate declines a pending invitation, or steps away from an accepted
/// one (both remove access). Sets the row to `revoked`.
pub async fn decline_as_delegate(db: &PgPool, delegate: Uuid, id: Uuid) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE mail.delegations SET status = 'revoked', accepted_at = NULL \
         WHERE id = $1 AND delegate_user_id = $2 AND status IN ('pending', 'accepted')",
    )
    .bind(id)
    .bind(delegate)
    .execute(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Délégation : refus échoué");
        e
    })
    .context("Refus d'une délégation")?;
    Ok(res.rows_affected())
}

// ── The single enforcement point ────────────────────────────────────────────

/// Resolves the user id a delegated (or ordinary) request must be scoped to.
///
/// * `on_behalf_of == None` — the caller acts for themselves: returns `auth_user.id`
///   unchanged, so every existing (non-delegated) route behaves exactly as before.
/// * `on_behalf_of == Some(grantor)` — the caller claims to act as `grantor`:
///   the (grantor, delegate) delegation is fetched and [`authorize_acting_as`]
///   decides. On success the GRANTOR's id is returned (the request scopes to the
///   grantor's mailbox) and the delegated access is logged; otherwise a generic
///   `403` — never a reason a caller could probe.
pub async fn resolve_acting_user(
    db: &PgPool,
    auth_user: &AuthUser,
    on_behalf_of: Option<Uuid>,
) -> Result<Uuid, MailError> {
    let Some(grantor) = on_behalf_of else {
        return Ok(auth_user.id);
    };

    // Cheap refusal before touching the database: acting as self is never a
    // delegation.
    if grantor == auth_user.id {
        return Err(MailError::Forbidden);
    }

    let found = find_pair(db, grantor, auth_user.id)
        .await
        .map_err(MailError::Internal)?
        .map(|d| DelegationAuth {
            delegate_user_id: d.delegate_user_id,
            status:           d.status,
        });

    match authorize_acting_as(auth_user.id, grantor, found.as_ref()) {
        Ok(acting) => {
            // Sensitive action: record WHO acted as WHOM. No message content, no
            // secret — only the two account ids.
            tracing::info!(
                delegate = %auth_user.id,
                grantor  = %grantor,
                "Accès délégué autorisé"
            );
            Ok(acting)
        }
        Err(_) => Err(MailError::Forbidden),
    }
}

/// Returns the delegate's own email to stamp as `Sender:` on a delegated send —
/// but only when the delegation is accepted AND `can_send` is set. `Ok(None)`
/// means "no delegated send authority" (the caller must refuse the send), while
/// distinguishing it from a self-send where `on_behalf_of` was absent.
///
/// This is a second, send-specific gate on top of [`resolve_acting_user`]: a
/// delegate may be allowed to READ (accepted) yet not to SEND (`can_send=false`).
pub async fn send_authority(
    db: &PgPool,
    grantor: Uuid,
    delegate: Uuid,
) -> Result<bool, MailError> {
    let can: Option<bool> = sqlx::query_scalar(
        "SELECT can_send FROM mail.delegations \
         WHERE grantor_user_id = $1 AND delegate_user_id = $2 AND status = 'accepted'",
    )
    .bind(grantor)
    .bind(delegate)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Délégation : lecture du droit d'envoi échouée");
        MailError::Database(e)
    })?;
    Ok(can.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted_by(delegate: Uuid) -> DelegationAuth {
        DelegationAuth { delegate_user_id: delegate, status: STATUS_ACCEPTED.into() }
    }

    // ── authorize_acting_as: the whole access decision ───────────────────────

    #[test]
    fn accepted_delegation_lets_the_delegate_act_as_the_grantor() {
        let grantor = Uuid::new_v4();
        let delegate = Uuid::new_v4();
        let found = accepted_by(delegate);
        assert_eq!(
            authorize_acting_as(delegate, grantor, Some(&found)),
            Ok(grantor)
        );
    }

    #[test]
    fn pending_delegation_grants_no_access() {
        let grantor = Uuid::new_v4();
        let delegate = Uuid::new_v4();
        let found = DelegationAuth { delegate_user_id: delegate, status: STATUS_PENDING.into() };
        assert_eq!(authorize_acting_as(delegate, grantor, Some(&found)), Err(ActingDenied));
    }

    #[test]
    fn revoked_delegation_grants_no_access() {
        let grantor = Uuid::new_v4();
        let delegate = Uuid::new_v4();
        let found = DelegationAuth { delegate_user_id: delegate, status: STATUS_REVOKED.into() };
        assert_eq!(authorize_acting_as(delegate, grantor, Some(&found)), Err(ActingDenied));
    }

    #[test]
    fn no_delegation_grants_no_access() {
        let grantor = Uuid::new_v4();
        let delegate = Uuid::new_v4();
        assert_eq!(authorize_acting_as(delegate, grantor, None), Err(ActingDenied));
    }

    #[test]
    fn acting_as_self_is_refused() {
        let me = Uuid::new_v4();
        // Even with a bogus "accepted" row, a self target is refused outright.
        let found = accepted_by(me);
        assert_eq!(authorize_acting_as(me, me, Some(&found)), Err(ActingDenied));
    }

    #[test]
    fn an_accepted_row_for_a_different_delegate_does_not_help() {
        let grantor = Uuid::new_v4();
        let delegate = Uuid::new_v4();
        let someone_else = Uuid::new_v4();
        // The stored delegation names a DIFFERENT delegate; our caller must not
        // ride on it.
        let found = accepted_by(someone_else);
        assert_eq!(authorize_acting_as(delegate, grantor, Some(&found)), Err(ActingDenied));
    }

    // ── Status transitions ───────────────────────────────────────────────────

    #[test]
    fn only_pending_can_be_accepted() {
        assert!(can_accept(STATUS_PENDING));
        assert!(!can_accept(STATUS_ACCEPTED));
        assert!(!can_accept(STATUS_REVOKED));
    }

    #[test]
    fn pending_and_accepted_can_be_declined() {
        assert!(can_decline(STATUS_PENDING));
        assert!(can_decline(STATUS_ACCEPTED));
        assert!(!can_decline(STATUS_REVOKED));
    }

    #[test]
    fn only_revoked_can_be_regranted() {
        assert!(can_regrant(STATUS_REVOKED));
        assert!(!can_regrant(STATUS_PENDING));
        assert!(!can_regrant(STATUS_ACCEPTED));
    }

    // ── Email validation is the send-as implementation ───────────────────────
    #[test]
    fn email_validation_is_shared_with_send_as() {
        assert!(is_valid_email("delegate@example.com"));
        assert!(!is_valid_email("not-an-email"));
        assert_eq!(normalize_email("  Deleg@Example.COM "), "deleg@example.com");
    }
}
