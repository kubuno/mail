//! Runtime configuration of the served protocols, as the administrator left it
//! in the console.
//!
//! The values live in `core.settings` (declared by `module.toml`'s `[[settings]]`
//! blocks) and are read back through `/internal/modules/mail/settings` — the
//! module's own schema may not touch the core's tables, and a background listener
//! has no user token to use the public config route. The module is named in the
//! URL rather than left to be derived from the secret, so the read works on an
//! instance that shares one master secret between modules as well as on one that
//! issues a distinct derived secret per module.
//!
//! Every field here is reachable from the admin panel, and every field here is
//! read by code that acts on it. A setting that changes nothing is worse than an
//! absent one: it tells an administrator a protection is in place when it is not.
//!
//! ## Where the served domains come from
//!
//! Not from here alone. The instance declares its domains in the console and
//! *proves* them by DNS (`core.domains`); this module reads them back through
//! `/internal/domains` and serves the **verified** ones. The `server_domains`
//! setting survives as a **stop-gap list**, added on top, for names that can
//! never carry a public TXT record — `kubuno.local` on a laboratory machine.
//!
//! Two things follow, and both are deliberate:
//!
//!   * A domain that is declared but **not yet verified** is NOT served. A claim
//!     is not a proof, and accepting mail for a name somebody else controls is
//!     the whole failure this design exists to prevent.
//!   * A core that cannot be reached leaves the previously read instance
//!     domains in place ([`last_instance_domains`]). Emptying the list on a
//!     transient failure would make the instance refuse *all* incoming mail.

use std::net::IpAddr;
use std::sync::{OnceLock, RwLock};

use serde::Serialize;
use serde_json::Value;

use super::tls::TlsMode;
use crate::config::settings::Settings;

// ── Trusted upstream relays (mynetworks) ──────────────────────────────────────

/// A CIDR network, used to recognise a trusted internal relay ("upstream").
///
/// A single host with no `/prefix` is stored as a full-length prefix (`/32` for
/// IPv4, `/128` for IPv6). IPv4-mapped IPv6 addresses are folded onto their IPv4
/// form on both sides of a match, so a relay declared as `15.100.1.0/24` still
/// matches a connection that arrived over a dual-stack socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CidrNet {
    base:   IpAddr,
    prefix: u8,
}

impl CidrNet {
    /// Parses `1.2.3.0/24`, `2001:db8::/32`, or a bare address (treated as a
    /// host route). `None` on anything that is not a valid network — a mistyped
    /// entry must be dropped, never widened into a match-all.
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let (addr_part, prefix_part) = match raw.split_once('/') {
            Some((a, p)) => (a.trim(), Some(p.trim())),
            None => (raw, None),
        };
        let base: IpAddr = addr_part.parse().ok()?;
        let max = match base {
            IpAddr::V4(_) => 32u8,
            IpAddr::V6(_) => 128u8,
        };
        let prefix = match prefix_part {
            Some(p) => p.parse::<u8>().ok().filter(|n| *n <= max)?,
            None => max,
        };
        Some(CidrNet { base: normalize_ip(base), prefix })
    }

    /// True when `ip` falls inside this network.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.base, normalize_ip(ip)) {
            (IpAddr::V4(net), IpAddr::V4(addr)) => {
                let mask = ipv4_mask(self.prefix);
                u32::from(net) & mask == u32::from(addr) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(addr)) => {
                let mask = ipv6_mask(self.prefix);
                u128::from(net) & mask == u128::from(addr) & mask
            }
            // A v4 network never matches a v6 address, and vice versa: mapping
            // has already folded the only overlap that exists.
            _ => false,
        }
    }
}

/// Folds an IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) onto its IPv4 form so the
/// same client cannot present two identities depending on the socket family.
fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => ip,
        },
        v4 => v4,
    }
}

/// The high-`prefix`-bits mask for an IPv4 network. `prefix` is `<= 32`.
fn ipv4_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

/// The high-`prefix`-bits mask for an IPv6 network. `prefix` is `<= 128`.
fn ipv6_mask(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    }
}

/// What the three listeners need to run. Ports are `None` when the service is
/// switched off, which is also what a failed read falls back to: an instance
/// that cannot be asked must not start listening on guessed ports.
/// Which protocol a listener speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Smtp,
    Imap,
    Pop3,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Smtp => "smtp",
            Protocol::Imap => "imap",
            Protocol::Pop3 => "pop3",
        }
    }
}

/// One socket the server should listen on: a protocol, a port, how TLS is
/// handled, and — for SMTP — whether it is the submission role (MSA: auth
/// required) rather than the reception role (MX: no auth, accepts foreign mail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listener {
    pub protocol:   Protocol,
    pub port:       u16,
    pub tls:        TlsMode,
    /// SMTP only. `true` = submission (587/465, authenticated sending);
    /// `false` = reception (25, incoming mail from other servers).
    pub submission: bool,
}

/// What to do with a message whose authentication check failed.
///
/// Ordered by severity, so a caller can take the strictest of two verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PolicyAction {
    /// Deliver as if nothing happened. The `Authentication-Results` header is
    /// still stamped — the evidence is recorded, it just is not acted on.
    Ignore,
    /// Deliver, but flag the message so the interface and the user's own rules
    /// can see it.
    Mark,
    /// Deliver into the Spam folder rather than the inbox.
    Quarantine,
    /// Refuse at SMTP time, before accepting responsibility for the message.
    /// The sending server produces the bounce, so we never emit backscatter to
    /// a forged return path.
    Reject,
}

impl PolicyAction {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ignore" => Some(PolicyAction::Ignore),
            "mark" => Some(PolicyAction::Mark),
            "quarantine" => Some(PolicyAction::Quarantine),
            "reject" => Some(PolicyAction::Reject),
            _ => None,
        }
    }
}

/// Lowest TLS version a handshake may negotiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsFloor {
    Tls12,
    Tls13,
}

impl TlsFloor {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "tls1.2" | "tls12" => Some(TlsFloor::Tls12),
            "tls1.3" | "tls13" => Some(TlsFloor::Tls13),
            _ => None,
        }
    }
}

/// How much TLS a delivery to a remote server demands, in Postfix's vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OutboundTls {
    /// Never negotiate TLS. Only sensible for a destination known to be broken.
    None,
    /// Use TLS when offered, fall back to cleartext otherwise, and accept any
    /// certificate. Confidential against a passive observer only.
    May,
    /// TLS is mandatory: a destination that will not negotiate it is deferred,
    /// never delivered in the clear. The certificate is still not checked.
    Encrypt,
    /// TLS is mandatory AND the certificate chain and hostname are verified
    /// against the system trust store. Defeats an active attacker; will defer
    /// mail to the many MX hosts that still present a mismatched certificate.
    Verify,
}

impl OutboundTls {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" => Some(OutboundTls::None),
            "may" => Some(OutboundTls::May),
            "encrypt" => Some(OutboundTls::Encrypt),
            "verify" => Some(OutboundTls::Verify),
            _ => None,
        }
    }

    /// True when falling back to an unencrypted delivery is forbidden.
    pub fn requires_tls(self) -> bool {
        self >= OutboundTls::Encrypt
    }

    /// True when the peer's certificate must actually be trusted.
    pub fn verifies_certificate(self) -> bool {
        self == OutboundTls::Verify
    }
}

// ── The two sources of a served domain ───────────────────────────────────────

/// One domain as the **instance** declares it, verified or not.
///
/// Carried whole rather than reduced to the verified names, because the
/// addresses panel has to tell an operator *why* a domain is not served:
/// "declared, publish the TXT record" and "never declared here" are different
/// problems with different next steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstanceDomain {
    pub name:     String,
    /// `primary`, `secondary` or `alias`, verbatim from the core.
    pub kind:     String,
    pub verified: bool,
    /// The domain an alias lends its addresses to.
    pub parent:   Option<String>,
}

