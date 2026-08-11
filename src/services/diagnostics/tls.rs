//! The certificate the SMTP/IMAP/POP3 listeners present: is there one, is it
//! still valid, and does it cover the name this server announces?
//!
//! rustls loads a certificate and tells you nothing about it — by design, since
//! a server does not need to know its own expiry to serve. An administrator
//! does: a mail certificate expires silently at 3am and every client that
//! demands TLS stops collecting mail, with no error anywhere in this instance.
//! So the file is parsed here, read-only.
//!
//! Only the certificate is ever read. The private key configured beside it is
//! never opened, never parsed and never mentioned: there is nothing this page
//! could say about it that is worth the risk of holding it in memory.

use std::path::Path;

use chrono::Utc;
use x509_parser::prelude::*;

use crate::server::config::ServerConfig;

use super::{Check, Verdict};

/// Renew-now threshold. Let's Encrypt certificates last 90 days and renew at
/// 30; a light that only turns on at expiry is a light nobody can act on.
const RENEW_SOON_DAYS: i64 = 15;

pub fn check(cfg: &ServerConfig) -> Check {
    let scope = cfg.hostname.as_str();
    let expected = format!(
        "Certificat valide couvrant {scope} (SAN dNSName), avec sa clé privée, lisible par le service"
    );

    if !cfg.has_tls() {
        return Check::new("tls", scope, Verdict::Warn,
            "Aucun certificat configuré : les ports chiffrés (465, 993, 995) ne démarrent pas et \
             STARTTLS n'est pas proposé. Les mots de passe des clients circulent en clair.")
            .expected(expected)
    }

    let path = cfg.tls_cert_path.trim();
    let pem = match std::fs::read(Path::new(path)) {
        Ok(bytes) => bytes,
        Err(e) => {
            // Configured but unreachable: the listeners are down right now.
            tracing::error!(error = %e, "diagnostic : certificat TLS illisible");
            return Check::new("tls", scope, Verdict::Fail,
                format!("Certificat configuré mais illisible ({e}). Les services chiffrés ne \
                         peuvent pas démarrer."))
                .expected(expected).found(vec![path.to_string()])
        }
    };

    inspect(&pem, scope, Utc::now().timestamp())
        .expected(expected)
        .found_prefixed(format!("Fichier : {path}"))
}

/// The whole verdict, given the certificate bytes and a reference instant.
///
/// `now` is a parameter rather than read inside: "expired" and "expires soon"
/// are the two answers that matter most here and the only ones that cannot be
/// tested against a fixed certificate otherwise.
fn inspect(pem: &[u8], hostname: &str, now: i64) -> Check {
    // The leaf is the first CERTIFICATE block; the rest is the chain.
    let leaf = Pem::iter_from_buffer(pem)
        .filter_map(|entry| entry.ok())
        .find(|entry| entry.label == "CERTIFICATE");

    let Some(leaf) = leaf else {
        return Check::new("tls", hostname, Verdict::Fail,
            "Le fichier configuré ne contient aucun certificat au format PEM.")
    };

    let cert = match leaf.parse_x509() {
        Ok(cert) => cert,
        Err(e) => {
            return Check::new("tls", hostname, Verdict::Fail,
                format!("Certificat illisible : {e}"))
        }
    };

    let names = covered_names(&cert);
    let not_after  = cert.validity().not_after.timestamp();
    let not_before = cert.validity().not_before.timestamp();

    let mut found = vec![
        format!("Valide du {} au {}", cert.validity().not_before, cert.validity().not_after),
        format!("Noms couverts : {}",
            if names.is_empty() { "(aucun)".to_string() } else { names.join(", ") }),
    ];

    if now < not_before {
        return Check::new("tls", hostname, Verdict::Fail,
            "Le certificat n'est pas encore valide (sa date de début est dans le futur).")
            .found(found)
    }
    if now >= not_after {
        return Check::new("tls", hostname, Verdict::Fail,
            "Le certificat a EXPIRÉ. Tout client qui exige TLS ne peut plus se connecter.")
            .found(found)
    }

    let covers = names.iter().any(|n| matches_host(n, hostname));
    if !covers {
        found.push(format!("Nom annoncé : {hostname}"));
        return Check::new("tls", hostname, Verdict::Fail,
            format!("Le certificat ne couvre pas le nom annoncé « {hostname} » : les clients qui \
                     vérifient le certificat refusent la connexion."))
            .found(found)
    }

    let days_left = (not_after - now) / 86_400;
    if days_left <= RENEW_SOON_DAYS {
        Check::new("tls", hostname, Verdict::Warn,
            format!("Certificat valide mais il expire dans {days_left} jour(s). \
                     Vérifiez que le renouvellement automatique fonctionne."))
            .found(found)
    } else {
        Check::new("tls", hostname, Verdict::Ok,
            format!("Certificat valide, couvrant {hostname}, encore bon pour {days_left} jours."))
            .found(found)
    }
}

