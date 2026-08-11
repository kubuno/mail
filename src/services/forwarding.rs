//! Automatic forwarding — re-sending an incoming message to another address.
//!
//! The client edits a list of destination addresses (each enabled or not) and a
//! "keep or archive the local copy" choice; this module persists those rules and,
//! on every message delivered into a local mailbox, decides whether to re-send a
//! copy and does so through the instance's own send path — the same one the
//! vacation responder uses.
//!
//! ── Why the anti-loop rules are the hard part ────────────────────────────────
//! Forwarding that re-sends the wrong thing re-sends it forever: two mailboxes
//! that forward to each other bounce the same message back and forth, a bounce
//! forwarded as a fresh message generates a bounce of a bounce, and an
//! auto-reply forwarded onward fans out. The industry answer (a `Delivered-To` /
//! `X-Forwarded-For` style marker, plus the RFC 3834 refusals) is what
//! [`evaluate_forward_guard`] implements:
//!
//!   * our own [`FORWARD_MARKER`] already present → the message has been through
//!     forwarding once; re-sending it is a loop. This is the hard stop that
//!     bounds the `deliver_local` → forward → `deliver_local` recursion at
//!     depth one, since the copy we emit carries the marker;
//!   * `Auto-Submitted:` present and not `no` → an automatic message (vacation
//!     reply, notification) is never forwarded;
//!   * a null return-path (`<>` / empty) or a `MAILER-DAEMON` / `postmaster`
//!     sender → a bounce or system notification, never forwarded;
//!   * `forward_to` == the mailbox itself → forwarding to yourself is a loop.
//!
//! The forwarded copy is stamped with [`FORWARD_MARKER`] carrying the mailbox
//! that forwarded it, so the far side (or a re-entry into local delivery) has the
//! evidence to stop.

use anyhow::{Context, Result};
use sqlx::PgPool;
use uuid::Uuid;

use crate::server::config::ServerConfig;
use crate::server::{hygiene, queue, resolve};

/// The trace header stamped on every forwarded copy, carrying the mailbox that
/// forwarded it. Its presence on an incoming message is the loop signal — we
/// never forward a message that already has it (see [`evaluate_forward_guard`]).
pub const FORWARD_MARKER: &str = "X-Kubuno-Forwarded";

// ── Persisted configuration ─────────────────────────────────────────────────

/// One forwarding rule: a destination and its flags, as the delivery path needs.
#[derive(Debug, Clone)]
pub struct ForwardingRule {
    pub forward_to: String,
    pub enabled:    bool,
    pub keep_copy:  bool,
}

/// The rule row as it comes back from Postgres.
type RuleRow = (String, bool, bool);

/// Reads every forwarding rule of a user, in insertion order.
pub async fn load(db: &PgPool, user_id: Uuid) -> Result<Vec<ForwardingRule>> {
    let rows: Vec<RuleRow> = sqlx::query_as(
        "SELECT forward_to, enabled, keep_copy
         FROM mail.forwarding_rules WHERE user_id = $1
         ORDER BY created_at, forward_to",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .context("Lecture des règles de transfert")?;

    Ok(rows
        .into_iter()
        .map(|(forward_to, enabled, keep_copy)| ForwardingRule { forward_to, enabled, keep_copy })
        .collect())
}

/// Replaces a user's whole set of forwarding rules in one transaction: the UI
/// edits the list as a whole and saves it as a whole, so the server mirrors that
/// — delete what was there, insert what is there now — rather than diffing.
///
/// `rules` is `(forward_to, enabled, keep_copy)`; addresses are lower-cased here.
pub async fn replace(db: &PgPool, user_id: Uuid, rules: &[(String, bool, bool)]) -> Result<()> {
    let mut tx = db.begin().await.context("Ouverture de la transaction de transfert")?;

    sqlx::query("DELETE FROM mail.forwarding_rules WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Purge des règles de transfert échouée");
            e
        })
        .context("Purge des règles de transfert")?;

    for (forward_to, enabled, keep_copy) in rules {
        sqlx::query(
            "INSERT INTO mail.forwarding_rules (user_id, forward_to, enabled, keep_copy, updated_at)
             VALUES ($1, LOWER($2), $3, $4, NOW())
             ON CONFLICT (user_id, forward_to) DO UPDATE SET
                enabled = EXCLUDED.enabled,
                keep_copy = EXCLUDED.keep_copy,
                updated_at = NOW()",
        )
        .bind(user_id)
        .bind(forward_to.trim())
        .bind(enabled)
        .bind(keep_copy)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Insertion d'une règle de transfert échouée");
            e
        })
        .context("Insertion d'une règle de transfert")?;
    }

    tx.commit().await.context("Validation des règles de transfert")?;
    Ok(())
}

