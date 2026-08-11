//! Inbound message authentication (SPF, DKIM, DMARC) and the
//! `Authentication-Results:` header, via the `mail-auth` crate.
//!
//! Filled by the B5-verify step. What this decides feeds the spam pipeline and
//! is recorded so a client can see why a message is trusted or not.

use std::net::IpAddr;

use mail_auth::{
    AuthenticatedMessage, AuthenticationResults, DkimOutput, DkimResult, DmarcResult,
    MessageAuthenticator, SpfOutput, SpfResult, dmarc::verify::DmarcParameters,
    spf::verify::SpfParameters,
};

use crate::server::config::CidrNet;

/// The verdicts for one received message.
#[derive(Debug, Clone, Default)]
pub struct AuthVerdict {
    pub spf:   Option<String>,   // pass | fail | softfail | neutral | none | temperror | permerror
    pub dkim:  Option<String>,   // pass | fail | none | ...
    pub dmarc: Option<String>,   // pass | fail | none
    /// The domain the DKIM signature validated for (`d=`), if any — mirrors the
    /// `signed_by` column the module already stores.
    pub dkim_domain: Option<String>,
    /// The full `Authentication-Results:` header value (without the field name),
    /// ready to prepend, or empty when nothing could be checked.
    pub header: String,
}

