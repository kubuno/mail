//! Reception hygiene for locally delivered mail: the cheap boundary checks a
//! receiving MTA owes every message before it becomes a stored row.
//!
//! These re-implement the lessons Postfix's `cleanup` daemon encodes: cap the
//! number of hops so a misrouted message cannot loop between two servers
//! forever, stamp our own trace header without ever letting a hostile byte forge
//! a second one, and refuse an envelope address that could not possibly be real.
//!
//! Every function here is pure and free of I/O so it can be unit-tested without
//! a socket or a database.

use chrono::Utc;

/// Hard ceiling on the number of `Received:` trace headers a message may already
/// carry before we refuse to deliver it. Matches Postfix's `var_hopcount_limit`
/// default: a message relayed 50 times is looping, not travelling, and each
/// extra pass only makes the loop tighter.
///
/// This is the FLOOR of the protection, not the knob: the administrator's
/// `hopcount_limit` may only make the check stricter (see `smtp::cmd_data`).
/// `deliver::deliver_local` keeps using this constant as its own last-resort
/// guard, so a delivery driven from anywhere else is still bounded.
pub const HOPCOUNT_LIMIT: usize = 50;

/// Longest hostname `is_fqdn_helo` will consider (Postfix's
/// `VALID_HOSTNAME_LEN`, itself the RFC 1035 ceiling).
const MAX_HOSTNAME_LEN: usize = 255;

/// Longest single DNS label (RFC 1035 §2.3.4).
const MAX_LABEL_LEN: usize = 63;

/// Upper bound (in characters) on a value we splice into a header line, so a
/// hostile peer name or hostname cannot bloat the header block.
const MAX_HEADER_VALUE_LEN: usize = 200;

/// True when `hay` begins with `needle`, comparing ASCII case-insensitively.
fn starts_with_ignore_ascii_case(hay: &[u8], needle: &[u8]) -> bool {
    hay.len() >= needle.len() && hay[..needle.len()].eq_ignore_ascii_case(needle)
}

/// Counts the `Received:` header lines in the message's header block.
///
/// Only the header block counts: scanning stops at the first empty line — the
/// blank line that separates headers from body — so a line that merely starts
/// with "Received:" inside the body is never mistaken for a hop. Both CRLF and
/// bare-LF framing are accepted; the match is anchored at the start of the line
/// and case-insensitive; folded continuation lines (leading space or tab)
/// belong to the previous header and are skipped.
pub fn hop_count(raw: &[u8]) -> usize {
    let mut count = 0usize;
    for line in raw.split(|&b| b == b'\n') {
        // Strip a trailing CR so CRLF and bare-LF framing behave the same.
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            // The blank line ends the header block: nothing past it is a hop.
            break;
        }
        if line[0] == b' ' || line[0] == b'\t' {
            // Folded continuation of the previous header, never a new one.
            continue;
        }
        if starts_with_ignore_ascii_case(line, b"received:") {
            count += 1;
        }
    }
    count
}

/// Neutralises a string before it is spliced into a header value.
///
/// Any control character — CR and LF above all, but also NUL and the rest of the
/// C0 range — becomes a space, so an inserted value can never close its own
/// header and open another (header injection). The result is also bounded in
/// length to keep a single value from bloating the header block.
pub fn sanitize_header_value(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_HEADER_VALUE_LEN)
        .collect()
}

/// Bounded validation of an envelope address (MAIL FROM / RCPT TO).
///
/// Rejects the empty string (the special null sender `<>` is allowed elsewhere,
/// by the caller, not by this predicate), anything longer than the RFC 5321
/// ceiling of 320 octets, any control character (the vector for splitting one
/// field into several), and any non-empty address without an `@`.
pub fn valid_envelope_address(addr: &str) -> bool {
    if addr.is_empty() {
        return false;
    }
    if addr.len() > 320 {
        return false;
    }
    if addr.chars().any(|c| c.is_control()) {
        return false;
    }
    if !addr.contains('@') {
        return false;
    }
    true
}

