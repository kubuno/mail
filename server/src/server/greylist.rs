//! Greylisting for the reception listener (RFC 6647 §2.2).
//!
//! The bet is simple and it is the reason greylisting still works: a real MTA
//! has a queue and retries, most spam engines fire once and move on. So the
//! first time a (client network, envelope sender, envelope recipient) triplet
//! shows up we answer `450` and write it down; the retry that comes back a few
//! minutes later is delivered and the triplet is trusted from then on. A
//! legitimate correspondent pays one delay, once.
//!
//! What this deliberately does NOT do, because each of these turns greylisting
//! from a filter into an outage:
//!
//! * it never applies to a submission listener — an interactive mail client
//!   does not retry, it shows the user an error;
//! * it never applies to an authenticated session, for the same reason;
//! * it never applies to a sender the operator allow-listed;
//! * it never applies to a trusted upstream relay — that relay opens a large
//!   number of legitimate connections and deferring it would stall real mail;
//! * and a database failure lets the mail THROUGH. A filter that cannot reach
//!   its state must not become a mail outage; the failure is logged loudly and
//!   the message is delivered.
//!
//! It is also decided at RCPT time, which is the only place it can be: the
//! triplet needs the recipient, and deferring before DATA means the sender
//! never transmits the body at all.
//!
//! The decision itself ([`decide`]) is pure so it can be tested without a
//! database; everything touching PostgreSQL is a thin wrapper around it.

use std::net::{IpAddr, Ipv6Addr};
use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{DateTime, Duration, Utc};
use kubuno_db::DbPool;

use crate::server::config::ServerConfig;
use crate::server::hygiene;

/// How long a triplet that PASSED stays trusted after its last use. postgrey's
/// default; there is no admin setting for it, and there does not need to be —
/// it only decides how often a long-silent correspondent pays the delay again.
const TRUST_TTL_DAYS: i64 = 35;

/// Minimum interval between two purge runs. The purge is a bounded, indexed
/// DELETE, but it has no business running on every RCPT.
const PURGE_EVERY_SECS: i64 = 3_600;

/// Unix timestamp of the last purge, so one instance runs it about hourly
/// whatever the traffic. `0` = never.
static LAST_PURGE: AtomicI64 = AtomicI64::new(0);

/// What the caller must do with this RCPT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Accept the recipient.
    Pass,
    /// Answer `450 4.7.1` and ask the sender to come back later.
    Defer,
}

/// The stored row, reduced to the two columns the decision reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub first_seen: DateTime<Utc>,
    pub passed_at:  Option<DateTime<Utc>>,
}

/// The decision, spelled out so the caller knows which write to perform and the
/// tests can assert on the reason rather than just on pass/defer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Never seen: record the triplet on probation and defer.
    FirstContact,
    /// On probation and the retry came back before the delay elapsed. A sender
    /// that hammers us every second is not retrying, it is spinning.
    TooSoon,
    /// On probation, but the retry window has run out without a retry: the
    /// probation restarts rather than being credited.
    Expired,
    /// Retried after the delay and inside the window: the triplet is trusted
    /// from now on.
    Promote,
    /// Already trusted.
    Trusted,
}

impl Outcome {
    /// What the SMTP session should answer.
    pub fn verdict(self) -> Verdict {
        match self {
            Outcome::Promote | Outcome::Trusted => Verdict::Pass,
            Outcome::FirstContact | Outcome::TooSoon | Outcome::Expired => Verdict::Defer,
        }
    }
}

/// Whether greylisting applies to this session at all.
///
/// Kept as one predicate for the same reason `relay_decision` is: the
/// exemptions are the dangerous part, and they should be readable in one place
/// rather than spread over a chain of early returns.
pub fn applies(
    enabled: bool,
    submission: bool,
    authenticated: bool,
    allowlisted: bool,
    trusted_upstream: bool,
) -> bool {
    enabled && !submission && !authenticated && !allowlisted && !trusted_upstream
}