/// Verifies SPF/DKIM/DMARC for a received message.
///
/// Builds a `mail_auth::MessageAuthenticator` over the system DNS resolver,
/// runs SPF against `peer_ip`/`helo`/`envelope_from`, verifies DKIM over `raw`,
/// evaluates DMARC alignment against the `From:` header, and formats an
/// `Authentication-Results` value stamped with `hostname`.
///
/// ⚠️ The CALLER must strip any pre-existing `Authentication-Results` header from
/// the message before prepending the value produced here — otherwise an attacker
/// could forge `dmarc=pass`. This function only PRODUCES the value; it never
/// touches the message.
///
/// `trusted_upstreams` are the networks the operator declared as internal relays.
/// When `peer_ip` belongs to one, the connection is a trusted forwarder and its
/// IP is NOT the sender's: SPF is instead evaluated against the real client read
/// from the message's `Received:` headers. If no real client IP can be extracted
/// SPF is neutralised to `none` — the relay must never be the cause of an SPF
/// `fail`. DKIM and DMARC are unchanged: DKIM does not depend on the IP, and
/// DMARC aligns on the recomputed SPF result plus DKIM.
pub async fn verify(
    peer_ip: IpAddr,
    trusted_upstreams: &[CidrNet],
    helo: &str,
    envelope_from: &str,
    raw: &[u8],
    hostname: &str,
) -> AuthVerdict {
    // Build the authenticator over the host's resolv.conf. A transient failure
    // here means we simply cannot authenticate this message — return an empty
    // verdict rather than panicking.
    let authenticator = match MessageAuthenticator::new_system_conf() {
        Ok(a) => a,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "impossible de construire le resolveur DNS pour l'authentification du message"
            );
            return AuthVerdict::default();
        }
    };

    // Normalize the envelope sender: an empty or null (`<>`) reverse-path means
    // we fall back to the HELO identity for SPF and DMARC.
    let mail_from = normalize_mail_from(envelope_from);
    let mail_from_domain = domain_of(mail_from).unwrap_or(helo);

    // Which IP SPF is actually evaluated against. Normally the peer's; but a
    // connection from a trusted upstream relay carries the relay's IP, so we
    // read the real client from the `Received:` header the relay stamped.
    // `None` = trusted relay but no usable client IP — SPF becomes `none`.
    let spf_ip: Option<IpAddr> = if trusted_upstreams.iter().any(|net| net.contains(peer_ip)) {
        let real = client_ip_from_received(raw, trusted_upstreams);
        if real.is_none() {
            tracing::warn!(
                relay = %peer_ip,
                "Réception via relais de confiance : IP client réelle introuvable dans les en-têtes Received — SPF neutralisé"
            );
        }
        real
    } else {
        Some(peer_ip)
    };

    // --- SPF (RFC 7208) -----------------------------------------------------
    // mail-auth bounds the 10-lookup limit internally; we never bypass it.
    let spf_output: SpfOutput = match spf_ip {
        Some(ip) => {
            let spf_params = if mail_from.is_empty() {
                SpfParameters::verify_ehlo(ip, helo, hostname)
            } else {
                SpfParameters::verify(ip, helo, hostname, mail_from)
            };
            authenticator.verify_spf(spf_params).await
        }
        // A trusted relay whose real client we could not recover: SPF is
        // neutral, never a fail attributable to the relay.
        None => SpfOutput::new(mail_from_domain.to_string()).with_result(SpfResult::None),
    };
    let spf_str = spf_result_str(spf_output.result());

    let mut verdict = AuthVerdict {
        spf: Some(spf_str.to_string()),
        ..Default::default()
    };

    // The IP stamped into the header: the one SPF actually judged, falling back
    // to the peer when there was none to judge.
    let header_ip = spf_ip.unwrap_or(peer_ip);

    // Start assembling the Authentication-Results header value.
    let mut auth_results = AuthenticationResults::new(hostname);
    auth_results = if mail_from.is_empty() {
        auth_results.with_spf_ehlo_result(&spf_output, header_ip, helo)
    } else {
        auth_results.with_spf_mailfrom_result(&spf_output, header_ip, mail_from, helo)
    };

    // --- DKIM (RFC 6376) & DMARC (RFC 7489) --------------------------------
    // Both need the parsed message. If it cannot be parsed we keep only the SPF
    // verdict and leave dkim/dmarc unevaluated.
    if let Some(message) = AuthenticatedMessage::parse(raw) {
        let dkim_outputs: Vec<DkimOutput> = authenticator.verify_dkim(&message).await;

        verdict.dkim = Some(dkim_summary(&dkim_outputs).to_string());
        // `d=` of the first passing signature, mirrored into the DB `signed_by`.
        verdict.dkim_domain = dkim_outputs
            .iter()
            .find(|o| matches!(o.result(), DkimResult::Pass))
            .and_then(|o| o.signature())
            .map(|sig| sig.d.clone());

        // The RFC5322.From domain used for DMARC alignment reporting.
        let header_from = domain_of(message.from()).unwrap_or("").to_string();

        let dmarc_output = authenticator
            .verify_dmarc(DmarcParameters::new(
                &message,
                &dkim_outputs,
                mail_from_domain,
                &spf_output,
            ))
            .await;
        verdict.dmarc = Some(
            dmarc_summary(dmarc_output.spf_result(), dmarc_output.dkim_result()).to_string(),
        );

        auth_results = auth_results
            .with_dkim_results(&dkim_outputs, &header_from)
            .with_dmarc_result(&dmarc_output);
    }

    // `Display` renders `<hostname>; <results>` — exactly the header value we
    // want, without the `Authentication-Results:` field name.
    verdict.header = auth_results.to_string();
    verdict
}

/// Strips an optional angle-bracket reverse-path and treats the null sender
/// (`<>`) as empty, so SPF/DMARC fall back to the HELO identity.
fn normalize_mail_from(envelope_from: &str) -> &str {
    let trimmed = envelope_from.trim();
    let unwrapped = trimmed
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(trimmed);
    unwrapped.trim()
}

/// Extracts the domain part of an `local@domain` address, if present.
fn domain_of(address: &str) -> Option<&str> {
    address.rsplit_once('@').map(|(_, domain)| domain)
}

// ── Real client IP behind a trusted relay ─────────────────────────────────────

