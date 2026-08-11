//! The DNS half of the diagnostic: MX, DKIM, DMARC and PTR.
//!
//! Every lookup goes through `Answer`, which keeps the one distinction that
//! decides a verdict: "the name does not exist / has no such record" (an
//! answer, and often a failure) versus "the resolver did not reply" (not an
//! answer, and never a failure). Collapsing the two is how a monitoring page
//! ends up waking someone for a two-second SERVFAIL.

use hickory_resolver::config::{NameServerConfigGroup, ResolveHosts, ResolverConfig};
use hickory_resolver::name_server::TokioConnectionProvider;
use hickory_resolver::{Resolver, TokioResolver};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use super::{Check, DkimTarget, Verdict};

/// Builds a resolver from the system configuration. `None` when even that
/// fails — reported once, as `Unknown`, rather than per check.
pub fn resolver() -> Option<TokioResolver> {
    match TokioResolver::builder_tokio() {
        Ok(builder) => Some(builder.build()),
        Err(e) => {
            tracing::error!(error = %e, "diagnostic : résolveur DNS indisponible");
            None
        }
    }
}

// ── Fresh resolver (cache-bypassing) ──────────────────────────────────────────

/// How long a query is given before it is treated as unavailable (an `Unknown`,
/// never a `Fail`).
const FRESH_TIMEOUT: Duration = Duration::from_secs(5);

/// Public recursive resolvers the diagnostic queries instead of the machine's
/// own — Cloudflare, Google and Quad9, several for redundancy so an unreachable
/// one just makes the pool try the next.
///
/// Two reasons not to go to the domain's *own* authoritative servers directly:
/// many hosts (OVH among them) answer `REFUSED` to queries that do not come
/// through a recursive resolver, and a domain's in-zone NS records can be stale
/// or wrong while the parent delegation is correct — a recursive resolver
/// follows the real delegation, an ad-hoc authoritative query does not.
const PUBLIC_RESOLVERS: [IpAddr; 5] = [
    IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), // Cloudflare
    IpAddr::V4(Ipv4Addr::new(1, 0, 0, 1)), // Cloudflare
    IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), // Google
    IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4)), // Google
    IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)), // Quad9
];

/// A resolver aimed at [`PUBLIC_RESOLVERS`] with OUR cache turned off.
///
/// The point of a diagnostic is to show what is published *now*. The system
/// resolver (systemd-resolved, a corporate forwarder…) hands back whatever it
/// cached under the record's TTL — often an hour or more — so an operator who
/// has just repointed their MX keeps seeing the old one long after the change is
/// live. Querying a public recursive resolver, our positive cache disabled, is
/// always reachable and reflects the zone as it currently stands.
///
/// `None` only if the resolver cannot be built at all; the caller then falls
/// back to the system resolver via [`resolver_for`], so a build failure is still
/// an `Unknown`, never a hard `Fail`.
pub fn fresh_resolver() -> Option<TokioResolver> {
    // Both UDP and TCP per IP, so a truncated large record (a DKIM key) retries
    // over TCP. `true`: trust these recursive resolvers' negative answers.
    let group = NameServerConfigGroup::from_ips_clear(&PUBLIC_RESOLVERS, 53, true);
    let config = ResolverConfig::from_parts(None, Vec::new(), group);
    let mut builder = Resolver::builder_with_config(config, TokioConnectionProvider::default());
    {
        let opts = builder.options_mut();
        opts.cache_size = 0; // no positive cache — always the live record
        opts.timeout = FRESH_TIMEOUT;
        opts.use_hosts_file = ResolveHosts::Never; // /etc/hosts must not shadow a zone
    }
    Some(builder.build())
}

/// Picks the fresh resolver when one could be built, otherwise the system one.
/// Generic so the fallback choice is unit-testable without a live resolver.
pub fn resolver_for<'a, R>(fresh: &'a Option<R>, system: &'a R) -> &'a R {
    fresh.as_ref().unwrap_or(system)
}

/// The outcome of one lookup, with absence and unavailability kept apart.
pub enum Answer<T> {
    Records(Vec<T>),
    /// NXDOMAIN or NODATA — the DNS answered, and the answer is "nothing".
    Absent,
    /// SERVFAIL, timeout, refused… The DNS did not answer.
    Unavailable,
}