/// Why a domain is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainSource {
    /// A verified domain of the instance. The normal case.
    Instance,
    /// Only in the stop-gap `server_domains` setting: served because an
    /// operator asked, with no proof behind it.
    Extra,
    /// Both — verified by the instance *and* listed in the stop-gap setting.
    /// Harmless, and worth showing: the setting entry is redundant.
    Both,
}

impl DomainSource {
    pub fn as_str(self) -> &'static str {
        match self {
            DomainSource::Instance => "instance",
            DomainSource::Extra => "extra",
            DomainSource::Both => "both",
        }
    }
}

/// What the listeners need to run. `listeners` is empty when everything is off,
/// which is also the fallback on a failed settings read: an instance that
/// cannot be asked must not start listening on guessed ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    // ── Identity ────────────────────────────────────────────────────────────
    pub hostname: String,
    /// The core's base URL and internal secret, carried here so a local
    /// delivery — reached deep in the call stack with only this config in hand —
    /// can publish a push-notification event to the core. Populated by
    /// [`fetch`] from the module `Settings`; empty on the compiled default,
    /// which the publisher treats as "nowhere to publish to".
    pub core_url:        String,
    pub internal_secret: String,
    /// The domains actually served: the **verified** domains of the instance,
    /// plus the stop-gap list. Derived — never assigned from outside, see
    /// [`ServerConfig::recompute_domains`]. Every delivery path reads this one
    /// and nothing else.
    pub domains:  Vec<String>,
    /// Same names, same order, each with the reason it is served.
    pub domain_sources: Vec<(String, DomainSource)>,
    /// Every domain the instance declares, verified or not. Not all are served;
    /// the panel uses this to say which and why.
    pub instance_domains: Vec<InstanceDomain>,
    /// The `server_domains` setting, normalised. Served unconditionally: it is
    /// the escape hatch for names DNS can never prove.
    pub extra_domains: Vec<String>,
    pub bind:     String,

    // ── Listeners ───────────────────────────────────────────────────────────
    pub listeners: Vec<Listener>,

    // ── Transport encryption ────────────────────────────────────────────────
    /// PEM certificate chain and private key for the TLS listeners. Empty = no
    /// TLS offered (the StartTls/Implicit listeners simply will not start).
    pub tls_cert_path:   String,
    pub tls_key_path:    String,
    pub tls_min_version: TlsFloor,

    // ── Reception limits (MX) ───────────────────────────────────────────────
    pub max_message_bytes:   usize,
    pub max_recipients:      usize,
    /// A message carrying this many `Received:` headers is looping, not
    /// travelling. Postfix's `hopcount_limit`.
    pub hopcount_limit:      usize,
    /// Negative replies tolerated in one session before it is closed.
    pub max_protocol_errors: u32,
    /// Refuse a HELO/EHLO that is not a fully-qualified domain name. Cheap, and
    /// a large share of robots fail it. Never resolve the name — plenty of
    /// legitimate senders announce a name with no DNS record.
    pub require_fqdn_helo:   bool,
    /// Networks whose incoming connections are trusted internal relays, like
    /// Postfix's `mynetworks`. For a connection from one of these the peer IP is
    /// the relay's, not the sender's: SPF is evaluated against the real client
    /// read from the relay's `Received:` header, and the relay is exempt from
    /// greylisting, the envelope block lists and the per-IP connection cap.
    /// Empty (the default) = nobody is trusted, the strict current behaviour.
    pub trusted_upstreams:   Vec<CidrNet>,

    // ── Sessions and anti-abuse ─────────────────────────────────────────────
    /// Simultaneous connections allowed from one client IP, across all
    /// protocols (Dovecot's `mail_max_userip_connections`, default 10).
    pub max_conn_per_ip:      u32,
    /// Delay each further authentication failure from the same IP (Dovecot's
    /// auth-penalty: 0, 2, 4, 8, 15 s). Turning it off makes password guessing
    /// as fast as the network allows.
    pub auth_penalty_enabled: bool,
    /// Master switch for the OpenPGP/GPG feature: users may manage keys and
    /// sign/encrypt/verify/decrypt only when this is on. Off ⇒ the mail settings
    /// "Chiffrement" tab and all PGP routes refuse to operate.
    pub gpg_enabled:          bool,
    /// How long an idle IMAP session outside IDLE is held. RFC 3501 asks for at
    /// least 30 minutes.
    pub imap_idle_minutes:    u64,

    // ── Message authentication of INCOMING mail ─────────────────────────────
    /// The verdicts are computed on every reception; these decide whether the
    /// result changes anything.
    pub spf_fail_action:     PolicyAction,
    pub spf_softfail_action: PolicyAction,
    pub dkim_fail_action:    PolicyAction,
    /// Apply the policy the sending domain publishes in its DMARC record
    /// (`p=`). Off means an authenticated `dmarc=fail` is delivered like any
    /// other message — every domain, ours included, can be impersonated.
    pub dmarc_honor_policy:  bool,
    /// What `p=quarantine` and `p=reject` are worth here. Lets an operator
    /// soften a sender's policy without turning DMARC off entirely.
    pub dmarc_reject_action: PolicyAction,
    pub dmarc_quarantine_action: PolicyAction,

    // ── Content and attachment compliance ───────────────────────────────────
    /// Filename extensions (no dot, lower case) refused as attachments. Read on
    /// the DECLARED name, not on the file's magic bytes.
    pub attachment_blocked_extensions: Vec<String>,
    /// Size ceiling for ONE attachment, in bytes. `0` = no per-attachment
    /// ceiling (the whole-message ceiling still applies).
    pub attachment_max_bytes:          usize,
    /// Refuse a password-protected ZIP: nothing can scan its contents.
    pub attachment_block_encrypted:    bool,
    /// What an attachment rule match costs.
    pub attachment_action:             PolicyAction,
    /// Expressions (lower case) refused in the subject or in either body part.
    pub content_blocked_expressions:   Vec<String>,
    /// What a content rule match costs.
    pub content_action:                PolicyAction,
    /// HTML appended to the body of messages composed on this instance. Empty =
    /// nothing appended.
    pub append_footer_html:            String,

    // ── Delivery restriction ────────────────────────────────────────────────
    /// When non-empty, ONLY these sender domains may deliver mail here. Our own
    /// domains are always allowed — the restriction is about the outside world.
    pub restrict_inbound_domains:  Vec<String>,
    /// When non-empty, mail may ONLY be sent to these recipient domains. Our own
    /// domains are always allowed.
    pub restrict_outbound_domains: Vec<String>,

    // ── Sending limits ──────────────────────────────────────────────────────
    /// Recipients one user may be delivered to over a rolling 24 hours. `0` =
    /// no limit. Counted on the outbound queue, so it bounds what actually
    /// leaves the instance, not what was typed.
    pub send_max_recipients_per_day: i64,

    // ── End-user access ─────────────────────────────────────────────────────
    /// Users may set up automatic forwarding of their incoming mail. Off means
    /// existing rules stop firing AND no new one can be enabled.
    pub allow_auto_forwarding: bool,

    // ── Retention ───────────────────────────────────────────────────────────
    /// Days a message stays in Spam before it is purged. `0` = never purged.
    pub spam_retention_days:  i64,
    /// Days a message stays in Trash before it is purged. `0` = never purged.
    pub trash_retention_days: i64,
    /// Mailbox of this instance that receives a copy of every message the SMTP
    /// services accept or send. Empty = no journalling.
    pub archive_address:      String,

    // ── Anti-spam at the connection level ───────────────────────────────────
    pub greylisting_enabled:     bool,
    pub greylist_delay_secs:     i64,
    pub greylist_window_hours:   i64,
    /// Addresses and `@domain` entries that bypass greylisting and the block
    /// lists. They never bypass DMARC: an allow-listed domain that fails
    /// alignment is exactly the phishing case.
    pub allowlist_senders:       Vec<String>,
    /// Senders (address or `@domain`) whose remote images every account displays
    /// without asking — the instance-wide counterpart of each user's own list.
    pub image_allowlist:         Vec<String>,
    pub blocklist_senders:       Vec<String>,
    pub blocklist_domains:       Vec<String>,
    /// Prefix added to the subject of a message classified as spam. Rewriting
    /// the subject invalidates the message's DKIM signature, so this only
    /// applies when the message is not being relayed on.
    pub spam_subject_prefix:     String,

    // ── Outbound ────────────────────────────────────────────────────────────
    /// Whether the outbound queue worker actually delivers to remote servers.
    /// Off by default: an instance must not start sending to the internet until
    /// an administrator turns it on (and has published SPF/DKIM/DMARC).
    pub outbound_enabled:        bool,
    pub dkim_signing_enabled:    bool,
    /// Refuse to hand a message to the queue when its sending domain has no
    /// DKIM key, instead of sending it unsigned. Unsigned mail is rejected by
    /// the large providers past a few thousand messages a day.
    pub dkim_require_signature:  bool,
    pub outbound_tls:            OutboundTls,
    /// Destination domains for which transport encryption is MANDATORY,
    /// whatever `outbound_tls` says. A subdomain of a listed domain matches too.
    /// Postfix's per-destination TLS policy, reduced to the one decision that
    /// matters: never in the clear to these.
    pub tls_required_domains:    Vec<String>,
    /// How long a message may stay in the queue being retried before it is
    /// bounced. Postfix's `maximal_queue_lifetime`, in hours.
    pub outbound_lifetime_hours: i64,
    /// First retry delay; it then doubles up to `outbound_max_backoff_hours`.
    pub outbound_min_backoff_secs: i64,
    pub outbound_max_backoff_hours: i64,

    // ── Automatic address attribution ───────────────────────────────────────
    /// When true, a verified primary domain triggers a mailbox for every account
    /// that has none, and every new account is served the same way. `mail.
    /// autoprovision_enabled`.
    pub autoprovision: bool,
    /// The rule the local part of an automatic address is built from. Tokens:
    /// `{prenom}`, `{nom}`, `{p}`, `{n}`, `{username}`. `mail.address_format`.
    pub address_format: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            hostname:  default_hostname(),
            core_url:        String::new(),
            internal_secret: String::new(),
            domains:   Vec::new(),
            domain_sources:   Vec::new(),
            instance_domains: Vec::new(),
            extra_domains:    Vec::new(),
            bind:      "127.0.0.1".to_string(),
            listeners: Vec::new(),

            tls_cert_path:   String::new(),
            tls_key_path:    String::new(),
            tls_min_version: TlsFloor::Tls12,

            max_message_bytes:   25 * 1024 * 1024,
            max_recipients:      100,
            hopcount_limit:      50,
            max_protocol_errors: 10,
            require_fqdn_helo:   false,
            trusted_upstreams:   Vec::new(),

            max_conn_per_ip:      10,
            auth_penalty_enabled: true,
            gpg_enabled:          false,
            imap_idle_minutes:    30,

            spf_fail_action:         PolicyAction::Mark,
            spf_softfail_action:     PolicyAction::Mark,
            dkim_fail_action:        PolicyAction::Ignore,
            dmarc_honor_policy:      true,
            dmarc_reject_action:     PolicyAction::Reject,
            dmarc_quarantine_action: PolicyAction::Quarantine,

            attachment_blocked_extensions: Vec::new(),
            attachment_max_bytes:          0,
            attachment_block_encrypted:    false,
            attachment_action:             PolicyAction::Reject,
            content_blocked_expressions:   Vec::new(),
            content_action:                PolicyAction::Quarantine,
            append_footer_html:            String::new(),
            autoprovision:                 true,
            address_format:                "{prenom}.{nom}".to_string(),

            restrict_inbound_domains:  Vec::new(),
            restrict_outbound_domains: Vec::new(),

            send_max_recipients_per_day: 0,

            allow_auto_forwarding: true,

            spam_retention_days:  0,
            trash_retention_days: 0,
            archive_address:      String::new(),

            greylisting_enabled:   false,
            greylist_delay_secs:   300,
            greylist_window_hours: 4,
            allowlist_senders:     Vec::new(),
            image_allowlist:       Vec::new(),
            blocklist_senders:     Vec::new(),
            blocklist_domains:     Vec::new(),
            spam_subject_prefix:   String::new(),

            outbound_enabled:          false,
            dkim_signing_enabled:      true,
            dkim_require_signature:    false,
            outbound_tls:              OutboundTls::May,
            tls_required_domains:      Vec::new(),
            outbound_lifetime_hours:   120,
            outbound_min_backoff_secs: 300,
            outbound_max_backoff_hours: 4,
        }
    }
}

