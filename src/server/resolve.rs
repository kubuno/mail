//! Turning one envelope recipient into what must actually happen to it.
//!
//! Until `migrations/000024_mail_addresses`, a recipient was resolved by
//! guessing: a login in `mail.mailbox_credentials` that happens to look like an
//! address, then the address of an account we merely poll at another provider.
//! Neither can express an alias, a catch-all, a distribution list or a mailbox
//! whose owner is away, so the address became a first-class object and this
//! module is the single place that reads it.
//!
//! # The order, and why it is that order
//!
//! 1. `mail.mailboxes`      — an exact address we own: file it there.
//! 2. `mail.aliases`        — an exact address that is a redirection: expand it.
//! 3. `mail.mailing_lists`  — an exact address that is a membership: expand it.
//! 4. the domain's catch-all alias (`@domain`): expand it.
//! 5. legacy: `mail.mailbox_credentials`, then `mail.accounts`.
//! 6. nothing matched: `550 5.1.1 No such user here`.
//!
//! Steps 1–3 are exact matches and mutually exclusive (the migration enforces
//! that with a trigger), so their relative order only decides which table is
//! read first, never who wins. Step 4 is the wildcard and therefore comes after
//! every exact match — a catch-all that shadowed a real mailbox would make the
//! mailbox unreachable without anybody touching it. Step 5 is kept because
//! users receive mail through it TODAY; removing it would lose mail silently.
//!
//! ⚠️ The order above puts the catch-all BEFORE the legacy paths, as documented
//! in the migration. Creating a catch-all on a domain therefore captures the
//! addresses that used to reach a user through step 5. That is a deliberate,
//! visible administrative act — but it is worth knowing before creating one.
//!
//! # Expansion is recursive, so it is bounded
//!
//! An alias destination or a list member is itself an address to resolve: it may
//! be a mailbox, another list, or somewhere else entirely. Two aliases pointing
//! at each other is a mistake somebody WILL make, and it must cost a rejected
//! recipient, never a server that stops answering. Three independent nets, all
//! taken from Postfix's `cleanup_map1n` (`virtual_alias_recursion_limit`,
//! `virtual_alias_expansion_limit`, and its `been_here` table):
//!
//! * loop detection — an address already expanded is never expanded twice;
//! * [`MAX_DEPTH`] — a ceiling on nesting, in case a loop somehow escapes;
//! * [`MAX_FANOUT`] — a ceiling on how many addresses one recipient becomes.
//!
//! Hitting a limit is answered with a **temporary** refusal, exactly as Postfix
//! does (`4.6.0 Alias expansion error`): a broken alias table is something an
//! administrator fixes in minutes, and a 5xx would bounce, for good, mail that
//! becomes deliverable again as soon as it is fixed.
//!
//! # What is never allowed to be silent
//!
//! An expansion that produces nothing is refused explicitly. A remote
//! destination that cannot be forwarded (outbound delivery is off) refuses the
//! recipient rather than dropping that branch. And a database error propagates
//! as `Err`, which the SMTP front-end answers with `451` — answering "no such
//! user" because the database hiccuped bounces perfectly valid mail for good.

use std::collections::{HashSet, VecDeque};

use anyhow::{Context, Result};
use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use crate::server::config::ServerConfig;
use crate::server::deliver::LocalTarget;
use crate::server::hygiene;

/// How deep an alias may nest before the recipient is refused.
///
/// Postfix's `virtual_alias_recursion_limit` defaults to 1000, a number that is
/// only safe because a loop is caught first by its `been_here` table — the
/// depth limit there is a last resort, not the working limit. The same is true
/// here, and a chain of more than ten redirections is a configuration mistake
/// long before it is a legitimate setup. Ten also bounds the worst case at ten
/// round-trips to the database per recipient, which is what keeps a hostile
/// (or merely broken) alias table from turning RCPT into a slow path.
pub const MAX_DEPTH: usize = 10;

/// How many addresses one envelope recipient may expand to.
///
/// Postfix's `virtual_alias_expansion_limit`, same default: past this the
/// message is not accepted and the sender is asked to try again later.
pub const MAX_FANOUT: usize = 1000;

// ── What the directory holds ─────────────────────────────────────────────────

/// A local mailbox, as far as delivery cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxRow {
    pub address:     String,
    pub user_id:     Uuid,
    /// 0 = no limit. See the TODO in [`LocalDelivery::quota_bytes`].
    pub quota_bytes: i64,
    pub is_active:   bool,
}

/// A redirection. `address` is a whole address, or `@domain` for a catch-all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasRow {
    pub address:      String,
    pub destinations: Vec<String>,
    pub is_active:    bool,
}

/// A distribution list and the answer to "who may post here".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    pub address:         String,
    pub policy:          PostPolicy,
    pub allowed_senders: Vec<String>,
    pub members:         Vec<String>,
    pub is_active:       bool,
}

/// Who may send TO a list. Answering this wrong is how an internal list becomes
/// an open spam relay, so the default of an unknown value is the restrictive
/// one — the same default the column itself carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PostPolicy {
    /// Open to the world. A public contact address; expect abuse.
    Anyone,
    /// Only an address that is a member of the list.
    Members,
    /// Only a sender in one of the instance's own domains.
    #[default]
    Internal,
    /// Only the addresses (or `@domain` entries) in `allowed_senders`.
    Allowed,
}

impl PostPolicy {
    /// Reads the column value. An unrecognised one is logged and treated as
    /// `internal`: a policy nobody can parse must not silently become "anyone".
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "anyone" => Self::Anyone,
            "members" => Self::Members,
            "internal" => Self::Internal,
            "allowed" => Self::Allowed,
            other => {
                tracing::error!(policy = %other, "Politique de publication inconnue — repli sur « internal »");
                Self::Internal
            }
        }
    }
}

// ── What resolution produces ─────────────────────────────────────────────────

/// One local mailbox a message must be filed into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDelivery {
    /// The address that resolved here: the envelope recipient itself, or the
    /// alias destination / list member it expanded to. Kept because it is what
    /// belongs in the trace headers and the logs — "delivered to `contact@`"
    /// when the mail actually landed in `alice@` is a support call.
    pub address: String,
    pub target:  LocalTarget,
    /// The mailbox's quota, 0 meaning unlimited.
    ///
    /// TODO(quota): not enforced yet. Enforcing it needs the size of what is
    /// already stored, and `mail.messages` has no size column — the bodies are
    /// stored parsed apart, so any total computed from them is an estimate, and
    /// an estimate here means refusing legitimate mail with `452 4.2.2` on a
    /// mailbox that is not actually full. The refusal must stay temporary when
    /// it lands: a quota is a passing state, and a 5xx would bounce for good
    /// mail that the next deletion makes deliverable.
    pub quota_bytes: i64,
}

