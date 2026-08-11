//! "Send mail as" identities — the ownership-verified sender addresses.
//!
//! A user may add an address they want to send mail as; before it can be used
//! the instance mails a confirmation code TO that address and the user enters it
//! back, proving control (Gmail's flow). This module owns the persisted rows and
//! the pure decision logic; the HTTP surface and the code-delivery live in
//! `handlers::send_as`.
//!
//! Everything that decides *whether a code is accepted* is a pure function here
//! (`code_is_valid`, `can_resend`, `is_valid_email`, `generate_code`), taking an
//! injected `now` rather than reading the clock, so every branch is unit-tested
//! without a database.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use sqlx::PgPool;
use uuid::Uuid;

/// Length of a confirmation code — long enough not to be guessable in the resend
/// window, short enough to retype from another mailbox.
pub const CODE_LEN: usize = 8;

/// How long a freshly issued code stays valid.
pub const CODE_TTL_HOURS: i64 = 24;

/// A code may be regenerated (add / resend) at most once per this many seconds —
/// the minimal anti-abuse guard against hammering a stranger's inbox.
pub const RESEND_MIN_INTERVAL_SECS: i64 = 60;

// ── Pure logic (unit-tested, no clock, no database) ─────────────────────────

/// Generates a confirmation code from an unambiguous alphabet — no O/0, no
/// l/1/I, since the user retypes it into another mail client and a lookalike
/// reads as a wrong code with no way to tell.
pub fn generate_code() -> String {
    // Unambiguous alphabet: excludes O/0, I/1 AND L (an l/1 lookalike) — the
    // last was present by mistake, which the shape test occasionally caught.
    const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..CODE_LEN)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Trims and lower-cases an address for storage and comparison.
pub fn normalize_email(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

/// A deliberately conservative RFC 5321-ish check: exactly one `@`, a non-empty
/// local part, a domain that has a dot and no leading/trailing dot, no
/// whitespace, and within the 320-char column. Not a full grammar — just enough
/// to refuse obvious junk before we try to mail a code to it.
pub fn is_valid_email(email: &str) -> bool {
    let e = email.trim();
    if e.is_empty() || e.len() > 320 || e.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    let mut parts = e.split('@');
    let (local, domain) = match (parts.next(), parts.next(), parts.next()) {
        (Some(l), Some(d), None) => (l, d),
        _ => return false, // zero or more than one '@'
    };
    if local.is_empty() || domain.is_empty() {
        return false;
    }
    if domain.starts_with('.') || domain.ends_with('.') || !domain.contains('.') {
        return false;
    }
    // Reject an empty label (e.g. "a..b").
    !domain.split('.').any(|label| label.is_empty())
}

/// Is the entered code accepted? True only when a code is pending, it has not
/// expired at `now`, and it matches `input` (case-insensitive, trimmed).
///
/// Pure by design: `now` is injected so expiry is tested deterministically.
pub fn code_is_valid(
    stored: Option<&str>,
    expires_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    input: &str,
) -> bool {
    let stored = match stored {
        Some(s) if !s.is_empty() => s,
        _ => return false, // no pending code (never issued, or already verified)
    };
    let expires_at = match expires_at {
        Some(e) => e,
        None => return false,
    };
    if now > expires_at {
        return false; // expired
    }
    let input = input.trim();
    !input.is_empty() && stored.eq_ignore_ascii_case(input)
}

/// May a new code be issued now? Only once the minimum interval has elapsed
/// since the row was last written (`updated_at`), bounding resend frequency.
pub fn can_resend(updated_at: DateTime<Utc>, now: DateTime<Utc>, min_interval_secs: i64) -> bool {
    now - updated_at >= Duration::seconds(min_interval_secs)
}

/// A fresh code together with the instant it stops being accepted.
pub struct IssuedCode {
    pub code:       String,
    pub expires_at: DateTime<Utc>,
}

/// Issues a new code valid for [`CODE_TTL_HOURS`] from `now`.
pub fn issue_code(now: DateTime<Utc>) -> IssuedCode {
    IssuedCode {
        code:       generate_code(),
        expires_at: now + Duration::hours(CODE_TTL_HOURS),
    }
}

// ── Persisted rows ──────────────────────────────────────────────────────────

/// The public view of a send-as identity — never carries the code.
#[derive(Debug, Clone)]
pub struct SendAsAddress {
    pub id:             Uuid,
    pub email:          String,
    pub display_name:   String,
    pub verified:       bool,
    pub treat_as_alias: bool,
    pub created_at:     DateTime<Utc>,
}

type PublicRow = (Uuid, String, String, bool, bool, DateTime<Utc>);

fn to_public(r: PublicRow) -> SendAsAddress {
    SendAsAddress {
        id:             r.0,
        email:          r.1,
        display_name:   r.2,
        verified:       r.3,
        treat_as_alias: r.4,
        created_at:     r.5,
    }
}

/// One row's verification state, read for a resend or a verify. Carries the
/// code, so it must never be serialised to a client.
pub struct SecretState {
    pub email:        String,
    pub display_name: String,
    pub verified:     bool,
    pub code:         Option<String>,
    pub expires_at:   Option<DateTime<Utc>>,
    pub updated_at:   DateTime<Utc>,
}

type SecretRow = (String, String, bool, Option<String>, Option<DateTime<Utc>>, DateTime<Utc>);

// ── Database access (always scoped by user_id) ──────────────────────────────

/// Every send-as identity of `user_id`, newest first. Public columns only — the
/// code never leaves the database.
pub async fn list(db: &PgPool, user_id: Uuid) -> Result<Vec<SendAsAddress>> {
    let rows: Vec<PublicRow> = sqlx::query_as(
        "SELECT id, email, display_name, verified, treat_as_alias, created_at
         FROM mail.send_as_addresses
         WHERE user_id = $1
         ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .context("Lecture des adresses d'envoi")?;
    Ok(rows.into_iter().map(to_public).collect())
}

/// Does this user already have this address (case-insensitive)?
pub async fn exists(db: &PgPool, user_id: Uuid, email: &str) -> Result<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM mail.send_as_addresses
             WHERE user_id = $1 AND lower(email) = lower($2)
         )",
    )
    .bind(user_id)
    .bind(email)
    .fetch_one(db)
    .await
    .context("Vérification d'une adresse d'envoi existante")?;
    Ok(exists)
}

