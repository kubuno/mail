//! Vacation responder — the out-of-office auto-reply.
//!
//! The client edits a responder (active window, subject, rich-text body, and an
//! optional "contacts only" scope); this module persists it and, on every
//! message delivered into a local mailbox, decides whether to answer and does so
//! through the instance's own send path.
//!
//! ── Why the anti-loop rules are the hard part (RFC 3834) ─────────────────────
//! An auto-responder that answers indiscriminately is an incident, not a
//! feature: two responders will bounce "I'm away" at each other forever, a reply
//! to a mailing-list post fans out to the whole list, and answering a bounce
//! generates a bounce of a bounce. RFC 3834 (Recommendations for Automatic
//! Responses to Electronic Mail) is the standard that prevents this, and
//! [`evaluate_loop_guard`] implements its refusals verbatim:
//!
//!   * `Auto-Submitted:` present and not `no` → the message is itself automatic;
//!   * `Precedence: bulk | list | junk` → bulk mail / lists are never answered;
//!   * any `List-*` header → a mailing list;
//!   * a null return-path (`<>` / empty) → a bounce or notification;
//!   * a `MAILER-DAEMON` / `postmaster` / `no-reply` sender → a system address;
//!   * `From` == the mailbox itself → do not answer yourself.
//!
//! On top of the header guard, [`rate_limited`] bounds even a legitimate
//! correspondent to ONE reply per interval (four days, as Gmail does), recorded
//! in `mail.vacation_sent`. And the reply we emit carries `Auto-Submitted:
//! auto-replied` so the OTHER side's guard stops the loop from our end too.
//!
//! ── "contacts only" ─────────────────────────────────────────────────────────
//! The mail module does not link the standalone `contacts` module (cross-module
//! imports are forbidden, and no internal contacts API is exposed to it here), so
//! "my contacts" resolves to `mail.address_index` — the addresses this user has
//! actually corresponded with, the same source that backs recipient
//! auto-completion. Documented as a deliberate scope, not the global address book.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use lettre::message::header::{Header, HeaderName, HeaderValue};
use lettre::message::{header::ContentType, Mailbox, Message, MultiPart, SinglePart};
use sqlx::PgPool;
use uuid::Uuid;

use crate::server::config::ServerConfig;
use crate::server::{hygiene, queue, resolve};

/// At most one auto-reply per sender per this many days — Gmail's rule. Prevents
/// a legitimate but chatty correspondent from receiving the same notice again
/// and again while the responder is on.
pub const RESPONSE_INTERVAL_DAYS: i64 = 4;

// ── Persisted configuration ─────────────────────────────────────────────────

/// One user's responder configuration, as the delivery path needs it.
#[derive(Debug, Clone)]
pub struct ResponderConfig {
    pub enabled:       bool,
    pub start_date:    Option<NaiveDate>,
    pub end_date:      Option<NaiveDate>,
    pub subject:       String,
    pub message_html:  String,
    pub contacts_only: bool,
}

/// The responder row as it comes back from Postgres, before being reshaped into
/// [`ResponderConfig`].
type ResponderRow = (bool, Option<NaiveDate>, Option<NaiveDate>, String, String, bool);

/// Reads a user's responder, or `None` when they never configured one.
pub async fn load(db: &PgPool, user_id: Uuid) -> Result<Option<ResponderConfig>> {
    let row: Option<ResponderRow> =
        sqlx::query_as(
            "SELECT enabled, start_date, end_date, subject, message_html, contacts_only
             FROM mail.vacation_responders WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(db)
        .await
        .context("Lecture du répondeur d'absence")?;

    Ok(row.map(|(enabled, start_date, end_date, subject, message_html, contacts_only)| {
        ResponderConfig { enabled, start_date, end_date, subject, message_html, contacts_only }
    }))
}

/// Inserts or replaces a user's responder configuration.
#[allow(clippy::too_many_arguments)]
pub async fn upsert(
    db: &PgPool,
    user_id: Uuid,
    enabled: bool,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
    subject: &str,
    message_html: &str,
    contacts_only: bool,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO mail.vacation_responders
            (user_id, enabled, start_date, end_date, subject, message_html, contacts_only, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())
         ON CONFLICT (user_id) DO UPDATE SET
            enabled = EXCLUDED.enabled,
            start_date = EXCLUDED.start_date,
            end_date = EXCLUDED.end_date,
            subject = EXCLUDED.subject,
            message_html = EXCLUDED.message_html,
            contacts_only = EXCLUDED.contacts_only,
            updated_at = NOW()",
    )
    .bind(user_id)
    .bind(enabled)
    .bind(start_date)
    .bind(end_date)
    .bind(subject)
    .bind(message_html)
    .bind(contacts_only)
    .execute(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Enregistrement du répondeur d'absence échoué");
        e
    })
    .context("Enregistrement du répondeur d'absence")?;
    Ok(())
}