/// Returns `raw` with a local `Received:` trace header prepended.
///
/// `peer` and `hostname` are both passed in (never read from a global) and both
/// run through [`sanitize_header_value`], so neither can inject CR/LF and forge
/// a second header. The date is RFC 2822 formatted.
pub fn prepend_received(raw: &[u8], peer: &str, hostname: &str) -> Vec<u8> {
    let date = Utc::now().format("%a, %d %b %Y %H:%M:%S %z");
    let header = format!(
        "Received: from {} by {} with SMTP; {}\r\n",
        sanitize_header_value(peer),
        sanitize_header_value(hostname),
        date,
    );
    let mut out = Vec::with_capacity(header.len() + raw.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(raw);
    out
}

// ── HELO/EHLO screening ──────────────────────────────────────────────────────
//
// Postfix's `reject_non_fqdn_helo_hostname`, re-implemented. Two branches, the
// same way `smtpd_check.c` splits them: a name starting with `[` is an address
// literal and is validated as an address (`reject_invalid_hostaddr`); anything
// else must pass `valid_hostname()` AND contain a dot.
//
// ⚠️ The name is NEVER resolved. Postfix keeps that as a separate, explicitly
// discouraged restriction (`reject_unknown_helo_hostname`) precisely because a
// large share of legitimate senders announce a name with no DNS record — a
// receiver that resolves HELO rejects real mail and pays a DNS round-trip per
// connection to do it.

/// True when a HELO/EHLO argument is a fully-qualified domain name, or a
/// well-formed address literal.
///
/// Accepts `mail.example.org` and `[192.0.2.1]` / `[IPv6:2001:db8::1]`; refuses
/// a single-label name (`localhost`), a syntactically broken name (`a..b`,
/// `-x.example`), an all-numeric name (`1.2.3.4` unbracketed) and a malformed
/// literal (`[not-an-ip]`). Pure: no DNS, no I/O.
pub fn is_fqdn_helo(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    // An address literal is checked as an address, never as a name.
    if let Some(inner) = name.strip_prefix('[') {
        return match inner.strip_suffix(']') {
            Some(addr) => is_address_literal(addr),
            // A `[` with no closing `]` is malformed, not a hostname.
            None => false,
        };
    }
    // Postfix truncates a single trailing dot ("host.example.org." is the same
    // name); a trailing ".." stays malformed.
    let candidate = match name.strip_suffix('.') {
        Some(shorter) if !shorter.ends_with('.') && !shorter.is_empty() => shorter,
        Some(_) => return false,
        None => name,
    };
    valid_hostname(candidate) && candidate.contains('.')
}

/// The body of an address literal, brackets already removed: a dotted-quad IPv4
/// address, or an IPv6 address — with the RFC 5321 §4.1.3 `IPv6:` tag, which we
/// also tolerate missing since some senders omit it.
fn is_address_literal(addr: &str) -> bool {
    if addr.is_empty() {
        return false;
    }
    if let Some(v6) = strip_ipv6_tag(addr) {
        return v6.parse::<std::net::Ipv6Addr>().is_ok();
    }
    if addr.contains(':') {
        return addr.parse::<std::net::Ipv6Addr>().is_ok();
    }
    addr.parse::<std::net::Ipv4Addr>().is_ok()
}

/// Strips the case-insensitive `IPv6:` tag of an address literal.
fn strip_ipv6_tag(addr: &str) -> Option<&str> {
    let tag = addr.get(..5)?;
    tag.eq_ignore_ascii_case("IPv6:").then(|| &addr[5..])
}

/// Postfix's `valid_hostname()`, minus the wildcard option we have no use for.
///
/// Letters, digits, dots, hyphens and — as Postfix puts it, "grr.." —
/// underscores; no empty label, no label over 63 octets, no leading or trailing
/// hyphen in a label, nothing over 255 octets, and never an all-numeric name
/// (that is an address written without its brackets).
fn valid_hostname(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_HOSTNAME_LEN {
        return false;
    }
    let mut non_numeric = false;
    for label in name.split('.') {
        if label.is_empty() || label.len() > MAX_LABEL_LEN {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        for ch in label.chars() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                if !ch.is_ascii_digit() {
                    non_numeric = true;
                }
            } else if ch == '-' {
                non_numeric = true;
            } else {
                return false;
            }
        }
    }
    non_numeric
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_count_is_zero_without_any_received() {
        let raw = b"From: a@b.c\r\nTo: d@e.f\r\nSubject: hi\r\n\r\nbody\r\n";
        assert_eq!(hop_count(raw), 0);
    }

    #[test]
    fn hop_count_sums_every_received_header() {
        let raw = b"Received: from x by y\r\nReceived: from p by q\r\nFrom: a@b.c\r\n\r\nbody";
        assert_eq!(hop_count(raw), 2);
    }

    #[test]
    fn hop_count_is_case_insensitive() {
        let raw = b"received: from x\r\nRECEIVED: from y\r\nReCeIvEd: from z\r\n\r\nbody";
        assert_eq!(hop_count(raw), 3);
    }

    #[test]
    fn hop_count_stops_at_the_blank_line() {
        // The "Received:" below lives in the BODY and must not be counted.
        let raw = b"Received: from x by y\r\n\r\nReceived: from body-forgery\r\n";
        assert_eq!(hop_count(raw), 1);
    }

    #[test]
    fn hop_count_ignores_folded_continuation_lines() {
        // A single Received whose value is folded over two lines is one hop.
        let raw = b"Received: from x by y\r\n\t(with a folded tail)\r\nFrom: a@b.c\r\n\r\nbody";
        assert_eq!(hop_count(raw), 1);
    }

    #[test]
    fn hop_count_accepts_bare_lf_framing() {
        let raw = b"Received: from x\nReceived: from y\n\nbody";
        assert_eq!(hop_count(raw), 2);
    }

    #[test]
    fn sanitize_neutralises_cr_and_lf() {
        let injected = "evil\r\nBcc: victim@example.org";
        let clean = sanitize_header_value(injected);
        assert!(!clean.contains('\r'), "aucun CR ne doit survivre");
        assert!(!clean.contains('\n'), "aucun LF ne doit survivre");
        // The injected "Bcc:" text stays, but folded onto the same line: with no
        // CR/LF it can no longer be read as a header of its own.
        assert_eq!(clean, "evil  Bcc: victim@example.org");
    }

    #[test]
    fn sanitize_neutralises_nul_and_other_controls() {
        let clean = sanitize_header_value("a\0b\tc");
        assert_eq!(clean, "a b c");
    }

    #[test]
    fn sanitize_bounds_the_length() {
        let long = "a".repeat(500);
        let clean = sanitize_header_value(&long);
        assert_eq!(clean.chars().count(), MAX_HEADER_VALUE_LEN);
    }

    #[test]
    fn valid_address_accepts_a_plain_mailbox() {
        assert!(valid_envelope_address("alice@example.org"));
    }

    #[test]
    fn valid_address_rejects_the_empty_string() {
        assert!(!valid_envelope_address(""));
    }

    #[test]
    fn valid_address_rejects_an_overlong_address() {
        let long = format!("{}@example.org", "a".repeat(320));
        assert!(!valid_envelope_address(&long));
    }

    #[test]
    fn valid_address_rejects_crlf_injection() {
        assert!(!valid_envelope_address("alice@example.org\r\nRCPT TO:<evil@x>"));
    }

    #[test]
    fn valid_address_rejects_an_address_without_at() {
        assert!(!valid_envelope_address("not-an-address"));
    }

    #[test]
    fn prepend_puts_received_first_and_keeps_the_body() {
        let raw = b"From: a@b.c\r\nSubject: hi\r\n\r\nbody\r\n";
        let out = prepend_received(raw, "mail.peer.example", "kubuno.local");
        assert!(out.starts_with(b"Received: from mail.peer.example by kubuno.local with SMTP;"));
        // The original message is preserved intact after our header.
        assert!(out.windows(raw.len()).any(|w| w == raw));
        // Adding our header lifts the hop count by exactly one.
        assert_eq!(hop_count(&out), hop_count(raw) + 1);
    }

    // ── HELO/EHLO screening ─────────────────────────────────────────────────

    #[test]
    fn fqdn_helo_accepts_a_real_hostname() {
        assert!(is_fqdn_helo("mail.example.org"));
        assert!(is_fqdn_helo("a.b.c.d.example.com"));
        assert!(is_fqdn_helo("xn--caf-dma.example"));
        // A hyphen or an underscore inside a label is fine.
        assert!(is_fqdn_helo("mx-1.example.org"));
        assert!(is_fqdn_helo("mail_relay.example.org"));
        // Surrounding whitespace is not the client's fault.
        assert!(is_fqdn_helo("  mail.example.org  "));
    }

    #[test]
    fn fqdn_helo_refuses_a_single_label_name() {
        assert!(!is_fqdn_helo("localhost"));
        assert!(!is_fqdn_helo("PC-DE-JEAN"));
        assert!(!is_fqdn_helo(""));
        assert!(!is_fqdn_helo("   "));
    }

    #[test]
    fn fqdn_helo_refuses_a_malformed_name() {
        assert!(!is_fqdn_helo("a..b"));
        assert!(!is_fqdn_helo(".example.org"));
        assert!(!is_fqdn_helo("-bad.example.org"));
        assert!(!is_fqdn_helo("bad-.example.org"));
        assert!(!is_fqdn_helo("exa mple.org"));
        assert!(!is_fqdn_helo("exa\u{0}mple.org"));
        assert!(!is_fqdn_helo("évidemment.example.org"));
        assert!(!is_fqdn_helo("example.org.."));
        let long_label = format!("{}.example.org", "a".repeat(64));
        assert!(!is_fqdn_helo(&long_label));
        // 30 valid labels of ten characters: well past the 255-octet ceiling.
        let long_name = format!("{}.org", vec!["abcdefghij"; 30].join("."));
        assert!(!is_fqdn_helo(&long_name));
    }

    /// A trailing dot is the same name; Postfix truncates it rather than
    /// refusing the sender over a formality.
    #[test]
    fn fqdn_helo_tolerates_a_single_trailing_dot() {
        assert!(is_fqdn_helo("mail.example.org."));
    }

    /// An unbracketed numeric name is an address written wrong, and Postfix's
    /// `valid_hostname()` refuses it outright.
    #[test]
    fn fqdn_helo_refuses_an_all_numeric_name() {
        assert!(!is_fqdn_helo("192.0.2.1"));
        assert!(!is_fqdn_helo("1.2"));
    }

    /// RFC 5321 §4.1.3: a client with no name of its own announces an address
    /// literal, and that is perfectly legitimate.
    #[test]
    fn fqdn_helo_accepts_a_well_formed_address_literal() {
        assert!(is_fqdn_helo("[192.0.2.1]"));
        assert!(is_fqdn_helo("[IPv6:2001:db8::1]"));
        assert!(is_fqdn_helo("[ipv6:::1]"));
        // The tag is optional in practice.
        assert!(is_fqdn_helo("[2001:db8::1]"));
    }

    #[test]
    fn fqdn_helo_refuses_a_malformed_address_literal() {
        assert!(!is_fqdn_helo("[not-an-ip]"));
        assert!(!is_fqdn_helo("[192.0.2.999]"));
        assert!(!is_fqdn_helo("[192.0.2.1"));
        assert!(!is_fqdn_helo("[]"));
        assert!(!is_fqdn_helo("[IPv6:]"));
    }

    #[test]
    fn prepend_sanitises_the_inserted_values() {
        // A peer name / hostname carrying CRLF must NOT be able to open a second
        // header: the injected text survives, but only folded into the single
        // Received line, never at the start of a line of its own.
        let out = prepend_received(b"body", "evil\r\nBcc: victim@x", "host\r\nX-Forged: 1");
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains("\nBcc:"), "l'injection via le peer n'ouvre aucun en-tête");
        assert!(!text.contains("\nX-Forged"), "l'injection via le hostname n'ouvre aucun en-tête");
        // Exactly one Received header was produced.
        assert_eq!(hop_count(&out), 1);
    }
}