/// The real client's IP, read from the message's `Received:` chain, when the
/// connection itself came from a trusted upstream relay.
///
/// Each `Received:` header records the hop that added it: `from <host> ([ip]) by
/// …` means "this server received the message from `[ip]`". The headers are
/// prepended, so the newest is on top. We walk them top-down and return the
/// FIRST recorded source IP that is neither one of our own trusted relays (a hop
/// inside the trusted chain) nor a private/reserved address — that is the real
/// external client. `None` when the chain yields nothing usable, in which case
/// the caller neutralises SPF rather than blaming the relay.
///
/// Pure: no DNS, no I/O. The message is the only input.
pub fn client_ip_from_received(raw: &[u8], trusted: &[CidrNet]) -> Option<IpAddr> {
    for header in received_headers(raw) {
        let Some(ip) = extract_received_source_ip(&header) else {
            continue;
        };
        // A hop from one of our own relays: the real client is further down.
        if trusted.iter().any(|net| net.contains(ip)) {
            continue;
        }
        // A private/reserved address is not a routable external client; skip it
        // and keep looking rather than trust a NATed internal hop.
        if !is_public_ip(ip) {
            continue;
        }
        return Some(ip);
    }
    None
}

/// Unfolds the header block of `raw` and returns the value of every `Received:`
/// header, in file order (newest first). Continuation lines (starting with
/// whitespace) are joined onto the header they belong to. Stops at the blank
/// line that ends the header section.
fn received_headers(raw: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(raw);
    let mut headers: Vec<String> = Vec::new();
    let mut current: Option<String> = None;

    // A completed header: if it is a `Received:`, keep its value.
    let flush = |current: &mut Option<String>, headers: &mut Vec<String>| {
        if let Some(line) = current.take() {
            if line.get(..9).is_some_and(|p| p.eq_ignore_ascii_case("received:")) {
                headers.push(line[9..].trim().to_string());
            }
        }
    };

    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            break; // end of the header block
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            // A folded continuation of the current header.
            if let Some(cur) = current.as_mut() {
                cur.push(' ');
                cur.push_str(line.trim_start());
            }
            continue;
        }
        flush(&mut current, &mut headers);
        current = Some(line.to_string());
    }
    flush(&mut current, &mut headers);
    headers
}

/// The connecting client's IP recorded in one `Received:` header value.
///
/// Postfix writes it as the first bracketed token of the `from …` clause,
/// optionally prefixed `IPv6:` — `from host (host [1.2.3.4]) by …` or
/// `(unknown [IPv6:2001:db8::1])`. We take the first parseable bracketed
/// address, then fall back to a bare parenthesised address for the MTAs that
/// emit `from host (1.2.3.4)`.
fn extract_received_source_ip(value: &str) -> Option<IpAddr> {
    first_bracketed_ip(value).or_else(|| first_parenthesised_ip(value))
}

/// The first `[...]` token that parses as an IP address (Postfix's canonical
/// placement of the client address). Handles an `IPv6:` prefix.
fn first_bracketed_ip(value: &str) -> Option<IpAddr> {
    let mut rest = value;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            break;
        };
        let token = after[..close].trim();
        let token = token
            .strip_prefix("IPv6:")
            .or_else(|| token.strip_prefix("ipv6:"))
            .unwrap_or(token);
        if let Ok(ip) = token.parse::<IpAddr>() {
            return Some(ip);
        }
        rest = &after[close + 1..];
    }
    None
}

/// Fallback for MTAs that record the address bare inside parentheses. Scans each
/// `(...)` group for a token that parses as an IP; a token must look like an
/// address (contain `.` or `:`) so an SMTP `id` is never mistaken for one.
fn first_parenthesised_ip(value: &str) -> Option<IpAddr> {
    let mut rest = value;
    while let Some(open) = rest.find('(') {
        let after = &rest[open + 1..];
        let Some(close) = after.find(')') else {
            break;
        };
        for token in after[..close].split(|c: char| c.is_whitespace() || c == '[' || c == ']') {
            let token = token.trim_matches(|c: char| c == ',' || c == ';');
            // A token must look like an address so an SMTP `id` is never taken
            // for one.
            if token.contains('.') || token.contains(':') {
                if let Ok(ip) = token.parse::<IpAddr>() {
                    return Some(ip);
                }
            }
        }
        rest = &after[close + 1..];
    }
    None
}

