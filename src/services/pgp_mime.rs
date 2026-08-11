//! PGP/MIME assembly (RFC 3156) — wraps a message body into `multipart/signed`
//! and `multipart/encrypted`, using lettre's own MIME serializer (deterministic,
//! CRLF) and rPGP for the crypto (`services::pgp`).
//!
//! The structure is mirrored on the reference implementation `emersion/go-pgpmail`
//! (MIT) and RFC 3156 — we do NOT hand-roll the wire format:
//!   • signed:    `multipart/signed; protocol="application/pgp-signature";
//!                 micalg="pgp-sha256"` = the exact signed body part + an
//!                 `application/pgp-signature` part carrying the armored detached
//!                 signature.
//!   • encrypted: `multipart/encrypted; protocol="application/pgp-encrypted"` =
//!                 an `application/pgp-encrypted` control part (`Version: 1`) + an
//!                 `application/octet-stream` part carrying the armored message.
//!
//! The signature is computed over `content.formatted()` — the SAME bytes lettre
//! re-emits when it serialises `content` inside the signed multipart (same stored
//! boundary), so a verifier extracting the first part gets an identical byte
//! string. Bodies are forced to base64 so SMTP cannot alter them and break the
//! signature (the classic PGP/MIME interop failure).

use anyhow::{anyhow, Result};
use lettre::message::{
    header::{ContentTransferEncoding, ContentType},
    MultiPart, SinglePart,
};

use crate::services::pgp;

/// Text + HTML alternative body, base64-encoded so its bytes survive transport
/// intact (a signature over an 8-bit/whitespace-mutable body would break in SMTP).
pub fn alternative_body(body_text: &str, body_html: &str) -> MultiPart {
    MultiPart::alternative()
        .singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .header(ContentTransferEncoding::Base64)
                .body(body_text.to_string()),
        )
        .singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_HTML)
                .header(ContentTransferEncoding::Base64)
                .body(body_html.to_string()),
        )
}

/// Wrap `content` in an RFC 3156 `multipart/signed`, signed by `secret_armored`.
pub fn sign(content: MultiPart, secret_armored: &str) -> Result<MultiPart> {
    // Exactly the bytes lettre will re-emit for `content` inside the multipart.
    let content_bytes = content.formatted();
    let signature = pgp::sign_detached(secret_armored, &content_bytes)
        .map_err(|_| anyhow!("PGP signature"))?;

    let sig_part = SinglePart::builder()
        .header(ContentType::parse("application/pgp-signature").map_err(|e| anyhow!(e))?)
        .header(ContentTransferEncoding::SevenBit) // ASCII armor is 7-bit
        .body(signature);

    Ok(MultiPart::signed("application/pgp-signature".to_string(), "pgp-sha256".to_string())
        .multipart(content)
        .singlepart(sig_part))
}

/// Wrap a serialised MIME entity (`inner_mime`, the content part WITH its headers,
/// possibly itself a multipart/signed) in an RFC 3156 `multipart/encrypted` to the
/// given recipient public keys.
pub fn encrypt(inner_mime: &[u8], recipient_public_armored: &[String]) -> Result<MultiPart> {
    let ciphertext = pgp::encrypt(recipient_public_armored, inner_mime)
        .map_err(|_| anyhow!("PGP encryption"))?;

    let control = SinglePart::builder()
        .header(ContentType::parse("application/pgp-encrypted").map_err(|e| anyhow!(e))?)
        .header(ContentTransferEncoding::SevenBit)
        .body("Version: 1".to_string());

    let payload = SinglePart::builder()
        .header(ContentType::parse("application/octet-stream; name=\"encrypted.asc\"").map_err(|e| anyhow!(e))?)
        .header(ContentTransferEncoding::SevenBit)
        .body(ciphertext);

    Ok(MultiPart::encrypted("application/pgp-encrypted".to_string())
        .singlepart(control)
        .singlepart(payload))
}

/// Resolved key material + intent, handed to [`build_email`](crate::services::smtp_service::build_email)
/// so it can protect the body. The handler fills this from the DB.
pub struct PgpParams {
    pub sign:    bool,
    pub encrypt: bool,
    /// Sender's armored secret key — required when `sign` is true.
    pub sender_secret_armored: String,
    /// Recipients' (and the sender's own, for the Sent copy) armored public keys —
    /// required when `encrypt` is true.
    pub recipient_public_armored: Vec<String>,
}

