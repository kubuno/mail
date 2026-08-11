//! Administration of the addresses this instance OWNS: mailboxes, aliases,
//! distribution lists and per-domain policy.
//!
//! Until migration 000024 the instance had no such notion — a recipient was
//! guessed from `mailbox_credentials.username` (a login) and then from
//! `accounts.email_address` (the user's address at ANOTHER provider). Neither
//! can express an alias, a catch-all, a list or a quota, so the address became a
//! first-class object. These handlers are the panel behind it.
//!
//! ── What is deliberately NOT here ────────────────────────────────────────────
//! The list of local domains. It is the **instance** that declares its domains,
//! in the console, with a DNS proof (`core.domains`); this module reads them
//! back and serves the verified ones, plus the `server_domains` stop-gap list
//! for names DNS can never prove. `mail.domain_policies` only holds what a
//! domain may additionally SAY about itself. Two competing answers to "is this
//! domain ours" is exactly the split that delivers mail nowhere — so every write
//! below checks the address's domain against that union, read live from the
//! core, and a policy row for a domain that is no longer served is reported as
//! inert rather than treated as an answer.
//!
//! ── Uniqueness across the three tables ───────────────────────────────────────
//! An address may be a mailbox, an alias OR a list, never two at once. The
//! migration installs a trigger that raises `unique_violation`, but a raw SQL
//! error is not an answer an operator can act on: every write here first asks
//! [`address_owner`] what already holds the address and returns a sentence
//! naming it. The trigger stays as the backstop for the race between the two,
//! and [`translate_conflict`] turns that case into a sentence too.

pub mod aliases;
pub mod directory;
pub mod domains;
pub mod lists;
pub mod mailboxes;

use rand::Rng;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{errors::MailError, middleware::AuthUser, server::config, server::hygiene, state::AppState};

/// Page size when the caller does not ask for one.
const DEFAULT_LIMIT: i64 = 50;

/// Hard ceiling on a page. A panel that can ask for the whole table in one go is
/// a way to turn one careless click into a multi-second query.
const MAX_LIMIT: i64 = 200;

/// RFC 5321 §4.5.3.1.1: longest local part.
const MAX_LOCAL_LEN: usize = 64;

/// RFC 1035: longest domain name, and longest single label.
const MAX_DOMAIN_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;

/// Most destinations an alias may fan out to, and most members a list may hold
/// in one write. Not a policy on what is reasonable — a bound on what a single
/// request can cost, since every entry is validated and stored.
pub(crate) const MAX_DESTINATIONS: usize = 200;

/// What a given address already is. Used to refuse the second use of an address
/// with a sentence that names the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddressKind {
    Mailbox,
    Alias,
    List,
}

impl AddressKind {
    /// French label, phrased to drop into "l'adresse X est déjà …".
    pub(crate) fn label(self) -> &'static str {
        match self {
            AddressKind::Mailbox => "une boîte",
            AddressKind::Alias => "un alias",
            AddressKind::List => "une liste de diffusion",
        }
    }
}

/// Only an administrator configures instance-wide addressing. Same shape as
/// `dkim.rs` and `diagnostics.rs`: the role arrives in a header the core proxy
/// injects, so there is one place it is read and one place it is compared.
pub(crate) fn require_admin(user: &AuthUser) -> Result<(), MailError> {
    if user.role == "admin" {
        Ok(())
    } else {
        Err(MailError::Forbidden)
    }
}

// ── Address normalisation ────────────────────────────────────────────────────

/// A normalised local address, split the way the schema stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedAddress {
    /// Lower-cased and rebuilt from its parts: `alice@example.com`, or
    /// `@example.com` for a catch-all.
    pub address: String,
    pub domain: String,
    pub is_catch_all: bool,
}