// ── Pure decision logic (RFC 3834), unit-tested ─────────────────────────────

/// The header/sender facts the loop guard reasons about, pulled from the
/// incoming message by the delivery path.
pub struct LoopGuardInput<'a> {
    /// Value of the incoming `Auto-Submitted` header, if any.
    pub auto_submitted:  Option<&'a str>,
    /// Value of the incoming `Precedence` header, if any.
    pub precedence:      Option<&'a str>,
    /// Whether the incoming message carries any `List-*` header.
    pub has_list_header: bool,
    /// The SMTP envelope sender (Return-Path). `<>` / empty for a bounce.
    pub envelope_from:   &'a str,
    /// The `From` address of the incoming message.
    pub from_email:      &'a str,
    /// The local mailbox that received the message (the responder's own address).
    pub mailbox_email:   &'a str,
}

/// The guard's verdict. `Skip` carries a stable reason string for the logs.
#[derive(Debug, PartialEq, Eq)]
pub enum LoopDecision {
    Respond,
    Skip(&'static str),
}

/// RFC 3834 — decide whether an auto-reply is permitted for this message.
///
/// This is the single place the "never auto-reply to X" rules live; it is pure
/// so every branch is unit-tested. It does NOT consider the active window, the
/// contacts scope or the rate-limit — those need the database and are handled by
/// [`maybe_autorespond`].
pub fn evaluate_loop_guard(input: &LoopGuardInput) -> LoopDecision {
    // §5.1 — never respond to another automatic message. Only `no` is safe;
    // `auto-replied`, `auto-generated`, `auto-notified`, anything else → skip.
    if let Some(v) = input.auto_submitted {
        if !v.trim().eq_ignore_ascii_case("no") {
            return LoopDecision::Skip("auto-submitted");
        }
    }

    // Bulk mail and mailing-list traffic are never answered.
    if let Some(p) = input.precedence {
        let p = p.trim().to_ascii_lowercase();
        if p == "bulk" || p == "list" || p == "junk" {
            return LoopDecision::Skip("precedence-bulk");
        }
    }

    // Any List-* header (List-Id, List-Unsubscribe, List-Post…) is a mailing
    // list, whose posts must never draw an auto-reply.
    if input.has_list_header {
        return LoopDecision::Skip("list-header");
    }

    // §2 — an auto-reply MUST use a null return-path and MUST NOT be sent TO one:
    // a `<>` / empty envelope sender is a bounce or notification.
    let env = strip_brackets(input.envelope_from);
    if env.is_empty() {
        return LoopDecision::Skip("null-return-path");
    }

    // System addresses that must never be answered (either the envelope sender or
    // the header From being one is enough).
    if is_system_sender(&env) || is_system_sender(input.from_email) {
        return LoopDecision::Skip("system-sender");
    }

    // Never answer ourselves: a message whose sender is this very mailbox.
    let box_lc = input.mailbox_email.trim().to_ascii_lowercase();
    if env.eq_ignore_ascii_case(&box_lc)
        || input.from_email.trim().eq_ignore_ascii_case(&box_lc)
    {
        return LoopDecision::Skip("self");
    }

    LoopDecision::Respond
}

/// Is `today` inside the responder's active window? An absent bound is open on
/// that side.
pub fn within_window(start: Option<NaiveDate>, end: Option<NaiveDate>, today: NaiveDate) -> bool {
    if let Some(s) = start {
        if today < s {
            return false;
        }
    }
    if let Some(e) = end {
        if today > e {
            return false;
        }
    }
    true
}

/// Have we already answered this sender within the interval? `None` (never
/// answered) is never rate-limited.
pub fn rate_limited(last_sent: Option<DateTime<Utc>>, now: DateTime<Utc>, interval_days: i64) -> bool {
    match last_sent {
        Some(prev) => now - prev < Duration::days(interval_days),
        None => false,
    }
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

/// A system address an auto-reply must never go to: the daemon/postmaster
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

// ── Custom RFC 5322 headers for the reply ───────────────────────────────────

// lettre's `Header::parse` returns `Result<Self, BoxError>`, a crate-private
// alias spelled out here (same trick as `services::autocrypt`).
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Defines a lettre header type carrying a verbatim ASCII value. The value is
/// already well-formed (an address list or a token), so it is emitted as-is.
macro_rules! verbatim_header {
    ($ty:ident, $name:literal) => {
        #[derive(Clone)]
        struct $ty(String);
        impl Header for $ty {
            fn name() -> HeaderName {
                HeaderName::new_from_ascii_str($name)
            }
            fn parse(s: &str) -> Result<Self, BoxError> {
                Ok($ty(s.to_string()))
            }
            fn display(&self) -> HeaderValue {
                HeaderValue::dangerous_new_pre_encoded(Self::name(), self.0.clone(), self.0.clone())
            }
        }
    };
}

verbatim_header!(AutoSubmittedHeader, "Auto-Submitted");
verbatim_header!(PrecedenceHeader, "Precedence");
verbatim_header!(InReplyToHeader, "In-Reply-To");
verbatim_header!(ReferencesHeader, "References");

// ── Orchestration: decide + send, on a delivered message ────────────────────

/// The incoming-message facts the responder needs, gathered once by the caller.
pub struct Incoming<'a> {
    pub envelope_from:   &'a str,
    pub from_email:      &'a str,
    pub message_id:      Option<&'a str>,
    pub references:      &'a [String],
    pub auto_submitted:  Option<&'a str>,
    pub precedence:      Option<&'a str>,
    pub has_list_header: bool,
    /// The message was filed into Spam — an auto-reply would answer junk.
    pub is_spam:         bool,
}

/// Auto-replies to `incoming` on behalf of the mailbox that just received it,
/// when a responder is active and every RFC 3834 guard permits it.
///
/// Best-effort throughout: an auto-reply is a courtesy, never a reason to fail
/// or delay the delivery that already succeeded, so every error is logged and
/// swallowed rather than propagated.
#[allow(clippy::too_many_arguments)]
pub async fn maybe_autorespond(
    db: &PgPool,
    cfg: &ServerConfig,
    attachments_dir: &str,
    recipient_user_id: Uuid,
    recipient_account_id: Uuid,
    recipient_address: &str,
    incoming: &Incoming<'_>,
) {
    // A message that landed in Spam never draws a reply.
    if incoming.is_spam {
        return;
    }

    let config = match load(db, recipient_user_id).await {
        Ok(Some(c)) if c.enabled => c,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!(error = %e, "Répondeur d'absence : lecture de la configuration");
            return;
        }
    };