impl ServerConfig {
    /// True when at least one listener should be running.
    pub fn any_enabled(&self) -> bool {
        !self.listeners.is_empty()
    }

    /// Replaces what the instance declares and rebuilds the served set.
    pub fn set_instance_domains(&mut self, declared: Vec<InstanceDomain>) {
        self.instance_domains = declared;
        self.recompute_domains();
    }

    /// Rebuilds `domains` and `domain_sources` from the two sources.
    ///
    /// The instance's verified domains come first and in the order the core
    /// gave them (primary first), so the panel and the diagnostics read in the
    /// order an operator thinks. Deduplicated: a name in both sources is served
    /// once, tagged `Both`.
    fn recompute_domains(&mut self) {
        let mut sources: Vec<(String, DomainSource)> = Vec::new();

        for declared in &self.instance_domains {
            // A claim is not a proof. An unverified domain is declared, listed
            // by the panel, and NOT served.
            if !declared.verified {
                continue;
            }
            let source = if self.extra_domains.contains(&declared.name) {
                DomainSource::Both
            } else {
                DomainSource::Instance
            };
            if !sources.iter().any(|(name, _)| name == &declared.name) {
                sources.push((declared.name.clone(), source));
            }
        }
        for extra in &self.extra_domains {
            if !sources.iter().any(|(name, _)| name == extra) {
                sources.push((extra.clone(), DomainSource::Extra));
            }
        }

        self.domains = sources.iter().map(|(name, _)| name.clone()).collect();
        self.domain_sources = sources;
    }

    /// Why this domain is served, or `None` when it is not.
    pub fn domain_source(&self, domain: &str) -> Option<DomainSource> {
        self.domain_sources
            .iter()
            .find(|(name, _)| name == domain)
            .map(|(_, source)| *source)
    }

    /// What the instance says about this name: `verified`, `pending` (declared,
    /// no proof yet) or `absent` (never declared here).
    pub fn instance_state(&self, domain: &str) -> &'static str {
        match self.instance_domains.iter().find(|d| d.name == domain) {
            Some(d) if d.verified => "verified",
            Some(_) => "pending",
            None => "absent",
        }
    }

    /// True when a certificate is configured, i.e. TLS can be offered.
    pub fn has_tls(&self) -> bool {
        !self.tls_cert_path.trim().is_empty() && !self.tls_key_path.trim().is_empty()
    }

    /// Is this address one of ours — i.e. should its mail be delivered here
    /// rather than relayed?
    pub fn is_local_domain(&self, address: &str) -> bool {
        let domain = match address.rsplit_once('@') {
            Some((_, d)) => d.trim_end_matches('>').to_ascii_lowercase(),
            None => return false,
        };
        self.domains.iter().any(|d| d == &domain)
    }

    /// True when this connection comes from a declared trusted upstream relay.
    /// Empty list = false for everyone, which is the strict default.
    pub fn is_trusted_upstream(&self, ip: IpAddr) -> bool {
        self.trusted_upstreams.iter().any(|net| net.contains(ip))
    }

    /// True when this envelope sender is explicitly trusted by the operator.
    /// Matches a whole address or an `@domain` entry.
    pub fn is_allowlisted(&self, address: &str) -> bool {
        matches_list(&self.allowlist_senders, address)
    }

    /// True when a message from `address` may be accepted at all.
    ///
    /// An empty list (the default) allows everyone. Our own domains always
    /// pass: the restriction is about the outside world, and a rule that cut
    /// internal mail would be a foot-gun with no upside. A null return path
    /// (a bounce) is never restricted either — refusing a delivery report would
    /// hide the failures of our own outgoing mail.
    pub fn inbound_sender_allowed(&self, address: &str) -> bool {
        if self.restrict_inbound_domains.is_empty() || address.trim().is_empty() {
            return true;
        }
        self.is_local_domain(address) || domain_listed(&self.restrict_inbound_domains, address)
    }

    /// True when a message may be sent to `address`. Same rules, other way.
    pub fn outbound_recipient_allowed(&self, address: &str) -> bool {
        if self.restrict_outbound_domains.is_empty() || address.trim().is_empty() {
            return true;
        }
        self.is_local_domain(address) || domain_listed(&self.restrict_outbound_domains, address)
    }

    /// True when delivery to this destination domain must be encrypted, whatever
    /// the general outbound level says. A subdomain of a listed domain matches.
    pub fn tls_required_for(&self, domain: &str) -> bool {
        let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
        if domain.is_empty() {
            return false;
        }
        self.tls_required_domains
            .iter()
            .any(|listed| domain == *listed || domain.ends_with(&format!(".{listed}")))
    }

    /// True when this envelope sender is refused instance-wide, by address or
    /// by domain.
    pub fn is_blocklisted(&self, address: &str) -> bool {
        if matches_list(&self.blocklist_senders, address) {
            return true;
        }
        match address.rsplit_once('@') {
            Some((_, domain)) => {
                let domain = domain.trim_end_matches('>').to_ascii_lowercase();
                self.blocklist_domains.iter().any(|d| d == &domain)
            }
            None => false,
        }
    }
}