/// Parses and lower-cases an address written by an administrator.
///
/// The syntactic predicate is [`hygiene::valid_envelope_address`] — the very one
/// the SMTP path applies to RCPT TO — so an address this panel accepts is
/// exactly one an envelope can carry, and there is a single definition of that.
/// What is added here is the structure the schema needs and the envelope check
/// does not provide: the split into local part and domain (the `domain` column
/// is indexed and is what every per-domain view groups by), and the rejection of
/// the characters that would make the stored value unusable in an envelope even
/// though it contains an `@`.
///
/// A local part that is empty means `@example.com`: the catch-all.
pub(crate) fn parse_address(raw: &str) -> Result<ParsedAddress, MailError> {
    let trimmed = raw.trim().trim_start_matches('<').trim_end_matches('>').trim();
    // Stored lower-cased so every lookup is a plain equality on an indexed
    // column — the reason migration 000024 gives for doing it at write time.
    let address = trimmed.to_ascii_lowercase();

    if !hygiene::valid_envelope_address(&address) {
        return Err(MailError::Validation(format!(
            "Adresse invalide : « {trimmed} » n'est pas une adresse e-mail utilisable"
        )));
    }

    // `valid_envelope_address` guarantees an `@`, so this cannot be None; the
    // match is written out rather than unwrapped so the guarantee is not load-
    // bearing at runtime.
    let Some((local, domain)) = address.rsplit_once('@') else {
        return Err(MailError::Validation(format!(
            "Adresse invalide : « {trimmed} » n'a pas de domaine"
        )));
    };

    let domain = normalize_domain(domain)?;

    if local.is_empty() {
        return Ok(ParsedAddress {
            address: format!("@{domain}"),
            domain,
            is_catch_all: true,
        });
    }

    if local.len() > MAX_LOCAL_LEN {
        return Err(MailError::Validation(format!(
            "Adresse invalide : la partie locale de « {trimmed} » dépasse {MAX_LOCAL_LEN} caractères"
        )));
    }
    // A second `@`, a space or a header/envelope separator makes the stored
    // value unusable where it will be used, however well it round-trips here.
    if local.contains(['@', ' ', '\t', '<', '>', ',', ';', ':', '"', '\\', '(', ')', '[', ']']) {
        return Err(MailError::Validation(format!(
            "Adresse invalide : « {trimmed} » contient un caractère interdit avant l'arobase"
        )));
    }

    Ok(ParsedAddress {
        address: format!("{local}@{domain}"),
        domain,
        is_catch_all: false,
    })
}

/// A syntactically usable mail domain, lower-cased and stripped of a leading
/// `@` so `@example.com` and `example.com` name the same domain.
pub(crate) fn normalize_domain(raw: &str) -> Result<String, MailError> {
    let domain = raw
        .trim()
        .trim_start_matches('@')
        .trim_end_matches('.')
        .to_ascii_lowercase();

    let labels_ok = !domain.is_empty()
        && domain.contains('.')
        && domain.len() <= MAX_DOMAIN_LEN
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= MAX_LABEL_LEN
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });

    if !labels_ok {
        return Err(MailError::Validation(format!(
            "Domaine invalide : « {} »",
            raw.trim()
        )));
    }
    Ok(domain)
}

/// Parses a destination (alias target, list member, allowed sender).
///
/// A destination is always a whole address: `@example.com` on the right-hand
/// side would mean "everything in that domain", which is not a recipient and
/// cannot be delivered to.
pub(crate) fn parse_destination(raw: &str) -> Result<String, MailError> {
    let parsed = parse_address(raw)?;
    if parsed.is_catch_all {
        return Err(MailError::Validation(format!(
            "« {} » n'est pas une destination : un attrape-tout ne peut être qu'à gauche",
            raw.trim()
        )));
    }
    Ok(parsed.address)
}