/// What one envelope recipient turns into once expanded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expansion {
    /// Mailboxes to file into, deduplicated, in discovery order.
    pub local:  Vec<LocalDelivery>,
    /// Addresses outside our domains, to be handed to the outbound queue.
    pub remote: Vec<String>,
}

impl Expansion {
    /// Total number of addresses this recipient became.
    pub fn len(&self) -> usize {
        self.local.len() + self.remote.len()
    }

    pub fn is_empty(&self) -> bool {
        self.local.is_empty() && self.remote.is_empty()
    }
}

/// Why a recipient cannot be served, and the exact SMTP line that says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing at all matched.
    NoSuchUser,
    /// The address exists but its owner (or the administrator) turned it off.
    /// Distinguishing this from "no such user" is what every MTA does, and it
    /// is the difference between "you have the wrong address" and "this person
    /// is not reachable here any more".
    MailboxDisabled,
    /// The address exists but has nowhere to file mail yet. Temporary on
    /// purpose: an administrator creating the missing account makes it work.
    MailboxNotReady,
    /// The list's `post_policy` does not accept this sender.
    PostingDenied,
    /// The expansion succeeded and produced no deliverable address.
    ExpandsToNothing,
    /// Nesting or fan-out limit hit — a broken alias table, which is temporary.
    ExpansionError,
    /// A destination outside our domains, with outbound delivery switched off.
    OutboundDisabled,
    /// TODO(quota): produced once mailbox usage can be measured; see
    /// [`LocalDelivery::quota_bytes`].
    MailboxFull,
}

impl Refusal {
    /// The reply line, enhanced status code included (RFC 3463).
    pub fn reply(self) -> &'static str {
        match self {
            Self::NoSuchUser => "550 5.1.1 No such user here",
            Self::MailboxDisabled => "550 5.2.1 Mailbox disabled",
            Self::MailboxNotReady => "450 4.2.1 Mailbox temporarily unavailable",
            Self::PostingDenied => "550 5.7.2 You are not allowed to post to this list",
            Self::ExpandsToNothing => "550 5.1.1 Address expands to no deliverable recipient",
            Self::ExpansionError => "450 4.6.0 Alias expansion error, try again later",
            Self::OutboundDisabled => "451 4.3.5 Cannot forward: outbound delivery is disabled",
            Self::MailboxFull => "452 4.2.2 Mailbox is over quota, try again later",
        }
    }
}

/// The verdict for one recipient. `Err` is reserved for a failure of OURS (the
/// database), which the caller answers with `451`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Accept(Expansion),
    Refuse(Refusal),
}

// ── The directory the resolver reads ─────────────────────────────────────────

/// Every lookup the resolver needs, behind a trait so the expansion logic — the
/// part that has limits, loops and policies to get right — is testable without
/// a database.
#[async_trait]
pub trait Directory: Send + Sync {
    /// Exact-address mailbox, active or not: the resolver needs to tell a
    /// disabled mailbox from a missing one.
    async fn mailbox(&self, address: &str) -> Result<Option<MailboxRow>>;

    /// Exact-address alias.
    async fn alias(&self, address: &str) -> Result<Option<AliasRow>>;

    /// Exact-address distribution list, members included.
    async fn mailing_list(&self, address: &str) -> Result<Option<ListRow>>;

    /// The domain's `@domain` alias, if it has one.
    async fn catch_all(&self, domain: &str) -> Result<Option<AliasRow>>;

    /// The pre-`000024` paths: a mailbox credential's login, then an account
    /// address. Returns the user the address belongs to.
    async fn legacy_user(&self, address: &str) -> Result<Option<Uuid>>;

    /// The account row a delivery for this user is filed under — every message
    /// and thread hangs off one (`mail.messages.account_id` is NOT NULL).
    async fn filing_account(&self, user_id: Uuid, address: &str) -> Result<Option<Uuid>>;
}

// ── Posting policy ───────────────────────────────────────────────────────────

/// Whether `sender` may post to a list. Pure, so every branch is a test.
///
/// ⚠️ `Internal` trusts the envelope sender's domain, which an unauthenticated
/// client can forge outright. What makes it real is the SPF/DKIM/DMARC verdict
/// computed at DATA (`server::authres`) and the reject action the administrator
/// sets: this predicate answers "does the address claim to be internal", the
/// authentication policy answers "is the claim true". An authenticated session
/// needs neither — it has already proved who it is.
pub fn post_allowed(
    list: &ListRow,
    cfg: &ServerConfig,
    sender: &str,
    authenticated: bool,
) -> bool {
    let sender = sender.trim().to_ascii_lowercase();

    match list.policy {
        PostPolicy::Anyone => true,
        // A null sender (`<>`) is a bounce. It is never a member, never on an
        // allow list and never internal, so it only reaches an open list.
        _ if sender.is_empty() => false,
        PostPolicy::Members => list
            .members
            .iter()
            .any(|m| m.trim().eq_ignore_ascii_case(&sender)),
        PostPolicy::Internal => authenticated || cfg.is_local_domain(&sender),
        // Whole addresses and `@domain` entries, the same shape the operator's
        // allow/block lists already use elsewhere in the server.
        PostPolicy::Allowed => list.allowed_senders.iter().any(|entry| {
            let entry = entry.trim().to_ascii_lowercase();
            if let Some(domain) = entry.strip_prefix('@') {
                sender
                    .rsplit_once('@')
                    .is_some_and(|(_, d)| d == domain)
            } else {
                !entry.is_empty() && entry == sender
            }
        }),
    }
}

// ── The resolver ─────────────────────────────────────────────────────────────

/// Lowercased, angle brackets and whitespace stripped: the shape every lookup
/// compares against, since the tables store addresses lowercased.
fn normalize(address: &str) -> String {
    address
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_ascii_lowercase()
}

fn domain_of(address: &str) -> Option<&str> {
    address.rsplit_once('@').map(|(_, d)| d).filter(|d| !d.is_empty())
}