/// Matches `address` against a list holding whole addresses and `@domain`
/// entries, case-insensitively.
fn matches_list(list: &[String], address: &str) -> bool {
    let address = address.trim().to_ascii_lowercase();
    if address.is_empty() {
        return false;
    }
    let domain = address.rsplit_once('@').map(|(_, d)| d.to_string());
    list.iter().any(|entry| {
        if let Some(entry_domain) = entry.strip_prefix('@') {
            domain.as_deref() == Some(entry_domain)
        } else {
            entry == &address
        }
    })
}

/// True when `address`'s domain — or one of its parent domains — is in `list`.
/// The entries are already lower-cased and stripped of a leading `@` by the
/// settings reader.
fn domain_listed(list: &[String], address: &str) -> bool {
    let domain = match address.trim().rsplit_once('@') {
        Some((_, d)) => d.trim_end_matches('>').trim_end_matches('.').to_ascii_lowercase(),
        // Not an address: treat the whole string as a domain, which is what a
        // caller holding a bare destination domain passes.
        None => address.trim().trim_end_matches('.').to_ascii_lowercase(),
    };
    if domain.is_empty() {
        return false;
    }
    list.iter()
        .any(|listed| domain == *listed || domain.ends_with(&format!(".{listed}")))
}

fn default_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "kubuno.local".to_string())
}

// ── The instance's domains, and the reason they are remembered ───────────────
//
// The last reading that succeeded, process-wide. A core that is restarting must
// not empty the served set: the module would start refusing mail for its own
// domains, and — worse — silently, since nothing about a shrunken list looks
// like a failure. Kept here rather than in `ServerConfig` so all four callers of
// `fetch` (supervisor, outbound worker, addresses panel, diagnostics) share one
// answer instead of drifting apart.
//
// Empty until the first success, which is exactly right on a cold start: the
// stop-gap list alone serves, and that is what it is for.
fn last_instance_domains() -> &'static RwLock<Vec<InstanceDomain>> {
    static CACHE: OnceLock<RwLock<Vec<InstanceDomain>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(Vec::new()))
}

/// Reads the instance settings from the core. Any failure yields `None` so the
/// caller keeps the configuration it already had rather than tearing listeners
/// down because the core was briefly unreachable.
///
/// Two calls per cycle, never more: the settings, then the domain registry.
/// Neither is per-domain — the registry answers whole.
pub async fn fetch(http: &reqwest::Client, settings: &Settings) -> Option<ServerConfig> {
    let url = format!("{}/internal/modules/mail/settings", settings.core.url);
    let response = http
        .get(&url)
        .header("X-Internal-Secret", settings.core.internal_secret.as_str())
        .send()
        .await
        .map_err(|e| tracing::warn!(error = %e, "Lecture des réglages du serveur de messagerie"))
        .ok()?;

    if !response.status().is_success() {
        tracing::warn!(status = %response.status(), "Réglages du serveur de messagerie refusés par le core");
        return None;
    }

    let body: Value = response
        .json()
        .await
        .map_err(|e| tracing::warn!(error = %e, "Réglages du serveur : réponse illisible"))
        .ok()?;

    let mut config = from_settings(body.get("settings")?);
    // Carry the core coordinates so delivery-time code (which only holds this
    // config) can publish push events back to the core.
    config.core_url = settings.core.url.clone();
    config.internal_secret = settings.core.internal_secret.clone();
    config.set_instance_domains(fetch_instance_domains(http, settings).await);
    Some(config)
}

/// The declared domains of the instance, or — if the core cannot be asked — the
/// last set that was read successfully.
async fn fetch_instance_domains(http: &reqwest::Client, settings: &Settings) -> Vec<InstanceDomain> {
    match read_instance_domains(http, settings).await {
        Some(domains) => {
            match last_instance_domains().write() {
                Ok(mut cache) => *cache = domains.clone(),
                // A poisoned lock means a previous holder panicked. The reading
                // is still good; only the memo is lost.
                Err(e) => tracing::error!(error = %e, "Domaines de l'instance : mémoire du dernier état inutilisable"),
            }
            domains
        }
        None => {
            let remembered = last_instance_domains()
                .read()
                .map(|cache| cache.clone())
                .unwrap_or_default();
            tracing::warn!(
                remembered = remembered.len(),
                "Domaines de l'instance illisibles — conservation du dernier état connu"
            );
            remembered
        }
    }
}

/// One `GET /internal/domains`. `None` on any failure, so the caller can tell a
/// failed read from an instance that genuinely declares nothing.
async fn read_instance_domains(
    http: &reqwest::Client,
    settings: &Settings,
) -> Option<Vec<InstanceDomain>> {
    let url = format!("{}/internal/domains", settings.core.url);
    let response = http
        .get(&url)
        .header("X-Internal-Secret", settings.core.internal_secret.as_str())
        .send()
        .await
        .map_err(|e| tracing::warn!(error = %e, "Lecture des domaines de l'instance"))
        .ok()?;

    if !response.status().is_success() {
        tracing::warn!(status = %response.status(), "Domaines de l'instance refusés par le core");
        return None;
    }

    let body: Value = response
        .json()
        .await
        .map_err(|e| tracing::warn!(error = %e, "Domaines de l'instance : réponse illisible"))
        .ok()?;

    Some(parse_instance_domains(&body))
}

/// Maps the core's payload. A row without a usable name is dropped rather than
/// guessed at: a nameless domain would either serve nothing or, normalised
/// wrongly, serve something else.
pub fn parse_instance_domains(body: &Value) -> Vec<InstanceDomain> {
    let mut out: Vec<InstanceDomain> = Vec::new();
    let Some(rows) = body.get("domains").and_then(Value::as_array) else {
        return out;
    };
    for row in rows {
        let Some(name) = row
            .get("name")
            .and_then(Value::as_str)
            .map(|n| n.trim().trim_start_matches('@').to_ascii_lowercase())
            .filter(|n| !n.is_empty())
        else {
            continue;
        };
        if out.iter().any(|d| d.name == name) {
            continue;
        }
        out.push(InstanceDomain {
            name,
            kind: row
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("secondary")
                .to_string(),
            verified: row.get("verified").and_then(Value::as_bool).unwrap_or(false),
            parent: row
                .get("parent")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|p| !p.is_empty()),
        });
    }
    out
}

