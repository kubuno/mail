//! The DNS records an operator must publish, generated ready to paste.
//!
//! The diagnostic answers "what is wrong"; this answers "what to publish". They
//! share one report because the second needs the first: every generated record
//! carries the verdict of the check that looks for it, so a single view shows
//! both the value to paste AND whether it is already live — the operator never
//! copies a record that is already published, nor trusts a green next to one
//! that is not.
//!
//! Nothing here queries DNS: it is a pure transformation of the configuration,
//! the signing keys and the checks already run, plus one fact resolved once
//! upstream (the public IP). That keeps it unit-testable without a network.

use std::net::Ipv4Addr;

use serde::Serialize;

use super::{Check, DkimTarget, Verdict};

/// The preference every generated MX carries. A single MX needs no ranking, but
/// a value must be published, and 10 is the near-universal convention.
const MX_PREFERENCE: u16 = 10;

/// One record to publish in a DNS zone, prefilled and ready to copy.
#[derive(Debug, Clone, Serialize)]
pub struct DnsRecord {
    /// The served domain this record is grouped under.
    pub domain:    String,
    /// `a` | `mx` | `spf` | `dkim` | `dmarc` — drives the icon and the grouping.
    pub key:       String,
    /// FQDN of the entry, e.g. `kubuno._domainkey.kubuno.com`.
    pub name:      String,
    /// `A` | `MX` | `TXT`.
    pub rtype:     String,
    /// The whole record value, ready to paste as one entry.
    pub value:     String,
    /// DKIM only: the bare public key (the base64 after `p=`), for the registrars
    /// — OVH among them — that take the key alone rather than the full TXT.
    pub value_alt: Option<String>,
    /// MX only: the preference (10).
    pub priority:  Option<u16>,
    /// One sentence, in French: what this record is for.
    pub help:      String,
    /// The verdict of the check that looks for this record, so the operator sees
    /// at a glance what is already published versus missing.
    pub status:    Verdict,
}

/// Builds the full set of records for every served domain.
///
/// `public_ip` is the address `hostname` resolves to (resolved once by the
/// caller); `None` when it does not resolve, in which case the A record shows a
/// placeholder and the SPF falls back to `mx`. `relay_enabled` decides the SPF
/// form: an instance that hands outbound mail to a smarthost sends from the
/// relay's IP, not its own, so asserting `ip4:<own-ip> -all` would fail SPF for
/// every relayed message — `mx ~all` is the honest default there.
pub fn build(
    hostname:      &str,
    domains:       &[String],
    keys:          &[DkimTarget],
    public_ip:     Option<Ipv4Addr>,
    relay_enabled: bool,
    checks:        &[Check],
) -> Vec<DnsRecord> {
    // The A record lives in the zone of the served domain the hostname belongs
    // to (mail.kubuno.com → kubuno.com). Longest match wins, so a hostname under
    // a nested served domain lands in the more specific zone. When no served
    // domain owns it — the hostname sits under a domain served elsewhere — it is
    // attached to the first served domain so it is never dropped from the page.
    let a_owner = owning_domain(hostname, domains).or_else(|| domains.first().cloned());

    let mut records = Vec::new();

    for domain in domains {
        if a_owner.as_deref() == Some(domain.as_str()) {
            records.push(a_record(hostname, domain, public_ip));
        }
        records.push(mx_record(hostname, domain, checks));
        records.push(spf_record(domain, public_ip, relay_enabled, checks));
        for key in keys.iter().filter(|k| k.is_active && &k.domain == domain) {
            records.push(dkim_record(key, checks));
        }
        records.push(dmarc_record(domain, checks));
    }

    records
}

/// The generic warnings shown above the records — the two facts that publishing
/// a zone from this page will not tell the operator on its own.
pub fn advisories() -> Vec<String> {
    vec![
        "Publier ces enregistrements MX déplace le courrier de ces domaines vers cette \
         instance : les boîtes hébergées ailleurs pour ces mêmes domaines cesseront de \
         recevoir. Ne les publiez qu'une fois prêt à héberger tout leur courrier ici."
            .to_string(),
        "Le PTR (résolution inverse) ne se règle pas dans la zone DNS mais chez l'hébergeur \
         de l'adresse IP publique ; il est partagé avec tout autre domaine servi par cette \
         même IP, qui recevront donc le même nom inverse."
            .to_string(),
    ]
}

// ── One record per kind ──────────────────────────────────────────────────────

fn a_record(hostname: &str, domain: &str, public_ip: Option<Ipv4Addr>) -> DnsRecord {
    // No dedicated check exists for the A record: its truth is exactly whether
    // the hostname resolved, which is the fact we already hold.
    let (value, status) = match public_ip {
        Some(ip) => (ip.to_string(), Verdict::Ok),
        None => ("l'adresse IP publique de votre serveur".to_string(), Verdict::Unknown),
    };
    DnsRecord {
        domain:    domain.to_string(),
        key:       "a".to_string(),
        name:      hostname.to_string(),
        rtype:     "A".to_string(),
        value,
        value_alt: None,
        priority:  None,
        help:      format!(
            "Donne une adresse IP au nom que ce serveur annonce ({hostname}) ; \
             tous les MX ci-dessous pointent vers ce nom."
        ),
        status,
    }
}

