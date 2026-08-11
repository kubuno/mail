//! What a mail administrator opens FIRST: one light per requirement, showing
//! what is expected and what is actually published.
//!
//! ── Why this is a page and not a paragraph in the settings ──────────────────
//! A mail server has some fifty settings and six external facts that decide
//! whether its mail is delivered at all — MX, SPF, DKIM, DMARC, PTR and a valid
//! certificate. None of them lives in this instance: they live in someone's DNS
//! zone and in a file on disk, and every one of them is a silent failure. The
//! settings screen cannot show them, so this does, and the operator only opens
//! the settings when a light here is not green.
//!
//! ── Honesty rules, and why they matter more than the lights ─────────────────
//!   • Where there is no single correct value — SPF's mechanisms, DMARC's
//!     policy — this reports WHAT IS PUBLISHED and does not invent a green.
//!     A tick next to a record nobody checked teaches an operator to trust the
//!     page instead of reading the record.
//!   • A DNS lookup that fails to answer is `Unknown`, never `Fail`. A resolver
//!     that timed out for two seconds must not tell someone their mail server
//!     is broken at 3am.
//!   • Absence, on the other hand, IS an answer: NXDOMAIN/NODATA on `_dmarc`
//!     means no DMARC record, and that is a real failure.

pub mod dns;
pub mod records;
pub mod spf;
pub mod tls;

use serde::Serialize;

pub use records::DnsRecord;

use crate::server::config::ServerConfig;

/// The signing keys the diagnostic checks against DNS. Only the public half is
/// needed here — the private key never leaves the outbound worker.
pub struct DkimTarget {
    pub domain:     String,
    pub selector:   String,
    pub algorithm:  String,
    pub public_key: String,
    pub is_active:  bool,
}

/// The colour of one light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Requirement met, with no room for interpretation.
    Ok,
    /// Met, but weakly — or a deadline approaching (a certificate expiring).
    Warn,
    /// Definitely not met. Mail is being rejected, or will be.
    Fail,
    /// Published, but only the operator can say whether it is right. Never
    /// dressed up as a green.
    Info,
    /// Could not be determined — a resolver that did not answer, a file that
    /// could not be read. Explicitly not a failure.
    Unknown,
}

/// One line of the report.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// Stable identity, e.g. `spf:example.com` — usable as a React key and as
    /// something to link an alert to later.
    pub id:       String,
    /// `mx` | `spf` | `dkim` | `dmarc` | `ptr` | `tls`, for the icon.
    pub kind:     String,
    /// The domain, hostname or selector this line is about.
    pub scope:    String,
    pub verdict:  Verdict,
    /// One sentence, in French, saying what was concluded.
    pub summary:  String,
    /// What a correct configuration looks like — shown as-is so it can be
    /// copied into a zone file.
    pub expected: Option<String>,
    /// What is actually published/found. Empty means nothing was found.
    pub found:    Vec<String>,
}

impl Check {
    fn new(kind: &str, scope: &str, verdict: Verdict, summary: impl Into<String>) -> Self {
        Check {
            id:       format!("{kind}:{scope}"),
            kind:     kind.to_string(),
            scope:    scope.to_string(),
            verdict,
            summary:  summary.into(),
            expected: None,
            found:    Vec::new(),
        }
    }

    fn expected(mut self, value: impl Into<String>) -> Self {
        self.expected = Some(value.into());
        self
    }

    fn found(mut self, values: Vec<String>) -> Self {
        self.found = values;
        self
    }

    /// Prepends one line to what was found — used to name the file a
    /// certificate came from without threading the path through the parser.
    fn found_prefixed(mut self, first: impl Into<String>) -> Self {
        self.found.insert(0, first.into());
        self
    }

    fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
}

/// The whole report.
#[derive(Debug, Serialize)]
pub struct Report {
    /// The name this server announces in HELO and the name the PTR must match.
    pub hostname:   String,
    /// The domains whose mail is delivered here.
    pub domains:    Vec<String>,
    /// True when the administrator has not declared any local domain yet — with
    /// none, most checks have nothing to look at, and saying so beats six
    /// identical "not found" lines.
    pub configured: bool,
    pub checks:     Vec<Check>,
    /// The records to publish, ready to paste, one set per served domain. This
    /// is the "what to publish" half of the page, beside the "what is wrong"
    /// half above; each record carries the verdict of its matching check.
    pub records:    Vec<DnsRecord>,
    /// Generic warnings shown above the records — facts publishing a zone from
    /// this page cannot tell the operator on its own (moving MX, the PTR).
    pub advisories: Vec<String>,
}

/// Runs every check. Each one is independent, so a domain whose DNS is
/// unreachable does not stop the rest of the report.
pub async fn run(cfg: &ServerConfig, keys: &[DkimTarget], relay_enabled: bool) -> Report {
    let mut checks = Vec::new();

    let resolver = dns::resolver();

    // Resolved once, then reused to fill the A record and the SPF `ip4:` — it is
    // the same lookup the PTR check makes, but named here for the records.
    let mut public_ip = None;

    match &resolver {
        Some(system) => {
            // Built once and shared by every domain check: a resolver pointed at
            // public recursive resolvers with our cache off, so MX, SPF, DMARC,
            // DKIM and the announced name's A record all reflect what is
            // published now — not what the machine's resolver cached under the
            // record's TTL. Falls back to the system resolver if it cannot build.
            let fresh = dns::fresh_resolver();
            let r = dns::resolver_for(&fresh, system);

            public_ip = dns::resolve_ipv4(r, &cfg.hostname).await;
            for domain in &cfg.domains {
                checks.push(dns::check_mx(r, domain, &cfg.hostname).await);
                checks.push(spf::check(r, domain).await);
                checks.push(dns::check_dmarc(r, domain).await);
            }
            for key in keys {
                checks.push(dns::check_dkim(r, key).await);
            }
            // PTR stays on the SYSTEM resolver, on purpose: the reverse zone
            // (in-addr.arpa) is delegated by the IP's host — the VPS provider —
            // not the mail domain, so it is looked up the ordinary way. It also
            // changes rarely, so a cached answer is fine here.
            checks.push(dns::check_ptr(system, &cfg.hostname).await);
        }
        None => {
            // No resolver at all: one honest line rather than a wall of reds.
            checks.push(Check::new(
                "ptr",
                &cfg.hostname,
                Verdict::Unknown,
                "Résolveur DNS indisponible : aucune vérification DNS n'a pu être faite.",
            ));
        }
    }

    checks.push(tls::check(cfg));

    let records = records::build(
        &cfg.hostname,
        &cfg.domains,
        keys,
        public_ip,
        relay_enabled,
        &checks,
    );

    Report {
        hostname:   cfg.hostname.clone(),
        domains:    cfg.domains.clone(),
        configured: !cfg.domains.is_empty(),
        checks,
        records,
        advisories: records::advisories(),
    }
}