// ── Pure decision logic (anti-loop), unit-tested ────────────────────────────

/// The header/sender facts the loop guard reasons about, pulled from the
/// incoming message by the delivery path.
pub struct ForwardGuardInput<'a> {
    /// The incoming message already carries our [`FORWARD_MARKER`] — it has been
    /// forwarded once, so forwarding it again is a loop.
    pub already_forwarded: bool,
    /// Value of the incoming `Auto-Submitted` header, if any.
    pub auto_submitted:    Option<&'a str>,
    /// The SMTP envelope sender (Return-Path). `<>` / empty for a bounce.
    pub envelope_from:     &'a str,
    /// The `From` address of the incoming message.
    pub from_email:        &'a str,
    /// The local mailbox that received the message (its canonical address).
    pub mailbox_email:     &'a str,
    /// The destination this rule would forward to.
    pub forward_to:        &'a str,
}

/// The guard's verdict. `Skip` carries a stable reason string for the logs.
#[derive(Debug, PartialEq, Eq)]
pub enum ForwardDecision {
    Forward,
    Skip(&'static str),
}

/// Decide whether a copy of this message may be forwarded to `forward_to`.
///
/// This is the single place the "never forward X" rules live; it is pure so
/// every branch is unit-tested. It does NOT consider whether a rule exists or is
/// enabled — that needs the database and is handled by [`maybe_forward`].
pub fn evaluate_forward_guard(input: &ForwardGuardInput) -> ForwardDecision {
    // Hard stop: a message already stamped by us has been forwarded once. This
    // is what bounds the deliver_local → forward → deliver_local recursion, and
    // what breaks a mutual-forwarding loop between two mailboxes.
    if input.already_forwarded {
        return ForwardDecision::Skip("already-forwarded");
    }

    // Never forward another automatic message (a vacation reply, a read receipt,
    // a notification). Only `no` is safe; anything else means "automatic".
    if let Some(v) = input.auto_submitted {
        if !v.trim().eq_ignore_ascii_case("no") {
            return ForwardDecision::Skip("auto-submitted");
        }
    }

    // A null return-path (`<>` / empty) is a bounce or notification: forwarding
    // it would re-send a delivery failure as if it were fresh mail.
    let env = strip_brackets(input.envelope_from);
    if env.is_empty() {
        return ForwardDecision::Skip("bounce");
    }

    // A daemon/postmaster/no-reply sender (envelope or header From) is a system
    // notification — a bounce in spirit — and is never forwarded.
    if is_system_sender(&env) || is_system_sender(input.from_email) {
        return ForwardDecision::Skip("bounce");
    }

    // Forwarding to the mailbox itself is an immediate loop.
    let fwd = input.forward_to.trim().to_ascii_lowercase();
    let box_lc = input.mailbox_email.trim().to_ascii_lowercase();
    if fwd.is_empty() || fwd == box_lc {
        return ForwardDecision::Skip("self");
    }

    ForwardDecision::Forward
}

/// Lower-cases and strips the angle brackets of an envelope address; `<>`
/// becomes the empty string.
fn strip_brackets(addr: &str) -> String {
    addr.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_ascii_lowercase()
}

/// A system address whose mail must never be forwarded: the daemon/postmaster
/// mailboxes and any `no-reply` variant.
fn is_system_sender(addr: &str) -> bool {
    let addr = addr.trim().to_ascii_lowercase();
    if addr.is_empty() {
        return false; // emptiness is handled by the null-return-path rule
    }
    let local = addr.split('@').next().unwrap_or("");
    matches!(local, "mailer-daemon" | "postmaster")
        || local.contains("no-reply")
        || local.contains("noreply")
}

/// Returns `raw` with the [`FORWARD_MARKER`] header prepended, carrying the
/// mailbox that forwarded it. The value is sanitised so it cannot inject a
/// second header. Prepending a header is valid RFC 5322 (the order of distinct
/// header fields is not significant).
fn stamp_forward_marker(raw: &[u8], mailbox: &str) -> Vec<u8> {
    let header = format!("{}: {}\r\n", FORWARD_MARKER, hygiene::sanitize_header_value(mailbox));
    let mut out = Vec::with_capacity(header.len() + raw.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(raw);
    out
}

// ── Orchestration: decide + forward, on a delivered message ─────────────────

/// The incoming-message facts the forwarder needs, gathered once by the caller.
pub struct Incoming<'a> {
    pub envelope_from:     &'a str,
    pub from_email:        &'a str,
    pub auto_submitted:    Option<&'a str>,
    /// The incoming message already carries our [`FORWARD_MARKER`].
    pub already_forwarded: bool,
    /// The message was filed into Spam — junk is never forwarded.
    pub is_spam:           bool,
}

/// Forwards a copy of the just-delivered `raw` to every active destination the
/// recipient configured, when the anti-loop guard permits it. When a forwarded
/// rule asks not to keep the local copy, archives the stored message out of the
/// inbox once at least one copy actually left.
///
/// Best-effort throughout: forwarding is a courtesy, never a reason to fail or
/// delay the delivery that already succeeded, so every error is logged and
/// swallowed rather than propagated.
#[allow(clippy::too_many_arguments)]
pub async fn maybe_forward(
    db: &PgPool,
    cfg: &ServerConfig,
    attachments_dir: &str,
    recipient_user_id: Uuid,
    recipient_account_id: Uuid,
    recipient_address: &str,
    incoming: &Incoming<'_>,
    raw: &[u8],
    stored_msg_id: Uuid,
) {
    // Junk is never forwarded.
    if incoming.is_spam {
        return;
    }

    let rules = match load(db, recipient_user_id).await {
        Ok(rules) => rules,
        Err(e) => {
            tracing::warn!(error = %e, "Transfert : lecture des règles");
            return;
        }
    };
    let active: Vec<ForwardingRule> = rules.into_iter().filter(|r| r.enabled).collect();
    if active.is_empty() {
        return;
    }

    // The mailbox's canonical address: used both to stamp the marker and as the
    // self-forward check target.
    let mailbox = mailbox_address(db, recipient_account_id)
        .await
        .unwrap_or_else(|| recipient_address.trim().to_ascii_lowercase());

    let marked = stamp_forward_marker(raw, &mailbox);

    let mut any_forwarded = false;
    let mut want_archive = false;
    for rule in &active {
        let guard = ForwardGuardInput {
            already_forwarded: incoming.already_forwarded,
            auto_submitted:    incoming.auto_submitted,
            envelope_from:     incoming.envelope_from,
            from_email:        incoming.from_email,
            mailbox_email:     &mailbox,
            forward_to:        &rule.forward_to,
        };
        if let ForwardDecision::Skip(reason) = evaluate_forward_guard(&guard) {
            tracing::debug!(reason, to = %rule.forward_to, "Transfert : ignoré (anti-boucle)");
            continue;
        }

        match deliver_forward(db, cfg, attachments_dir, incoming.envelope_from, &rule.forward_to, &marked).await {
            Ok(true) => {
                any_forwarded = true;
                if !rule.keep_copy {
                    want_archive = true;
                }
                tracing::info!(from = %mailbox, to = %rule.forward_to, "Message réexpédié");
            }
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %e, to = %rule.forward_to, "Transfert : envoi échoué"),
        }
    }