/// True when `ip` is a routable public address — not private, loopback,
/// link-local, unspecified, documentation or otherwise reserved. Used to reject
/// an internal hop recorded in a `Received:` header as the "real" client.
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.octets()[0] == 0)
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return false;
            }
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let seg0 = v6.segments()[0];
            let unique_local = (seg0 & 0xfe00) == 0xfc00; // fc00::/7
            let link_local = (seg0 & 0xffc0) == 0xfe80; // fe80::/10
            !(unique_local || link_local)
        }
    }
}

/// Maps an SPF result to its `Authentication-Results` token.
fn spf_result_str(result: SpfResult) -> &'static str {
    match result {
        SpfResult::Pass => "pass",
        SpfResult::Fail => "fail",
        SpfResult::SoftFail => "softfail",
        SpfResult::Neutral => "neutral",
        SpfResult::TempError => "temperror",
        SpfResult::PermError => "permerror",
        SpfResult::None => "none",
    }
}

/// Maps a single DKIM result to its `Authentication-Results` token.
fn dkim_result_str(result: &DkimResult) -> &'static str {
    match result {
        DkimResult::Pass => "pass",
        DkimResult::Neutral(_) => "neutral",
        DkimResult::Fail(_) => "fail",
        DkimResult::PermError(_) => "permerror",
        DkimResult::TempError(_) => "temperror",
        DkimResult::None => "none",
    }
}

/// Collapses several DKIM signature results into a single verdict, favouring a
/// passing signature (a message may carry more than one).
fn dkim_summary(outputs: &[DkimOutput]) -> &'static str {
    if outputs.is_empty() {
        return "none";
    }
    // Preference order: a pass wins; then the most informative failure.
    let ranked = |r: &DkimResult| match r {
        DkimResult::Pass => 0,
        DkimResult::Fail(_) => 1,
        DkimResult::TempError(_) => 2,
        DkimResult::PermError(_) => 3,
        DkimResult::Neutral(_) => 4,
        DkimResult::None => 5,
    };
    outputs
        .iter()
        .map(|o| o.result())
        .min_by_key(|r| ranked(r))
        .map(dkim_result_str)
        .unwrap_or("none")
}

/// Reproduces mail-auth's `Authentication-Results` DMARC verdict: aligned SPF OR
/// aligned DKIM yields `pass`; otherwise the non-`none` side is reported.
fn dmarc_summary(spf: &DmarcResult, dkim: &DmarcResult) -> &'static str {
    if matches!(spf, DmarcResult::Pass) || matches!(dkim, DmarcResult::Pass) {
        "pass"
    } else if !matches!(spf, DmarcResult::None) {
        dmarc_result_str(spf)
    } else if !matches!(dkim, DmarcResult::None) {
        dmarc_result_str(dkim)
    } else {
        "none"
    }
}

