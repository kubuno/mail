//! Web Key Directory (WKD) — GnuPG/Sequoia-compatible OpenPGP key discovery.
//!
//! WKD maps an e-mail address to a URL that serves the owner's public key as a
//! BINARY OpenPGP certificate over HTTPS, so a sender can encrypt to a
//! correspondent it has never exchanged keys with. The mapping is defined by
//! `draft-koch-openpgp-webkey-service` (the GnuPG spec) and mirrored on
//! Sequoia's `net::wkd` module — we do NOT hand-roll the wire format:
//!
//!   * the local-part is ASCII-down-cased (upper-case ASCII → lower-case,
//!     non-ASCII unchanged), then hashed with **SHA-1**;
//!   * the 160-bit digest is encoded with **z-base-32** (RFC 6189 §5.1.6,
//!     alphabet `ybndrfg8ejkmcpqxot1uwisza345h769`) to a fixed 32-char string;
//!   * two URL forms locate the key —
//!     advanced `https://openpgpkey.<domain>/.well-known/openpgpkey/<domain>/hu/<hash>?l=<localpart>`
//!     and direct `https://<domain>/.well-known/openpgpkey/hu/<hash>?l=<localpart>`.
//!
//! The z-base-32 alphabet and the hash rule are verified in tests against the
//! canonical vectors from the spec / Sequoia (`joe.doe`, `test1`).

use std::time::Duration;

use sha1::{Digest, Sha1};

/// z-base-32 alphabet (Zooko's, RFC 6189 §5.1.6): 32 symbols, no vowels except
/// what avoids visual confusion. This EXACT ordering is what makes the WKD hash
/// interoperable — the test vectors below fail loudly if it is wrong.
const ZBASE32_ALPHABET: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";

/// Encode bytes with z-base-32 (big-endian bit order, no padding). For a 20-byte
/// SHA-1 digest this yields exactly 32 characters.
pub fn zbase32_encode(data: &[u8]) -> String {
    let total_bits = data.len() * 8;
    let mut out = String::with_capacity(total_bits.div_ceil(5));
    let mut bit = 0;
    while bit < total_bits {
        let mut idx: u8 = 0;
        for j in 0..5 {
            let b = bit + j;
            // Bits past the end of the input are zero (z-base-32 pads the final
            // group on the right); for 160 bits it divides evenly so this never
            // triggers, but keeping it correct makes the encoder reusable.
            let set = if b < total_bits {
                (data[b / 8] >> (7 - (b % 8))) & 1
            } else {
                0
            };
            idx = (idx << 1) | set;
        }
        out.push(ZBASE32_ALPHABET[idx as usize] as char);
        bit += 5;
    }
    out
}

/// The WKD hash of a local-part: z-base-32(SHA-1(ascii-down-cased local-part)).
pub fn local_part_hash(local_part: &str) -> String {
    // `to_ascii_lowercase` maps upper-case ASCII to lower-case and leaves
    // non-ASCII untouched — exactly the spec's mapping.
    let mapped = local_part.to_ascii_lowercase();
    let digest = Sha1::digest(mapped.as_bytes());
    zbase32_encode(&digest)
}

/// Split `addr` into `(local_part, domain)`, both non-empty, domain lower-cased.
fn split_address(addr: &str) -> Option<(&str, String)> {
    let (local, domain) = addr.rsplit_once('@')?;
    if local.is_empty() || domain.is_empty() {
        return None;
    }
    Some((local, domain.to_ascii_lowercase()))
}

/// The advanced then direct WKD base URLs (without the `?l=` query, which the
/// caller adds so the local-part is percent-encoded by the HTTP client) for an
/// address. Advanced is tried first, as the spec recommends.
pub fn candidate_urls(addr: &str) -> Option<[String; 2]> {
    let (local, domain) = split_address(addr)?;
    let hash = local_part_hash(local);
    let advanced =
        format!("https://openpgpkey.{domain}/.well-known/openpgpkey/{domain}/hu/{hash}");
    let direct = format!("https://{domain}/.well-known/openpgpkey/hu/{hash}");
    Some([advanced, direct])
}

/// Discover a correspondent's OpenPGP public key via WKD. Tries the advanced
/// method, then the direct method; parses the first well-formed, self-consistent
/// certificate that actually covers `addr`, and returns it re-armored together
/// with its fingerprint. Returns `None` when nothing is published or the served
/// bytes are not a valid key for this address.
///
/// Network access is bounded (short timeout, capped body): a WKD lookup reaches
/// a domain the recipient controls, so it must never hang a send or let a hostile
/// server stream an unbounded response.
pub async fn discover(http: &reqwest::Client, addr: &str) -> Option<(String, String)> {
    let (local, _domain) = split_address(addr)?;
    let local = local.to_string();
    let urls = candidate_urls(addr)?;

    for url in urls {
        let resp = match http
            .get(&url)
            .query(&[("l", local.as_str())])
            .timeout(Duration::from_secs(8))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };
        // Cap the body: a valid certificate is a few KB; anything much larger is
        // either not a key or an attempt to exhaust memory.
        let bytes = match resp.bytes().await {
            Ok(b) if b.len() <= 512 * 1024 => b,
            _ => continue,
        };
        let Ok((armored, fingerprint, emails)) =
            crate::services::pgp::import_public_bytes(&bytes)
        else {
            continue;
        };
        // The served key must actually be for the address we asked about; a WKD
        // server handing back an unrelated key must not be trusted for it.
        if emails.iter().any(|e| e.eq_ignore_ascii_case(addr)) {
            return Some((armored, fingerprint));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Canonical vectors from draft-koch-openpgp-webkey-service and Sequoia's
    // wkd tests. If the z-base-32 alphabet or the down-casing rule were wrong,
    // these would not match — this is the guard the mission asked for.
    #[test]
    fn hash_matches_spec_vector_joe_doe() {
        // "Joe.Doe" down-cased to "joe.doe" → this exact hash (GnuPG WKD spec).
        assert_eq!(local_part_hash("Joe.Doe"), "iy9q119eutrkn8s1mk4r39qejnbu3n5q");
        // Down-casing means the mixed-case and lower-case inputs agree.
        assert_eq!(local_part_hash("joe.doe"), "iy9q119eutrkn8s1mk4r39qejnbu3n5q");
    }

    #[test]
    fn hash_matches_sequoia_vector_test1() {
        assert_eq!(local_part_hash("test1"), "stnkabub89rpcphiz4ppbxixkwyt1pic");
    }

    #[test]
    fn zbase32_length_is_32_for_sha1() {
        // A 20-byte (160-bit) digest encodes to exactly 32 z-base-32 chars.
        let out = zbase32_encode(&[0u8; 20]);
        assert_eq!(out.len(), 32);
        // All-zero input maps to the first symbol repeated.
        assert!(out.chars().all(|c| c == 'y'));
    }

    #[test]
    fn urls_have_both_methods() {
        let [advanced, direct] = candidate_urls("Joe.Doe@Example.ORG").expect("urls");
        assert_eq!(
            advanced,
            "https://openpgpkey.example.org/.well-known/openpgpkey/example.org/hu/iy9q119eutrkn8s1mk4r39qejnbu3n5q"
        );
        assert_eq!(
            direct,
            "https://example.org/.well-known/openpgpkey/hu/iy9q119eutrkn8s1mk4r39qejnbu3n5q"
        );
    }

    #[test]
    fn rejects_addresses_without_local_or_domain() {
        assert!(candidate_urls("@example.org").is_none());
        assert!(candidate_urls("nobody").is_none());
        assert!(candidate_urls("a@").is_none());
    }
}