/// Apply the requested protection to `content`, per the params. `(false,false)`
/// returns the content unchanged.
pub fn wrap(content: MultiPart, params: &PgpParams) -> Result<MultiPart> {
    match (params.sign, params.encrypt) {
        (true, true) => sign_and_encrypt(content, &params.sender_secret_armored, &params.recipient_public_armored),
        (false, true) => encrypt(&content.formatted(), &params.recipient_public_armored),
        (true, false) => sign(content, &params.sender_secret_armored),
        (false, false) => Ok(content),
    }
}

/// Sign then encrypt: the recipient sees a signed message once decrypted.
pub fn sign_and_encrypt(
    content: MultiPart,
    secret_armored: &str,
    recipient_public_armored: &[String],
) -> Result<MultiPart> {
    let signed = sign(content, secret_armored)?;
    encrypt(&signed.formatted(), recipient_public_armored)
}

// ── Incoming (RFC 3156): decrypt + verify at read time ────────────────────────

/// The verdict on a message's detached signature.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignatureVerdict {
    /// The signature checked out against the correspondent's public key.
    pub valid: bool,
    /// Fingerprint of the key the signature was checked against (when known).
    pub fingerprint: Option<String>,
}

/// The result of unwrapping an incoming PGP/MIME (or inline-PGP) message: the
/// readable body plus what protection it carried.
#[derive(Debug, Clone, Default)]
pub struct IncomingPgp {
    pub body_html: Option<String>,
    pub body_text: Option<String>,
    /// The message was encrypted (and we managed to decrypt it).
    pub encrypted: bool,
    /// Present when the message (or its decrypted inner part) was signed.
    pub signed: Option<SignatureVerdict>,
}

/// Cheap pre-filter: does this raw message look like OpenPGP at all? Used by the
/// sync path to decide whether to preserve the raw MIME. A false positive only
/// costs a little storage; a false negative would drop the ciphertext, so the
/// markers are matched liberally (protocol strings AND armored block headers).
pub fn looks_like_pgp(raw: &[u8]) -> bool {
    let text = String::from_utf8_lossy(raw);
    text.contains("application/pgp-encrypted")
        || text.contains("application/pgp-signature")
        || text.contains("-----BEGIN PGP MESSAGE-----")
        || text.contains("-----BEGIN PGP SIGNED MESSAGE-----")
}

/// Unwrap an incoming message: decrypt it with any of the reader's `secrets`, and
/// verify a signature against the sender's `sender_public_armored` when present.
///
/// Handles the three shapes that reach a mailbox: PGP/MIME `multipart/encrypted`
/// (possibly wrapping a `multipart/signed`), PGP/MIME `multipart/signed`, and an
/// inline `-----BEGIN PGP MESSAGE-----` block in the body. Returns an empty
/// `IncomingPgp` (no body, no verdict) when the message is not actually PGP or
/// cannot be opened — the caller then keeps the stored body untouched.
pub fn parse_incoming(
    raw_mime: &[u8],
    secrets: &[String],
    sender_public_armored: Option<&str>,
) -> Result<IncomingPgp> {
    Ok(parse_entity(raw_mime, secrets, sender_public_armored))
}