    // "Archive the local copy" only makes sense once a copy actually left, and
    // only touches a message still in the inbox (a filter may have moved it).
    if any_forwarded && want_archive {
        if let Err(e) = sqlx::query(
            "UPDATE mail.messages SET folder = 'archive' WHERE id = $1 AND folder = 'inbox'",
        )
        .bind(stored_msg_id)
        .execute(db)
        .await
        {
            tracing::error!(error = %e, msg = %stored_msg_id, "Transfert : archivage de la copie locale échoué");
        }
    }
}

/// Hands the stamped copy to the right send path — local delivery for a
/// destination in our own domains, the outbound queue otherwise. Returns whether
/// the copy actually left (delivered or queued); a remote destination with
/// outbound delivery off cannot leave.
async fn deliver_forward(
    db: &PgPool,
    cfg: &ServerConfig,
    attachments_dir: &str,
    envelope_from: &str,
    forward_to: &str,
    marked_raw: &[u8],
) -> Result<bool> {
    let fwd_lc = forward_to.trim().to_ascii_lowercase();

    if cfg.is_local_domain(&fwd_lc) {
        // A local destination: file the copy straight into their mailbox, exactly
        // as an inbound message would be. The marker travels with `marked_raw`, so
        // that delivery's own forwarder refuses to forward it again.
        let directory = resolve::PgDirectory::new(db);
        match resolve::resolve(&directory, cfg, envelope_from, true, &fwd_lc).await {
            Ok(resolve::Outcome::Accept(expansion)) => {
                let mut delivered = false;
                for delivery in expansion.local {
                    // `deliver_local` → `maybe_forward` → `deliver_local` is
                    // mutual async recursion; `Box::pin` gives the future a size.
                    // It terminates at depth two: the copy carries the marker, so
                    // the destination's own guard refuses to forward it onward.
                    match Box::pin(crate::server::deliver::deliver_local(
                        db,
                        cfg,
                        envelope_from,
                        &delivery.address,
                        delivery.target,
                        marked_raw,
                        attachments_dir,
                        crate::server::deliver::Disposition::Inbox,
                        None,
                        None,
                    ))
                    .await
                    {
                        Ok(_) => delivered = true,
                        Err(e) => tracing::warn!(error = %e, "Transfert : dépôt local de la copie"),
                    }
                }
                Ok(delivered)
            }
            Ok(resolve::Outcome::Refuse(_)) => Ok(false),
            Err(e) => Err(e),
        }
    } else if cfg.outbound_enabled {
        // A remote destination: hand to the outbound queue. The original envelope
        // sender is preserved so a delivery failure reaches the real sender, as a
        // `.forward` does.
        let stamped = hygiene::prepend_received(marked_raw, "local", &cfg.hostname);
        let domain = fwd_lc.rsplit_once('@').map(|(_, d)| d.to_string()).unwrap_or_default();
        queue::enqueue_with_lifetime(
            db,
            None,
            None,
            envelope_from,
            &stamped,
            false,
            &[(fwd_lc, domain)],
            cfg.outbound_lifetime_hours,
        )
        .await?;
        Ok(true)
    } else {
        // Remote destination, outbound disabled: nowhere to go.
        Ok(false)
    }
}

