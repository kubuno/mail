//! DKIM signing of outbound messages (RFC 6376), via the `mail-auth` crate.
//!
//! Signing is what keeps our mail out of Gmail's and Outlook's spam folders; a
//! wrong canonicalisation (a stray space) breaks the body hash and sends the
//! message to spam, which is exactly why we lean on a mature library instead of
//! hand-rolling the canonicalisation.

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use mail_auth::common::crypto::{Ed25519Key, RsaKey, Sha256};
use mail_auth::dkim::generate::DkimKeyPair;
use mail_auth::dkim::{Canonicalization, DkimSigner, Done, NeedDomain, Signature};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

/// A signing key as stored (decrypted) for one domain.
pub struct SigningKey {
    pub domain: String,
    pub selector: String,
    /// "rsa-sha256" or "ed25519-sha256".
    pub algorithm: String,
    /// PEM (RSA PKCS#8) or the raw ed25519 key material, as generated.
    pub private_key: String,
}

/// Headers covered by `h=`. From is MANDATORY and appears twice: signing it once
/// more than it occurs in the message ("oversigning") pins the count, so a relay
/// that appends a second, forged From header invalidates the signature instead
/// of slipping a spoofed identity past the DKIM check. mail-auth honours a header
/// name listed more times than it is present by appending the surplus name to
/// `h=`, which is exactly the oversigning encoding.
///
/// MIME-Version and Content-Type are signed too: without them a relay could
/// rewrite the MIME structure (swap the body parts, change the boundary) and
/// still pass DKIM, and receivers such as Gmail expect the content headers to be
/// under the signature. Every message we build carries both.
const SIGNED_HEADERS: [&str; 8] =
    ["From", "Subject", "Date", "To", "Message-ID", "MIME-Version", "Content-Type", "From"];

/// Returns `raw` with a `DKIM-Signature:` header prepended, signed with `key`.
///
/// On any error (bad key material, signing failure) the error is propagated so
/// the delivery worker can defer — a message is NEVER returned unsigned.
pub fn sign(raw: &[u8], key: &SigningKey) -> Result<Vec<u8>> {
    match key.algorithm.as_str() {
        "rsa-sha256" => {
            let der = pem_to_der(&key.private_key)?;
            let rsa = RsaKey::<Sha256>::from_key_der(PrivateKeyDer::Pkcs8(
                PrivatePkcs8KeyDer::from(der),
            ))
            .map_err(|e| anyhow::anyhow!("cannot load RSA DKIM key: {e}"))?;
            let signature = build_signer(DkimSigner::from_key(rsa), key)
                .sign(raw)
                .map_err(|e| anyhow::anyhow!("DKIM signing failed: {e}"))?;
            Ok(render(&signature, raw))
        }
        "ed25519-sha256" => {
            let der = STANDARD
                .decode(key.private_key.as_bytes())
                .context("invalid base64 in ed25519 DKIM key")?;
            let ed = Ed25519Key::from_pkcs8_der(&der)
                .map_err(|e| anyhow::anyhow!("cannot load ed25519 DKIM key: {e}"))?;
            let signature = build_signer(DkimSigner::from_key(ed), key)
                .sign(raw)
                .map_err(|e| anyhow::anyhow!("DKIM signing failed: {e}"))?;
            Ok(render(&signature, raw))
        }
        other => anyhow::bail!("unsupported DKIM algorithm: {other}"),
    }
}

/// Applies domain/selector/headers and pins relaxed/relaxed canonicalisation.
///
/// relaxed/relaxed is the recommended pairing: relaxed header canonicalisation
/// folds whitespace and lowercases names, and relaxed body canonicalisation
/// tolerates trailing whitespace and line-ending rewrites — both survive the
/// benign reformatting that relays routinely apply, where `simple` would break.
fn build_signer<T: mail_auth::common::crypto::SigningKey>(
    signer: DkimSigner<T, NeedDomain>,
    key: &SigningKey,
) -> DkimSigner<T, Done> {
    signer
        .domain(key.domain.as_str())
        .selector(key.selector.as_str())
        .headers(SIGNED_HEADERS)
        .header_canonicalization(Canonicalization::Relaxed)
        .body_canonicalization(Canonicalization::Relaxed)
}