    // Active window, against the server's current date.
    if !within_window(config.start_date, config.end_date, Utc::now().date_naive()) {
        return;
    }

    // RFC 3834 anti-loop.
    let guard = LoopGuardInput {
        auto_submitted:  incoming.auto_submitted,
        precedence:      incoming.precedence,
        has_list_header: incoming.has_list_header,
        envelope_from:   incoming.envelope_from,
        from_email:      incoming.from_email,
        mailbox_email:   recipient_address,
    };
    if let LoopDecision::Skip(reason) = evaluate_loop_guard(&guard) {
        tracing::debug!(reason, from = %incoming.from_email, "Répondeur d'absence : pas de réponse (anti-boucle)");
        return;
    }

    // Where the reply goes: the envelope sender (Return-Path) is the correct
    // target (RFC 3834 §4); the header From is the fallback.
    let reply_to = normalize_reply_target(incoming.envelope_from, incoming.from_email);
    if reply_to.is_empty() {
        return;
    }

    // "contacts only" — answer only correspondents already in the address index.
    if config.contacts_only {
        match is_known_contact(db, recipient_user_id, &reply_to).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!(from = %reply_to, "Répondeur d'absence : expéditeur hors contacts");
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "Répondeur d'absence : vérification des contacts");
                return;
            }
        }
    }

    // Rate-limit: one reply per sender per interval.
    match last_reply_at(db, recipient_user_id, &reply_to).await {
        Ok(last) if rate_limited(last, Utc::now(), RESPONSE_INTERVAL_DAYS) => {
            tracing::debug!(from = %reply_to, "Répondeur d'absence : déjà répondu récemment");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = %e, "Répondeur d'absence : lecture du plafond d'envoi");
            return;
        }
    }

    // The reply's From: the box's own canonical address, falling back to the
    // address the sender wrote to.
    let from_box = mailbox_address(db, recipient_account_id).await.unwrap_or_else(|| {
        recipient_address.trim().to_ascii_lowercase()
    });

    match send_reply(
        db,
        cfg,
        attachments_dir,
        &from_box,
        &reply_to,
        &config,
        incoming.message_id,
        incoming.references,
    )
    .await
    {
        Ok(true) => {
            if let Err(e) = record_reply(db, recipient_user_id, &reply_to).await {
                tracing::warn!(error = %e, "Répondeur d'absence : enregistrement du plafond");
            }
            tracing::info!(from = %from_box, to = %reply_to, "Réponse d'absence envoyée");
        }
        Ok(false) => {}
        Err(e) => tracing::warn!(error = %e, "Répondeur d'absence : envoi échoué"),
    }
}

