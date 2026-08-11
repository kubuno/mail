//! The outbound queue's database operations, on PostgreSQL.
//!
//! These encode the Postfix qmgr reliability invariants (see
//! project_mail_server_pro_roadmap): durability before ack (the enqueue commits
//! before the caller answers the client), per-recipient state advanced in the
//! same transaction as the delivery result (idempotency, no double send),
//! atomic claim with `FOR UPDATE SKIP LOCKED` + a lease so a dead worker's rows
//! are reclaimed, and bounded exponential backoff.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::config::ServerConfig;

/// Backoff bounds when the administrator has said nothing, after Postfix's
/// minimal/maximal_backoff_time.
const MIN_BACKOFF_SECS: i64 = 300; // 5 min
const MAX_BACKOFF_HOURS: i64 = 4;

/// Bounds on the configured queue lifetime. A message that never expires can
/// never be reported as undeliverable, and one that expires instantly is
/// bounced before its first retry — so a mistaken setting is clamped, not obeyed.
const MIN_LIFETIME_HOURS: i64 = 1;
const MAX_LIFETIME_HOURS: i64 = 720; // 30 days

/// The retry pacing the administrator configured: Postfix's
/// `minimal_backoff_time` and `maximal_backoff_time`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub min_secs:  i64,
    pub max_hours: i64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { min_secs: MIN_BACKOFF_SECS, max_hours: MAX_BACKOFF_HOURS }
    }
}

impl Backoff {
    pub fn from_config(cfg: &ServerConfig) -> Self {
        Self {
            min_secs:  cfg.outbound_min_backoff_secs,
            max_hours: cfg.outbound_max_backoff_hours,
        }
    }

    /// Delay before the next attempt: the minimum doubled once per failed
    /// attempt, capped at the maximum. `attempts` is clamped before shifting so
    /// the shift can never exceed the width of an `i64`, and the multiplication
    /// saturates rather than wrapping — a large minimum must not roll over into
    /// a tiny (or negative) delay.
    pub fn delay_secs(self, attempts: i32) -> i64 {
        let min = self.min_secs.max(1);
        let max = self.max_hours.max(1).saturating_mul(3_600).max(min);
        let shift = attempts.clamp(0, 20) as u32;
        min.saturating_mul(1_i64 << shift).min(max)
    }
}

/// When a message enqueued now should stop being retried. `None` means "leave it
/// to the column default" — the SQL safety net stays in place, we simply do not
/// override it. Migrations are frozen once applied, so the configured lifetime
/// has to be applied here, by the code.
fn expiry_from_now(lifetime_hours: i64) -> Option<DateTime<Utc>> {
    let hours = lifetime_hours.clamp(MIN_LIFETIME_HOURS, MAX_LIFETIME_HOURS);
    chrono::Duration::try_hours(hours).and_then(|d| Utc::now().checked_add_signed(d))
}

/// One recipient claimed for a delivery attempt, with its message's payload.
#[derive(sqlx::FromRow)]
pub struct Claimed {
    pub recipient_id:  Uuid,
    pub message_id:    Uuid,
    pub recipient:     String,
    pub domain:        String,
    pub envelope_from: String,
    pub raw:           Vec<u8>,
    pub attempts:      i32,
    pub is_dsn:        bool,
    pub expired:       bool,
}

/// Enqueues one message and its recipients in a SINGLE transaction: the caller
/// may answer "accepted" only once this has committed, which is the durability
/// invariant. `recipients` is `(address, domain)`. Returns the message id.
///
/// Kept for callers that still express the queue lifetime in days; prefer
/// [`enqueue_with_lifetime`], which takes the administrator's
/// `outbound_lifetime_hours` directly.
#[allow(clippy::too_many_arguments)]
pub async fn enqueue(
    db: &PgPool,
    user_id: Option<Uuid>,
    account_id: Option<Uuid>,
    envelope_from: &str,
    raw: &[u8],
    is_dsn: bool,
    recipients: &[(String, String)],
    expires_in_days: i64,
) -> Result<Uuid> {
    enqueue_with_lifetime(
        db,
        user_id,
        account_id,
        envelope_from,
        raw,
        is_dsn,
        recipients,
        expires_in_days.saturating_mul(24),
    )
    .await
}