/// Validates, lower-cases and de-duplicates a list of destinations, refusing the
/// owning address itself.
///
/// A destination equal to its own alias is the loop somebody WILL configure, and
/// it costs a rejected recipient at best. Refusing it here is the cheapest place
/// — the resolver's depth limit is the backstop, not the answer. De-duplication
/// is not cosmetic either: a duplicated destination is a duplicated delivery.
pub(crate) fn parse_destinations(raw: &[String], own: &str) -> Result<Vec<String>, MailError> {
    if raw.len() > MAX_DESTINATIONS {
        return Err(MailError::Validation(format!(
            "Trop de destinations : {MAX_DESTINATIONS} au maximum"
        )));
    }

    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    for entry in raw {
        if entry.trim().is_empty() {
            continue;
        }
        let address = parse_destination(entry)?;
        if address == own {
            return Err(MailError::Validation(format!(
                "« {address} » ne peut pas être sa propre destination : cette boucle rejetterait le destinataire"
            )));
        }
        if !out.contains(&address) {
            out.push(address);
        }
    }

    if out.is_empty() {
        return Err(MailError::Validation(
            "Au moins une destination est requise. Pour suspendre sans supprimer, utilisez « actif : non »".into(),
        ));
    }
    Ok(out)
}

/// Same validation as a destination list, but an empty result is allowed: an
/// empty `allowed_senders` is meaningful (it is only consulted by the `allowed`
/// policy, which refuses to be saved empty — see `lists.rs`).
pub(crate) fn parse_address_set(raw: &[String]) -> Result<Vec<String>, MailError> {
    if raw.len() > MAX_DESTINATIONS {
        return Err(MailError::Validation(format!(
            "Trop d'adresses : {MAX_DESTINATIONS} au maximum"
        )));
    }
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    for entry in raw {
        if entry.trim().is_empty() {
            continue;
        }
        let address = parse_destination(entry)?;
        if !out.contains(&address) {
            out.push(address);
        }
    }
    Ok(out)
}

// ── Local domains (the instance registry ∪ the stop-gap list, read live) ─────

/// The whole server configuration, read from the core rather than from a copy
/// kept here.
///
/// Creating an address in a domain nothing routes to us produces a row that will
/// never receive anything, and the operator finds out weeks later. The read
/// costs one internal HTTP call per write, which is the same trade `diagnostics`
/// makes and for the same reason: a stale answer here is worse than a slow one.
pub(crate) async fn server_config(state: &AppState) -> Result<config::ServerConfig, MailError> {
    let http = reqwest::Client::new();
    config::fetch(&http, &state.settings).await.ok_or_else(|| {
        MailError::Internal(anyhow::anyhow!(
            "Configuration du serveur de messagerie illisible : impossible de vérifier que le domaine est servi par cette instance"
        ))
    })
}

/// The domains this instance serves: the **verified** domains declared in the
/// console, plus the `server_domains` stop-gap list.
pub(crate) async fn local_domains(state: &AppState) -> Result<Vec<String>, MailError> {
    Ok(server_config(state).await?.domains)
}

/// Refuses a domain this instance does not serve.
pub(crate) fn require_local_domain(domains: &[String], domain: &str) -> Result<(), MailError> {
    if domains.iter().any(|d| d == domain) {
        return Ok(());
    }
    let served = if domains.is_empty() {
        "aucun domaine n'est servi : déclarez-en un dans Instance ▸ Domaines et vérifiez-le, \
         ou ajoutez-le à la liste d'appoint des réglages du serveur de messagerie"
            .to_string()
    } else {
        format!("domaines servis : {}", domains.join(", "))
    };
    Err(MailError::Validation(format!(
        "Le domaine « {domain} » n'est pas servi par cette instance : une adresse y serait créée sans jamais rien recevoir ({served})"
    )))
}

// ── Cross-table uniqueness ───────────────────────────────────────────────────