fn mx_record(hostname: &str, domain: &str, checks: &[Check]) -> DnsRecord {
    DnsRecord {
        domain:    domain.to_string(),
        key:       "mx".to_string(),
        name:      domain.to_string(),
        rtype:     "MX".to_string(),
        // The trailing dot marks an absolute name: without it some registrars
        // append the zone and the MX points at mail.kubuno.com.kubuno.com.
        value:     format!("{hostname}."),
        value_alt: None,
        priority:  Some(MX_PREFERENCE),
        help:      "Indique aux autres serveurs où livrer le courrier de ce domaine."
            .to_string(),
        status:    verdict_of(checks, "mx", domain),
    }
}

fn spf_record(
    domain:        &str,
    public_ip:     Option<Ipv4Addr>,
    relay_enabled: bool,
    checks:        &[Check],
) -> DnsRecord {
    let (value, help) = spf_value(public_ip, relay_enabled);
    DnsRecord {
        domain:    domain.to_string(),
        key:       "spf".to_string(),
        name:      domain.to_string(),
        rtype:     "TXT".to_string(),
        value,
        value_alt: None,
        priority:  None,
        help:      help.to_string(),
        status:    verdict_of(checks, "spf", domain),
    }
}

/// The SPF value and its one-line explanation. Split out so both branches are
/// exercised without a network.
fn spf_value(public_ip: Option<Ipv4Addr>, relay_enabled: bool) -> (String, &'static str) {
    match public_ip {
        // A known IP AND direct-to-MX sending: name that IP and reject the rest.
        Some(ip) if !relay_enabled => (
            format!("v=spf1 ip4:{ip} -all"),
            "Déclare que seule cette IP envoie du courrier pour le domaine ; « -all » \
             rejette tout autre expéditeur.",
        ),
        // Unknown IP, or an outbound relay in front (mail leaves from the relay's
        // IP, not this one): let « mx » authorise whatever the MX designates.
        _ => (
            "v=spf1 mx ~all".to_string(),
            "« mx » autorise les serveurs désignés par vos MX à envoyer pour le domaine ; \
             « ~all » marque le reste comme suspect sans le rejeter d'emblée.",
        ),
    }
}

fn dkim_record(key: &DkimTarget, checks: &[Check]) -> DnsRecord {
    let name = format!("{}._domainkey.{}", key.selector, key.domain);
    let algo = if key.algorithm.starts_with("ed25519") { "ed25519" } else { "rsa" };
    DnsRecord {
        domain:    key.domain.clone(),
        key:       "dkim".to_string(),
        name:      name.clone(),
        rtype:     "TXT".to_string(),
        value:     format!("v=DKIM1; k={algo}; p={}", key.public_key),
        // The bare key, for registrars that store the p= value on its own.
        value_alt: Some(key.public_key.clone()),
        priority:  None,
        help:      "Publie la clé publique qui vérifie la signature DKIM de ce domaine."
            .to_string(),
        status:    verdict_of(checks, "dkim", &name),
    }
}