/// Same, with the queue lifetime stated in hours — Postfix's
/// `maximal_queue_lifetime`, which the administrator sets in the console.
///
/// `expires_at` is computed and written HERE rather than left to the column
/// default: migration 000019 is applied and therefore frozen, so its
/// `NOW() + INTERVAL '5 days'` can only stay as a safety net for rows inserted
/// without the column. A lifetime we cannot express falls back to exactly that.
#[allow(clippy::too_many_arguments)]
pub async fn enqueue_with_lifetime(
    db: &PgPool,
    user_id: Option<Uuid>,
    account_id: Option<Uuid>,
    envelope_from: &str,
    raw: &[u8],
    is_dsn: bool,
    recipients: &[(String, String)],
    lifetime_hours: i64,
) -> Result<Uuid> {
    let mut tx = db.begin().await.context("Ouverture transaction d'enfilement")?;

    // Two shapes on purpose: omitting the column is what lets the SQL DEFAULT
    // act as the net. Binding NULL would insert NULL and violate NOT NULL.
    let expires_at = expiry_from_now(lifetime_hours);
    let sql = if expires_at.is_some() {
        r#"INSERT INTO mail.outbound_messages
             (user_id, account_id, envelope_from, raw, is_dsn, expires_at)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id"#
    } else {
        r#"INSERT INTO mail.outbound_messages
             (user_id, account_id, envelope_from, raw, is_dsn)
           VALUES ($1, $2, $3, $4, $5)
           RETURNING id"#
    };

    let mut insert = sqlx::query_scalar::<_, Uuid>(sql)
        .bind(user_id)
        .bind(account_id)
        .bind(envelope_from)
        .bind(raw)
        .bind(is_dsn);
    if let Some(at) = expires_at {
        insert = insert.bind(at);
    }

    let message_id: Uuid = insert
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "File sortante : insertion du message impossible");
            e
        })
        .context("Insertion du message sortant")?;

    for (recipient, domain) in recipients {
        sqlx::query(
            r#"INSERT INTO mail.outbound_recipients (message_id, recipient, domain)
               VALUES ($1, $2, $3)"#,
        )
        .bind(message_id)
        .bind(recipient)
        .bind(domain.to_ascii_lowercase())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "File sortante : insertion d'un destinataire impossible");
            e
        })
        .context("Insertion d'un destinataire sortant")?;
    }

    tx.commit().await.context("Validation de l'enfilement")?;
    Ok(message_id)
}

/// Atomically claims up to `limit` due recipients for this worker, leasing them
/// for `lease`. `FOR UPDATE SKIP LOCKED` lets several workers run without ever
/// handing the same recipient to two of them; the lease makes a crashed
/// worker's rows claimable again once it expires (orphan reclaim).
pub async fn claim_batch(
    db: &PgPool,
    worker_id: Uuid,
    lease: Duration,
    limit: i64,
) -> Result<Vec<Claimed>> {
    let rows = sqlx::query_as::<_, Claimed>(
        r#"WITH due AS (
               SELECT r.id
               FROM mail.outbound_recipients r
               WHERE r.status IN ('queued', 'deferred')
                 AND r.next_attempt_at <= NOW()
                 AND (r.locked_until IS NULL OR r.locked_until < NOW())
               ORDER BY r.next_attempt_at
               LIMIT $3
               FOR UPDATE SKIP LOCKED
           )
           UPDATE mail.outbound_recipients r
           SET status = 'delivering', locked_by = $1, locked_until = NOW() + ($2 || ' seconds')::interval
           FROM due, mail.outbound_messages m
           WHERE r.id = due.id AND m.id = r.message_id
           RETURNING
               r.id            AS recipient_id,
               r.message_id    AS message_id,
               r.recipient     AS recipient,
               r.domain        AS domain,
               m.envelope_from AS envelope_from,
               m.raw           AS raw,
               r.attempts      AS attempts,
               m.is_dsn        AS is_dsn,
               (NOW() >= m.expires_at) AS expired"#,
    )
    .bind(worker_id)
    .bind(lease.as_secs().to_string())
    .bind(limit)
    .fetch_all(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "File sortante : réclamation impossible");
        e
    })
    .context("Réclamation de lot sortant")?;

    Ok(rows)
}

/// Marks a recipient delivered. Called in response to a 2xx on the final `.`.
pub async fn mark_sent(db: &PgPool, recipient_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE mail.outbound_recipients
         SET status = 'sent', delivered_at = NOW(), locked_by = NULL, locked_until = NULL
         WHERE id = $1",
    )
    .bind(recipient_id)
    .execute(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "File sortante : marquage 'sent' impossible");
        e
    })
    .context("Marquage 'sent'")?;
    Ok(())
}