/// Resolves one envelope recipient, expanding aliases and lists.
///
/// `recipient` must already have been decided local by the caller
/// (`smtp::relay_decision`); anything else is a caller bug and is refused.
/// `sender` is the envelope sender, needed by the lists' posting policy.
pub async fn resolve<D: Directory + ?Sized>(
    dir: &D,
    cfg: &ServerConfig,
    sender: &str,
    authenticated: bool,
    recipient: &str,
) -> Result<Outcome> {
    let top = normalize(recipient);
    if !hygiene::valid_envelope_address(&top) {
        return Ok(Outcome::Refuse(Refusal::NoSuchUser));
    }
    if !cfg.is_local_domain(&top) {
        // The relay decision is made before we are called and is the only place
        // it may be made. Reaching here means something bypassed it; refusing
        // is the only safe answer, and the log says where to look.
        tracing::error!(
            recipient = %top,
            "Résolution demandée pour une adresse hors des domaines locaux — refusée"
        );
        return Ok(Outcome::Refuse(Refusal::NoSuchUser));
    }

    let mut out = Expansion::default();
    // Postfix's `been_here`: an address that has already been expanded is never
    // expanded again, which is what makes A→B→A cost one skipped branch instead
    // of an unbounded walk.
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    queue.push_back((top.clone(), 0));

    while let Some((address, depth)) = queue.pop_front() {
        if !seen.insert(address.clone()) {
            tracing::warn!(
                address = %address, recipient = %top,
                "Boucle d'expansion : adresse déjà développée — branche ignorée"
            );
            continue;
        }
        if depth > MAX_DEPTH {
            tracing::warn!(
                recipient = %top, depth, limit = MAX_DEPTH,
                "Expansion d'alias trop profonde — destinataire refusé"
            );
            return Ok(Outcome::Refuse(Refusal::ExpansionError));
        }
        // Everything already produced, plus everything still to be looked at:
        // the count Postfix bounds, so a fan-out cannot be hidden in the queue.
        if out.len() + queue.len() + 1 > MAX_FANOUT {
            tracing::warn!(
                recipient = %top, produced = out.len(), pending = queue.len(), limit = MAX_FANOUT,
                "Expansion d'alias trop large — destinataire refusé"
            );
            return Ok(Outcome::Refuse(Refusal::ExpansionError));
        }

        // Only the address the client named gets a verdict of its own. Deeper
        // down, a dead or disabled destination costs its branch and no more:
        // one member on leave must not refuse the whole list.
        let top_level = depth == 0;

        // ── Outside our domains: this is an outgoing message ─────────────────
        if !cfg.is_local_domain(&address) {
            if !cfg.outbound_enabled {
                // Accepting and dropping the branch would make the message
                // vanish; accepting only the local half would answer 250 for an
                // address we cannot serve. Refuse, temporarily — switching
                // outbound on makes the sender's next attempt work.
                tracing::warn!(
                    recipient = %top, destination = %address,
                    "Destination distante impossible : envoi sortant désactivé — destinataire refusé"
                );
                return Ok(Outcome::Refuse(Refusal::OutboundDisabled));
            }
            out.remote.push(address);
            continue;
        }

        // ── 1. A mailbox we own ─────────────────────────────────────────────
        if let Some(mailbox) = dir.mailbox(&address).await? {
            if !mailbox.is_active {
                if top_level {
                    return Ok(Outcome::Refuse(Refusal::MailboxDisabled));
                }
                tracing::warn!(
                    recipient = %top, destination = %address,
                    "Boîte désactivée dans une expansion — destination ignorée"
                );
                continue;
            }
            match dir.filing_account(mailbox.user_id, &address).await? {
                Some(account_id) => out.local.push(LocalDelivery {
                    address,
                    target: LocalTarget { user_id: mailbox.user_id, account_id },
                    quota_bytes: mailbox.quota_bytes,
                }),
                None => {
                    // The mailbox exists but its owner has no mail account, so
                    // there is nowhere to put the message. Never accept it:
                    // accepting would mean losing it.
                    tracing::error!(
                        recipient = %top, destination = %address, user = %mailbox.user_id,
                        "Boîte locale sans compte mail : dépôt impossible"
                    );
                    if top_level {
                        return Ok(Outcome::Refuse(Refusal::MailboxNotReady));
                    }
                }
            }
            continue;
        }

        // ── 2. An alias ─────────────────────────────────────────────────────
        if let Some(alias) = dir.alias(&address).await? {
            if !push_alias(&alias, &top, top_level, depth, &mut queue) {
                return Ok(Outcome::Refuse(Refusal::MailboxDisabled));
            }
            continue;
        }

        // ── 3. A distribution list ──────────────────────────────────────────
        if let Some(list) = dir.mailing_list(&address).await? {
            if !list.is_active {
                if top_level {
                    return Ok(Outcome::Refuse(Refusal::MailboxDisabled));
                }
                tracing::warn!(
                    recipient = %top, destination = %address,
                    "Liste désactivée dans une expansion — destination ignorée"
                );
                continue;
            }
            if !post_allowed(&list, cfg, sender, authenticated) {
                // This is the check that keeps an internal list from becoming a
                // spam relay, so it refuses the recipient outright rather than
                // quietly dropping the branch.
                tracing::warn!(
                    list = %list.address, policy = ?list.policy, sender = %sender,
                    "Publication refusée : expéditeur non autorisé sur cette liste"
                );
                return Ok(Outcome::Refuse(Refusal::PostingDenied));
            }
            for member in &list.members {
                push_destination(member, &top, depth, &mut queue);
            }
            continue;
        }

        // ── 4. The domain's catch-all, and only now ─────────────────────────
        if let Some(domain) = domain_of(&address) {
            if let Some(alias) = dir.catch_all(domain).await? {
                if !push_alias(&alias, &top, top_level, depth, &mut queue) {
                    return Ok(Outcome::Refuse(Refusal::MailboxDisabled));
                }
                continue;
            }
        }

        // ── 5. The legacy paths, kept because mail arrives through them ─────
        if let Some(user_id) = dir.legacy_user(&address).await? {
            match dir.filing_account(user_id, &address).await? {
                Some(account_id) => {
                    out.local.push(LocalDelivery {
                        address,
                        target: LocalTarget { user_id, account_id },
                        quota_bytes: 0,
                    });
                    continue;
                }
                None => {
                    tracing::error!(
                        recipient = %top, destination = %address, user = %user_id,
                        "Destinataire local sans compte mail : dépôt impossible"
                    );
                    if top_level {
                        return Ok(Outcome::Refuse(Refusal::MailboxNotReady));
                    }
                    continue;
                }
            }
        }

        // ── 6. Nothing here answers to that name ────────────────────────────
        if top_level {
            return Ok(Outcome::Refuse(Refusal::NoSuchUser));
        }
        tracing::warn!(
            recipient = %top, destination = %address,
            "Destination inconnue dans une expansion — ignorée"
        );
    }

    if out.is_empty() {
        // An alias whose every destination is dead, or a list with no members.
        // Silence here is the black hole the migration warns about.
        tracing::warn!(
            recipient = %top,
            "Expansion sans aucun destinataire servable — destinataire refusé"
        );
        return Ok(Outcome::Refuse(Refusal::ExpandsToNothing));
    }

    dedup(&mut out);
    Ok(Outcome::Accept(out))
}