/// The names a certificate is valid for: the SAN dNSName entries, and — only
/// when there is no SAN at all — the legacy Common Name. Every current client
/// ignores the CN when a SAN exists, so mixing them would report a coverage the
/// clients do not honour.
fn covered_names(cert: &X509Certificate<'_>) -> Vec<String> {
    let san: Vec<String> = match cert.subject_alternative_name() {
        Ok(Some(ext)) => ext
            .value
            .general_names
            .iter()
            .filter_map(|name| match name {
                GeneralName::DNSName(dns) => Some((*dns).to_string()),
                _ => None,
            })
            .collect(),
        // A malformed or duplicated SAN extension: report no name rather than a
        // wrong one, and let the coverage check fail loudly.
        Ok(None) | Err(_) => Vec::new(),
    };
    if !san.is_empty() {
        return san;
    }
    cert.subject()
        .iter_common_name()
        .filter_map(|cn| cn.as_str().ok().map(str::to_string))
        .collect()
}

/// RFC 6125 hostname matching: exact, case-insensitive, plus a leading `*.`
/// wildcard that covers exactly one label.
fn matches_host(pattern: &str, host: &str) -> bool {
    let pattern = pattern.trim_end_matches('.');
    let host    = host.trim_end_matches('.');
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // `*.example.com` matches `mail.example.com`, never `example.com` nor
        // `a.b.example.com`.
        return match host.split_once('.') {
            Some((label, rest)) => !label.is_empty() && rest.eq_ignore_ascii_case(suffix),
            None => false,
        };
    }
    pattern.eq_ignore_ascii_case(host)
}

#[cfg(test)]
mod tests {
    use super::{inspect, matches_host};
    use crate::services::diagnostics::Verdict;

    /// Self-signed, `CN=mail.example.com`, SAN `mail.example.com` + `*.example.net`,
    /// valid 2026-08-07 → 2036-08-04. Fixed material so the parsing — dates and
    /// SAN extraction — is exercised for real against a reference instant, not
    /// against the clock.
    const CERT: &[u8] = include_bytes!("testdata/leaf.pem");

    /// 2030-01-01, comfortably inside the certificate's window.
    const INSIDE: i64 = 1_893_456_000;
    /// 2040-01-01, well past its expiry.
    const AFTER: i64 = 2_208_988_800;
    /// 2020-01-01, before it was issued.
    const BEFORE: i64 = 1_577_836_800;

    #[test]
    fn a_valid_certificate_covering_the_announced_name_is_ok() {
        let check = inspect(CERT, "mail.example.com", INSIDE);
        assert_eq!(check.verdict, Verdict::Ok, "{}", check.summary);
        assert!(check.found.iter().any(|l| l.contains("mail.example.com")));
    }

    #[test]
    fn the_wildcard_san_is_honoured() {
        assert_eq!(inspect(CERT, "smtp.example.net", INSIDE).verdict, Verdict::Ok);
    }

    #[test]
    fn a_name_the_certificate_does_not_cover_fails() {
        let check = inspect(CERT, "mail.example.org", INSIDE);
        assert_eq!(check.verdict, Verdict::Fail);
        assert!(check.summary.contains("ne couvre pas"));
    }

    #[test]
    fn expiry_is_read_from_the_certificate_not_guessed() {
        let check = inspect(CERT, "mail.example.com", AFTER);
        assert_eq!(check.verdict, Verdict::Fail);
        assert!(check.summary.contains("EXPIRÉ"));
    }

    #[test]
    fn a_certificate_not_yet_valid_fails_too() {
        assert_eq!(inspect(CERT, "mail.example.com", BEFORE).verdict, Verdict::Fail);
    }

    #[test]
    fn the_last_days_before_expiry_are_a_warning_not_a_failure() {
        // Ten days before notAfter (2036-08-04 15:14:09 UTC = 2 101 475 649).
        let ten_days_before = 2_101_475_649 - 10 * 86_400;
        let check = inspect(CERT, "mail.example.com", ten_days_before);
        assert_eq!(check.verdict, Verdict::Warn, "{}", check.summary);
    }

    #[test]
    fn a_file_that_is_not_a_certificate_fails_without_panicking() {
        assert_eq!(inspect(b"pas du PEM", "mail.example.com", INSIDE).verdict, Verdict::Fail);
    }

    #[test]
    fn exact_match_ignores_case_and_trailing_dot() {
        assert!(matches_host("Mail.Example.COM", "mail.example.com"));
        assert!(matches_host("mail.example.com.", "mail.example.com"));
    }

    #[test]
    fn wildcard_covers_exactly_one_label() {
        assert!(matches_host("*.example.com", "mail.example.com"));
        assert!(!matches_host("*.example.com", "example.com"));
        assert!(!matches_host("*.example.com", "a.mail.example.com"));
    }

    #[test]
    fn unrelated_names_do_not_match() {
        assert!(!matches_host("mail.example.com", "mail.example.net"));
        assert!(!matches_host("*.example.com", "mail.evil.com"));
    }
}