/// Maps the raw `{key: value}` object onto the struct. Values arrive as JSON of
/// whatever type the manifest declared, so each read states what it expects.
///
/// Every read falls back to the compiled default rather than to a permissive
/// value: a settings payload that is missing a key (an older core, a failed
/// migration) must not silently disable a protection.
pub fn from_settings(settings: &Value) -> ServerConfig {
    let defaults = ServerConfig::default();

    let str_of = |key: &str| -> Option<String> {
        settings
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let bool_of = |key: &str, fallback: bool| {
        settings.get(key).and_then(Value::as_bool).unwrap_or(fallback)
    };
    let int_of = |key: &str, min: i64, max: i64, fallback: i64| -> i64 {
        settings
            .get(key)
            .and_then(Value::as_i64)
            .filter(|n| (min..=max).contains(n))
            .unwrap_or(fallback)
    };
    // An enum whose stored value is unknown falls back to the default rather
    // than to the most permissive variant.
    let action_of = |key: &str, fallback: PolicyAction| -> PolicyAction {
        str_of(key).and_then(|raw| PolicyAction::parse(&raw)).unwrap_or(fallback)
    };
    let port_of = |key: &str| -> Option<u16> {
        settings
            .get(key)
            .and_then(Value::as_i64)
            .filter(|p| (1..=65535).contains(p))
            .map(|p| p as u16)
    };
    // A multiline list setting: one entry per line, lowercased, blanks dropped.
    // A `#` line is a comment: it can never match an address anyway, and letting
    // administrators label sections keeps a long list readable.
    let list_of = |key: &str| -> Vec<String> {
        str_of(key)
            .map(|raw| {
                raw.lines()
                    .filter(|line| !line.trim_start().starts_with('#'))
                    .flat_map(|line| line.split(','))
                    .map(|entry| entry.trim().to_ascii_lowercase())
                    .filter(|entry| !entry.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };

    // A list of domains: same shape as `list_of`, with the two decorations an
    // operator naturally types stripped (`@example.com`, `example.com.`) so a
    // match never fails on punctuation.
    let domain_list_of = |key: &str| -> Vec<String> {
        list_of(key)
            .into_iter()
            .map(|entry| entry.trim_start_matches('@').trim_end_matches('.').to_string())
            .filter(|entry| !entry.is_empty())
            .collect()
    };
    // A list of filename extensions. `.exe`, `exe` and `*.exe` all mean the
    // same thing to a human; they must mean the same thing here too.
    let extension_list_of = |key: &str| -> Vec<String> {
        list_of(key)
            .into_iter()
            .map(|entry| entry.trim_start_matches('*').trim_start_matches('.').to_string())
            .filter(|entry| !entry.is_empty())
            .collect()
    };
    // Free-text expressions, one per line. Unlike the address lists these are
    // NOT split on commas — a comma is an ordinary character in a phrase — and
    // they are lower-cased once here so the scan compares like with like.
    let expression_list_of = |key: &str| -> Vec<String> {
        str_of(key)
            .map(|raw| {
                raw.lines()
                    .map(|line| line.trim().to_lowercase())
                    .filter(|line| !line.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };

    // Trusted upstream relays: CIDR networks, one per line. An entry that does
    // not parse is dropped with a warning rather than silently — a mistyped
    // network that matched nothing would quietly cancel the exemption it was
    // meant to grant. `list_of` already lowercases, which is harmless for both
    // IPv4 and (hex) IPv6 literals.
    let trusted_upstreams: Vec<CidrNet> = list_of("trusted_upstreams")
        .into_iter()
        .filter_map(|entry| match CidrNet::parse(&entry) {
            Some(net) => Some(net),
            None => {
                tracing::warn!(entry = %entry, "Relais de confiance : entrée CIDR invalide — ignorée");
                None
            }
        })
        .collect();

    let cert = str_of("tls_cert_path").unwrap_or_default();
    let key = str_of("tls_key_path").unwrap_or_default();
    let has_tls = !cert.is_empty() && !key.is_empty();

    // Assemble the listener list from the flat settings. A TLS listener
    // (StartTls advertises STARTTLS; Implicit is TLS-from-first-byte) is only
    // added when a certificate is configured — offering STARTTLS with no
    // certificate would just fail every handshake.
    let mut listeners: Vec<Listener> = Vec::new();
    let mut add = |enabled: bool, port_key: &str, fallback: u16,
                   protocol: Protocol, tls: TlsMode, submission: bool| {
        if !enabled {
            return;
        }
        if tls == TlsMode::Implicit && !has_tls {
            return;
        }
        let effective_tls = if tls == TlsMode::StartTls && !has_tls { TlsMode::None } else { tls };
        listeners.push(Listener {
            protocol,
            port: port_of(port_key).unwrap_or(fallback),
            tls: effective_tls,
            submission,
        });
    };

    // Reception (MX, port 25): no auth, opportunistic STARTTLS.
    add(bool_of("smtp_server_enabled", false), "smtp_server_port", 2525, Protocol::Smtp, TlsMode::StartTls, false);
    // Submission (MSA, port 587): auth required, STARTTLS.
    add(bool_of("submission_enabled", false), "submission_port", 587, Protocol::Smtp, TlsMode::StartTls, true);
    // Submission over implicit TLS (SMTPS, port 465).
    add(bool_of("smtps_enabled", false), "smtps_port", 465, Protocol::Smtp, TlsMode::Implicit, true);
    // IMAP (port 143): STARTTLS.
    add(bool_of("imap_server_enabled", false), "imap_server_port", 1143, Protocol::Imap, TlsMode::StartTls, false);
    // IMAP over implicit TLS (IMAPS, port 993).
    add(bool_of("imaps_enabled", false), "imaps_port", 993, Protocol::Imap, TlsMode::Implicit, false);
    // POP3 (port 110): STLS.
    add(bool_of("pop3_server_enabled", false), "pop3_server_port", 1110, Protocol::Pop3, TlsMode::StartTls, false);
    // POP3 over implicit TLS (POP3S, port 995).
    add(bool_of("pop3s_enabled", false), "pop3s_port", 995, Protocol::Pop3, TlsMode::Implicit, false);

    // The stop-gap list, on its own. The served set is only complete once the
    // instance's verified domains are folded in — `fetch` does that, and
    // `recompute_domains` below leaves `domains` correct in the meantime (a
    // caller that never adds instance domains serves the stop-gap list alone).
    let mut extra_domains: Vec<String> = str_of("server_domains")
        .map(|raw| {
            raw.lines()
                .flat_map(|line| line.split(','))
                .map(|d| d.trim().trim_start_matches('@').to_ascii_lowercase())
                .filter(|d| !d.is_empty())
                .collect()
        })
        .unwrap_or_default();
    extra_domains.dedup();

    let mut config = ServerConfig {
        hostname: str_of("server_hostname").unwrap_or(defaults.hostname),
        // Filled in by `fetch` from the module Settings, not from the core-side
        // settings payload parsed here.
        core_url:        String::new(),
        internal_secret: String::new(),
        domains: Vec::new(),
        domain_sources: Vec::new(),
        instance_domains: Vec::new(),
        extra_domains,
        bind: str_of("server_bind_address").unwrap_or(defaults.bind),
        listeners,

        tls_cert_path:   cert,
        tls_key_path:    key,
        tls_min_version: str_of("tls_min_version")
            .and_then(|raw| TlsFloor::parse(&raw))
            .unwrap_or(defaults.tls_min_version),

        max_message_bytes: settings
            .get("server_max_message_mb")
            .and_then(Value::as_i64)
            .filter(|mb| (1..=2048).contains(mb))
            .map(|mb| mb as usize * 1024 * 1024)
            .unwrap_or(defaults.max_message_bytes),
        max_recipients: int_of("smtp_max_recipients", 1, 10_000, defaults.max_recipients as i64) as usize,
        hopcount_limit: int_of("smtp_hopcount_limit", 5, 200, defaults.hopcount_limit as i64) as usize,
        max_protocol_errors: int_of("smtp_max_errors", 1, 1_000, defaults.max_protocol_errors as i64) as u32,
        require_fqdn_helo: bool_of("smtp_require_fqdn_helo", defaults.require_fqdn_helo),
        trusted_upstreams,

        max_conn_per_ip: int_of("server_max_conn_per_ip", 1, 10_000, defaults.max_conn_per_ip as i64) as u32,
        auth_penalty_enabled: bool_of("auth_penalty_enabled", defaults.auth_penalty_enabled),
        gpg_enabled: bool_of("gpg_enabled", defaults.gpg_enabled),
        imap_idle_minutes: int_of("imap_idle_timeout_min", 1, 1_440, defaults.imap_idle_minutes as i64) as u64,

        spf_fail_action:         action_of("spf_fail_action", defaults.spf_fail_action),
        spf_softfail_action:     action_of("spf_softfail_action", defaults.spf_softfail_action),
        dkim_fail_action:        action_of("dkim_fail_action", defaults.dkim_fail_action),
        dmarc_honor_policy:      bool_of("dmarc_honor_policy", defaults.dmarc_honor_policy),
        dmarc_reject_action:     action_of("dmarc_reject_action", defaults.dmarc_reject_action),
        dmarc_quarantine_action: action_of("dmarc_quarantine_action", defaults.dmarc_quarantine_action),

        attachment_blocked_extensions: extension_list_of("attachment_blocked_extensions"),
        attachment_max_bytes: settings
            .get("attachment_max_mb")
            .and_then(Value::as_i64)
            .filter(|mb| (0..=2048).contains(mb))
            .map(|mb| mb as usize * 1024 * 1024)
            .unwrap_or(defaults.attachment_max_bytes),
        attachment_block_encrypted: bool_of("attachment_block_encrypted", defaults.attachment_block_encrypted),
        attachment_action:          action_of("attachment_action", defaults.attachment_action),
        content_blocked_expressions: expression_list_of("content_blocked_expressions"),
        content_action:              action_of("content_action", defaults.content_action),
        append_footer_html:          str_of("append_footer_html").unwrap_or_default(),
        autoprovision:               bool_of("autoprovision_enabled", defaults.autoprovision),
        address_format:              str_of("address_format").unwrap_or(defaults.address_format),

        restrict_inbound_domains:  domain_list_of("restrict_inbound_domains"),
        restrict_outbound_domains: domain_list_of("restrict_outbound_domains"),

        send_max_recipients_per_day: int_of(
            "send_max_recipients_per_day", 0, 1_000_000, defaults.send_max_recipients_per_day,
        ),

        allow_auto_forwarding: bool_of("allow_auto_forwarding", defaults.allow_auto_forwarding),

        spam_retention_days:  int_of("spam_retention_days", 0, 3_650, defaults.spam_retention_days),
        trash_retention_days: int_of("trash_retention_days", 0, 3_650, defaults.trash_retention_days),
        archive_address: str_of("archive_address")
            .map(|raw| raw.to_ascii_lowercase())
            .unwrap_or_default(),

        greylisting_enabled:   bool_of("greylisting_enabled", defaults.greylisting_enabled),
        greylist_delay_secs:   int_of("greylist_delay_secs", 30, 3_600, defaults.greylist_delay_secs),
        greylist_window_hours: int_of("greylist_window_hours", 1, 168, defaults.greylist_window_hours),
        allowlist_senders:     list_of("allowlist_senders"),
        image_allowlist:       list_of("image_allowlist"),
        blocklist_senders:     list_of("blocklist_senders"),
        blocklist_domains:     list_of("blocklist_domains"),
        spam_subject_prefix:   str_of("spam_subject_prefix").unwrap_or_default(),

        outbound_enabled:         bool_of("outbound_enabled", defaults.outbound_enabled),
        dkim_signing_enabled:     bool_of("dkim_signing_enabled", defaults.dkim_signing_enabled),
        dkim_require_signature:   bool_of("dkim_require_signature", defaults.dkim_require_signature),
        outbound_tls: str_of("outbound_tls_level")
            .and_then(|raw| OutboundTls::parse(&raw))
            .unwrap_or(defaults.outbound_tls),
        tls_required_domains: domain_list_of("tls_required_domains"),
        outbound_lifetime_hours: int_of("outbound_lifetime_hours", 1, 720, defaults.outbound_lifetime_hours),
        outbound_min_backoff_secs: int_of("outbound_min_backoff_secs", 60, 7_200, defaults.outbound_min_backoff_secs),
        outbound_max_backoff_hours: int_of("outbound_max_backoff_hours", 1, 24, defaults.outbound_max_backoff_hours),
    };

    config.recompute_domains();
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn port_of(cfg: &ServerConfig, protocol: Protocol, submission: bool) -> Option<u16> {
        cfg.listeners
            .iter()
            .find(|l| l.protocol == protocol && l.submission == submission)
            .map(|l| l.port)
    }

    #[test]
    fn disabled_services_have_no_listener() {
        let cfg = from_settings(&json!({
            "smtp_server_enabled": false, "smtp_server_port": 2525,
            "imap_server_enabled": true,  "imap_server_port": 1143,
        }));
        assert_eq!(port_of(&cfg, Protocol::Smtp, false), None);
        assert_eq!(port_of(&cfg, Protocol::Imap, false), Some(1143));
        assert!(cfg.any_enabled());
    }

    #[test]
    fn starttls_downgrades_to_plaintext_without_a_certificate() {
        // IMAP asked for, no certificate: the listener still runs, plaintext.
        let cfg = from_settings(&json!({ "imap_server_enabled": true }));
        let imap = cfg.listeners.iter().find(|l| l.protocol == Protocol::Imap).expect("imap");
        assert_eq!(imap.tls, TlsMode::None);
    }

    #[test]
    fn implicit_tls_listeners_need_a_certificate() {
        // IMAPS asked for but no cert → no listener at all (a TLS-only port
        // with no certificate can never complete a handshake).
        let without = from_settings(&json!({ "imaps_enabled": true }));
        assert!(without.listeners.is_empty());

        let with = from_settings(&json!({
            "imaps_enabled": true,
            "tls_cert_path": "/etc/x/cert.pem", "tls_key_path": "/etc/x/key.pem",
        }));
        let imaps = with.listeners.iter().find(|l| l.protocol == Protocol::Imap).expect("imaps");
        assert_eq!(imaps.tls, TlsMode::Implicit);
        assert_eq!(imaps.port, 993);
    }

    #[test]
    fn submission_is_a_distinct_smtp_listener() {
        let cfg = from_settings(&json!({
            "smtp_server_enabled": true, "submission_enabled": true,
        }));
        assert_eq!(port_of(&cfg, Protocol::Smtp, false), Some(2525));
        assert_eq!(port_of(&cfg, Protocol::Smtp, true), Some(587));
    }

    fn declared(name: &str, verified: bool) -> InstanceDomain {
        InstanceDomain {
            name:   name.to_string(),
            kind:   "secondary".to_string(),
            verified,
            parent: None,
        }
    }

    #[test]
    fn domains_are_split_and_normalised() {
        let cfg = from_settings(&json!({ "server_domains": " Example.COM , @kubuno.local " }));
        assert_eq!(cfg.domains, vec!["example.com", "kubuno.local"]);
        assert!(cfg.is_local_domain("Someone@Example.com"));
        assert!(!cfg.is_local_domain("someone@elsewhere.net"));
        assert!(!cfg.is_local_domain("not-an-address"));
    }

    #[test]
    fn domains_may_also_be_given_one_per_line() {
        let cfg = from_settings(&json!({ "server_domains": "example.com\n kubuno.local \n" }));
        assert_eq!(cfg.domains, vec!["example.com", "kubuno.local"]);
    }

    // ── The union, and the four things it has to get right ──────────────────

    /// The instance decides, the stop-gap list adds. Verified domains come
    /// first, in the order the console gave them.
    #[test]
    fn served_domains_are_the_union_of_the_instance_and_the_stop_gap_list() {
        let mut cfg = from_settings(&json!({ "server_domains": "kubuno.local" }));
        cfg.set_instance_domains(vec![declared("toiledev.com", true)]);

        assert_eq!(cfg.domains, vec!["toiledev.com", "kubuno.local"]);
        assert_eq!(cfg.domain_source("toiledev.com"), Some(DomainSource::Instance));
        assert_eq!(cfg.domain_source("kubuno.local"), Some(DomainSource::Extra));
        assert_eq!(cfg.domain_source("elsewhere.net"), None);
        assert!(cfg.is_local_domain("marie@toiledev.com"));
        assert!(cfg.is_local_domain("admin@KUBUNO.LOCAL"));
    }

    /// A name in both sources is served ONCE. A duplicate would make every
    /// "which domains do we serve" answer read wrong, and `is_local_domain`
    /// walk it twice.
    #[test]
    fn a_domain_in_both_sources_is_served_once() {
        let mut cfg = from_settings(&json!({ "server_domains": "toiledev.com\ntoiledev.com" }));
        cfg.set_instance_domains(vec![declared("toiledev.com", true)]);

        assert_eq!(cfg.domains, vec!["toiledev.com"]);
        assert_eq!(cfg.domain_source("toiledev.com"), Some(DomainSource::Both));
    }

    /// A claim is not a proof: a declared domain whose TXT record is not
    /// published is listed, reported `pending`, and NOT served.
    #[test]
    fn a_declared_but_unverified_domain_is_not_served() {
        let mut cfg = from_settings(&json!({}));
        cfg.set_instance_domains(vec![declared("pas-encore.fr", false), declared("prouve.fr", true)]);

        assert_eq!(cfg.domains, vec!["prouve.fr"]);
        assert!(!cfg.is_local_domain("marie@pas-encore.fr"));
        assert_eq!(cfg.instance_state("pas-encore.fr"), "pending");
        assert_eq!(cfg.instance_state("prouve.fr"), "verified");
        assert_eq!(cfg.instance_state("jamais-vu.fr"), "absent");
        // Listed all the same — the panel must be able to show it.
        assert_eq!(cfg.instance_domains.len(), 2);
    }

    /// The escape hatch, and the reason it exists: `core.domains` is empty on a
    /// fresh instance, and without the stop-gap list nothing would be served —
    /// i.e. every incoming message refused.
    #[test]
    fn an_empty_registry_still_serves_the_stop_gap_list() {
        let mut cfg = from_settings(&json!({ "server_domains": "kubuno.local" }));
        cfg.set_instance_domains(Vec::new());
        assert_eq!(cfg.domains, vec!["kubuno.local"]);
        assert!(cfg.is_local_domain("admin@kubuno.local"));
    }

    /// A silent core must never empty the served set. `fetch` keeps the last
    /// reading; this checks the shape that keeps it true — re-applying the
    /// remembered domains reproduces the previous union exactly.
    #[test]
    fn a_silent_core_keeps_the_previously_read_domains() {
        let settings = json!({ "server_domains": "kubuno.local" });
        let mut before = from_settings(&settings);
        before.set_instance_domains(vec![declared("toiledev.com", true)]);

        // Next cycle: the settings still read, the registry did not. The
        // remembered list is applied again rather than an empty one.
        let remembered = before.instance_domains.clone();
        let mut after = from_settings(&settings);
        after.set_instance_domains(remembered);

        assert_eq!(after.domains, before.domains);
        assert!(after.is_local_domain("marie@toiledev.com"));

        // And the failure mode being guarded against: had the module applied an
        // empty registry instead, only the stop-gap list would remain.
        let mut without = from_settings(&settings);
        without.set_instance_domains(Vec::new());
        assert!(!without.is_local_domain("marie@toiledev.com"));
    }

    /// The core's payload, as `/internal/domains` sends it.
    #[test]
    fn the_core_payload_is_read_whole_and_normalised() {
        let parsed = parse_instance_domains(&json!({
            "domains": [
                { "name": "Toiledev.COM", "kind": "primary",   "verified": true,  "verified_at": "2026-08-05T10:00:00Z", "parent": null },
                { "name": "alias.fr",     "kind": "alias",     "verified": true,  "verified_at": null, "parent": "toiledev.com" },
                { "name": "attente.fr",   "kind": "secondary", "verified": false, "verified_at": null, "parent": null },
                // A row with no usable name is dropped, never guessed at.
                { "kind": "secondary", "verified": true },
                { "name": "   ", "kind": "secondary", "verified": true },
            ]
        }));

        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].name, "toiledev.com");
        assert_eq!(parsed[0].kind, "primary");
        assert!(parsed[0].verified);
        assert_eq!(parsed[1].parent.as_deref(), Some("toiledev.com"));
        assert!(!parsed[2].verified);

        // A malformed payload reads as "declares nothing", not as a panic.
        assert!(parse_instance_domains(&json!({})).is_empty());
        assert!(parse_instance_domains(&json!({ "domains": "oops" })).is_empty());
    }

    #[test]
    fn out_of_range_port_falls_back_to_the_default() {
        let cfg = from_settings(&json!({ "pop3_server_enabled": true, "pop3_server_port": 0 }));
        assert_eq!(port_of(&cfg, Protocol::Pop3, false), Some(1110));
    }

    /// A settings payload that predates a knob must not turn its protection
    /// off — every missing key falls back to the compiled default.
    #[test]
    fn missing_keys_keep_the_compiled_defaults() {
        let cfg = from_settings(&json!({}));
        let defaults = ServerConfig::default();
        assert_eq!(cfg.max_recipients, defaults.max_recipients);
        assert_eq!(cfg.hopcount_limit, defaults.hopcount_limit);
        assert!(cfg.auth_penalty_enabled);
        assert!(cfg.dmarc_honor_policy);
        assert_eq!(cfg.dmarc_reject_action, PolicyAction::Reject);
        assert_eq!(cfg.outbound_tls, OutboundTls::May);
    }

    /// An out-of-range or misspelled value is a configuration mistake, and the
    /// safe reading of a mistake is "as shipped", not "as permissive as
    /// possible".
    #[test]
    fn invalid_values_fall_back_rather_than_disable() {
        let cfg = from_settings(&json!({
            "smtp_max_recipients": 0,
            "dmarc_reject_action": "whatever",
            "outbound_tls_level": "dane",
            "tls_min_version": "sslv3",
        }));
        assert_eq!(cfg.max_recipients, 100);
        assert_eq!(cfg.dmarc_reject_action, PolicyAction::Reject);
        assert_eq!(cfg.outbound_tls, OutboundTls::May);
        assert_eq!(cfg.tls_min_version, TlsFloor::Tls12);
    }

    #[test]
    fn policy_actions_are_ordered_by_severity() {
        assert!(PolicyAction::Reject > PolicyAction::Quarantine);
        assert!(PolicyAction::Quarantine > PolicyAction::Mark);
        assert!(PolicyAction::Mark > PolicyAction::Ignore);
    }

    #[test]
    fn outbound_levels_say_what_they_demand() {
        assert!(!OutboundTls::May.requires_tls());
        assert!(OutboundTls::Encrypt.requires_tls());
        assert!(!OutboundTls::Encrypt.verifies_certificate());
        assert!(OutboundTls::Verify.verifies_certificate());
    }

    // ── Trusted upstream relays ─────────────────────────────────────────────

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("ip valide")
    }

    #[test]
    fn cidr_parses_networks_and_bare_hosts() {
        let net = CidrNet::parse("15.100.1.0/24").expect("réseau valide");
        assert!(net.contains(ip("15.100.1.1")));
        assert!(net.contains(ip("15.100.1.254")));
        assert!(!net.contains(ip("15.100.2.1")));

        // A bare address is a host route: only itself.
        let host = CidrNet::parse("192.0.2.7").expect("hôte valide");
        assert!(host.contains(ip("192.0.2.7")));
        assert!(!host.contains(ip("192.0.2.8")));

        // /0 matches everything; a full-length prefix only the exact address.
        assert!(CidrNet::parse("0.0.0.0/0").expect("v4 all").contains(ip("203.0.113.9")));
        assert!(CidrNet::parse("::/0").expect("v6 all").contains(ip("2001:db8::1")));
    }

    #[test]
    fn cidr_handles_ipv6_and_mapped_addresses() {
        let net = CidrNet::parse("2001:db8::/32").expect("réseau v6 valide");
        assert!(net.contains(ip("2001:db8:1:2:3:4:5:6")));
        assert!(!net.contains(ip("2001:db9::1")));

        // An IPv4-mapped IPv6 address is the same client as its IPv4 form, on
        // both sides of the match.
        let v4net = CidrNet::parse("15.100.1.0/24").expect("réseau v4 valide");
        assert!(v4net.contains(ip("::ffff:15.100.1.5")));
    }

    #[test]
    fn cidr_rejects_malformed_entries() {
        assert!(CidrNet::parse("").is_none());
        assert!(CidrNet::parse("not-an-ip").is_none());
        assert!(CidrNet::parse("15.100.1.0/33").is_none()); // v4 prefix too long
        assert!(CidrNet::parse("2001:db8::/129").is_none()); // v6 prefix too long
        assert!(CidrNet::parse("15.100.1.0/abc").is_none());
    }

    #[test]
    fn trusted_upstreams_are_read_from_the_setting() {
        let cfg = from_settings(&json!({
            "trusted_upstreams": "15.100.1.0/24\n oops \n2001:db8::/32",
        }));
        // The two valid entries are kept; the garbage line is dropped.
        assert_eq!(cfg.trusted_upstreams.len(), 2);
        assert!(cfg.is_trusted_upstream(ip("15.100.1.1")));
        assert!(cfg.is_trusted_upstream(ip("2001:db8::abcd")));
        assert!(!cfg.is_trusted_upstream(ip("203.0.113.1")));
    }

    /// The safe default: with no setting, nobody is a trusted upstream and the
    /// strict current behaviour is preserved.
    #[test]
    fn no_setting_means_nobody_is_trusted() {
        let cfg = from_settings(&json!({}));
        assert!(cfg.trusted_upstreams.is_empty());
        assert!(!cfg.is_trusted_upstream(ip("15.100.1.1")));
    }

    /// A stocked list ships with section headers; they must not become entries.
    #[test]
    fn list_comments_are_not_entries() {
        let cfg = from_settings(&json!({
            "image_allowlist": "# Grandes plateformes\n@github.com\n   # commentaire indenté\n@ovhcloud.com\n",
        }));
        assert_eq!(cfg.image_allowlist, vec!["@github.com", "@ovhcloud.com"]);
    }

    #[test]
    fn lists_accept_addresses_and_domains_one_per_line() {
        let cfg = from_settings(&json!({
            "blocklist_senders": "Spammer@Example.COM\n\n  bad@other.net  ",
            "blocklist_domains": "casino.example\n",
            "allowlist_senders": "@partenaire.fr",
        }));
        assert!(cfg.is_blocklisted("spammer@example.com"));
        assert!(cfg.is_blocklisted("anyone@casino.example"));
        assert!(!cfg.is_blocklisted("someone@example.com"));
        assert!(cfg.is_allowlisted("qui-que-ce-soit@partenaire.fr"));
        assert!(!cfg.is_allowlisted("qui-que-ce-soit@ailleurs.fr"));
        // An empty envelope sender (a bounce) matches nothing.
        assert!(!cfg.is_blocklisted(""));
        assert!(!cfg.is_allowlisted(""));
    }

    // ── Delivery restriction, mandatory TLS, compliance lists ───────────────

    /// The default must let everything through: a restriction nobody asked for
    /// would silently cut an instance off from the internet.
    #[test]
    fn an_empty_restriction_allows_everyone() {
        let cfg = from_settings(&json!({}));
        assert!(cfg.inbound_sender_allowed("anyone@elsewhere.net"));
        assert!(cfg.outbound_recipient_allowed("anyone@elsewhere.net"));
    }

    #[test]
    fn a_restriction_allows_the_listed_domains_their_subdomains_and_our_own() {
        let mut cfg = from_settings(&json!({
            "restrict_inbound_domains":  "@Partenaire.FR\nclient.example.",
            "restrict_outbound_domains": "partenaire.fr",
            "server_domains":            "kubuno.local",
        }));
        cfg.set_instance_domains(vec![declared("toiledev.com", true)]);

        assert!(cfg.inbound_sender_allowed("marie@partenaire.fr"));
        assert!(cfg.inbound_sender_allowed("marie@compta.partenaire.fr"));
        assert!(cfg.inbound_sender_allowed("x@client.example"));
        // Our own domains are never cut off by the restriction.
        assert!(cfg.inbound_sender_allowed("admin@toiledev.com"));
        assert!(cfg.inbound_sender_allowed("admin@kubuno.local"));
        // …and a bounce (null return path) is never restricted.
        assert!(cfg.inbound_sender_allowed(""));
        assert!(!cfg.inbound_sender_allowed("spam@ailleurs.net"));
        // A near-miss must not match by suffix alone.
        assert!(!cfg.inbound_sender_allowed("x@fauxpartenaire.fr"));

        assert!(cfg.outbound_recipient_allowed("marie@partenaire.fr"));
        assert!(!cfg.outbound_recipient_allowed("marie@ailleurs.net"));
    }

    #[test]
    fn mandatory_tls_matches_a_domain_and_its_subdomains() {
        let cfg = from_settings(&json!({ "tls_required_domains": "Banque.example\n@sante.fr" }));
        assert!(cfg.tls_required_for("banque.example"));
        assert!(cfg.tls_required_for("mx.banque.example"));
        assert!(cfg.tls_required_for("sante.fr"));
        assert!(!cfg.tls_required_for("autre.example"));
        assert!(!cfg.tls_required_for("fausse-banque.example"));
        assert!(!cfg.tls_required_for(""));
    }

    /// The three decorations an operator types for an extension all mean the
    /// same thing.
    #[test]
    fn blocked_extensions_are_normalised() {
        let cfg = from_settings(&json!({ "attachment_blocked_extensions": ".EXE\n*.scr\nvbs" }));
        assert_eq!(cfg.attachment_blocked_extensions, vec!["exe", "scr", "vbs"]);
    }

    /// A phrase may contain a comma; the expression list must not be split on
    /// one the way the address lists are.
    #[test]
    fn blocked_expressions_keep_their_commas_and_are_lower_cased() {
        let cfg = from_settings(&json!({
            "content_blocked_expressions": "Confidentiel, Défense\n\n  Ne Pas Diffuser  ",
        }));
        assert_eq!(
            cfg.content_blocked_expressions,
            vec!["confidentiel, défense", "ne pas diffuser"]
        );
    }

    #[test]
    fn the_new_protections_default_to_off_and_survive_a_missing_payload() {
        let cfg = from_settings(&json!({}));
        assert!(cfg.attachment_blocked_extensions.is_empty());
        assert_eq!(cfg.attachment_max_bytes, 0);
        assert!(!cfg.attachment_block_encrypted);
        assert_eq!(cfg.send_max_recipients_per_day, 0);
        assert_eq!(cfg.spam_retention_days, 0);
        assert_eq!(cfg.trash_retention_days, 0);
        // …except the two that must NOT silently loosen: forwarding stays as it
        // was, and a matched rule defaults to the strict action.
        assert!(cfg.allow_auto_forwarding);
        assert_eq!(cfg.attachment_action, PolicyAction::Reject);
        assert_eq!(cfg.content_action, PolicyAction::Quarantine);
    }

    #[test]
    fn the_attachment_ceiling_is_read_in_megabytes() {
        let cfg = from_settings(&json!({ "attachment_max_mb": 5 }));
        assert_eq!(cfg.attachment_max_bytes, 5 * 1024 * 1024);
        // Out of range → the compiled default (no ceiling), never a wild value.
        assert_eq!(from_settings(&json!({ "attachment_max_mb": 99_999 })).attachment_max_bytes, 0);
    }
}