fn dmarc_record(domain: &str, checks: &[Check]) -> DnsRecord {
    DnsRecord {
        domain:    domain.to_string(),
        key:       "dmarc".to_string(),
        name:      format!("_dmarc.{domain}"),
        rtype:     "TXT".to_string(),
        value:     format!("v=DMARC1; p=none; rua=mailto:postmaster@{domain}"),
        value_alt: None,
        priority:  None,
        help:      "Demande aux destinataires comment traiter un message usurpant ce \
                    domaine ; « p=none » observe sans rejeter et fait remonter des rapports."
            .to_string(),
        status:    verdict_of(checks, "dmarc", domain),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// The served domain the hostname belongs to, longest match first.
fn owning_domain(hostname: &str, domains: &[String]) -> Option<String> {
    domains
        .iter()
        .filter(|d| hostname == d.as_str() || hostname.ends_with(&format!(".{d}")))
        .max_by_key(|d| d.len())
        .cloned()
}

/// The verdict of the check with this `kind` and `scope`, or `Unknown` when no
/// such check ran — a record whose status could not be established shows grey,
/// never a false green or red.
fn verdict_of(checks: &[Check], kind: &str, scope: &str) -> Verdict {
    checks
        .iter()
        .find(|c| c.kind == kind && c.scope == scope)
        .map(|c| c.verdict)
        .unwrap_or(Verdict::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn dkim(domain: &str, selector: &str, active: bool) -> DkimTarget {
        DkimTarget {
            domain:     domain.to_string(),
            selector:   selector.to_string(),
            algorithm:  "rsa-sha256".to_string(),
            public_key: "MIIBIjANBgkqTESTKEY".to_string(),
            is_active:  active,
        }
    }

    fn check(kind: &str, scope: &str, verdict: Verdict) -> Check {
        Check::new(kind, scope, verdict, "")
    }

    fn find<'a>(records: &'a [DnsRecord], key: &str, domain: &str) -> &'a DnsRecord {
        records
            .iter()
            .find(|r| r.key == key && r.domain == domain)
            .expect("record present")
    }

    #[test]
    fn every_kind_is_generated_for_a_served_domain() {
        let keys = [dkim("kubuno.com", "sel1", true)];
        let records = build(
            "mail.kubuno.com",
            &["kubuno.com".to_string()],
            &keys,
            Some(Ipv4Addr::new(203, 0, 113, 7)),
            false,
            &[],
        );

        let a = find(&records, "a", "kubuno.com");
        assert_eq!(a.name, "mail.kubuno.com");
        assert_eq!(a.rtype, "A");
        assert_eq!(a.value, "203.0.113.7");
        assert_eq!(a.status, Verdict::Ok);

        let mx = find(&records, "mx", "kubuno.com");
        assert_eq!(mx.rtype, "MX");
        assert_eq!(mx.name, "kubuno.com");
        assert_eq!(mx.value, "mail.kubuno.com.");
        assert_eq!(mx.priority, Some(10));

        let dkim = find(&records, "dkim", "kubuno.com");
        assert_eq!(dkim.name, "sel1._domainkey.kubuno.com");
        assert_eq!(dkim.value, "v=DKIM1; k=rsa; p=MIIBIjANBgkqTESTKEY");
        // The bare key is offered as well, for OVH-style registrars.
        assert_eq!(dkim.value_alt.as_deref(), Some("MIIBIjANBgkqTESTKEY"));

        let dmarc = find(&records, "dmarc", "kubuno.com");
        assert_eq!(dmarc.name, "_dmarc.kubuno.com");
        assert_eq!(dmarc.value, "v=DMARC1; p=none; rua=mailto:postmaster@kubuno.com");
    }

    #[test]
    fn spf_names_the_ip_when_known_and_no_relay() {
        let (value, _) = spf_value(Some(Ipv4Addr::new(198, 51, 100, 4)), false);
        assert_eq!(value, "v=spf1 ip4:198.51.100.4 -all");
    }

    #[test]
    fn spf_falls_back_to_mx_without_an_ip() {
        let (value, _) = spf_value(None, false);
        assert_eq!(value, "v=spf1 mx ~all");
    }

    #[test]
    fn spf_falls_back_to_mx_when_a_relay_is_in_front() {
        // Even with a known IP, a smarthost sends from ITS address, so the own
        // IP must not be asserted as the sole sender.
        let (value, _) = spf_value(Some(Ipv4Addr::new(198, 51, 100, 4)), true);
        assert_eq!(value, "v=spf1 mx ~all");
    }

    #[test]
    fn status_is_taken_from_the_matching_check() {
        let keys = [dkim("kubuno.com", "sel1", true)];
        let checks = [
            check("mx", "kubuno.com", Verdict::Ok),
            check("spf", "kubuno.com", Verdict::Info),
            check("dkim", "sel1._domainkey.kubuno.com", Verdict::Fail),
            check("dmarc", "kubuno.com", Verdict::Ok),
        ];
        let records = build(
            "mail.kubuno.com",
            &["kubuno.com".to_string()],
            &keys,
            None,
            false,
            &checks,
        );

        assert_eq!(find(&records, "mx", "kubuno.com").status, Verdict::Ok);
        assert_eq!(find(&records, "spf", "kubuno.com").status, Verdict::Info);
        assert_eq!(find(&records, "dkim", "kubuno.com").status, Verdict::Fail);
        assert_eq!(find(&records, "dmarc", "kubuno.com").status, Verdict::Ok);
    }

    #[test]
    fn a_missing_check_leaves_the_record_unknown_not_green() {
        let records = build(
            "mail.kubuno.com",
            &["kubuno.com".to_string()],
            &[],
            None,
            false,
            &[],
        );
        assert_eq!(find(&records, "mx", "kubuno.com").status, Verdict::Unknown);
    }

    #[test]
    fn only_active_keys_produce_a_record() {
        let keys = [
            dkim("kubuno.com", "live", true),
            dkim("kubuno.com", "retired", false),
        ];
        let records = build(
            "mail.kubuno.com",
            &["kubuno.com".to_string()],
            &keys,
            None,
            false,
            &[],
        );
        let dkims: Vec<_> = records.iter().filter(|r| r.key == "dkim").collect();
        assert_eq!(dkims.len(), 1);
        assert_eq!(dkims[0].name, "live._domainkey.kubuno.com");
    }

    #[test]
    fn the_a_record_is_emitted_once_under_its_owning_domain() {
        let domains = ["kubuno.com".to_string(), "toiledev.com".to_string()];
        let records = build("mail.kubuno.com", &domains, &[], None, false, &[]);
        let a: Vec<_> = records.iter().filter(|r| r.key == "a").collect();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].domain, "kubuno.com");
    }
}