/// Queues an alias's destinations. Returns `false` when the alias is disabled
/// and it is the address the client named — the caller turns that into a
/// "mailbox disabled" refusal.
fn push_alias(
    alias: &AliasRow,
    recipient: &str,
    top_level: bool,
    depth: usize,
    queue: &mut VecDeque<(String, usize)>,
) -> bool {
    if !alias.is_active {
        if top_level {
            return false;
        }
        tracing::warn!(
            recipient = %recipient, alias = %alias.address,
            "Alias désactivé dans une expansion — destination ignorée"
        );
        return true;
    }
    for destination in &alias.destinations {
        push_destination(destination, recipient, depth, queue);
    }
    true
}

/// Queues one destination, after checking it could be an address at all. A
/// malformed row must cost its own branch, never the whole recipient.
fn push_destination(
    destination: &str,
    recipient: &str,
    depth: usize,
    queue: &mut VecDeque<(String, usize)>,
) {
    let address = normalize(destination);
    if !hygiene::valid_envelope_address(&address) {
        tracing::warn!(
            recipient = %recipient, destination = %destination,
            "Destination d'expansion invalide — ignorée"
        );
        return;
    }
    queue.push_back((address, depth + 1));
}

/// Removes the duplicates an expansion produces on its own — two aliases
/// leading to the same mailbox, or a member who is also named directly. Local
/// deliveries are deduplicated on where they LAND (the account row), because
/// that is what would receive two copies of the same message.
fn dedup(expansion: &mut Expansion) {
    let mut seen_local: HashSet<LocalTarget> = HashSet::new();
    expansion.local.retain(|d| seen_local.insert(d.target));

    let mut seen_remote: HashSet<String> = HashSet::new();
    expansion.remote.retain(|a| seen_remote.insert(a.clone()));
}

// ── The PostgreSQL directory ─────────────────────────────────────────────────

/// The real directory: `mail` schema, plain queries, every failure logged
/// before it is returned.
pub struct PgDirectory<'a> {
    db: &'a PgPool,
}

impl<'a> PgDirectory<'a> {
    pub fn new(db: &'a PgPool) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Directory for PgDirectory<'_> {
    async fn mailbox(&self, address: &str) -> Result<Option<MailboxRow>> {
        let row: Option<(String, Uuid, i64, bool)> = sqlx::query_as(
            "SELECT address, user_id, quota_bytes, is_active
             FROM mail.mailboxes WHERE address = $1",
        )
        .bind(address)
        .fetch_optional(self.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, address = %address, "Lecture des boîtes locales");
            e
        })
        .context("Lecture des boîtes locales")?;

        Ok(row.map(|(address, user_id, quota_bytes, is_active)| MailboxRow {
            address,
            user_id,
            quota_bytes,
            is_active,
        }))
    }