/// Maps a DMARC alignment result to its `Authentication-Results` token.
fn dmarc_result_str(result: &DmarcResult) -> &'static str {
    match result {
        DmarcResult::Pass => "pass",
        DmarcResult::Fail(_) => "fail",
        DmarcResult::TempError(_) => "temperror",
        DmarcResult::PermError(_) => "permerror",
        DmarcResult::None => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_null_and_bracketed_sender() {
        assert_eq!(normalize_mail_from("<>"), "");
        assert_eq!(normalize_mail_from(""), "");
        assert_eq!(normalize_mail_from("<alice@example.com>"), "alice@example.com");
        assert_eq!(normalize_mail_from("  bob@example.org "), "bob@example.org");
    }

    #[test]
    fn extracts_domain() {
        assert_eq!(domain_of("alice@example.com"), Some("example.com"));
        assert_eq!(domain_of("mailer-daemon"), None);
    }

    #[test]
    fn dmarc_summary_prefers_alignment() {
        assert_eq!(
            dmarc_summary(&DmarcResult::Pass, &DmarcResult::None),
            "pass"
        );
        assert_eq!(
            dmarc_summary(&DmarcResult::None, &DmarcResult::Pass),
            "pass"
        );
        assert_eq!(dmarc_summary(&DmarcResult::None, &DmarcResult::None), "none");
    }

    /// The header value must be `<hostname>; <results>` with the tokens we set,
    /// and must NOT include the `Authentication-Results:` field name.
    #[test]
    fn formats_authentication_results_header() {
        let ip: IpAddr = "192.0.2.10".parse().expect("valid ip");
        let spf = SpfOutput::new("example.com".to_string()).with_result(SpfResult::Pass);
        let header = AuthenticationResults::new("mx.kubuno.local")
            .with_spf_mailfrom_result(&spf, ip, "alice@example.com", "mail.example.com")
            .to_string();

        assert!(header.starts_with("mx.kubuno.local"));
        assert!(!header.contains("Authentication-Results:"));
        assert!(header.contains("spf=pass"));
        assert!(header.contains("smtp.mailfrom=alice@example.com"));
    }

    // ── Real client IP behind a trusted relay ───────────────────────────────

    fn net(cidr: &str) -> CidrNet {
        CidrNet::parse(cidr).expect("cidr valide")
    }

    /// The canonical Postfix single-relay case: the top `Received:` was written
    /// by the trusted relay and records the real client.
    #[test]
    fn extracts_the_real_client_from_a_single_relay() {
        // 81.2.69.142 is a genuinely public address; the RFC 5737 ranges
        // (203.0.113/24 …) are documentation and are treated as non-public.
        let raw = b"Received: from mail.sender.example (mail.sender.example [81.2.69.142])\r\n\
            \tby vps.relay.example (Postfix) with ESMTPS id ABC123\r\n\
            \tfor <user@kubuno.local>; Fri, 08 Aug 2026 10:00:00 +0000\r\n\
            From: Alice <alice@sender.example>\r\n\
            Subject: hi\r\n\
            \r\n\
            body\r\n";
        let trusted = [net("15.100.1.0/24")];
        assert_eq!(
            client_ip_from_received(raw, &trusted),
            Some("81.2.69.142".parse().expect("ip"))
        );
    }

    /// A chain of two trusted relays: both hops recorded inside the trusted
    /// networks are skipped, the real client one hop further down is returned.
    #[test]
    fn skips_a_chain_of_trusted_relays() {
        let raw = b"Received: from vps1.relay.example (vps1.relay.example [15.100.1.2])\r\n\
            \tby vps2.relay.example (Postfix) with ESMTP id DEF456; date\r\n\
            Received: from realclient.example (unknown [8.8.4.4])\r\n\
            \tby vps1.relay.example (Postfix) with ESMTP id GHI789; date\r\n\
            From: Bob <bob@realclient.example>\r\n\
            \r\n\
            body\r\n";
        let trusted = [net("15.100.1.0/24")];
        assert_eq!(
            client_ip_from_received(raw, &trusted),
            Some("8.8.4.4".parse().expect("ip"))
        );
    }

    /// IPv6, written by Postfix with the `IPv6:` bracket prefix.
    #[test]
    fn extracts_an_ipv6_client() {
        let raw = b"Received: from mail.sender.example (mail.sender.example [IPv6:2606:2800:220:1:248:1893:25c8:1946])\r\n\
            \tby vps.relay.example (Postfix) with ESMTPS id ABC; date\r\n\
            From: Alice <alice@sender.example>\r\n\
            \r\n\
            body\r\n";
        let trusted = [net("15.100.1.0/24")];
        assert_eq!(
            client_ip_from_received(raw, &trusted),
            Some("2606:2800:220:1:248:1893:25c8:1946".parse().expect("ip"))
        );
    }

    /// No `Received:` at all — nothing to extract.
    #[test]
    fn no_received_header_yields_none() {
        let raw = b"From: Alice <alice@sender.example>\r\n\
            Subject: hi\r\n\
            \r\n\
            body\r\n";
        assert!(client_ip_from_received(raw, &[net("15.100.1.0/24")]).is_none());
    }

    /// A private address recorded in the chain is not a routable client: it is
    /// skipped, and with nothing else usable the result is `None`.
    #[test]
    fn a_private_recorded_ip_is_ignored() {
        let raw = b"Received: from internal (internal [10.0.0.5])\r\n\
            \tby vps.relay.example (Postfix) with ESMTP id ABC; date\r\n\
            From: Alice <alice@sender.example>\r\n\
            \r\n\
            body\r\n";
        assert!(client_ip_from_received(raw, &[net("15.100.1.0/24")]).is_none());
    }

    /// The bare-parenthesis fallback, for MTAs that do not bracket the address.
    #[test]
    fn extracts_from_a_bare_parenthesised_address() {
        let raw = b"Received: from mail.sender.example (45.33.32.156)\r\n\
            \tby vps.relay.example with ESMTP id ABC; date\r\n\
            From: Alice <alice@sender.example>\r\n\
            \r\n\
            body\r\n";
        assert_eq!(
            client_ip_from_received(raw, &[net("15.100.1.0/24")]),
            Some("45.33.32.156".parse().expect("ip"))
        );
    }

    /// When the trusted relay itself is the only source recorded (e.g. the real
    /// client hop is missing), there is no external client to blame SPF on.
    #[test]
    fn only_trusted_hops_yields_none() {
        let raw = b"Received: from vps.relay.example (vps.relay.example [15.100.1.1])\r\n\
            \tby kubuno.local (Kubuno) with ESMTP id ABC; date\r\n\
            From: Alice <alice@sender.example>\r\n\
            \r\n\
            body\r\n";
        assert!(client_ip_from_received(raw, &[net("15.100.1.0/24")]).is_none());
    }

    #[test]
    fn public_ip_classification() {
        // RFC 5737 documentation ranges are not routable clients.
        assert!(!is_public_ip("203.0.113.7".parse::<IpAddr>().expect("ip")));
        assert!(is_public_ip("81.2.69.142".parse().expect("ip")));
        assert!(!is_public_ip("10.0.0.1".parse().expect("ip")));
        assert!(!is_public_ip("192.168.1.1".parse().expect("ip")));
        assert!(!is_public_ip("127.0.0.1".parse().expect("ip")));
        assert!(!is_public_ip("::1".parse().expect("ip")));
        assert!(!is_public_ip("fe80::1".parse().expect("ip")));
        assert!(!is_public_ip("fc00::1".parse().expect("ip")));
        assert!(is_public_ip("2606:2800:220:1:248:1893:25c8:1946".parse().expect("ip")));
    }

    /// Offline check: a DKIM-signed message parses and exposes its signature and
    /// From domain (full verification would need DNS, which we don't do here).
    #[test]
    fn parses_dkim_signed_message() {
        let raw = b"DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed; d=example.com;\r\n\
            \ts=selector; h=from:to:subject; bh=uGROUP=; b=abcDEF123=\r\n\
            From: Alice <alice@example.com>\r\n\
            To: Bob <bob@example.org>\r\n\
            Subject: hello\r\n\
            \r\n\
            Body of the message.\r\n";
        let message = AuthenticatedMessage::parse(raw).expect("message parses");
        assert_eq!(domain_of(message.from()), Some("example.com"));
        assert!(
            !message.dkim_headers.is_empty(),
            "the DKIM-Signature header should have been located"
        );
    }
}