/// What already holds this address, if anything. `exclude` skips the row being
/// updated so renaming an object to its own address is not a conflict.
pub(crate) async fn address_owner(
    db: &PgPool,
    address: &str,
    exclude: Option<Uuid>,
) -> Result<Option<AddressKind>, MailError> {
    let probes = [
        (AddressKind::Mailbox, "SELECT 1 FROM mail.mailboxes WHERE address = $1 AND ($2::uuid IS NULL OR id <> $2)"),
        (AddressKind::Alias, "SELECT 1 FROM mail.aliases WHERE address = $1 AND ($2::uuid IS NULL OR id <> $2)"),
        (AddressKind::List, "SELECT 1 FROM mail.mailing_lists WHERE address = $1 AND ($2::uuid IS NULL OR id <> $2)"),
    ];

    for (kind, sql) in probes {
        let taken: Option<i32> = sqlx::query_scalar(sql)
            .bind(address)
            .bind(exclude)
            .fetch_optional(db)
            .await
            .map_err(db_error("vérification d'unicité d'adresse"))?;
        if taken.is_some() {
            return Ok(Some(kind));
        }
    }
    Ok(None)
}

/// Refuses an address already used by another object, naming that object.
pub(crate) async fn require_address_free(
    db: &PgPool,
    address: &str,
    exclude: Option<Uuid>,
) -> Result<(), MailError> {
    match address_owner(db, address, exclude).await? {
        Some(kind) => Err(MailError::Conflict(format!(
            "L'adresse « {address} » est déjà {} : une adresse ne peut pas être deux objets à la fois",
            kind.label()
        ))),
        None => Ok(()),
    }
}

// ── Error mapping ────────────────────────────────────────────────────────────

/// Logs a database error with its context and turns it into the opaque 500 the
/// API returns. Every query below goes through this, so no path can return
/// without having logged first.
pub(crate) fn db_error(context: &'static str) -> impl Fn(sqlx::Error) -> MailError {
    move |e| {
        tracing::error!(error = %e, context, "adresses : erreur base de données");
        MailError::Database(e)
    }
}

/// Turns the constraint the database enforced into a sentence.
///
/// The pre-check in [`require_address_free`] answers the ordinary case with a
/// precise message; what reaches here is the race between that check and the
/// write, plus the catch-all index. Neither may surface as raw SQL — an operator
/// cannot act on `duplicate key value violates unique constraint`.
pub(crate) fn translate_conflict(
    e: sqlx::Error,
    address: &str,
    domain: &str,
    context: &'static str,
) -> MailError {
    if let sqlx::Error::Database(ref db_err) = e {
        if db_err.code().as_deref() == Some("23505") {
            if db_err.constraint() == Some("idx_mail_aliases_catch_all") {
                return MailError::Conflict(format!(
                    "Le domaine « {domain} » a déjà un attrape-tout : il ne peut y en avoir qu'un, sinon la distribution dépendrait de l'ordre des lignes"
                ));
            }
            return MailError::Conflict(format!(
                "L'adresse « {address} » vient d'être attribuée à un autre objet (boîte, alias ou liste). Rechargez la liste."
            ));
        }
    }
    db_error(context)(e)
}

// ── Pagination ───────────────────────────────────────────────────────────────

/// Shared `?limit=&offset=&q=&domain=&active=` shape of the three list routes.
#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    /// Free-text search on the address and the human-readable fields.
    pub q: Option<String>,
    /// Restrict to one domain.
    pub domain: Option<String>,
    /// Restrict to active (`true`) or suspended (`false`) objects.
    pub active: Option<bool>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl ListQuery {
    /// Clamped page bounds. An absurd `limit` is capped rather than refused: the
    /// caller asked for "everything", and a page is a correct answer to that.
    pub(crate) fn page(&self) -> (i64, i64) {
        let limit = self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let offset = self.offset.unwrap_or(0).max(0);
        (limit, offset)
    }

    /// The search pattern as an `ILIKE` argument, or `None` when not searching.
    /// `%` and `_` are escaped so a literal underscore in an address does not
    /// silently match any character.
    pub(crate) fn pattern(&self) -> Option<String> {
        let q = self.q.as_deref()?.trim();
        if q.is_empty() {
            return None;
        }
        let escaped = q
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        Some(format!("%{}%", escaped.to_lowercase()))
    }

    /// The domain filter, normalised. An unparsable domain matches nothing
    /// rather than erroring: it is a filter, not a write.
    pub(crate) fn domain_filter(&self) -> Option<String> {
        let d = self.domain.as_deref()?.trim();
        if d.is_empty() {
            return None;
        }
        Some(d.trim_start_matches('@').to_ascii_lowercase())
    }
}