    async fn alias(&self, address: &str) -> Result<Option<AliasRow>> {
        let row: Option<(String, Vec<String>, bool)> = sqlx::query_as(
            "SELECT address, destinations, is_active FROM mail.aliases WHERE address = $1",
        )
        .bind(address)
        .fetch_optional(self.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, address = %address, "Lecture des alias");
            e
        })
        .context("Lecture des alias")?;

        Ok(row.map(|(address, destinations, is_active)| AliasRow {
            address,
            destinations,
            is_active,
        }))
    }

    async fn mailing_list(&self, address: &str) -> Result<Option<ListRow>> {
        let row: Option<(Uuid, String, String, Vec<String>, bool)> = sqlx::query_as(
            "SELECT id, address, post_policy, allowed_senders, is_active
             FROM mail.mailing_lists WHERE address = $1",
        )
        .bind(address)
        .fetch_optional(self.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, address = %address, "Lecture des listes de diffusion");
            e
        })
        .context("Lecture des listes de diffusion")?;

        let (id, address, policy, allowed_senders, is_active) = match row {
            Some(row) => row,
            None => return Ok(None),
        };

        // Members are only read once the list is known: a lookup that misses —
        // the common case — costs one query, not two.
        let members: Vec<String> = sqlx::query_scalar(
            "SELECT address FROM mail.mailing_list_members WHERE list_id = $1",
        )
        .bind(id)
        .fetch_all(self.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, list = %address, "Lecture des membres de la liste");
            e
        })
        .context("Lecture des membres de la liste")?;

        Ok(Some(ListRow {
            address,
            policy: PostPolicy::parse(&policy),
            allowed_senders,
            members,
            is_active,
        }))
    }

    async fn catch_all(&self, domain: &str) -> Result<Option<AliasRow>> {
        // The partial unique index guarantees at most one per domain, so this
        // cannot depend on row order.
        let row: Option<(String, Vec<String>, bool)> = sqlx::query_as(
            "SELECT address, destinations, is_active
             FROM mail.aliases WHERE domain = $1 AND is_catch_all",
        )
        .bind(domain)
        .fetch_optional(self.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, domain = %domain, "Lecture de l'alias attrape-tout");
            e
        })
        .context("Lecture de l'alias attrape-tout")?;

        Ok(row.map(|(address, destinations, is_active)| AliasRow {
            address,
            destinations,
            is_active,
        }))
    }

    async fn legacy_user(&self, address: &str) -> Result<Option<Uuid>> {
        // The mailbox credential — what a mail client actually logs in with.
        let by_credential: Option<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM mail.mailbox_credentials WHERE username = $1")
                .bind(address)
                .fetch_optional(self.db)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Résolution du destinataire (identifiants de boîte)");
                    e
                })
                .context("Lecture des identifiants de boîte")?;

        if let Some((user_id,)) = by_credential {
            return Ok(Some(user_id));
        }

        // Failing that, one of the user's configured accounts.
        let by_account: Option<(Uuid,)> = sqlx::query_as(
            "SELECT user_id FROM mail.accounts
             WHERE LOWER(email_address) = $1
             ORDER BY is_default DESC, is_active DESC, created_at
             LIMIT 1",
        )
        .bind(address)
        .fetch_optional(self.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Résolution du destinataire (comptes)");
            e
        })
        .context("Lecture des comptes")?;

        Ok(by_account.map(|(user_id,)| user_id))
    }

    async fn filing_account(&self, user_id: Uuid, address: &str) -> Result<Option<Uuid>> {
        // A message to a hosted address belongs in ITS OWN local account, so the
        // local account bearing exactly this address wins over everything else —
        // otherwise a message to `test001@` could land in the owner's Gmail. Then
        // the historical order: the account carrying the address, the user's
        // default, any active one.
        let account: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM mail.accounts
             WHERE user_id = $1
             ORDER BY (kind = 'local' AND LOWER(email_address) = $2) DESC,
                      (LOWER(email_address) = $2) DESC,
                      is_default DESC, is_active DESC, created_at
             LIMIT 1",
        )
        .bind(user_id)
        .bind(address)
        .fetch_optional(self.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Choix du compte de dépôt");
            e
        })
        .context("Lecture du compte de dépôt")?;

        Ok(account.map(|(id,)| id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// An in-memory directory: the expansion logic is what has limits, loops
    /// and policies to get right, and none of that needs PostgreSQL.
    #[derive(Default)]
    struct Fake {
        mailboxes: HashMap<String, MailboxRow>,
        aliases:   HashMap<String, AliasRow>,
        lists:     HashMap<String, ListRow>,
        catch_all: HashMap<String, AliasRow>,
        legacy:    HashMap<String, Uuid>,
        /// Users with no account row, to exercise the "nowhere to file" path.
        no_account: HashSet<Uuid>,
    }

    fn uid(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    impl Fake {
        fn mailbox(mut self, address: &str, user: u8) -> Self {
            self.mailboxes.insert(
                address.into(),
                MailboxRow {
                    address: address.into(),
                    user_id: uid(user),
                    quota_bytes: 0,
                    is_active: true,
                },
            );
            self
        }

        fn disabled_mailbox(mut self, address: &str, user: u8) -> Self {
            self.mailboxes.insert(
                address.into(),
                MailboxRow {
                    address: address.into(),
                    user_id: uid(user),
                    quota_bytes: 0,
                    is_active: false,
                },
            );
            self
        }

        fn alias(mut self, address: &str, destinations: &[&str]) -> Self {
            self.aliases.insert(
                address.into(),
                AliasRow {
                    address:      address.into(),
                    destinations: destinations.iter().map(|d| (*d).into()).collect(),
                    is_active:    true,
                },
            );
            self
        }

        fn disabled_alias(mut self, address: &str, destinations: &[&str]) -> Self {
            self.aliases.insert(
                address.into(),
                AliasRow {
                    address:      address.into(),
                    destinations: destinations.iter().map(|d| (*d).into()).collect(),
                    is_active:    false,
                },
            );
            self
        }

        fn catch_all(mut self, domain: &str, destinations: &[&str]) -> Self {
            self.catch_all.insert(
                domain.into(),
                AliasRow {
                    address:      format!("@{domain}"),
                    destinations: destinations.iter().map(|d| (*d).into()).collect(),
                    is_active:    true,
                },
            );
            self
        }

        fn list(mut self, address: &str, policy: PostPolicy, members: &[&str], allowed: &[&str]) -> Self {
            self.lists.insert(
                address.into(),
                ListRow {
                    address:         address.into(),
                    policy,
                    allowed_senders: allowed.iter().map(|a| (*a).into()).collect(),
                    members:         members.iter().map(|m| (*m).into()).collect(),
                    is_active:       true,
                },
            );
            self
        }

        fn legacy(mut self, address: &str, user: u8) -> Self {
            self.legacy.insert(address.into(), uid(user));
            self
        }

        fn without_account(mut self, user: u8) -> Self {
            self.no_account.insert(uid(user));
            self
        }
    }

    #[async_trait]
    impl Directory for Fake {
        async fn mailbox(&self, address: &str) -> Result<Option<MailboxRow>> {
            Ok(self.mailboxes.get(address).cloned())
        }
        async fn alias(&self, address: &str) -> Result<Option<AliasRow>> {
            Ok(self.aliases.get(address).cloned())
        }
        async fn mailing_list(&self, address: &str) -> Result<Option<ListRow>> {
            Ok(self.lists.get(address).cloned())
        }
        async fn catch_all(&self, domain: &str) -> Result<Option<AliasRow>> {
            Ok(self.catch_all.get(domain).cloned())
        }
        async fn legacy_user(&self, address: &str) -> Result<Option<Uuid>> {
            Ok(self.legacy.get(address).copied())
        }
        async fn filing_account(&self, user_id: Uuid, _address: &str) -> Result<Option<Uuid>> {
            if self.no_account.contains(&user_id) {
                return Ok(None);
            }
            // One account per user in the fake: enough to tell two users apart
            // and to make the deduplication observable.
            Ok(Some(user_id))
        }
    }

    /// A directory whose every lookup fails, to prove a database problem is
    /// never turned into "no such user".
    struct Broken;

    #[async_trait]
    impl Directory for Broken {
        async fn mailbox(&self, _address: &str) -> Result<Option<MailboxRow>> {
            anyhow::bail!("base indisponible")
        }
        async fn alias(&self, _address: &str) -> Result<Option<AliasRow>> {
            anyhow::bail!("base indisponible")
        }
        async fn mailing_list(&self, _address: &str) -> Result<Option<ListRow>> {
            anyhow::bail!("base indisponible")
        }
        async fn catch_all(&self, _domain: &str) -> Result<Option<AliasRow>> {
            anyhow::bail!("base indisponible")
        }
        async fn legacy_user(&self, _address: &str) -> Result<Option<Uuid>> {
            anyhow::bail!("base indisponible")
        }
        async fn filing_account(&self, _user_id: Uuid, _address: &str) -> Result<Option<Uuid>> {
            anyhow::bail!("base indisponible")
        }
    }

    fn cfg() -> ServerConfig {
        ServerConfig {
            domains: vec!["kubuno.test".into(), "autre.test".into()],
            outbound_enabled: true,
            ..ServerConfig::default()
        }
    }

    async fn run(dir: &Fake, cfg: &ServerConfig, recipient: &str) -> Outcome {
        resolve(dir, cfg, "expediteur@dehors.test", false, recipient)
            .await
            .expect("la résolution ne doit pas échouer")
    }

    fn accepted(outcome: Outcome) -> Expansion {
        match outcome {
            Outcome::Accept(expansion) => expansion,
            Outcome::Refuse(refusal) => panic!("refus inattendu : {}", refusal.reply()),
        }
    }

    fn refused(outcome: Outcome) -> Refusal {
        match outcome {
            Outcome::Refuse(refusal) => refusal,
            Outcome::Accept(expansion) => panic!("acceptation inattendue : {expansion:?}"),
        }
    }

    fn addresses(expansion: &Expansion) -> Vec<&str> {
        expansion.local.iter().map(|d| d.address.as_str()).collect()
    }

    // ── Order ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_mailbox_wins_over_everything_else() {
        // The migration's trigger forbids this in the database, but the
        // resolver must not depend on the trigger to be right.
        let dir = Fake::default()
            .mailbox("alice@kubuno.test", 1)
            .alias("alice@kubuno.test", &["bob@kubuno.test"])
            .legacy("alice@kubuno.test", 9)
            .mailbox("bob@kubuno.test", 2);

        let expansion = accepted(run(&dir, &cfg(), "alice@kubuno.test").await);
        assert_eq!(addresses(&expansion), ["alice@kubuno.test"]);
        assert_eq!(expansion.local[0].target.user_id, uid(1));
    }

    #[tokio::test]
    async fn an_alias_is_read_before_a_list_and_before_the_catch_all() {
        let dir = Fake::default()
            .alias("contact@kubuno.test", &["alice@kubuno.test"])
            .list("contact@kubuno.test", PostPolicy::Anyone, &["bob@kubuno.test"], &[])
            .catch_all("kubuno.test", &["poubelle@kubuno.test"])
            .mailbox("alice@kubuno.test", 1)
            .mailbox("bob@kubuno.test", 2)
            .mailbox("poubelle@kubuno.test", 3);

        let expansion = accepted(run(&dir, &cfg(), "contact@kubuno.test").await);
        assert_eq!(addresses(&expansion), ["alice@kubuno.test"]);
    }

    #[tokio::test]
    async fn a_list_is_read_before_the_catch_all() {
        let dir = Fake::default()
            .list("equipe@kubuno.test", PostPolicy::Anyone, &["alice@kubuno.test"], &[])
            .catch_all("kubuno.test", &["poubelle@kubuno.test"])
            .mailbox("alice@kubuno.test", 1)
            .mailbox("poubelle@kubuno.test", 3);

        let expansion = accepted(run(&dir, &cfg(), "equipe@kubuno.test").await);
        assert_eq!(addresses(&expansion), ["alice@kubuno.test"]);
    }

    #[tokio::test]
    async fn the_catch_all_only_answers_when_nothing_else_did() {
        let dir = Fake::default()
            .catch_all("kubuno.test", &["poubelle@kubuno.test"])
            .mailbox("poubelle@kubuno.test", 3)
            .mailbox("alice@kubuno.test", 1);

        // An address nothing else claims: the catch-all takes it.
        let expansion = accepted(run(&dir, &cfg(), "inconnu@kubuno.test").await);
        assert_eq!(addresses(&expansion), ["poubelle@kubuno.test"]);

        // An address a mailbox claims: the catch-all must not shadow it.
        let expansion = accepted(run(&dir, &cfg(), "alice@kubuno.test").await);
        assert_eq!(addresses(&expansion), ["alice@kubuno.test"]);
    }

    #[tokio::test]
    async fn the_catch_all_is_scoped_to_its_own_domain() {
        let dir = Fake::default()
            .catch_all("kubuno.test", &["poubelle@kubuno.test"])
            .mailbox("poubelle@kubuno.test", 3);

        assert_eq!(
            refused(run(&dir, &cfg(), "inconnu@autre.test").await),
            Refusal::NoSuchUser
        );
    }

    #[tokio::test]
    async fn the_legacy_paths_still_receive_mail() {
        // The whole point of keeping them: users receive mail this way today.
        let dir = Fake::default().legacy("ancien@kubuno.test", 7);
        let expansion = accepted(run(&dir, &cfg(), "ancien@kubuno.test").await);
        assert_eq!(addresses(&expansion), ["ancien@kubuno.test"]);
        assert_eq!(expansion.local[0].target.user_id, uid(7));
    }

    #[tokio::test]
    async fn nothing_at_all_is_no_such_user() {
        let dir = Fake::default();
        assert_eq!(
            refused(run(&dir, &cfg(), "personne@kubuno.test").await),
            Refusal::NoSuchUser
        );
    }

    #[tokio::test]
    async fn a_recipient_outside_our_domains_is_never_resolved() {
        // relay_decision is the only place that call is made; reaching here
        // with a foreign address is a bug, and must not become a relay.
        let dir = Fake::default().mailbox("alice@dehors.test", 1);
        assert_eq!(
            refused(run(&dir, &cfg(), "alice@dehors.test").await),
            Refusal::NoSuchUser
        );
    }

    // ── Disabled vs missing ──────────────────────────────────────────────────

    #[tokio::test]
    async fn a_disabled_mailbox_is_not_a_missing_one() {
        let dir = Fake::default().disabled_mailbox("alice@kubuno.test", 1);
        assert_eq!(
            refused(run(&dir, &cfg(), "alice@kubuno.test").await),
            Refusal::MailboxDisabled
        );
        assert!(Refusal::MailboxDisabled.reply().starts_with("550 5.2.1"));
    }

    #[tokio::test]
    async fn a_disabled_member_costs_its_branch_not_the_whole_list() {
        let dir = Fake::default()
            .list("equipe@kubuno.test", PostPolicy::Anyone, &["alice@kubuno.test", "bob@kubuno.test"], &[])
            .disabled_mailbox("alice@kubuno.test", 1)
            .mailbox("bob@kubuno.test", 2);

        let expansion = accepted(run(&dir, &cfg(), "equipe@kubuno.test").await);
        assert_eq!(addresses(&expansion), ["bob@kubuno.test"]);
    }

    #[tokio::test]
    async fn a_disabled_alias_is_refused_rather_than_silently_dropped() {
        let dir = Fake::default()
            .disabled_alias("contact@kubuno.test", &["alice@kubuno.test"])
            .mailbox("alice@kubuno.test", 1);
        assert_eq!(
            refused(run(&dir, &cfg(), "contact@kubuno.test").await),
            Refusal::MailboxDisabled
        );
    }

    #[tokio::test]
    async fn a_disabled_alias_does_not_fall_through_to_the_catch_all() {
        let dir = Fake::default()
            .disabled_alias("contact@kubuno.test", &["alice@kubuno.test"])
            .catch_all("kubuno.test", &["poubelle@kubuno.test"])
            .mailbox("alice@kubuno.test", 1)
            .mailbox("poubelle@kubuno.test", 3);
        assert_eq!(
            refused(run(&dir, &cfg(), "contact@kubuno.test").await),
            Refusal::MailboxDisabled
        );
    }

    #[tokio::test]
    async fn a_mailbox_whose_owner_has_no_account_is_deferred_not_bounced() {
        let dir = Fake::default().mailbox("alice@kubuno.test", 1).without_account(1);
        let refusal = refused(run(&dir, &cfg(), "alice@kubuno.test").await);
        assert_eq!(refusal, Refusal::MailboxNotReady);
        assert!(refusal.reply().starts_with('4'), "jamais un refus définitif");
    }

    // ── Empty expansions ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn an_empty_list_is_refused_explicitly() {
        let dir = Fake::default().list("vide@kubuno.test", PostPolicy::Anyone, &[], &[]);
        assert_eq!(
            refused(run(&dir, &cfg(), "vide@kubuno.test").await),
            Refusal::ExpandsToNothing
        );
    }

    #[tokio::test]
    async fn an_alias_pointing_only_at_dead_addresses_is_refused() {
        let dir = Fake::default().alias("contact@kubuno.test", &["fantome@kubuno.test"]);
        assert_eq!(
            refused(run(&dir, &cfg(), "contact@kubuno.test").await),
            Refusal::ExpandsToNothing
        );
    }

    // ── Loops, depth, fan-out ────────────────────────────────────────────────

    #[tokio::test]
    async fn a_two_alias_loop_costs_a_branch_not_the_server() {
        // A→B→A, the mistake somebody will make. B also reaches a real mailbox,
        // so the message is still delivered — the loop is merely ignored.
        let dir = Fake::default()
            .alias("a@kubuno.test", &["b@kubuno.test"])
            .alias("b@kubuno.test", &["a@kubuno.test", "alice@kubuno.test"])
            .mailbox("alice@kubuno.test", 1);

        let expansion = accepted(run(&dir, &cfg(), "a@kubuno.test").await);
        assert_eq!(addresses(&expansion), ["alice@kubuno.test"]);
    }

    #[tokio::test]
    async fn an_alias_pointing_at_itself_terminates() {
        let dir = Fake::default().alias("a@kubuno.test", &["a@kubuno.test"]);
        assert_eq!(
            refused(run(&dir, &cfg(), "a@kubuno.test").await),
            Refusal::ExpandsToNothing
        );
    }

    #[tokio::test]
    async fn nesting_deeper_than_the_limit_is_refused_temporarily() {
        // A chain of distinct addresses: no loop to catch, only depth.
        let mut dir = Fake::default();
        for i in 0..(MAX_DEPTH + 5) {
            let from = format!("a{i}@kubuno.test");
            let to = format!("a{}@kubuno.test", i + 1);
            dir = dir.alias(&from, &[to.as_str()]);
        }

        let refusal = refused(run(&dir, &cfg(), "a0@kubuno.test").await);
        assert_eq!(refusal, Refusal::ExpansionError);
        assert!(refusal.reply().starts_with("450 4.6.0"), "temporaire : un alias cassé se répare");
    }

    #[tokio::test]
    async fn nesting_up_to_the_limit_still_delivers() {
        let mut dir = Fake::default().mailbox("fin@kubuno.test", 1);
        for i in 0..MAX_DEPTH {
            let from = format!("a{i}@kubuno.test");
            let to = if i + 1 == MAX_DEPTH {
                "fin@kubuno.test".to_string()
            } else {
                format!("a{}@kubuno.test", i + 1)
            };
            dir = dir.alias(&from, &[to.as_str()]);
        }

        let expansion = accepted(run(&dir, &cfg(), "a0@kubuno.test").await);
        assert_eq!(addresses(&expansion), ["fin@kubuno.test"]);
    }

    #[tokio::test]
    async fn a_fan_out_past_the_ceiling_is_refused_temporarily() {
        let members: Vec<String> = (0..(MAX_FANOUT + 10))
            .map(|i| format!("m{i}@kubuno.test"))
            .collect();
        let refs: Vec<&str> = members.iter().map(String::as_str).collect();

        let mut dir = Fake::default().list("geante@kubuno.test", PostPolicy::Anyone, &refs, &[]);
        for (i, member) in members.iter().enumerate() {
            dir = dir.mailbox(member, (i % 200) as u8);
        }

        assert_eq!(
            refused(run(&dir, &cfg(), "geante@kubuno.test").await),
            Refusal::ExpansionError
        );
    }

    // ── Posting policy ───────────────────────────────────────────────────────

    fn list_with(policy: PostPolicy) -> ListRow {
        ListRow {
            address:         "equipe@kubuno.test".into(),
            policy,
            allowed_senders: vec!["ami@dehors.test".into(), "@partenaire.test".into()],
            members:         vec!["alice@kubuno.test".into(), "bob@dehors.test".into()],
            is_active:       true,
        }
    }

    #[test]
    fn policy_anyone_accepts_everyone_including_bounces() {
        let list = list_with(PostPolicy::Anyone);
        assert!(post_allowed(&list, &cfg(), "spammeur@dehors.test", false));
        assert!(post_allowed(&list, &cfg(), "", false));
    }

    #[test]
    fn policy_members_accepts_only_members() {
        let list = list_with(PostPolicy::Members);
        assert!(post_allowed(&list, &cfg(), "alice@kubuno.test", false));
        assert!(post_allowed(&list, &cfg(), "BOB@Dehors.TEST", false), "insensible à la casse");
        assert!(!post_allowed(&list, &cfg(), "intrus@dehors.test", false));
        assert!(!post_allowed(&list, &cfg(), "", false), "un rebond n'est pas un membre");
    }

    #[test]
    fn policy_internal_accepts_our_domains_and_any_authenticated_user() {
        let list = list_with(PostPolicy::Internal);
        assert!(post_allowed(&list, &cfg(), "qui-que-ce-soit@kubuno.test", false));
        assert!(post_allowed(&list, &cfg(), "qui-que-ce-soit@autre.test", false));
        assert!(!post_allowed(&list, &cfg(), "intrus@dehors.test", false));
        // Authentication proves what the envelope only claims.
        assert!(post_allowed(&list, &cfg(), "intrus@dehors.test", true));
    }

    #[test]
    fn policy_allowed_accepts_addresses_and_domain_entries() {
        let list = list_with(PostPolicy::Allowed);
        assert!(post_allowed(&list, &cfg(), "ami@dehors.test", false));
        assert!(post_allowed(&list, &cfg(), "qui@partenaire.test", false));
        assert!(!post_allowed(&list, &cfg(), "autre@dehors.test", false));
        assert!(
            !post_allowed(&list, &cfg(), "alice@kubuno.test", false),
            "être interne ne suffit pas quand la politique est « allowed »"
        );
    }

    #[test]
    fn an_unknown_policy_falls_back_to_the_restrictive_one() {
        assert_eq!(PostPolicy::parse("n'importe quoi"), PostPolicy::Internal);
        assert_eq!(PostPolicy::parse("ANYONE"), PostPolicy::Anyone);
    }

    #[tokio::test]
    async fn an_unauthorised_sender_is_refused_with_5_7_2() {
        let dir = Fake::default()
            .list("interne@kubuno.test", PostPolicy::Internal, &["alice@kubuno.test"], &[])
            .mailbox("alice@kubuno.test", 1);

        // Sender outside our domains, unauthenticated: this is the check that
        // stops the list from becoming a relay.
        let refusal = refused(
            resolve(&dir, &cfg(), "spammeur@dehors.test", false, "interne@kubuno.test")
                .await
                .expect("résolution"),
        );
        assert_eq!(refusal, Refusal::PostingDenied);
        assert!(refusal.reply().starts_with("550 5.7.2"));

        // The same sender, authenticated, gets through.
        let expansion = accepted(
            resolve(&dir, &cfg(), "spammeur@dehors.test", true, "interne@kubuno.test")
                .await
                .expect("résolution"),
        );
        assert_eq!(addresses(&expansion), ["alice@kubuno.test"]);
    }

    #[tokio::test]
    async fn a_nested_list_is_policed_too() {
        let dir = Fake::default()
            .alias("contact@kubuno.test", &["interne@kubuno.test"])
            .list("interne@kubuno.test", PostPolicy::Internal, &["alice@kubuno.test"], &[])
            .mailbox("alice@kubuno.test", 1);

        assert_eq!(
            refused(
                resolve(&dir, &cfg(), "spammeur@dehors.test", false, "contact@kubuno.test")
                    .await
                    .expect("résolution")
            ),
            Refusal::PostingDenied
        );
    }

    // ── Remote destinations ──────────────────────────────────────────────────

    #[tokio::test]
    async fn an_alias_may_forward_outside_when_outbound_is_on() {
        let dir = Fake::default()
            .alias("contact@kubuno.test", &["alice@kubuno.test", "ailleurs@dehors.test"])
            .mailbox("alice@kubuno.test", 1);

        let expansion = accepted(run(&dir, &cfg(), "contact@kubuno.test").await);
        assert_eq!(addresses(&expansion), ["alice@kubuno.test"]);
        assert_eq!(expansion.remote, ["ailleurs@dehors.test"]);
    }

    #[tokio::test]
    async fn forwarding_outside_obeys_the_outbound_switch() {
        let cfg = ServerConfig { outbound_enabled: false, ..cfg() };
        let dir = Fake::default()
            .alias("contact@kubuno.test", &["alice@kubuno.test", "ailleurs@dehors.test"])
            .mailbox("alice@kubuno.test", 1);

        let refusal = refused(run(&dir, &cfg, "contact@kubuno.test").await);
        assert_eq!(refusal, Refusal::OutboundDisabled);
        assert!(refusal.reply().starts_with("451"), "temporaire : l'admin peut l'activer");
    }

    // ── Deduplication ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn two_paths_to_the_same_mailbox_deliver_once() {
        let dir = Fake::default()
            .alias("contact@kubuno.test", &["alice@kubuno.test", "alias2@kubuno.test"])
            .alias("alias2@kubuno.test", &["alice@kubuno.test"])
            .mailbox("alice@kubuno.test", 1);

        let expansion = accepted(run(&dir, &cfg(), "contact@kubuno.test").await);
        assert_eq!(expansion.local.len(), 1, "un seul exemplaire");
    }

    #[tokio::test]
    async fn a_remote_destination_named_twice_is_queued_once() {
        let dir = Fake::default()
            .alias("contact@kubuno.test", &["ailleurs@dehors.test", "relais@kubuno.test"])
            .alias("relais@kubuno.test", &["ailleurs@dehors.test"]);

        let expansion = accepted(run(&dir, &cfg(), "contact@kubuno.test").await);
        assert_eq!(expansion.remote, ["ailleurs@dehors.test"]);
    }

    // ── Malformed rows ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_malformed_destination_costs_its_branch_only() {
        let dir = Fake::default()
            .alias("contact@kubuno.test", &["pas une adresse", "alice@kubuno.test"])
            .mailbox("alice@kubuno.test", 1);

        let expansion = accepted(run(&dir, &cfg(), "contact@kubuno.test").await);
        assert_eq!(addresses(&expansion), ["alice@kubuno.test"]);
    }

    #[tokio::test]
    async fn addresses_are_matched_case_insensitively() {
        let dir = Fake::default().mailbox("alice@kubuno.test", 1);
        let expansion = accepted(run(&dir, &cfg(), "<Alice@Kubuno.TEST>").await);
        assert_eq!(addresses(&expansion), ["alice@kubuno.test"]);
    }

    // ── Failures of ours ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_database_failure_is_never_no_such_user() {
        // The caller turns Err into 451. Turning it into 550 here would bounce
        // valid mail for good because a query hiccuped.
        let outcome = resolve(&Broken, &cfg(), "qui@dehors.test", false, "alice@kubuno.test").await;
        assert!(outcome.is_err(), "une panne de base remonte comme Err, pas comme un refus");
    }
}