/// The recursive core: one MIME entity (the whole message, or the inner entity a
/// decryption yielded). Peels an encryption layer, then a signature layer, then
/// extracts the body.
fn parse_entity(raw: &[u8], secrets: &[String], sender_public: Option<&str>) -> IncomingPgp {
    let text = String::from_utf8_lossy(raw);

    // ── Encryption layer ──────────────────────────────────────────────────────
    if text.contains("-----BEGIN PGP MESSAGE-----") {
        if let Some(armored) = extract_block(&text, "PGP MESSAGE") {
            for secret in secrets {
                if let Ok(plain) = pgp::decrypt(secret, &armored) {
                    // The plaintext is itself a MIME entity (PGP/MIME) or bare
                    // text (inline PGP) — recurse so a signed-then-encrypted
                    // message still surfaces its signature verdict.
                    let mut inner = parse_entity(&plain, secrets, sender_public);
                    inner.encrypted = true;
                    return inner;
                }
            }
            // Ciphertext we hold no key for: report it as encrypted, no body.
            return IncomingPgp { encrypted: true, ..Default::default() };
        }
    }

    // ── Signature layer (multipart/signed) ────────────────────────────────────
    if text.contains("-----BEGIN PGP SIGNATURE-----") {
        if let (Some(signed_part), Some(sig)) =
            (first_signed_part(raw), extract_block(&text, "PGP SIGNATURE"))
        {
            let verdict = verify_signed(&signed_part, &sig, sender_public);
            let mut inner = extract_body(&signed_part);
            inner.signed = Some(verdict);
            return inner;
        }
    }

    // ── Plain body ────────────────────────────────────────────────────────────
    extract_body(raw)
}

/// Extract the readable body of a MIME entity (or bare text) into html/text,
/// sanitising HTML with the shared e-mail policy.
fn extract_body(raw: &[u8]) -> IncomingPgp {
    use mail_parser::MessageParser;

    let parsed = MessageParser::default().parse(raw);
    let (html, text) = match &parsed {
        Some(p) => (
            p.body_html(0).map(|s| s.into_owned()),
            p.body_text(0).map(|s| s.into_owned()),
        ),
        None => (None, None),
    };
    let body_html = html.map(|h| crate::services::html_sanitize::sanitize_email_html(&h));
    // Content that is not a MIME entity (inline-PGP plaintext) parses to no body;
    // fall back to the raw bytes as text so the reader still sees something.
    let body_text = if body_html.is_none() && text.is_none() {
        Some(String::from_utf8_lossy(raw).trim().to_string())
    } else {
        text
    };
    IncomingPgp { body_html, body_text, encrypted: false, signed: None }
}

/// Verify a detached signature over the signed part against the sender's key.
///
/// The exact bytes a signer covered are ambiguous at the boundary: RFC 1847 says
/// the CRLF preceding the delimiter belongs to the boundary (so it is EXCLUDED),
/// yet some senders (lettre among them) sign the part INCLUDING that trailing
/// CRLF. `signed_part` is the lettre form (trailing CRLF kept); we try it, then
/// the RFC-strict form (one trailing CRLF removed), and accept either.
fn verify_signed(signed_part: &[u8], sig_armored: &str, sender_public: Option<&str>) -> SignatureVerdict {
    let Some(pubkey) = sender_public else {
        // No key on file for this correspondent: a signature is present but there
        // is nothing to check it against.
        return SignatureVerdict { valid: false, fingerprint: None };
    };
    // Reuse import_public to derive the fingerprint of the verifying key.
    let fingerprint = pgp::import_public(pubkey).ok().map(|(_, fp, _)| fp);

    let stripped: &[u8] = signed_part
        .strip_suffix(b"\r\n")
        .or_else(|| signed_part.strip_suffix(b"\n"))
        .unwrap_or(signed_part);
    let valid = [signed_part, stripped]
        .iter()
        .any(|candidate| pgp::verify_detached(pubkey, candidate, sig_armored).unwrap_or(false));

    SignatureVerdict { valid, fingerprint }
}

/// The armored block `-----BEGIN {kind}----- … -----END {kind}-----`, if present.
fn extract_block(text: &str, kind: &str) -> Option<String> {
    let begin = format!("-----BEGIN {kind}-----");
    let end = format!("-----END {kind}-----");
    let b = text.find(&begin)?;
    let e = text.find(&end)? + end.len();
    (e > b).then(|| text[b..e].to_string())
}

/// The `boundary=` of an entity's Content-Type header (handles a quoted value and
/// a value folded onto a continuation line).
fn boundary_of(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let header_end = text.find("\r\n\r\n").unwrap_or(text.len());
    let headers = &text[..header_end];
    let idx = headers.to_ascii_lowercase().find("boundary=")?;
    let rest = headers[idx + "boundary=".len()..].trim_start();
    let value = if let Some(stripped) = rest.strip_prefix('"') {
        stripped.split('"').next()?
    } else {
        rest.split(|c: char| c == ';' || c.is_whitespace()).next()?
    };
    (!value.is_empty()).then(|| value.to_string())
}