// ── Mailbox credentials ──────────────────────────────────────────────────────

/// 24 characters from an unambiguous alphabet — no O/0, no l/1/I.
///
/// A local twin of the one in `handlers::mailbox`: people retype these into a
/// phone, and a lookalike character reads as a wrong password with no way to
/// tell. Kept local rather than shared so this module owns what it hands out.
pub(crate) fn generate_password() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..24)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_is_lower_cased_and_unwrapped() {
        let parsed = parse_address(" <Alice.Martin@Example.COM> ").expect("valide");
        assert_eq!(parsed.address, "alice.martin@example.com");
        assert_eq!(parsed.domain, "example.com");
        assert!(!parsed.is_catch_all);
    }

    #[test]
    fn an_at_domain_is_the_catch_all() {
        let parsed = parse_address("@Example.com").expect("valide");
        assert_eq!(parsed.address, "@example.com");
        assert_eq!(parsed.domain, "example.com");
        assert!(parsed.is_catch_all);
    }

    #[test]
    fn a_trailing_dot_on_the_domain_is_normalised_away() {
        let parsed = parse_address("bob@example.com.").expect("valide");
        assert_eq!(parsed.address, "bob@example.com");
    }

    #[test]
    fn what_cannot_travel_in_an_envelope_is_refused() {
        assert!(parse_address("").is_err());
        assert!(parse_address("not-an-address").is_err());
        assert!(parse_address("alice@localhost").is_err());
        assert!(parse_address("a b@example.com").is_err());
        assert!(parse_address("a@b@example.com").is_err());
        assert!(parse_address("alice@example.com\r\nRCPT TO:<evil@x>").is_err());
        assert!(parse_address(&format!("{}@example.com", "x".repeat(65))).is_err());
    }

    #[test]
    fn domain_normalisation_matches_the_dns_shape() {
        assert_eq!(normalize_domain(" @Example.COM. ").ok().as_deref(), Some("example.com"));
        assert!(normalize_domain("localhost").is_err());
        assert!(normalize_domain("exa mple.com").is_err());
        assert!(normalize_domain("-bad.example.com").is_err());
        assert!(normalize_domain("bad-.example.com").is_err());
        assert!(normalize_domain(&format!("{}.com", "x".repeat(64))).is_err());
    }

    #[test]
    fn a_catch_all_is_not_a_destination() {
        assert!(parse_destination("@example.com").is_err());
        assert_eq!(parse_destination(" Bob@Example.com ").ok().as_deref(), Some("bob@example.com"));
    }

    #[test]
    fn destinations_are_deduplicated_case_insensitively() {
        let out = parse_destinations(
            &[
                "Bob@example.com".into(),
                "bob@EXAMPLE.com".into(),
                "  ".into(),
                "carol@example.com".into(),
            ],
            "contact@example.com",
        )
        .expect("valide");
        assert_eq!(out, vec!["bob@example.com", "carol@example.com"]);
    }

    #[test]
    fn an_alias_pointing_at_itself_is_refused() {
        let err = parse_destinations(
            &["Contact@Example.com".into()],
            "contact@example.com",
        );
        assert!(err.is_err());
    }

    #[test]
    fn an_empty_destination_list_is_refused_rather_than_becoming_a_black_hole() {
        assert!(parse_destinations(&[], "contact@example.com").is_err());
        assert!(parse_destinations(&["  ".into()], "contact@example.com").is_err());
    }

    #[test]
    fn too_many_destinations_is_refused() {
        let many: Vec<String> = (0..=MAX_DESTINATIONS)
            .map(|i| format!("u{i}@example.com"))
            .collect();
        assert!(parse_destinations(&many, "contact@example.com").is_err());
    }

    #[test]
    fn an_allowed_sender_set_may_be_empty_but_is_still_validated() {
        assert_eq!(parse_address_set(&[]).expect("valide"), Vec::<String>::new());
        assert!(parse_address_set(&["nope".into()]).is_err());
    }

    #[test]
    fn a_domain_we_do_not_serve_is_refused() {
        let served = vec!["example.com".to_string()];
        assert!(require_local_domain(&served, "example.com").is_ok());
        assert!(require_local_domain(&served, "elsewhere.org").is_err());
        assert!(require_local_domain(&[], "example.com").is_err());
    }

    #[test]
    fn page_bounds_are_clamped() {
        let q = ListQuery { limit: Some(10_000), offset: Some(-5), ..Default::default() };
        assert_eq!(q.page(), (MAX_LIMIT, 0));
        assert_eq!(ListQuery::default().page(), (DEFAULT_LIMIT, 0));
    }

    #[test]
    fn search_wildcards_are_escaped() {
        let q = ListQuery { q: Some("a_b%c".into()), ..Default::default() };
        assert_eq!(q.pattern().as_deref(), Some("%a\\_b\\%c%"));
        assert!(ListQuery { q: Some("   ".into()), ..Default::default() }.pattern().is_none());
    }

    #[test]
    fn generated_passwords_are_long_and_unambiguous() {
        let pw = generate_password();
        assert_eq!(pw.chars().count(), 24);
        assert!(!pw.contains(['0', 'O', 'l', '1', 'I']));
        assert_ne!(pw, generate_password());
    }
}