/// Inserts a new (unverified) identity with its pending code, returning the
/// public row.
pub async fn insert(
    db: &PgPool,
    user_id: Uuid,
    email: &str,
    display_name: &str,
    code: &str,
    expires_at: DateTime<Utc>,
) -> Result<SendAsAddress> {
    let row: PublicRow = sqlx::query_as(
        "INSERT INTO mail.send_as_addresses
             (user_id, email, display_name, verified, verification_code, verification_expires_at)
         VALUES ($1, $2, $3, FALSE, $4, $5)
         RETURNING id, email, display_name, verified, treat_as_alias, created_at",
    )
    .bind(user_id)
    .bind(email)
    .bind(display_name)
    .bind(code)
    .bind(expires_at)
    .fetch_one(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Insertion d'une adresse d'envoi échouée");
        e
    })
    .context("Insertion d'une adresse d'envoi")?;
    Ok(to_public(row))
}

/// Reads a single identity's verification state, scoped to its owner.
pub async fn load_secret(db: &PgPool, user_id: Uuid, id: Uuid) -> Result<Option<SecretState>> {
    let row: Option<SecretRow> = sqlx::query_as(
        "SELECT email, display_name, verified, verification_code, verification_expires_at, updated_at
         FROM mail.send_as_addresses
         WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await
    .context("Lecture d'une adresse d'envoi")?;

    Ok(row.map(|(email, display_name, verified, code, expires_at, updated_at)| SecretState {
        email,
        display_name,
        verified,
        code,
        expires_at,
        updated_at,
    }))
}

/// Replaces the pending code and its expiry for a resend. The `updated_at`
/// trigger bumps the resend clock. Returns the address on success.
pub async fn regenerate_code(
    db: &PgPool,
    user_id: Uuid,
    id: Uuid,
    code: &str,
    expires_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "UPDATE mail.send_as_addresses
         SET verification_code = $3, verification_expires_at = $4
         WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .bind(code)
    .bind(expires_at)
    .execute(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Régénération du code d'envoi échouée");
        e
    })
    .context("Régénération du code d'envoi")?;
    Ok(())
}

/// Marks an identity verified and clears its now-spent code.
pub async fn mark_verified(db: &PgPool, user_id: Uuid, id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE mail.send_as_addresses
         SET verified = TRUE, verification_code = NULL, verification_expires_at = NULL
         WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .execute(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Validation d'une adresse d'envoi échouée");
        e
    })
    .context("Validation d'une adresse d'envoi")?;
    Ok(())
}