/// The whole decision, given the stored state and the two configured durations.
///
/// `now` is passed in (the caller reads it from the DATABASE, so a clock skew
/// between the module and PostgreSQL cannot make a triplet look older or
/// younger than it is).
pub fn decide(
    now: DateTime<Utc>,
    entry: Option<Entry>,
    delay: Duration,
    window: Duration,
) -> Outcome {
    let Some(entry) = entry else {
        return Outcome::FirstContact;
    };
    // A triplet that already passed stays passed: the point of remembering it
    // is that the sender is never delayed twice.
    if entry.passed_at.is_some() {
        return Outcome::Trusted;
    }
    let age = now - entry.first_seen;
    if age > window {
        // The sender never came back inside the window. Whatever is knocking
        // now starts its probation from scratch.
        Outcome::Expired
    } else if age < delay {
        Outcome::TooSoon
    } else {
        Outcome::Promote
    }
}

/// Reduces a client address to the network the triplet is keyed on: `/24` for
/// IPv4, `/64` for IPv6.
///
/// Keying on the exact address looks stricter and is in fact broken: a large
/// sender retries from a DIFFERENT machine of the same farm, so the retry would
/// present a brand-new triplet and be deferred again, forever. This is
/// postgrey's `--lookup-by-subnet`, which is its default for exactly that
/// reason. IPv4-mapped IPv6 addresses are folded onto their IPv4 form so the
/// same client cannot present two identities.
pub fn client_network(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.0/24", o[0], o[1], o[2])
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => client_network(IpAddr::V4(v4)),
            None => {
                let s = v6.segments();
                let net = Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0);
                format!("{net}/64")
            }
        },
    }
}

/// What the lookup brings back: the two stored columns.
type StoredRow = (DateTime<Utc>, Option<DateTime<Utc>>);

/// Runs the greylist check for one RCPT and records the result.
///
/// Returns [`Verdict::Pass`] whenever anything goes wrong: a filter that cannot
/// reach its state must not become a mail outage.
pub async fn check(
    db: &DbPool,
    cfg: &ServerConfig,
    client_ip: IpAddr,
    sender: &str,
    recipient: &str,
) -> Verdict {
    // Input validation before the database, as everywhere else in this module.
    // The recipient has already been parsed and resolved by the caller; the
    // sender may legitimately be the null return path (a DSN), which is stored
    // as the empty string.
    if !hygiene::valid_envelope_address(recipient) {
        tracing::error!(recipient = %recipient, "Greylisting : destinataire invalide — contrôle ignoré");
        return Verdict::Pass;
    }
    if !sender.is_empty() && !hygiene::valid_envelope_address(sender) {
        tracing::error!("Greylisting : expéditeur d'enveloppe invalide — contrôle ignoré");
        return Verdict::Pass;
    }

    let network = client_network(client_ip);
    let sender = sender.trim().to_ascii_lowercase();
    let recipient = recipient.trim().to_ascii_lowercase();
    let delay = Duration::seconds(cfg.greylist_delay_secs);
    let window = Duration::hours(cfg.greylist_window_hours);

    // One read. The decision and every timestamp written below use ONE clock,
    // the module's: SQLite has no server clock to defer to, and mixing the two
    // would skew the retry window by the clock difference.
    let row: Option<StoredRow> = match crate::db::query_as(
        "SELECT first_seen, passed_at \
           FROM mail.greylist \
          WHERE client_net = $1 AND sender = $2 AND recipient = $3",
    )
    .bind(&network)
    .bind(&sender)
    .bind(&recipient)
    .fetch_optional(db)
    .await
    {
        Ok(value) => value,
        Err(e) => {
            tracing::error!(error = %e, "Greylisting : lecture impossible — message laissé passer");
            return Verdict::Pass;
        }
    };

    let now = Utc::now();
    let entry = row.map(|(first_seen, passed_at)| Entry { first_seen, passed_at });
    let outcome = decide(now, entry, delay, window);

    // The write matching the decision. A concurrent session on the SAME triplet
    // can race us here; the worst it costs is one extra deferral, which is the
    // benign side of the trade.
    let backend = db.backend();
    let write_sql = match outcome {
        Outcome::FirstContact => format!(
            "INSERT {}INTO mail.greylist (client_net, sender, recipient, first_seen, last_seen) \
             VALUES ($1, $2, $3, $4, $4){}",
            backend.insert_ignore_prefix(),
            backend.on_conflict_do_nothing(&["client_net", "sender", "recipient"]),
        ),
        Outcome::TooSoon | Outcome::Trusted => "UPDATE mail.greylist SET last_seen = $4 \
              WHERE client_net = $1 AND sender = $2 AND recipient = $3"
            .to_string(),
        Outcome::Expired => "UPDATE mail.greylist SET first_seen = $4, last_seen = $4, passed_at = NULL \
              WHERE client_net = $1 AND sender = $2 AND recipient = $3"
            .to_string(),
        Outcome::Promote => "UPDATE mail.greylist SET passed_at = $4, last_seen = $4 \
              WHERE client_net = $1 AND sender = $2 AND recipient = $3"
            .to_string(),
    };

    if let Err(e) = crate::db::query(write_sql)
        .bind(&network)
        .bind(&sender)
        .bind(&recipient)
        .bind(now)
        .execute(db)
        .await
    {
        // The state was not updated. Deferring anyway would mean deferring the
        // same triplet forever, since the retry would find nothing recorded.
        tracing::error!(error = %e, "Greylisting : écriture impossible — message laissé passer");
        return Verdict::Pass;
    }

    match outcome {
        Outcome::FirstContact | Outcome::Expired => tracing::info!(
            client = %network, recipient = %recipient,
            "Greylisting : premier contact du triplet — ajourné"
        ),
        Outcome::TooSoon => tracing::debug!(client = %network, "Greylisting : nouvelle tentative trop tôt"),
        Outcome::Promote => tracing::info!(
            client = %network, recipient = %recipient,
            "Greylisting : triplet confirmé par une nouvelle tentative"
        ),
        Outcome::Trusted => {}
    }

    maybe_purge(db, window);
    outcome.verdict()
}