/// Routing and authorisation, exercised over real HTTP against the real router.
///
/// The pool is built lazily and is never connected: every case below is a path
/// that answers before it would touch the database, which is precisely the
/// property worth pinning. A route that stopped being admin-only, or that fell
/// out of the router entirely, would answer 200 or 404 here instead of 403.
#[cfg(test)]
mod routing_tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::{config::Settings, router, state::AppState};

    /// A router over a pool that is never connected.
    fn test_router() -> axum::Router {
        let db = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(PgConnectOptions::new().host("127.0.0.1").database("unused"));
        let settings = Settings::load().expect("réglages par défaut");
        router::build(AppState {
            db,
            settings: Arc::new(settings),
        })
    }

    fn request(method: &str, uri: &str, role: Option<&str>, body: &'static str) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(role) = role {
            builder = builder
                .header("X-Kubuno-User-Id", "00000000-0000-0000-0000-000000000001")
                .header("X-Kubuno-User-Email", "someone@example.com")
                .header("X-Kubuno-User-Role", role);
        }
        builder
            .header("Content-Type", "application/json")
            .body(Body::from(body))
            .expect("requête")
    }

    async fn status(method: &str, uri: &str, role: Option<&str>, body: &'static str) -> StatusCode {
        test_router()
            .oneshot(request(method, uri, role, body))
            .await
            .expect("réponse")
            .status()
    }

    const ID: &str = "00000000-0000-0000-0000-000000000002";

    /// Every route, with a body its DTO accepts.
    ///
    /// The bodies matter: axum runs the `Json` extractor before the handler
    /// body, so a payload missing a required field answers 422 and the admin
    /// check is never reached. That is not a leak — nothing runs, nothing is
    /// read — but it does mean a test posting `{}` would pass while proving
    /// nothing about the guard.
    fn routes() -> Vec<(&'static str, String, &'static str)> {
        vec![
            ("GET", "/admin/mailboxes".into(), ""),
            ("POST", "/admin/mailboxes".into(),
             r#"{"address":"a@example.com","user_id":"00000000-0000-0000-0000-000000000002"}"#),
            ("GET", format!("/admin/mailboxes/{ID}"), ""),
            ("PATCH", format!("/admin/mailboxes/{ID}"), "{}"),
            ("DELETE", format!("/admin/mailboxes/{ID}"), ""),
            ("POST", format!("/admin/mailboxes/{ID}/credential"), "{}"),
            ("GET", "/admin/aliases".into(), ""),
            ("POST", "/admin/aliases".into(),
             r#"{"address":"a@example.com","destinations":["b@example.com"]}"#),
            ("GET", format!("/admin/aliases/{ID}"), ""),
            ("PATCH", format!("/admin/aliases/{ID}"), "{}"),
            ("DELETE", format!("/admin/aliases/{ID}"), ""),
            ("GET", "/admin/mailing-lists".into(), ""),
            ("POST", "/admin/mailing-lists".into(),
             r#"{"address":"a@example.com","name":"Équipe"}"#),
            ("GET", format!("/admin/mailing-lists/{ID}"), ""),
            ("PATCH", format!("/admin/mailing-lists/{ID}"), "{}"),
            ("DELETE", format!("/admin/mailing-lists/{ID}"), ""),
            ("PUT", format!("/admin/mailing-lists/{ID}/members"), r#"{"addresses":[]}"#),
            ("POST", format!("/admin/mailing-lists/{ID}/members"), r#"{"addresses":[]}"#),
            ("DELETE", format!("/admin/mailing-lists/{ID}/members"), r#"{"addresses":[]}"#),
            ("GET", "/admin/domains".into(), ""),
            ("PUT", "/admin/domains/example.com".into(), "{}"),
            ("DELETE", "/admin/domains/example.com".into(), ""),
        ]
    }

    #[tokio::test]
    async fn every_address_route_is_registered_and_reserved_to_administrators() {
        // 404 would mean the route is absent; 403 means it matched and the admin
        // guard fired, which is the intended answer for a plain user.
        for (method, uri, body) in routes() {
            assert_eq!(
                status(method, &uri, Some("user"), body).await,
                StatusCode::FORBIDDEN,
                "{method} {uri} devrait être réservée aux administrateurs"
            );
        }
    }

    #[tokio::test]
    async fn an_unauthenticated_caller_is_refused_before_anything_else() {
        for (method, uri, body) in routes() {
            assert_eq!(
                status(method, &uri, None, body).await,
                StatusCode::UNAUTHORIZED,
                "{method} {uri} devrait exiger une authentification"
            );
        }
    }

    #[tokio::test]
    async fn an_unknown_admin_route_is_still_a_404() {
        // Guards against the guard: the 403s above must come from the handlers,
        // not from some blanket rule that answers 403 for anything under /admin.
        assert_eq!(
            status("GET", "/admin/nonexistent", Some("user"), "").await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn an_admin_is_refused_on_the_input_before_the_database_is_touched() {
        // These would hang on a connection attempt if validation ran after the
        // first query — so this also pins the "validate before any DB work" rule.
        let cases = [
            ("POST", "/admin/mailboxes", r#"{"address":"pas-une-adresse","user_id":"00000000-0000-0000-0000-000000000002"}"#),
            ("POST", "/admin/mailboxes", r#"{"address":"@example.com","user_id":"00000000-0000-0000-0000-000000000002"}"#),
            ("POST", "/admin/aliases", r#"{"address":"nope","destinations":["a@b.com"]}"#),
            ("POST", "/admin/mailing-lists", r#"{"address":"nope","name":"Équipe"}"#),
            ("PUT", "/admin/domains/localhost", r#"{}"#),
        ];

        for (method, uri, body) in cases {
            let req = Request::builder()
                .method(method)
                .uri(uri)
                .header("X-Kubuno-User-Id", "00000000-0000-0000-0000-000000000001")
                .header("X-Kubuno-User-Role", "admin")
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .expect("requête");
            let status = test_router().oneshot(req).await.expect("réponse").status();
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{method} {uri} devrait refuser l'entrée avant toute requête SQL"
            );
        }
    }
}