/// Deletes an identity; returns how many rows went (0 = not this user's).
pub async fn delete(db: &PgPool, user_id: Uuid, id: Uuid) -> Result<u64> {
    let res = sqlx::query(
        "DELETE FROM mail.send_as_addresses WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .execute(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Suppression d'une adresse d'envoi échouée");
        e
    })
    .context("Suppression d'une adresse d'envoi")?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339).unwrap().with_timezone(&Utc)
    }

    // ── Email validation ────────────────────────────────────────────────────
    #[test]
    fn accepts_ordinary_addresses() {
        for e in ["a@b.com", "user.name+tag@sub.example.org", "X@Y.CO", " trim@me.com "] {
            assert!(is_valid_email(e), "{e} should be valid");
        }
    }

    #[test]
    fn rejects_malformed_addresses() {
        for e in [
            "", "no-at", "a@", "@b.com", "a@b", "a@@b.com", "a@b..com",
            "a@.b.com", "a@b.com.", "has space@b.com", "a b@c.com",
        ] {
            assert!(!is_valid_email(e), "{e} should be invalid");
        }
    }

    #[test]
    fn normalize_lowercases_and_trims() {
        assert_eq!(normalize_email("  User@Example.COM "), "user@example.com");
    }

    // ── Code shape ──────────────────────────────────────────────────────────
    #[test]
    fn generated_code_has_the_expected_length_and_alphabet() {
        let code = generate_code();
        assert_eq!(code.len(), CODE_LEN);
        // Unambiguous alphabet: uppercase letters and digits only, none of O/0/I/1/L.
        assert!(code.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
        assert!(!code.contains(['O', '0', 'I', '1', 'L']));
    }

    #[test]
    fn issued_code_expires_after_the_ttl() {
        let now = t("2026-08-10T12:00:00Z");
        let issued = issue_code(now);
        assert_eq!(issued.expires_at, now + Duration::hours(CODE_TTL_HOURS));
        assert_eq!(issued.code.len(), CODE_LEN);
    }

    // ── code_is_valid: the acceptance decision ──────────────────────────────
    #[test]
    fn valid_code_within_window_is_accepted() {
        let now = t("2026-08-10T12:00:00Z");
        let exp = t("2026-08-11T12:00:00Z");
        assert!(code_is_valid(Some("ABCD2345"), Some(exp), now, "ABCD2345"));
        // Case-insensitive and trimmed on input.
        assert!(code_is_valid(Some("ABCD2345"), Some(exp), now, "  abcd2345 "));
    }

    #[test]
    fn wrong_code_is_rejected() {
        let now = t("2026-08-10T12:00:00Z");
        let exp = t("2026-08-11T12:00:00Z");
        assert!(!code_is_valid(Some("ABCD2345"), Some(exp), now, "ZZZZ9999"));
    }

    #[test]
    fn expired_code_is_rejected_even_when_it_matches() {
        let exp = t("2026-08-10T12:00:00Z");
        let now = t("2026-08-10T12:00:01Z"); // one second past expiry
        assert!(!code_is_valid(Some("ABCD2345"), Some(exp), now, "ABCD2345"));
    }

    #[test]
    fn no_pending_code_or_no_expiry_is_rejected() {
        let now = t("2026-08-10T12:00:00Z");
        let exp = t("2026-08-11T12:00:00Z");
        assert!(!code_is_valid(None, Some(exp), now, "ABCD2345"));
        assert!(!code_is_valid(Some(""), Some(exp), now, "ABCD2345"));
        assert!(!code_is_valid(Some("ABCD2345"), None, now, "ABCD2345"));
        assert!(!code_is_valid(Some("ABCD2345"), Some(exp), now, "   "));
    }

    // ── Resend guard ────────────────────────────────────────────────────────
    #[test]
    fn resend_is_blocked_within_the_interval_and_allowed_after() {
        let now = t("2026-08-10T12:00:00Z");
        // Just written → blocked.
        assert!(!can_resend(now, now, RESEND_MIN_INTERVAL_SECS));
        // 59 s ago → still blocked.
        assert!(!can_resend(now - Duration::seconds(59), now, RESEND_MIN_INTERVAL_SECS));
        // Exactly 60 s ago → allowed.
        assert!(can_resend(now - Duration::seconds(60), now, RESEND_MIN_INTERVAL_SECS));
        // Long ago → allowed.
        assert!(can_resend(now - Duration::hours(2), now, RESEND_MIN_INTERVAL_SECS));
    }
}
