//! Loading a DKIM signing key from the database and applying it.
//!
//! This is the DB+crypto glue between `dkim` (pure signing) and the worker: it
//! finds the key for a message's From domain, decrypts its private half with the
//! module's `MailCrypto`, and signs. Signing happens in the worker (which holds
//! the crypto key) rather than at enqueue, over the exact bytes about to go on
//! the wire.

use sqlx::PgPool;

use super::dkim::{self, SigningKey};
use crate::services::crypto::MailCrypto;

/// One row of `mail.dkim_keys` as this module reads it: selector, algorithm,
/// and the encrypted private key with its AES-GCM nonce.
type StoredKey = (String, String, Vec<u8>, Vec<u8>);

/// The outcome of trying to DKIM-sign a message.
///
/// The absence of a signature used to be silent — the message simply came back
/// unchanged, indistinguishable from a signed one. It is reported explicitly so
/// the worker can honour `dkim_require_signature` and defer instead of putting
/// unsigned mail on the wire.
pub enum Signed {
    /// The bytes carry a `DKIM-Signature` header.
    Signed(Vec<u8>),
    /// The original bytes, plus why they could not be signed. Still deliverable:
    /// it is the administrator's policy, not this function, that decides whether
    /// sending unsigned is acceptable.
    Unsigned { raw: Vec<u8>, reason: String },
}

/// Signs `raw` with the key configured for its `From:` domain, if any.
///
/// A message whose domain has no key comes back as `Unsigned` — DKIM is the
/// administrator's choice to configure, not a hard requirement to send, unless
/// they turned `dkim_require_signature` on. A key that is present but fails to
/// load or sign logs the error and yields `Unsigned` rather than dropping the
/// message.
pub async fn sign(db: &PgPool, crypto: &MailCrypto, raw: &[u8]) -> Signed {
    let unsigned = |reason: String| Signed::Unsigned { raw: raw.to_vec(), reason };

    let Some(domain) = from_domain(raw) else {
        return unsigned("le message n'a pas d'en-tête From: exploitable".to_string());
    };

    let row: Result<Option<StoredKey>, _> = sqlx::query_as(
        "SELECT selector, algorithm, private_key_enc, private_key_nonce \
         FROM mail.dkim_keys WHERE domain = $1",
    )
    .bind(domain.to_ascii_lowercase())
    .fetch_optional(db)
    .await;

    let row = match row {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(error = %e, domain, "DKIM : lecture de la clé impossible");
            return unsigned(format!("lecture de la clé DKIM de {domain} impossible"));
        }
    };

    let Some((selector, algorithm, enc, nonce)) = row else {
        return unsigned(format!("aucune clé DKIM configurée pour le domaine {domain}"));
    };

    // Never log the key material, nor the reason for a decryption failure beyond
    // the error type: this is a credential.
    let private_key = match crypto.decrypt(&enc, &nonce) {
        Ok(pem) => pem,
        Err(e) => {
            tracing::error!(error = %e, domain, "DKIM : déchiffrement de la clé impossible");
            return unsigned(format!("clé DKIM de {domain} illisible"));
        }
    };

    let key = SigningKey { domain: domain.to_string(), selector, algorithm, private_key };
    match dkim::sign(raw, &key) {
        Ok(signed) => Signed::Signed(signed),
        Err(e) => {
            tracing::error!(error = %e, domain, "DKIM : signature échouée");
            unsigned(format!("signature DKIM de {domain} échouée"))
        }
    }
}

/// The domain of the `From:` header (RFC 5322), which is what DMARC aligns on and
/// therefore what we sign for. Scans the header block only.
fn from_domain(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let head_end = text.find("\r\n\r\n").or_else(|| text.find("\n\n")).unwrap_or(text.len());
    let headers = &text[..head_end];

    let mut value = String::new();
    let mut in_from = false;
    for line in headers.split('\n') {
        let line = line.trim_end_matches('\r');
        if in_from {
            // Folded continuation lines start with whitespace.
            if line.starts_with(' ') || line.starts_with('\t') {
                value.push_str(line.trim());
                continue;
            }
            break;
        }
        if let Some(rest) = line.get(..5) {
            if rest.eq_ignore_ascii_case("from:") {
                value.push_str(line[5..].trim());
                in_from = true;
            }
        }
    }
    if value.is_empty() {
        return None;
    }
    // From may be "Name <local@domain>" or "local@domain".
    let addr = value.rsplit_once('<').map(|(_, a)| a.trim_end_matches('>')).unwrap_or(&value);
    addr.rsplit_once('@').map(|(_, d)| d.trim().trim_end_matches('>').to_string())
}

#[cfg(test)]
mod tests {
    use super::from_domain;

    #[test]
    fn extracts_from_domain() {
        assert_eq!(from_domain(b"From: Alice <alice@example.com>\r\nTo: x\r\n\r\nbody").as_deref(), Some("example.com"));
        assert_eq!(from_domain(b"From: bob@sub.example.org\r\n\r\n").as_deref(), Some("sub.example.org"));
        assert_eq!(from_domain(b"Subject: no from\r\n\r\n"), None);
    }

    #[test]
    fn only_scans_the_header_block() {
        // A "From:" in the body must not be mistaken for the header.
        assert_eq!(from_domain(b"Subject: x\r\n\r\nFrom: evil@attacker.example"), None);
    }
}