/// The reply target: prefer the envelope sender (minus its brackets); fall back
/// to the header From when the envelope was bracket-only.
fn normalize_reply_target(envelope_from: &str, from_email: &str) -> String {
    let env = strip_brackets(envelope_from);
    if !env.is_empty() {
        env
    } else {
        from_email.trim().to_ascii_lowercase()
    }
}

/// Builds the auto-reply and hands it to the right send path. Returns whether it
/// actually left (queued or delivered) — a remote recipient with outbound
/// delivery off cannot leave, and is not recorded against the rate-limit.
#[allow(clippy::too_many_arguments)]
async fn send_reply(
    db: &PgPool,
    cfg: &ServerConfig,
    attachments_dir: &str,
    from_box: &str,
    reply_to: &str,
    config: &ResponderConfig,
    orig_message_id: Option<&str>,
    orig_references: &[String],
) -> Result<bool> {
    let subject = reply_subject(&config.subject);
    let raw = build_reply(from_box, reply_to, &subject, &config.message_html, orig_message_id, orig_references)?;

    let reply_lc = reply_to.to_ascii_lowercase();

    if cfg.is_local_domain(&reply_lc) {
        // A local sender: file the reply straight into their mailbox, exactly as
        // an inbound message would be.
        let directory = resolve::PgDirectory::new(db);
        match resolve::resolve(&directory, cfg, from_box, true, &reply_lc).await {
            Ok(resolve::Outcome::Accept(expansion)) => {
                let mut delivered = false;
                for delivery in expansion.local {
                    // `deliver_local` → `maybe_autorespond` → `deliver_local` is
                    // mutual async recursion; `Box::pin` gives the future a size.
                    // It terminates at depth two: this reply carries
                    // `Auto-Submitted: auto-replied`, so the recipient's own guard
                    // refuses to answer it back.
                    match Box::pin(crate::server::deliver::deliver_local(
                        db,
                        cfg,
                        from_box,
                        &delivery.address,
                        delivery.target,
                        &raw,
                        attachments_dir,
                        crate::server::deliver::Disposition::Inbox,
                        None,
                        None,
                    ))
                    .await
                    {
                        Ok(_) => delivered = true,
                        Err(e) => tracing::warn!(error = %e, "Répondeur d'absence : dépôt local de la réponse"),
                    }
                }
                Ok(delivered)
            }
            Ok(resolve::Outcome::Refuse(_)) => Ok(false),
            Err(e) => Err(e),
        }
    } else if cfg.outbound_enabled {
        // A remote sender: hand to the outbound queue. RFC 3834 §3 — an
        // auto-reply is sent with a NULL return-path (empty envelope sender) so
        // it can never itself provoke a bounce loop.
        let stamped = hygiene::prepend_received(&raw, "local", &cfg.hostname);
        let domain = reply_lc.rsplit_once('@').map(|(_, d)| d.to_string()).unwrap_or_default();
        queue::enqueue_with_lifetime(
            db,
            None,
            None,
            "",
            &stamped,
            false,
            &[(reply_lc, domain)],
            cfg.outbound_lifetime_hours,
        )
        .await?;
        Ok(true)
    } else {
        // Remote recipient, outbound disabled: nowhere to go.
        Ok(false)
    }
}

