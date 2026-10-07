//! Autocrypt Level 1 header handling (<https://autocrypt.org/level1.html>).
//!
//! Autocrypt piggy-backs public-key discovery on ordinary mail: every message a
//! user sends carries an `Autocrypt:` header advertising their key, so the
//! recipient's client learns it passively and can later encrypt back. This module
//! parses that header from incoming mail and builds it for outgoing mail; storage
//! and the "keep the most recent" policy live in the sync/send paths.
//!
//! Header grammar (Level 1 §2.1): a `;`-separated attribute list.
//!   * `addr`           — REQUIRED, the sender address the key is for;
//!   * `keydata` — REQUIRED, base64 of the BINARY public key (may be folded
//!     across lines, so whitespace is stripped);
//!   * `prefer-encrypt` — OPTIONAL, `mutual` or `nopreference`;
//!   * attribute names starting with `_` are non-critical and ignored when
//!     unknown; any OTHER unknown attribute is "critical" and makes the whole
//!     header invalid (§2.1) — we return `None` so it is skipped entirely.

use base64::Engine;
use lettre::message::header::{Header, HeaderName, HeaderValue};

// lettre's `Header::parse` returns `Result<Self, BoxError>`; `BoxError` is a
// crate-private alias for this exact boxed-error type, which we spell out here.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A parsed, valid Autocrypt header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutocryptHeader {
    /// The advertised address (lower-cased for comparison).
    pub addr: String,
    /// The decoded BINARY public key (feed to `pgp::import_public_bytes`).
    pub keydata: Vec<u8>,
    /// `prefer-encrypt=mutual` was present.
    pub prefer_mutual: bool,
}

/// Parse one `Autocrypt:` header value. Returns `None` if it is malformed or
/// carries an unknown critical attribute (Level 1 §2.1 — such a header is
/// ignored, not partially honoured).
pub fn parse(value: &str) -> Option<AutocryptHeader> {
    let mut addr: Option<String> = None;
    let mut keydata_b64 = String::new();
    let mut has_keydata = false;
    let mut prefer_mutual = false;

    for attr in value.split(';') {
        let attr = attr.trim();
        if attr.is_empty() {
            continue;
        }
        // `keydata` base64 contains '=' padding, so split on the FIRST '=' only.
        let (key, val) = attr.split_once('=')?;
        let key = key.trim();
        let val = val.trim();
        match key {
            "addr" => addr = Some(val.to_ascii_lowercase()),
            "prefer-encrypt" => prefer_mutual = val.eq_ignore_ascii_case("mutual"),
            "keydata" => {
                // Folding may have left whitespace inside the base64; strip it.
                keydata_b64 = val.chars().filter(|c| !c.is_whitespace()).collect();
                has_keydata = true;
            }
            // Unknown non-critical (leading '_') → ignore; unknown critical →
            // the whole header is invalid.
            other if other.starts_with('_') => {}
            _ => return None,
        }
    }

    let addr = addr?;
    if !has_keydata {
        return None;
    }
    let keydata = base64::engine::general_purpose::STANDARD
        .decode(keydata_b64.as_bytes())
        .ok()?;
    if keydata.is_empty() {
        return None;
    }
    Some(AutocryptHeader { addr, keydata, prefer_mutual })
}

/// Build the `Autocrypt:` header value (the part after `Autocrypt: `) for our own
/// key, folding the base64 keydata so no physical line exceeds the SMTP limit.
pub fn header_value(addr: &str, public_key_binary: &[u8], prefer_mutual: bool) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(public_key_binary);
    // Fold the base64 into continuation lines (CRLF + one leading space), well
    // under the 78-char soft / 998-char hard header line limits.
    let folded = b64
        .as_bytes()
        .chunks(72)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\r\n ");
    let prefer = if prefer_mutual { "; prefer-encrypt=mutual" } else { "" };
    format!("addr={addr}{prefer}; keydata=\r\n {folded}")
}

/// A lettre header carrying a pre-built Autocrypt value. `header_value` already
/// folds and is pure ASCII, so it is emitted verbatim (no further RFC 2047
/// encoding). Attach it to the OUTER message headers — Autocrypt is always sent
/// in the clear, even for an encrypted message.
#[derive(Debug, Clone)]
pub struct AutocryptMimeHeader(pub String);

impl Header for AutocryptMimeHeader {
    fn name() -> HeaderName {
        HeaderName::new_from_ascii_str("Autocrypt")
    }

    fn parse(s: &str) -> Result<Self, BoxError> {
        Ok(AutocryptMimeHeader(s.to_string()))
    }

    fn display(&self) -> HeaderValue {
        // Pre-encoded: the value is ASCII and already line-folded. The unfolded
        // raw form (whitespace removed) is kept for round-tripping.
        let raw: String = self.0.chars().filter(|c| !c.is_whitespace()).collect();
        HeaderValue::dangerous_new_pre_encoded(Self::name(), raw, self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::pgp;

    #[test]
    fn emit_then_parse_roundtrips() {
        let k = pgp::generate("Alice <alice@example.com>").expect("gen");
        let binary = pgp::public_armored_to_binary(&k.public_armored).expect("binary");

        let value = header_value("alice@example.com", &binary, true);
        // Shape per the spec.
        assert!(value.starts_with("addr=alice@example.com; prefer-encrypt=mutual; keydata="));
        // No physical line exceeds the soft limit.
        assert!(value.split("\r\n").all(|line| line.len() <= 78), "a folded line is too long");

        let parsed = parse(&value).expect("parse");
        assert_eq!(parsed.addr, "alice@example.com");
        assert!(parsed.prefer_mutual);
        assert_eq!(parsed.keydata, binary);

        // The decoded keydata is a usable certificate for that address.
        let (_, fp, emails) = pgp::import_public_bytes(&parsed.keydata).expect("import");
        assert_eq!(fp, k.fingerprint);
        assert_eq!(emails, vec!["alice@example.com".to_string()]);
    }

    #[test]
    fn parse_downcases_addr_and_ignores_noncritical() {
        let k = pgp::generate("Bob <bob@example.com>").expect("gen");
        let binary = pgp::public_armored_to_binary(&k.public_armored).expect("binary");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&binary);

        // Mixed-case addr + an unknown NON-critical attribute (leading '_').
        let value = format!("addr=Bob@Example.Com; _extra=whatever; keydata={b64}");
        let parsed = parse(&value).expect("parse");
        assert_eq!(parsed.addr, "bob@example.com");
        assert!(!parsed.prefer_mutual);
        assert_eq!(parsed.keydata, binary);
    }

    #[test]
    fn parse_rejects_unknown_critical_attribute() {
        let value = "addr=a@b.com; danger=1; keydata=AAAA";
        assert!(parse(value).is_none(), "unknown critical attribute must void the header");
    }

    #[test]
    fn parse_requires_addr_and_keydata() {
        assert!(parse("keydata=AAAA").is_none());
        assert!(parse("addr=a@b.com").is_none());
    }
}