/// Runs the housekeeping DELETE at most once an hour, off the session's path.
///
/// The listener has no scheduler of its own, so the trigger is the traffic
/// itself; the compare-and-swap makes sure only one caller wins the slot.
fn maybe_purge(db: &DbPool, window: Duration) {
    let now = Utc::now().timestamp();
    let last = LAST_PURGE.load(Ordering::Relaxed);
    if now - last < PURGE_EVERY_SECS {
        return;
    }
    if LAST_PURGE
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return; // another session took this slot
    }

    // Detached: an SMTP client must never wait on our housekeeping.
    let db = db.clone();
    let probation_secs = window.num_seconds().max(1);
    tokio::spawn(async move {
        // The same two lifetimes as PostgreSQL's `mail.purge_greylist()`, as one
        // portable DELETE with the cutoffs computed here: a row still on
        // probation past its retry window, and a passed row unused for the
        // trust TTL. (The PostgreSQL function is left in place, unused.)
        let now = Utc::now();
        let probation_cutoff = now - Duration::seconds(probation_secs);
        let trust_cutoff = now - Duration::days(TRUST_TTL_DAYS);
        match crate::db::query(
            "DELETE FROM mail.greylist \
              WHERE (passed_at IS NULL     AND first_seen < $1) \
                 OR (passed_at IS NOT NULL AND last_seen  < $2)",
        )
        .bind(probation_cutoff)
        .bind(trust_cutoff)
        .execute(&db)
        .await
        .map(|done| done.rows_affected())
        {
            Ok(removed) if removed > 0 => {
                tracing::info!(removed, "Greylisting : triplets expirés purgés")
            }
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "Greylisting : purge impossible"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).expect("horodatage valide")
    }

    fn delay() -> Duration {
        Duration::seconds(300)
    }

    fn window() -> Duration {
        Duration::hours(4)
    }

    // ── Exemptions ──────────────────────────────────────────────────────────

    #[test]
    fn greylisting_is_off_unless_the_operator_turned_it_on() {
        assert!(!applies(false, false, false, false, false));
        assert!(applies(true, false, false, false, false));
    }

    /// A submission client is an interactive mail app: it shows the user an
    /// error, it does not queue and retry. Greylisting it breaks sending.
    #[test]
    fn submission_and_authenticated_sessions_are_exempt() {
        assert!(!applies(true, true, false, false, false));
        assert!(!applies(true, false, true, false, false));
        assert!(!applies(true, true, true, false, false));
    }

    /// The allow-list buys a bypass of greylisting and of the block lists —
    /// and of nothing else. DMARC is decided later, on the message itself.
    #[test]
    fn an_allowlisted_sender_is_exempt() {
        assert!(!applies(true, false, false, true, false));
    }

    /// A trusted upstream relay forwards a large volume of legitimate mail and
    /// does not "retry" the way a foreign MTA does — deferring it stalls real
    /// mail, so it is exempt.
    #[test]
    fn a_trusted_upstream_is_exempt() {
        assert!(!applies(true, false, false, false, true));
    }

    // ── Decision ────────────────────────────────────────────────────────────

    #[test]
    fn a_triplet_never_seen_is_deferred() {
        let outcome = decide(at(0), None, delay(), window());
        assert_eq!(outcome, Outcome::FirstContact);
        assert_eq!(outcome.verdict(), Verdict::Defer);
    }

    #[test]
    fn a_retry_before_the_delay_is_deferred_again() {
        let entry = Entry { first_seen: at(0), passed_at: None };
        let outcome = decide(at(299), Some(entry), delay(), window());
        assert_eq!(outcome, Outcome::TooSoon);
        assert_eq!(outcome.verdict(), Verdict::Defer);
    }

    #[test]
    fn a_retry_after_the_delay_and_inside_the_window_passes() {
        let entry = Entry { first_seen: at(0), passed_at: None };
        let outcome = decide(at(300), Some(entry), delay(), window());
        assert_eq!(outcome, Outcome::Promote);
        assert_eq!(outcome.verdict(), Verdict::Pass);
    }

    /// The retry window is what makes greylisting bounded: a sender that took a
    /// week to come back gets a fresh probation, not a free pass.
    #[test]
    fn a_retry_after_the_window_starts_the_probation_over() {
        let entry = Entry { first_seen: at(0), passed_at: None };
        let outcome = decide(at(4 * 3600 + 1), Some(entry), delay(), window());
        assert_eq!(outcome, Outcome::Expired);
        assert_eq!(outcome.verdict(), Verdict::Defer);
    }

    /// The whole point: the delay is paid once, not on every message.
    #[test]
    fn a_triplet_that_already_passed_is_never_delayed_again() {
        let entry = Entry { first_seen: at(0), passed_at: Some(at(400)) };
        let outcome = decide(at(90 * 24 * 3600), Some(entry), delay(), window());
        assert_eq!(outcome, Outcome::Trusted);
        assert_eq!(outcome.verdict(), Verdict::Pass);
    }

    /// Boundary: exactly at the delay the retry counts, exactly at the window
    /// edge the entry is still alive.
    #[test]
    fn the_boundaries_favour_the_sender() {
        let entry = Entry { first_seen: at(0), passed_at: None };
        assert_eq!(decide(at(300), Some(entry), delay(), window()), Outcome::Promote);
        assert_eq!(decide(at(4 * 3600), Some(entry), delay(), window()), Outcome::Promote);
    }

    // ── Client normalisation ────────────────────────────────────────────────

    #[test]
    fn ipv4_is_keyed_on_its_slash_24() {
        let a: IpAddr = "192.0.2.10".parse().expect("ip valide");
        let b: IpAddr = "192.0.2.250".parse().expect("ip valide");
        assert_eq!(client_network(a), "192.0.2.0/24");
        // A retry from another host of the same farm is the SAME triplet.
        assert_eq!(client_network(a), client_network(b));
    }

    #[test]
    fn a_different_network_is_a_different_triplet() {
        let a: IpAddr = "192.0.2.10".parse().expect("ip valide");
        let b: IpAddr = "198.51.100.10".parse().expect("ip valide");
        assert_ne!(client_network(a), client_network(b));
    }

    #[test]
    fn ipv6_is_keyed_on_its_slash_64() {
        let a: IpAddr = "2001:db8:1:2:3:4:5:6".parse().expect("ip valide");
        let b: IpAddr = "2001:db8:1:2:ffff::1".parse().expect("ip valide");
        assert_eq!(client_network(a), "2001:db8:1:2::/64");
        assert_eq!(client_network(a), client_network(b));

        let c: IpAddr = "2001:db8:1:3::1".parse().expect("ip valide");
        assert_ne!(client_network(a), client_network(c));
    }

    /// An IPv4-mapped address is the same client as its IPv4 form; letting it
    /// key a second row would hand a free first contact to anyone who connects
    /// over a dual-stack socket.
    #[test]
    fn an_ipv4_mapped_address_folds_onto_its_ipv4_network() {
        let mapped: IpAddr = "::ffff:192.0.2.10".parse().expect("ip valide");
        let plain: IpAddr = "192.0.2.10".parse().expect("ip valide");
        assert_eq!(client_network(mapped), client_network(plain));
    }
}