/// The reply's Subject: the configured one, or a neutral default when blank.
fn reply_subject(configured: &str) -> String {
    let s = configured.trim();
    if s.is_empty() {
        "Réponse automatique".to_string()
    } else {
        s.to_string()
    }
}

/// Builds the RFC 5322 auto-reply bytes, threaded to the original message and
/// flagged automatic so the far side never answers it.
fn build_reply(
    from_box: &str,
    reply_to: &str,
    subject: &str,
    message_html: &str,
    orig_message_id: Option<&str>,
    orig_references: &[String],
) -> Result<Vec<u8>> {
    let from_mb: Mailbox = from_box.parse().context("Adresse expéditeur du répondeur invalide")?;
    let to_mb: Mailbox = reply_to.parse().context("Adresse destinataire du répondeur invalide")?;

    let body_html = if message_html.trim().is_empty() {
        "<p></p>".to_string()
    } else {
        message_html.to_string()
    };
    let body_text = html2text::from_read(body_html.as_bytes(), 80);

    let alternative = MultiPart::alternative()
        .singlepart(SinglePart::builder().header(ContentType::TEXT_PLAIN).body(body_text))
        .singlepart(SinglePart::builder().header(ContentType::TEXT_HTML).body(body_html));

    let mut builder = Message::builder()
        .from(from_mb)
        .to(to_mb)
        .subject(subject)
        // RFC 3834 §5 — flag the reply automatic so a well-behaved responder on
        // the other side does not answer it.
        .header(AutoSubmittedHeader("auto-replied".to_string()))
        .header(PrecedenceHeader("bulk".to_string()));

    // Thread the reply to the message it answers.
    if let Some(mid) = orig_message_id {
        let bracketed = ensure_brackets(mid);
        builder = builder.header(InReplyToHeader(bracketed.clone()));
        let mut refs: Vec<String> = orig_references.iter().map(|r| ensure_brackets(r)).collect();
        if !refs.iter().any(|r| r == &bracketed) {
            refs.push(bracketed);
        }
        builder = builder.header(ReferencesHeader(refs.join(" ")));
    } else if !orig_references.is_empty() {
        let refs: Vec<String> = orig_references.iter().map(|r| ensure_brackets(r)).collect();
        builder = builder.header(ReferencesHeader(refs.join(" ")));
    }

    let email = builder.multipart(alternative).context("Construction de la réponse d'absence")?;
    Ok(email.formatted())
}

/// Wraps a Message-ID in angle brackets if it has none.
fn ensure_brackets(id: &str) -> String {
    let id = id.trim();
    if id.starts_with('<') && id.ends_with('>') {
        id.to_string()
    } else {
        format!("<{}>", id.trim_matches(|c| c == '<' || c == '>'))
    }
}

// ── Database side-lookups ───────────────────────────────────────────────────

/// The canonical address of the local account that received the message.
async fn mailbox_address(db: &PgPool, account_id: Uuid) -> Option<String> {
    match sqlx::query_scalar::<_, String>(
        "SELECT email_address FROM mail.accounts WHERE id = $1",
    )
    .bind(account_id)
    .fetch_optional(db)
    .await
    {
        Ok(addr) => addr.map(|a| a.trim().to_ascii_lowercase()).filter(|a| a.contains('@')),
        Err(e) => {
            tracing::warn!(error = %e, "Répondeur d'absence : lecture de l'adresse de la boîte");
            None
        }
    }
}

/// Is `email` a correspondent this user already knows? The mail module's notion
/// of "contacts" is `mail.address_index` (see the module note above).
async fn is_known_contact(db: &PgPool, user_id: Uuid, email: &str) -> Result<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mail.address_index WHERE user_id = $1 AND email = LOWER($2))",
    )
    .bind(user_id)
    .bind(email)
    .fetch_one(db)
    .await
    .context("Vérification des contacts du répondeur")?;
    Ok(exists)
}