/// Prepends the produced `DKIM-Signature` header to `raw`.
fn render(signature: &Signature, raw: &[u8]) -> Vec<u8> {
    // `write(_, true)` emits the complete `DKIM-Signature: ...` header terminated
    // by CRLF, so writing it ahead of `raw` yields a valid message with the
    // signature as the very first header.
    let mut out = Vec::with_capacity(raw.len() + 256);
    signature.write(&mut out, true);
    out.extend_from_slice(raw);
    out
}

/// Generates a new signing key, returning (private_key_pem, public_key_base64).
///
/// The private half is meant to be encrypted and stored; the public half is
/// shown to the admin for the DNS TXT record `v=DKIM1; k=rsa; p=<base64>`
/// (or `k=ed25519`). RSA keys are 2048-bit — verifiers ignore keys under 1024.
pub fn generate(algorithm: &str) -> Result<(String, String)> {
    match algorithm {
        "rsa-sha256" => {
            let pair = DkimKeyPair::generate_rsa(2048)
                .map_err(|e| anyhow::anyhow!("RSA key generation failed: {e}"))?;
            // mail-auth emits PKCS#1 DER; wrap into the standard PKCS#8 (private)
            // and SubjectPublicKeyInfo (public) containers that tooling and DNS
            // verifiers expect.
            let (pkcs1_private, pkcs1_public) = pair.into_inner();
            let private_pem = to_pem("PRIVATE KEY", &rsa_pkcs1_to_pkcs8(&pkcs1_private));
            let public_b64 = STANDARD.encode(rsa_pkcs1_public_to_spki(&pkcs1_public));
            Ok((private_pem, public_b64))
        }
        "ed25519-sha256" => {
            let pair = DkimKeyPair::generate_ed25519()
                .map_err(|e| anyhow::anyhow!("ed25519 key generation failed: {e}"))?;
            let (pkcs8_private, raw_public) = pair.into_inner();
            // Store the PKCS#8 DER as base64 (loaded back verbatim by `sign`).
            let private = STANDARD.encode(pkcs8_private);
            let public_b64 = STANDARD.encode(raw_public);
            Ok((private, public_b64))
        }
        other => anyhow::bail!("unsupported DKIM algorithm: {other}"),
    }
}

// ─── DER / PEM helpers ─────────────────────────────────────────────────────
//
// Minimal, self-contained ASN.1 wrapping. This is key *encoding*, not the DKIM
// canonicalisation (that stays with mail-auth): it re-frames the PKCS#1 material
// mail-auth generates into the PKCS#8 and SubjectPublicKeyInfo envelopes.

/// AlgorithmIdentifier for rsaEncryption (OID 1.2.840.113549.1.1.1) + NULL params.
const RSA_ALGORITHM_ID: [u8; 15] = [
    0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00,
];

/// DER definite-length encoding of `len`.
fn der_len(len: usize) -> Vec<u8> {
    if len < 0x80 {
        vec![len as u8]
    } else {
        let be = len.to_be_bytes();
        let start = be.iter().position(|&b| b != 0).unwrap_or(be.len() - 1);
        let significant = &be[start..];
        let mut out = Vec::with_capacity(significant.len() + 1);
        out.push(0x80 | significant.len() as u8);
        out.extend_from_slice(significant);
        out
    }
}

/// DER TLV: `tag || length || content`.
fn der_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len() + 4);
    out.push(tag);
    out.extend_from_slice(&der_len(content.len()));
    out.extend_from_slice(content);
    out
}

/// Wraps a PKCS#1 RSAPrivateKey DER into a PKCS#8 PrivateKeyInfo DER.
fn rsa_pkcs1_to_pkcs8(pkcs1: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&[0x02, 0x01, 0x00]); // version INTEGER 0
    body.extend_from_slice(&RSA_ALGORITHM_ID);
    body.extend_from_slice(&der_tlv(0x04, pkcs1)); // privateKey OCTET STRING
    der_tlv(0x30, &body)
}

/// Wraps a PKCS#1 RSAPublicKey DER into a SubjectPublicKeyInfo DER.
fn rsa_pkcs1_public_to_spki(pkcs1_public: &[u8]) -> Vec<u8> {
    let mut bit_string = Vec::with_capacity(pkcs1_public.len() + 1);
    bit_string.push(0x00); // zero unused bits
    bit_string.extend_from_slice(pkcs1_public);
    let mut body = Vec::new();
    body.extend_from_slice(&RSA_ALGORITHM_ID);
    body.extend_from_slice(&der_tlv(0x03, &bit_string)); // subjectPublicKey BIT STRING
    der_tlv(0x30, &body)
}