/// The canonical address of the local account that received the message.
async fn mailbox_address(db: &PgPool, account_id: Uuid) -> Option<String> {
    match sqlx::query_scalar::<_, String>("SELECT email_address FROM mail.accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(db)
        .await
    {
        Ok(addr) => addr.map(|a| a.trim().to_ascii_lowercase()).filter(|a| a.contains('@')),
        Err(e) => {
            tracing::warn!(error = %e, "Transfert : lecture de l'adresse de la boîte");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input<'a>() -> ForwardGuardInput<'a> {
        ForwardGuardInput {
            already_forwarded: false,
            auto_submitted:    None,
            envelope_from:     "alice@example.com",
            from_email:        "alice@example.com",
            mailbox_email:     "me@kubuno.com",
            forward_to:        "elsewhere@other.com",
        }
    }

    // ── The one case we DO forward ──────────────────────────────────────────
    #[test]
    fn ordinary_mail_is_forwarded() {
        assert_eq!(evaluate_forward_guard(&base_input()), ForwardDecision::Forward);
    }

    // ── Every refusal ───────────────────────────────────────────────────────
    #[test]
    fn a_message_already_marked_is_not_forwarded_again() {
        let input = ForwardGuardInput { already_forwarded: true, ..base_input() };
        assert_eq!(evaluate_forward_guard(&input), ForwardDecision::Skip("already-forwarded"));
    }

    #[test]
    fn the_marker_wins_over_an_otherwise_forwardable_message() {
        // Even a perfectly ordinary message is refused once it carries the marker:
        // this is the loop stop, and it must not be overridden by later checks.
        let input = ForwardGuardInput {
            already_forwarded: true,
            auto_submitted:    Some("no"),
            ..base_input()
        };
        assert_eq!(evaluate_forward_guard(&input), ForwardDecision::Skip("already-forwarded"));
    }

    #[test]
    fn auto_submitted_not_no_is_refused() {
        for v in ["auto-replied", "auto-generated", "auto-notified", "AUTO-REPLIED"] {
            let input = ForwardGuardInput { auto_submitted: Some(v), ..base_input() };
            assert_eq!(evaluate_forward_guard(&input), ForwardDecision::Skip("auto-submitted"), "{v}");
        }
    }

    #[test]
    fn auto_submitted_no_is_still_forwarded() {
        let input = ForwardGuardInput { auto_submitted: Some("no"), ..base_input() };
        assert_eq!(evaluate_forward_guard(&input), ForwardDecision::Forward);
    }

    #[test]
    fn a_bounce_null_return_path_is_refused() {
        for env in ["<>", "", "  ", "< >"] {
            let input = ForwardGuardInput { envelope_from: env, ..base_input() };
            assert_eq!(evaluate_forward_guard(&input), ForwardDecision::Skip("bounce"), "{env:?}");
        }
    }

    #[test]
    fn a_system_sender_is_refused() {
        for addr in ["mailer-daemon@example.com", "postmaster@example.com",
                     "no-reply@shop.com", "noreply@bank.com", "No-Reply@X.com"] {
            let input = ForwardGuardInput { envelope_from: addr, from_email: addr, ..base_input() };
            assert_eq!(evaluate_forward_guard(&input), ForwardDecision::Skip("bounce"), "{addr}");
        }
    }

    #[test]
    fn a_system_sender_on_header_from_alone_is_refused() {
        let input = ForwardGuardInput {
            envelope_from: "bounce+abc@example.com",
            from_email:    "noreply@example.com",
            ..base_input()
        };
        assert_eq!(evaluate_forward_guard(&input), ForwardDecision::Skip("bounce"));
    }

    #[test]
    fn forwarding_to_oneself_is_refused() {
        let input = ForwardGuardInput {
            mailbox_email: "me@kubuno.com",
            forward_to:    "ME@Kubuno.com",
            ..base_input()
        };
        assert_eq!(evaluate_forward_guard(&input), ForwardDecision::Skip("self"));
    }

    #[test]
    fn an_empty_destination_is_refused() {
        let input = ForwardGuardInput { forward_to: "   ", ..base_input() };
        assert_eq!(evaluate_forward_guard(&input), ForwardDecision::Skip("self"));
    }

    // ── The marker we stamp ─────────────────────────────────────────────────
    #[test]
    fn forwarded_copy_carries_the_marker_with_the_mailbox() {
        let out = stamp_forward_marker(b"From: a@b.c\r\n\r\nbody\r\n", "me@kubuno.com");
        let text = String::from_utf8_lossy(&out);
        assert!(text.starts_with("X-Kubuno-Forwarded: me@kubuno.com\r\n"), "marker prepended: {text:?}");
        assert!(text.contains("From: a@b.c"), "original headers preserved");
    }

    #[test]
    fn the_marker_value_cannot_inject_a_second_header() {
        // A crafted mailbox address must not be able to smuggle a CRLF and forge
        // another header line: the value's CRLF is folded to spaces, so `Bcc:`
        // stays part of our one header value instead of starting a header of its
        // own.
        let out = stamp_forward_marker(b"body", "evil\r\nBcc: victim@x");
        let text = String::from_utf8_lossy(&out);
        // Only the marker's own CRLF (the one we wrote) precedes the body.
        assert_eq!(text.matches("\r\n").count(), 1, "no injected CRLF: {text:?}");
        assert!(!text.contains("\r\nBcc:"), "no forged header boundary: {text:?}");
        assert!(text.starts_with("X-Kubuno-Forwarded: "), "still a single marker header");
    }
}