/// When we last auto-replied to `from_email` on behalf of `user_id`.
async fn last_reply_at(db: &PgPool, user_id: Uuid, from_email: &str) -> Result<Option<DateTime<Utc>>> {
    let at: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT sent_at FROM mail.vacation_sent WHERE user_id = $1 AND from_email = LOWER($2)",
    )
    .bind(user_id)
    .bind(from_email)
    .fetch_optional(db)
    .await
    .context("Lecture du plafond d'envoi du répondeur")?;
    Ok(at)
}

/// Records that we just auto-replied to `from_email`, resetting its interval.
async fn record_reply(db: &PgPool, user_id: Uuid, from_email: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO mail.vacation_sent (user_id, from_email, sent_at)
         VALUES ($1, LOWER($2), NOW())
         ON CONFLICT (user_id, from_email) DO UPDATE SET sent_at = NOW()",
    )
    .bind(user_id)
    .bind(from_email)
    .execute(db)
    .await
    .context("Enregistrement de l'envoi du répondeur")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input<'a>() -> LoopGuardInput<'a> {
        LoopGuardInput {
            auto_submitted:  None,
            precedence:      None,
            has_list_header: false,
            envelope_from:   "alice@example.com",
            from_email:      "alice@example.com",
            mailbox_email:   "me@kubuno.com",
        }
    }

    // ── RFC 3834: the one case we DO answer ─────────────────────────────────
    #[test]
    fn ordinary_mail_from_a_person_is_answered() {
        assert_eq!(evaluate_loop_guard(&base_input()), LoopDecision::Respond);
    }

    // ── RFC 3834: every refusal ─────────────────────────────────────────────
    #[test]
    fn auto_submitted_not_no_is_refused() {
        for v in ["auto-replied", "auto-generated", "auto-notified", "AUTO-REPLIED"] {
            let input = LoopGuardInput { auto_submitted: Some(v), ..base_input() };
            assert_eq!(evaluate_loop_guard(&input), LoopDecision::Skip("auto-submitted"), "{v}");
        }
    }

    #[test]
    fn auto_submitted_no_is_still_answered() {
        let input = LoopGuardInput { auto_submitted: Some("no"), ..base_input() };
        assert_eq!(evaluate_loop_guard(&input), LoopDecision::Respond);
    }

    #[test]
    fn precedence_bulk_list_junk_are_refused() {
        for v in ["bulk", "list", "junk", "Bulk", " LIST "] {
            let input = LoopGuardInput { precedence: Some(v), ..base_input() };
            assert_eq!(evaluate_loop_guard(&input), LoopDecision::Skip("precedence-bulk"), "{v}");
        }
    }

    #[test]
    fn precedence_normal_is_answered() {
        let input = LoopGuardInput { precedence: Some("normal"), ..base_input() };
        assert_eq!(evaluate_loop_guard(&input), LoopDecision::Respond);
    }

    #[test]
    fn any_list_header_is_refused() {
        let input = LoopGuardInput { has_list_header: true, ..base_input() };
        assert_eq!(evaluate_loop_guard(&input), LoopDecision::Skip("list-header"));
    }

    #[test]
    fn null_return_path_is_refused() {
        for env in ["<>", "", "  ", "< >"] {
            let input = LoopGuardInput { envelope_from: env, ..base_input() };
            assert_eq!(evaluate_loop_guard(&input), LoopDecision::Skip("null-return-path"), "{env:?}");
        }
    }

    #[test]
    fn system_senders_are_refused() {
        for addr in ["mailer-daemon@example.com", "postmaster@example.com",
                     "no-reply@shop.com", "noreply@bank.com", "No-Reply@X.com"] {
            let input = LoopGuardInput { envelope_from: addr, from_email: addr, ..base_input() };
            assert_eq!(evaluate_loop_guard(&input), LoopDecision::Skip("system-sender"), "{addr}");
        }
    }

    #[test]
    fn system_sender_detected_on_header_from_even_when_envelope_is_clean() {
        let input = LoopGuardInput {
            envelope_from: "bounce+abc@example.com",
            from_email:    "noreply@example.com",
            ..base_input()
        };
        assert_eq!(evaluate_loop_guard(&input), LoopDecision::Skip("system-sender"));
    }

    #[test]
    fn replying_to_oneself_is_refused() {
        let me = "me@kubuno.com";
        let by_env = LoopGuardInput { envelope_from: me, from_email: "other@x.com", mailbox_email: me, ..base_input() };
        assert_eq!(evaluate_loop_guard(&by_env), LoopDecision::Skip("self"));
        let by_from = LoopGuardInput { envelope_from: "other@x.com", from_email: me, mailbox_email: me, ..base_input() };
        assert_eq!(evaluate_loop_guard(&by_from), LoopDecision::Skip("self"));
    }

    // ── Active window ───────────────────────────────────────────────────────
    #[test]
    fn window_bounds_are_inclusive_and_open_ended() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        // Fully bounded.
        assert!(within_window(Some(d("2026-08-01")), Some(d("2026-08-10")), d("2026-08-05")));
        assert!(within_window(Some(d("2026-08-01")), Some(d("2026-08-10")), d("2026-08-01")));
        assert!(within_window(Some(d("2026-08-01")), Some(d("2026-08-10")), d("2026-08-10")));
        assert!(!within_window(Some(d("2026-08-01")), Some(d("2026-08-10")), d("2026-07-31")));
        assert!(!within_window(Some(d("2026-08-01")), Some(d("2026-08-10")), d("2026-08-11")));
        // Open end: active forever after the start.
        assert!(within_window(Some(d("2026-08-01")), None, d("2030-01-01")));
        // No bounds at all: always active.
        assert!(within_window(None, None, d("2026-08-05")));
    }

    // ── Rate-limit ──────────────────────────────────────────────────────────
    #[test]
    fn rate_limit_blocks_within_interval_and_allows_after() {
        let now = DateTime::parse_from_rfc3339("2026-08-05T12:00:00Z").unwrap().with_timezone(&Utc);
        // Never answered → allowed.
        assert!(!rate_limited(None, now, RESPONSE_INTERVAL_DAYS));
        // Answered 1 day ago → blocked.
        let one_day_ago = now - Duration::days(1);
        assert!(rate_limited(Some(one_day_ago), now, RESPONSE_INTERVAL_DAYS));
        // Answered 3 days 23h ago → still blocked (just under 4 days).
        let almost = now - Duration::days(3) - Duration::hours(23);
        assert!(rate_limited(Some(almost), now, RESPONSE_INTERVAL_DAYS));
        // Answered 4 days + 1s ago → allowed again.
        let past = now - Duration::days(4) - Duration::seconds(1);
        assert!(!rate_limited(Some(past), now, RESPONSE_INTERVAL_DAYS));
    }

    // ── Reply target + helpers ──────────────────────────────────────────────
    #[test]
    fn reply_target_prefers_envelope_then_falls_back_to_from() {
        assert_eq!(normalize_reply_target("<Alice@Example.com>", "x@y.com"), "alice@example.com");
        assert_eq!(normalize_reply_target("<>", "From@Header.com"), "from@header.com");
    }

    #[test]
    fn message_ids_get_bracketed_exactly_once() {
        assert_eq!(ensure_brackets("abc@host"), "<abc@host>");
        assert_eq!(ensure_brackets("<abc@host>"), "<abc@host>");
        assert_eq!(ensure_brackets("  abc@host "), "<abc@host>");
    }

    #[test]
    fn reply_subject_defaults_when_blank() {
        assert_eq!(reply_subject("  "), "Réponse automatique");
        assert_eq!(reply_subject("Absent du bureau"), "Absent du bureau");
    }

    // The reply we build is itself marked automatic and threaded — proving the
    // far-side loop would be stopped by its own guard.
    #[test]
    fn built_reply_carries_the_antiloop_headers() {
        let raw = build_reply(
            "me@kubuno.com",
            "alice@example.com",
            "Absent",
            "<p>Je reviens lundi.</p>",
            Some("orig-123@example.com"),
            &["root-1@example.com".to_string()],
        )
        .expect("build");
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("Auto-Submitted: auto-replied"), "must flag itself automatic");
        assert!(text.contains("In-Reply-To: <orig-123@example.com>"));
        assert!(text.contains("<root-1@example.com>"));
        assert!(text.contains("<orig-123@example.com>")); // appended to References
    }
}