/// The raw bytes of the FIRST part of a `multipart/signed` entity — the content
/// the detached signature covers. Extracted between the first two boundary
/// delimiters, stripping only the CRLF that ends the OPENING delimiter line. The
/// trailing CRLF before the closing delimiter is kept: whether it is part of the
/// signed content is signer-dependent, so [`verify_signed`] tries it both ways.
/// For a message this module signed, this returns `content.formatted()` verbatim.
fn first_signed_part(raw: &[u8]) -> Option<Vec<u8>> {
    let boundary = boundary_of(raw)?;
    let delim = format!("--{boundary}");
    let positions = delimiter_positions(raw, delim.as_bytes());
    if positions.len() < 2 {
        return None;
    }
    let mut seg = &raw[positions[0] + delim.len()..positions[1]];
    // Drop the CRLF that terminates the opening "--boundary" delimiter line.
    if let Some(rest) = seg.strip_prefix(b"\r\n") {
        seg = rest;
    } else if let Some(rest) = seg.strip_prefix(b"\n") {
        seg = rest;
    }
    Some(seg.to_vec())
}

/// Byte offsets at which `delim` occurs at the start of a line.
fn delimiter_positions(raw: &[u8], delim: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if delim.is_empty() || raw.len() < delim.len() {
        return out;
    }
    for i in 0..=raw.len() - delim.len() {
        let at_line_start = i == 0 || raw[i - 1] == b'\n';
        if at_line_start && &raw[i..i + delim.len()] == delim {
            out.push(i);
        }
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::pgp;

    fn ascii(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn signed_part_verifies_and_is_embedded_verbatim() {
        let k = pgp::generate("Alice <alice@example.com>").expect("gen");
        let content = alternative_body("Bonjour", "<p>Bonjour</p>");
        let content_bytes = content.formatted();

        let signed = sign(content, &k.secret_armored).expect("sign");
        let signed_bytes = signed.formatted();
        let out = ascii(&signed_bytes);

        // RFC 3156 shape.
        assert!(out.contains("multipart/signed"));
        assert!(out.contains("protocol=\"application/pgp-signature\""));
        assert!(out.contains("micalg=\"pgp-sha256\""));
        assert!(out.contains("application/pgp-signature"));
        assert!(out.contains("BEGIN PGP SIGNATURE"));

        // The signed body appears verbatim in the multipart (same boundary) — so a
        // verifier extracting it gets exactly the bytes we signed.
        let needle = ascii(&content_bytes);
        assert!(out.contains(&needle), "signed body not embedded verbatim");

        // And the detached signature we produced actually verifies over those bytes.
        let sig_start = out.find("-----BEGIN PGP SIGNATURE-----").expect("sig start");
        let sig_end = out.find("-----END PGP SIGNATURE-----").expect("sig end") + "-----END PGP SIGNATURE-----".len();
        let sig = &out[sig_start..sig_end];
        assert!(pgp::verify_detached(&k.public_armored, &content_bytes, sig).expect("verify"));
    }

    #[test]
    fn encrypted_part_roundtrips() {
        let k = pgp::generate("Bob <bob@example.com>").expect("gen");
        let content = alternative_body("Secret", "<p>Secret</p>");
        let content_bytes = content.formatted();

        let enc = encrypt(&content_bytes, &[k.public_armored.clone()]).expect("encrypt");
        let out = ascii(&enc.formatted());

        assert!(out.contains("multipart/encrypted"));
        assert!(out.contains("protocol=\"application/pgp-encrypted\""));
        assert!(out.contains("Version: 1"));
        assert!(out.contains("application/octet-stream"));

        // Extract the armored message and decrypt it back to the original bytes.
        let msg_start = out.find("-----BEGIN PGP MESSAGE-----").expect("msg start");
        let msg_end = out.find("-----END PGP MESSAGE-----").expect("msg end") + "-----END PGP MESSAGE-----".len();
        let armored = &out[msg_start..msg_end];
        let decrypted = pgp::decrypt(&k.secret_armored, armored).expect("decrypt");
        assert_eq!(decrypted, content_bytes);
    }

    // ── Incoming (parse_incoming) ─────────────────────────────────────────────

    #[test]
    fn detects_pgp_shapes() {
        assert!(looks_like_pgp(b"Content-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\""));
        assert!(looks_like_pgp(b"...-----BEGIN PGP MESSAGE-----..."));
        assert!(!looks_like_pgp(b"Content-Type: text/plain\r\n\r\nhello"));
    }

    #[test]
    fn incoming_encrypted_roundtrips_to_body() {
        let k = pgp::generate("Dave <dave@example.com>").expect("gen");
        let content = alternative_body("Bonjour Dave", "<p>Bonjour Dave</p>");
        let enc = encrypt(&content.formatted(), &[k.public_armored.clone()]).expect("encrypt");
        let raw = enc.formatted();

        let out = parse_incoming(&raw, &[k.secret_armored.clone()], None).expect("parse");
        assert!(out.encrypted, "message reconnu comme chiffré");
        assert!(out.signed.is_none(), "pas de signature");
        assert!(out.body_html.as_deref().unwrap_or("").contains("Bonjour Dave"), "html: {:?}", out.body_html);
        assert_eq!(out.body_text.as_deref(), Some("Bonjour Dave"));
    }

    #[test]
    fn incoming_encrypted_without_key_reports_encrypted_no_body() {
        let k = pgp::generate("Eve <eve@example.com>").expect("gen");
        let content = alternative_body("secret", "<p>secret</p>");
        let enc = encrypt(&content.formatted(), &[k.public_armored.clone()]).expect("encrypt");
        let raw = enc.formatted();

        // No secret key on file → we know it is encrypted but cannot read it.
        let out = parse_incoming(&raw, &[], None).expect("parse");
        assert!(out.encrypted);
        assert!(out.body_html.is_none() && out.body_text.is_none());
    }

    #[test]
    fn incoming_signed_verifies_against_sender_key() {
        let k = pgp::generate("Frank <frank@example.com>").expect("gen");
        let content = alternative_body("Contrat", "<p>Contrat</p>");
        let signed = sign(content, &k.secret_armored).expect("sign");
        let raw = signed.formatted();

        let out = parse_incoming(&raw, &[], Some(&k.public_armored)).expect("parse");
        assert!(!out.encrypted);
        let verdict = out.signed.expect("verdict présent");
        assert!(verdict.valid, "signature valide");
        assert_eq!(verdict.fingerprint.as_deref(), Some(k.fingerprint.as_str()));
        assert!(out.body_html.as_deref().unwrap_or("").contains("Contrat"));
    }

    #[test]
    fn incoming_signed_flags_wrong_sender_key() {
        let signer = pgp::generate("Grace <grace@example.com>").expect("gen");
        let impostor = pgp::generate("Mallory <mallory@example.com>").expect("gen");
        let content = alternative_body("Ordre", "<p>Ordre</p>");
        let signed = sign(content, &signer.secret_armored).expect("sign");
        let raw = signed.formatted();

        // Verified against the WRONG public key → invalid, but body still shown.
        let out = parse_incoming(&raw, &[], Some(&impostor.public_armored)).expect("parse");
        let verdict = out.signed.expect("verdict présent");
        assert!(!verdict.valid, "signature invalide contre la mauvaise clé");
        assert!(out.body_html.as_deref().unwrap_or("").contains("Ordre"));
    }

    #[test]
    fn incoming_signed_then_encrypted_surfaces_both() {
        let k = pgp::generate("Heidi <heidi@example.com>").expect("gen");
        let content = alternative_body("Confidentiel", "<p>Confidentiel</p>");
        let sae = sign_and_encrypt(content, &k.secret_armored, &[k.public_armored.clone()]).expect("sae");
        let raw = sae.formatted();

        let out = parse_incoming(&raw, &[k.secret_armored.clone()], Some(&k.public_armored)).expect("parse");
        assert!(out.encrypted, "chiffré");
        let verdict = out.signed.expect("signé sous le chiffrement");
        assert!(verdict.valid, "signature interne valide");
        assert!(out.body_html.as_deref().unwrap_or("").contains("Confidentiel"));
    }
}