/// Encodes DER bytes as a PEM block with the given label (64-column body).
fn to_pem(label: &str, der: &[u8]) -> String {
    let b64 = STANDARD.encode(der);
    let mut out = String::new();
    out.push_str("-----BEGIN ");
    out.push_str(label);
    out.push_str("-----\r\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push_str("\r\n");
    }
    out.push_str("-----END ");
    out.push_str(label);
    out.push_str("-----\r\n");
    out
}

/// Extracts and base64-decodes the DER body of a PEM block.
fn pem_to_der(pem: &str) -> Result<Vec<u8>> {
    let b64: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .flat_map(|line| line.chars())
        .filter(|c| !c.is_whitespace())
        .collect();
    STANDARD
        .decode(b64.as_bytes())
        .context("invalid PEM base64 in DKIM private key")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_message() -> &'static [u8] {
        concat!(
            "From: alice@example.com\r\n",
            "To: bob@example.com\r\n",
            "Subject: Hello\r\n",
            "Date: Wed, 05 Aug 2026 00:00:00 +0000\r\n",
            "Message-ID: <1@example.com>\r\n",
            "\r\n",
            "Hello world\r\n",
        )
        .as_bytes()
    }

    #[test]
    fn rsa_generate_then_sign_produces_valid_header() {
        let (private_pem, public_b64) = generate("rsa-sha256").expect("generate RSA key");
        assert!(private_pem.contains("-----BEGIN PRIVATE KEY-----"));

        // Public key must be well-formed DER (a SubjectPublicKeyInfo SEQUENCE).
        let spki = STANDARD.decode(public_b64.as_bytes()).expect("public key base64");
        assert_eq!(spki.first(), Some(&0x30), "SPKI must start with a SEQUENCE");

        let key = SigningKey {
            domain: "example.com".into(),
            selector: "sel".into(),
            algorithm: "rsa-sha256".into(),
            private_key: private_pem,
        };

        let msg = sample_message();
        let out = sign(msg, &key).expect("sign");

        // The prepended header is exactly the leading bytes before the message.
        let header_len = out.len() - msg.len();
        let header = String::from_utf8(out[..header_len].to_vec()).expect("utf8 header");

        assert!(header.starts_with("DKIM-Signature:"));
        assert!(header.contains("d=example.com"), "missing d=: {header}");
        assert!(header.contains("s=sel"), "missing s=: {header}");
        assert!(header.contains("bh="), "missing bh=: {header}");
        assert!(header.contains("b="), "missing b=: {header}");
        let lower = header.to_ascii_lowercase();
        assert!(lower.contains("h="), "missing h=: {header}");
        assert!(lower.contains("from"), "h= must cover From: {header}");
        // Oversigning: From appears twice in the h= tag.
        assert_eq!(lower.matches("from").count(), 2, "From should be oversigned: {header}");
    }

    #[test]
    fn ed25519_generate_then_sign_produces_valid_header() {
        let (private, public_b64) = generate("ed25519-sha256").expect("generate ed25519 key");
        assert!(!public_b64.is_empty());

        let key = SigningKey {
            domain: "example.com".into(),
            selector: "ed".into(),
            algorithm: "ed25519-sha256".into(),
            private_key: private,
        };

        let msg = sample_message();
        let out = sign(msg, &key).expect("sign");
        let header_len = out.len() - msg.len();
        let header = String::from_utf8(out[..header_len].to_vec()).expect("utf8 header");

        assert!(header.starts_with("DKIM-Signature:"));
        assert!(header.contains("a=ed25519-sha256"), "{header}");
        assert!(header.contains("d=example.com"), "{header}");
    }

    #[test]
    fn unsupported_algorithm_errors() {
        assert!(generate("rsa-sha1").is_err());
        let key = SigningKey {
            domain: "example.com".into(),
            selector: "s".into(),
            algorithm: "rsa-sha1".into(),
            private_key: String::new(),
        };
        assert!(sign(sample_message(), &key).is_err());
    }
}