impl<T> Answer<T> {
    fn from_result<L, E>(result: Result<L, E>, extract: impl FnOnce(L) -> Vec<T>) -> Self
    where
        E: IsNoRecords,
    {
        match result {
            Ok(lookup) => {
                let records = extract(lookup);
                if records.is_empty() { Answer::Absent } else { Answer::Records(records) }
            }
            Err(e) if e.is_no_records() => Answer::Absent,
            Err(_) => Answer::Unavailable,
        }
    }
}

/// Lets `Answer` classify a resolver error without depending on its concrete
/// type in every call site.
pub trait IsNoRecords {
    fn is_no_records(&self) -> bool;
}

impl IsNoRecords for hickory_resolver::ResolveError {
    /// True only for a genuine *absence* — NXDOMAIN (the name does not exist) or
    /// NODATA (it exists with no record of this type).
    ///
    /// hickory folds `REFUSED` and `SERVFAIL` into the same `NoRecordsFound`
    /// variant as a real absence, so `is_no_records_found()` on its own would
    /// read a server that refused or failed to answer as "no such record" — and
    /// a refusing name server (OVH answers `REFUSED` to direct queries) would
    /// then show up as an empty, failing check instead of an honest `Unknown`.
    /// Only NXDOMAIN and NODATA are absence; every other response code means the
    /// query was not usefully answered → `Answer::Unavailable` → `Verdict::Unknown`.
    fn is_no_records(&self) -> bool {
        use hickory_resolver::proto::op::ResponseCode;
        use hickory_resolver::proto::ProtoErrorKind;
        use hickory_resolver::ResolveErrorKind;

        match self.kind() {
            ResolveErrorKind::Proto(proto) => match proto.kind() {
                ProtoErrorKind::NoRecordsFound { response_code, .. } => {
                    matches!(response_code, ResponseCode::NXDomain | ResponseCode::NoError)
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// TXT records of `name`, each record's character-strings concatenated — a long
/// SPF or DKIM record is split into 255-byte chunks on the wire and only means
/// anything joined back together.
pub async fn txt(resolver: &TokioResolver, name: &str) -> Answer<String> {
    Answer::from_result(resolver.txt_lookup(name).await, |lookup| {
        lookup
            .iter()
            .map(|rec| {
                rec.txt_data()
                    .iter()
                    .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
                    .collect::<String>()
            })
            .collect()
    })
}

/// A/AAAA addresses of `name`.
async fn addresses(resolver: &TokioResolver, name: &str) -> Answer<IpAddr> {
    Answer::from_result(resolver.lookup_ip(name).await, |lookup| lookup.iter().collect())
}

/// The first IPv4 address `name` resolves to — used to fill the A record and the
/// SPF `ip4:` mechanism with a concrete address. `None` when the name has no A
/// record or the resolver did not answer: the generated records then fall back
/// to a plain-text placeholder rather than inventing an address.
pub async fn resolve_ipv4(resolver: &TokioResolver, name: &str) -> Option<Ipv4Addr> {
    match addresses(resolver, name).await {
        Answer::Records(ips) => ips.into_iter().find_map(|ip| match ip {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        }),
        Answer::Absent | Answer::Unavailable => None,
    }
}

// ── MX ──────────────────────────────────────────────────────────────────────

/// Does mail addressed to this domain reach this instance?
///
/// A domain with no MX still receives mail (RFC 5321 falls back to the implicit
/// MX, the domain's own A record), so the absence is a warning, not a failure:
/// it works, but nothing says the mail is meant to come here.
pub async fn check_mx(resolver: &TokioResolver, domain: &str, hostname: &str) -> Check {
    let mx = match resolver.mx_lookup(domain).await {
        Ok(lookup) => {
            let mut records: Vec<(u16, String)> = lookup
                .iter()
                .filter(|rec| !rec.exchange().is_root()) // "." = null MX (RFC 7505)
                .map(|rec| {
                    (rec.preference(), rec.exchange().to_utf8().trim_end_matches('.').to_string())
                })
                .collect();
            records.sort_by_key(|(pref, _)| *pref);
            Answer::Records(records)
        }
        // Same classification as every other lookup: a refusing/failing server
        // is `Unavailable` (→ Unknown), only NXDOMAIN/NODATA is a real absence.
        Err(e) if e.is_no_records() => Answer::Absent,
        Err(_) => Answer::Unavailable,
    };

    let expected = format!("{domain}.  IN MX  10 {hostname}.");

    match mx {
        Answer::Unavailable => Check::new("mx", domain, Verdict::Unknown,
            "Impossible d'interroger le DNS pour cet enregistrement.")
            .expected(expected),
        Answer::Absent => Check::new("mx", domain, Verdict::Warn,
            "Aucun enregistrement MX. Le courrier arrive quand même via l'adresse A du domaine \
             (MX implicite), mais rien n'indique aux expéditeurs où le livrer.")
            .expected(expected),
        Answer::Records(records) => {
            let found: Vec<String> = records
                .iter()
                .map(|(pref, host)| format!("{pref} {host}"))
                .collect();
            let points_here = records
                .iter()
                .any(|(_, host)| host.eq_ignore_ascii_case(hostname));
            if points_here {
                Check::new("mx", domain, Verdict::Ok,
                    format!("Le courrier de ce domaine est dirigé vers {hostname}."))
                    .expected(expected).found(found)
            } else {
                // Legitimate (a relay in front), so not a failure — but it is
                // the first thing to look at when mail never arrives.
                Check::new("mx", domain, Verdict::Info,
                    format!("Les MX publiés ne citent pas {hostname} : le courrier de ce domaine \
                             est livré ailleurs, ou passe par un relais."))
                    .expected(expected).found(found)
            }
        }
    }
}

// ── DKIM ────────────────────────────────────────────────────────────────────

/// Does the published record still match the key we sign with?
///
/// The comparison is on the `p=` tag alone: everything else in the record
/// (`v=`, `k=`, `t=`, whitespace, the order of the tags) varies between DNS
/// providers without changing what verifiers do.
pub async fn check_dkim(resolver: &TokioResolver, key: &DkimTarget) -> Check {
    let name = format!("{}._domainkey.{}", key.selector, key.domain);
    let algo = if key.algorithm.starts_with("ed25519") { "ed25519" } else { "rsa" };
    let expected = format!("{name}.  IN TXT  \"v=DKIM1; k={algo}; p={}\"", key.public_key);
    let id = format!("dkim:{name}");

    let records = match txt(resolver, &name).await {
        Answer::Records(r) => r,
        Answer::Unavailable => {
            return Check::new("dkim", &name, Verdict::Unknown,
                "Impossible d'interroger le DNS pour cet enregistrement.")
                .expected(expected).with_id(id)
        }
        Answer::Absent => {
            // A key that is not the signing key is *supposed* to be unpublished
            // at first — that is the first step of a rotation, not a fault.
            let (verdict, msg) = if key.is_active {
                (Verdict::Fail, "Aucun enregistrement publié pour la clé qui signe ce domaine : \
                                 les signatures sont invérifiables et le courrier échoue DMARC.")
            } else {
                (Verdict::Info, "Enregistrement pas encore publié. Publiez-le, laissez-le se propager, \
                                 puis activez la clé.")
            };
            return Check::new("dkim", &name, verdict, msg).expected(expected).with_id(id)
        }
    };

    let published: Option<String> = records
        .iter()
        .filter(|r| r.contains("p="))
        .find_map(|r| tag(r, "p"));

    match published {
        Some(p) if p == key.public_key => {
            let msg = if key.is_active {
                "L'enregistrement publié correspond à la clé qui signe ce domaine."
            } else {
                "L'enregistrement publié correspond à cette clé : elle peut être activée."
            };
            Check::new("dkim", &name, Verdict::Ok, msg)
                .expected(expected).found(records).with_id(id)
        }
        Some(_) => Check::new("dkim", &name, Verdict::Fail,
            "Un enregistrement est publié sous ce sélecteur, mais sa clé publique n'est PAS celle \
             stockée ici : les signatures émises ne se vérifient pas.")
            .expected(expected).found(records).with_id(id),
        None => Check::new("dkim", &name, Verdict::Fail,
            "Un enregistrement TXT existe sous ce nom mais ne contient pas de clé publique (p=).")
            .expected(expected).found(records).with_id(id),
    }
}

// ── DMARC ───────────────────────────────────────────────────────────────────

/// Is a DMARC record published, and what does it ask for?
///
/// `p=none` is the floor Gmail, Yahoo and Microsoft impose on bulk senders: it
/// asks nothing of receivers, so it is reported as published fact rather than
/// as a green — there is no "correct" policy this page could assert.
pub async fn check_dmarc(resolver: &TokioResolver, domain: &str) -> Check {
    let name = format!("_dmarc.{domain}");
    let expected = format!("{name}.  IN TXT  \"v=DMARC1; p=none; rua=mailto:dmarc@{domain}\"");

    let records = match txt(resolver, &name).await {
        Answer::Records(r) => r,
        Answer::Unavailable => {
            return Check::new("dmarc", domain, Verdict::Unknown,
                "Impossible d'interroger le DNS pour cet enregistrement.")
                .expected(expected)
        }
        Answer::Absent => {
            return Check::new("dmarc", domain, Verdict::Fail,
                "Aucun enregistrement DMARC. Gmail, Yahoo et Microsoft exigent au minimum \
                 « p=none » de tout expéditeur en volume.")
                .expected(expected)
        }
    };

    let dmarc: Vec<String> = records
        .into_iter()
        .filter(|r| r.trim_start().to_ascii_lowercase().starts_with("v=dmarc1"))
        .collect();

    let Some(record) = dmarc.first().cloned() else {
        return Check::new("dmarc", domain, Verdict::Fail,
            "Des enregistrements TXT existent sous _dmarc mais aucun ne commence par « v=DMARC1 ».")
            .expected(expected)
    };

    if dmarc.len() > 1 {
        return Check::new("dmarc", domain, Verdict::Fail,
            "Plusieurs enregistrements DMARC : la politique est ignorée par les vérificateurs.")
            .expected(expected).found(dmarc)
    }

    match tag(&record, "p").as_deref() {
        Some("reject") => Check::new("dmarc", domain, Verdict::Ok,
            "Politique « reject » : un message usurpant ce domaine est refusé.")
            .expected(expected).found(dmarc),
        Some("quarantine") => Check::new("dmarc", domain, Verdict::Ok,
            "Politique « quarantine » : un message usurpant ce domaine part en indésirable.")
            .expected(expected).found(dmarc),
        Some("none") => Check::new("dmarc", domain, Verdict::Info,
            "Politique « none » : le plancher exigé par Gmail, Yahoo et Microsoft est atteint, \
             mais aucun message usurpant ce domaine n'est écarté. À durcir une fois les rapports lus.")
            .expected(expected).found(dmarc),
        _ => Check::new("dmarc", domain, Verdict::Fail,
            "L'enregistrement DMARC ne déclare pas de politique (p=) : il est sans effet.")
            .expected(expected).found(dmarc),
    }
}

// ── PTR ─────────────────────────────────────────────────────────────────────

/// Full-circle reverse DNS on the announced hostname.
///
/// Gmail and Yahoo require this of EVERY sender, not just bulk ones: the IP
/// must have a PTR, and the name it gives must resolve back to that same IP.
/// The IP checked is the one the announced hostname resolves to — that is the
/// identity this server presents, and the only one it can know from the inside.
pub async fn check_ptr(resolver: &TokioResolver, hostname: &str) -> Check {
    let expected = format!(
        "PTR de l'IP publique → {hostname}, et {hostname} → cette même IP (résolution circulaire)"
    );

    let ips = match addresses(resolver, hostname).await {
        Answer::Records(ips) => ips,
        Answer::Unavailable => {
            return Check::new("ptr", hostname, Verdict::Unknown,
                "Impossible de résoudre le nom annoncé.")
                .expected(expected)
        }
        Answer::Absent => {
            return Check::new("ptr", hostname, Verdict::Fail,
                format!("Le nom annoncé « {hostname} » n'a aucune adresse IP : il ne peut pas avoir \
                         de résolution inverse valide."))
                .expected(expected)
        }
    };

    let mut found = Vec::new();
    let mut matched = false;
    let mut unavailable = false;

    for ip in &ips {
        let names = match Answer::from_result(resolver.reverse_lookup(*ip).await, |lookup| {
            lookup
                .iter()
                .map(|ptr| ptr.to_utf8().trim_end_matches('.').to_string())
                .collect::<Vec<_>>()
        }) {
            Answer::Records(names) => names,
            Answer::Unavailable => {
                unavailable = true;
                found.push(format!("{ip} → (résolveur muet)"));
                continue;
            }
            Answer::Absent => {
                found.push(format!("{ip} → aucun PTR"));
                continue;
            }
        };

        for name in &names {
            // Full-circle: the name the PTR gives must resolve back to this IP.
            let circles = match addresses(resolver, name).await {
                Answer::Records(back) => back.contains(ip),
                Answer::Unavailable => { unavailable = true; false }
                Answer::Absent => false,
            };
            found.push(format!("{ip} → {name}{}", if circles { " ✓" } else { "" }));
            if circles {
                matched = true;
            }
        }
    }

    if matched {
        Check::new("ptr", hostname, Verdict::Ok,
            "La résolution inverse est complète : l'IP donne un nom qui redonne la même IP.")
            .expected(expected).found(found)
    } else if unavailable {
        Check::new("ptr", hostname, Verdict::Unknown,
            "Vérification incomplète : le résolveur n'a pas répondu à toutes les requêtes.")
            .expected(expected).found(found)
    } else {
        Check::new("ptr", hostname, Verdict::Fail,
            "Pas de résolution inverse circulaire. Gmail et Yahoo l'exigent de tout expéditeur ; \
             elle se demande à l'hébergeur de l'adresse IP, pas dans votre zone DNS.")
            .expected(expected).found(found)
    }
}

// ── Shared parsing ──────────────────────────────────────────────────────────

/// Value of tag `key` in a `;`-separated DNS record (`v=DKIM1; p=MIIB…`).
/// Tag names are case-insensitive, values are not.
pub fn tag(record: &str, key: &str) -> Option<String> {
    record.split(';').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        name.trim().eq_ignore_ascii_case(key).then(|| value.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_is_found_whatever_the_spacing_and_case() {
        let record = "v=DKIM1; K=rsa;  p=MIIBIjANBg ";
        assert_eq!(tag(record, "p").as_deref(), Some("MIIBIjANBg"));
        assert_eq!(tag(record, "k").as_deref(), Some("rsa"));
        assert_eq!(tag(record, "t"), None);
    }

    #[test]
    fn a_base64_value_keeps_its_padding() {
        // '=' inside the value must not be mistaken for another separator.
        assert_eq!(tag("v=DKIM1; p=AAAB==", "p").as_deref(), Some("AAAB=="));
    }

    #[test]
    fn falls_back_to_the_system_resolver_when_no_fresh_one() {
        // The fallback the caller relies on: when no fresh resolver could be
        // built, the system one is used — so the report still runs rather than
        // failing hard. Checked on a stand-in type, the choice being
        // resolver-agnostic.
        let system = 1u8;
        assert_eq!(*resolver_for(&None, &system), 1);
        assert_eq!(*resolver_for(&Some(2u8), &system), 2);
    }

    #[tokio::test]
    async fn a_fresh_resolver_is_built_and_answers_from_public_recursors() {
        // Building must succeed, and a query must resolve against the public
        // recursors with our cache off — the whole mechanism the diagnostic
        // relies on to bypass the machine's stale cache. Network-dependent, so it
        // asserts only that the resolver is usable, not any specific record.
        let Some(resolver) = fresh_resolver() else {
            panic!("fresh resolver should always build");
        };
        // one.one.one.one is an A record Cloudflare itself always publishes.
        if let Answer::Records(ips) = addresses(&resolver, "one.one.one.one").await {
            assert!(ips.iter().any(|ip| ip.is_ipv4()));
        }
        // An absent record must read as Absent (NODATA), not Unavailable.
        assert!(matches!(
            addresses(&resolver, "nonexistent.invalid").await,
            Answer::Absent | Answer::Unavailable
        ));
    }
}