/// Defers a recipient: bumps the attempt count, records the reason, and pushes
/// the next attempt out by an exponential backoff bounded by the administrator's
/// `outbound_min_backoff_secs` / `outbound_max_backoff_hours`.
pub async fn mark_deferred(
    db: &PgPool,
    recipient_id: Uuid,
    attempts: i32,
    code: u16,
    reason: &str,
    backoff: Backoff,
) -> Result<()> {
    let backoff = backoff.delay_secs(attempts);
    sqlx::query(
        "UPDATE mail.outbound_recipients
         SET status = 'deferred', attempts = attempts + 1,
             last_code = $2, last_reason = $3,
             next_attempt_at = NOW() + ($4 || ' seconds')::interval,
             locked_by = NULL, locked_until = NULL
         WHERE id = $1",
    )
    .bind(recipient_id)
    .bind(code as i32)
    .bind(reason)
    .bind(backoff.to_string())
    .execute(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "File sortante : marquage 'deferred' impossible");
        e
    })
    .context("Marquage 'deferred'")?;
    Ok(())
}

/// Marks a recipient permanently failed (5xx or expired). The worker then emits
/// a DSN — except for a message that is itself a DSN.
pub async fn mark_bounced(db: &PgPool, recipient_id: Uuid, code: u16, reason: &str) -> Result<()> {
    sqlx::query(
        "UPDATE mail.outbound_recipients
         SET status = 'bounced', last_code = $2, last_reason = $3,
             locked_by = NULL, locked_until = NULL
         WHERE id = $1",
    )
    .bind(recipient_id)
    .bind(code as i32)
    .bind(reason)
    .execute(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "File sortante : marquage 'bounced' impossible");
        e
    })
    .context("Marquage 'bounced'")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped pacing must not move when nobody configures anything.
    #[test]
    fn backoff_grows_then_caps() {
        let b = Backoff::default();
        assert_eq!(b.delay_secs(0), 300);
        assert_eq!(b.delay_secs(1), 600);
        assert_eq!(b.delay_secs(2), 1_200);
        assert_eq!(b.delay_secs(20), 4 * 3_600, "plafonné");
    }

    /// The bounds come from the console, and both ends are honoured.
    #[test]
    fn backoff_follows_the_configured_bounds() {
        let b = Backoff { min_secs: 60, max_hours: 1 };
        assert_eq!(b.delay_secs(0), 60);
        assert_eq!(b.delay_secs(1), 120);
        assert_eq!(b.delay_secs(6), 3_600, "plafonné à une heure");
        assert_eq!(b.delay_secs(30), 3_600);
    }

    /// A nonsensical setting must never produce a zero, negative, or wrapped
    /// delay — that would turn the queue into a tight retry loop against a
    /// remote server.
    #[test]
    fn backoff_never_overflows_or_goes_backwards() {
        for b in [
            Backoff { min_secs: 0, max_hours: 0 },
            Backoff { min_secs: -1, max_hours: -1 },
            Backoff { min_secs: i64::MAX, max_hours: i64::MAX },
            Backoff { min_secs: 7_200, max_hours: 1 },
        ] {
            for attempts in [-5, 0, 1, 20, i32::MAX] {
                let delay = b.delay_secs(attempts);
                assert!(delay > 0, "{b:?} après {attempts} tentatives → {delay}");
            }
        }
        // A minimum larger than the maximum is a contradiction; the minimum wins,
        // because retrying sooner than asked is the harmful direction.
        assert_eq!(Backoff { min_secs: 7_200, max_hours: 1 }.delay_secs(0), 7_200);
    }

    /// The lifetime is applied by the code (the migration is frozen) and is
    /// clamped, so a mistaken setting cannot make a message immortal nor expire
    /// it before its first retry.
    #[test]
    fn lifetime_is_clamped_into_the_future() {
        let now = Utc::now();
        let short = expiry_from_now(0).expect("échéance calculable");
        assert!(short > now, "une durée nulle est ramenée au plancher");

        let long = expiry_from_now(i64::MAX).expect("échéance calculable");
        assert!(
            long <= now + chrono::Duration::hours(MAX_LIFETIME_HOURS) + chrono::Duration::minutes(1),
            "une durée absurde est ramenée au plafond"
        );

        let normal = expiry_from_now(120).expect("échéance calculable");
        assert!(normal > now + chrono::Duration::hours(119));
        assert!(normal < now + chrono::Duration::hours(121));
    }

    /// The historical day-based entry point keeps meaning days.
    #[test]
    fn days_and_hours_agree() {
        let five_days = expiry_from_now(5 * 24).expect("échéance calculable");
        let hundred_twenty_hours = expiry_from_now(120).expect("échéance calculable");
        assert!((five_days - hundred_twenty_hours).num_seconds().abs() <= 1);
    }
}
