//! OpenPGP primitives for the mail module — key generation, import, and the
//! encrypt / decrypt / sign / verify operations, built on rPGP (pure Rust, no C
//! bindings, RustCrypto backend — consistent with the rest of the module).
//!
//! This is the "server-side" half of the hybrid GPG design: the crypto runs in
//! the backend and the (armored) secret key is held encrypted at rest via
//! `MailCrypto`. It is deliberately independent of storage and HTTP so it can be
//! unit-tested in isolation, and later reused by a client-side E2E path.
//!
//! Everything crosses the boundary as ASCII-armored text: armored keys are what
//! users import/export, and armored messages/signatures are what PGP/MIME parts
//! carry.

use pgp::composed::{
    ArmorOptions, Deserializable, DetachedSignature, EncryptionCaps, KeyType, Message,
    MessageBuilder, SecretKeyParamsBuilder, SignedPublicKey, SignedSecretKey, SubkeyParamsBuilder,
};
use pgp::crypto::ecc_curve::ECCCurve;
use pgp::crypto::hash::HashAlgorithm;
use pgp::crypto::sym::SymmetricKeyAlgorithm;
use pgp::types::{Fingerprint, KeyDetails, Password, SignedUser};
use rand::thread_rng;

/// Anything that can go wrong here collapses to one opaque error: the HTTP layer
/// turns it into a generic failure and we never leak key material in a message.
#[derive(Debug)]
pub struct PgpError(pub String);

impl std::fmt::Display for PgpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pgp error")
    }
}
impl std::error::Error for PgpError {}

type Result<T> = std::result::Result<T, PgpError>;

fn err<E: std::fmt::Display>(e: E) -> PgpError {
    PgpError(e.to_string())
}

/// A freshly generated or imported identity, in the armored form we persist.
pub struct KeyMaterial {
    pub public_armored: String,
    pub secret_armored: String,
    pub fingerprint: String,
    /// Lower-case e-mail addresses found in the key's user IDs.
    pub emails: Vec<String>,
}

/// Uppercase hex of a fingerprint, no spaces — the stable identifier we store.
/// Robust to whatever `Fingerprint`'s Display happens to be by keeping hex only.
fn fp_hex(fp: Fingerprint) -> String {
    fp.to_string()
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_uppercase()
}

// ── Generation ────────────────────────────────────────────────────────────────

/// Generate a modern Ed25519/Curve25519 identity for `user_id` (e.g.
/// `"Jane Doe <jane@example.com>"`). The primary key certifies and signs; a
/// single Curve25519 subkey encrypts — the layout every OpenPGP client expects.
pub fn generate(user_id: &str) -> Result<KeyMaterial> {
    let mut enckey = SubkeyParamsBuilder::default();
    enckey
        .key_type(KeyType::ECDH(ECCCurve::Curve25519Legacy))
        .can_sign(false)
        .can_encrypt(EncryptionCaps::All)
        .can_authenticate(false);

    let mut params = SecretKeyParamsBuilder::default();
    params
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(true)
        .can_encrypt(EncryptionCaps::None)
        .primary_user_id(user_id.to_string())
        .subkeys(vec![enckey.build().map_err(err)?]);

    let secret = params
        .build()
        .map_err(err)?
        .generate(thread_rng())
        .map_err(err)?;
    material_from_secret(&secret)
}

// ── Import ────────────────────────────────────────────────────────────────────

/// Import an armored secret key. A passphrase-protected key is accepted; we keep
/// it as given and rely on `MailCrypto` for protection at rest. (Deeper passphrase
/// handling — re-locking / per-session unlock — comes with the E2E wave.)
pub fn import_secret(armored: &str, _passphrase: Option<&str>) -> Result<KeyMaterial> {
    let (secret, _) = SignedSecretKey::from_string(armored).map_err(err)?;
    secret.verify_bindings().map_err(err)?;
    material_from_secret(&secret)
}

/// Import an armored public key (a correspondent's certificate). Returns the
/// re-serialized armor, its fingerprint, and the e-mail addresses it covers.
pub fn import_public(armored: &str) -> Result<(String, String, Vec<String>)> {
    let (public, _) = SignedPublicKey::from_string(armored).map_err(err)?;
    public.verify_bindings().map_err(err)?;
    let fp = fp_hex(public.fingerprint());
    let emails = emails_of(&public.details.users);
    let re_armored = public.to_armored_string(ArmorOptions::default()).map_err(err)?;
    Ok((re_armored, fp, emails))
}

/// Import a public key from its BINARY (non-armored) OpenPGP serialization —
/// the form a Web Key Directory serves and an Autocrypt `keydata` attribute
/// carries (after base64-decoding). Returns the same triple as
/// [`import_public`]: re-armored certificate, fingerprint, covered e-mails.
pub fn import_public_bytes(bytes: &[u8]) -> Result<(String, String, Vec<String>)> {
    let public = SignedPublicKey::from_bytes(bytes).map_err(err)?;
    public.verify_bindings().map_err(err)?;
    let fp = fp_hex(public.fingerprint());
    let emails = emails_of(&public.details.users);
    let re_armored = public.to_armored_string(ArmorOptions::default()).map_err(err)?;
    Ok((re_armored, fp, emails))
}

/// Serialize an armored public certificate to its BINARY OpenPGP form — what a
/// Web Key Directory must serve (`application/octet-stream`) and what an
/// Autocrypt `keydata` attribute base64-encodes. The full certificate is
/// emitted (no Autocrypt-style minimization); a full public key is still valid
/// Autocrypt.
pub fn public_armored_to_binary(public_armored: &str) -> Result<Vec<u8>> {
    use pgp::ser::Serialize;
    let (public, _) = SignedPublicKey::from_string(public_armored).map_err(err)?;
    public.to_bytes().map_err(err)
}

fn material_from_secret(secret: &SignedSecretKey) -> Result<KeyMaterial> {
    let public = SignedPublicKey::from(secret.clone());
    Ok(KeyMaterial {
        public_armored: public.to_armored_string(ArmorOptions::default()).map_err(err)?,
        secret_armored: secret.to_armored_string(ArmorOptions::default()).map_err(err)?,
        fingerprint: fp_hex(secret.fingerprint()),
        emails: emails_of(&secret.details.users),
    })
}

/// Lower-case e-mail addresses extracted from a key's User ID packets.
fn emails_of(users: &[SignedUser]) -> Vec<String> {
    let mut out = Vec::new();
    for u in users {
        let Some(id) = u.id.as_str() else { continue };
        // "Name <addr@host>" → addr@host ; a bare address is taken as-is.
        if let (Some(a), Some(b)) = (id.rfind('<'), id.rfind('>')) {
            if a < b {
                out.push(id[a + 1..b].trim().to_lowercase());
                continue;
            }
        }
        if id.contains('@') {
            out.push(id.trim().to_lowercase());
        }
    }
    out.sort();
    out.dedup();
    out
}

// ── Encryption / decryption ───────────────────────────────────────────────────

/// Encrypt `plaintext` to one or more recipient public keys, returning an armored
/// OpenPGP message (the body of a PGP/MIME `multipart/encrypted` part).
pub fn encrypt(recipient_public_armored: &[String], plaintext: &[u8]) -> Result<String> {
    if recipient_public_armored.is_empty() {
        return Err(PgpError("no recipients".into()));
    }
    let certs: Vec<SignedPublicKey> = recipient_public_armored
        .iter()
        .map(|a| SignedPublicKey::from_string(a).map(|(k, _)| k).map_err(err))
        .collect::<Result<_>>()?;

    let mut builder = MessageBuilder::from_bytes("", plaintext.to_vec())
        .seipd_v1(thread_rng(), SymmetricKeyAlgorithm::AES256);
    for cert in &certs {
        if let Some(sub) = cert.public_subkeys.iter().find(|k| k.algorithm().can_encrypt()) {
            builder.encrypt_to_key(thread_rng(), sub).map_err(err)?;
        } else if cert.primary_key.algorithm().can_encrypt() {
            builder.encrypt_to_key(thread_rng(), &cert.primary_key).map_err(err)?;
        } else {
            return Err(PgpError("recipient key cannot encrypt".into()));
        }
    }
    builder
        .to_armored_string(thread_rng(), ArmorOptions::default())
        .map_err(err)
}

/// Decrypt an armored OpenPGP message with our (unlocked-at-rest) secret key.
pub fn decrypt(secret_armored: &str, armored_message: &str) -> Result<Vec<u8>> {
    let (secret, _) = SignedSecretKey::from_string(secret_armored).map_err(err)?;
    let (msg, _) = Message::from_string(armored_message).map_err(err)?;
    let mut dec = msg.decrypt(&Password::empty(), &secret).map_err(err)?;
    if dec.is_compressed() {
        dec = dec.decompress().map_err(err)?;
    }
    dec.as_data_vec().map_err(err)
}

// ── Detached signatures (for PGP/MIME multipart/signed, wave 2) ───────────────

/// Produce an armored detached signature over `data`, signed by the primary key.
pub fn sign_detached(secret_armored: &str, data: &[u8]) -> Result<String> {
    let (secret, _) = SignedSecretKey::from_string(secret_armored).map_err(err)?;
    let sig = DetachedSignature::sign_binary_data(
        thread_rng(),
        &secret.primary_key,
        &Password::empty(),
        HashAlgorithm::Sha256,
        data,
    )
    .map_err(err)?;
    sig.to_armored_string(ArmorOptions::default()).map_err(err)
}

/// Verify an armored detached signature over `data` against a public key.
pub fn verify_detached(public_armored: &str, data: &[u8], signature_armored: &str) -> Result<bool> {
    let (public, _) = SignedPublicKey::from_string(public_armored).map_err(err)?;
    let (sig, _) = DetachedSignature::from_string(signature_armored).map_err(err)?;
    Ok(sig.verify(&public, data).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_encrypt_decrypt() {
        let alice = generate("Alice <alice@example.com>").expect("gen");
        assert!(alice.fingerprint.len() >= 40, "fp={}", alice.fingerprint);
        assert_eq!(alice.emails, vec!["alice@example.com".to_string()]);

        let plaintext = b"Bonjour, ceci est un secret.";
        let ct = encrypt(std::slice::from_ref(&alice.public_armored), plaintext).expect("encrypt");
        assert!(ct.contains("BEGIN PGP MESSAGE"));

        let pt = decrypt(&alice.secret_armored, &ct).expect("decrypt");
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn sign_then_verify() {
        let bob = generate("Bob <bob@example.com>").expect("gen");
        let data = b"contract terms";
        let sig = sign_detached(&bob.secret_armored, data).expect("sign");
        assert!(sig.contains("BEGIN PGP SIGNATURE"));
        assert!(verify_detached(&bob.public_armored, data, &sig).expect("verify"));
        assert!(!verify_detached(&bob.public_armored, b"tampered", &sig).expect("verify2"));
    }

    #[test]
    fn import_roundtrips_public() {
        let k = generate("Carol <carol@example.com>").expect("gen");
        let (re, fp, emails) = import_public(&k.public_armored).expect("import pub");
        assert_eq!(fp, k.fingerprint);
        assert_eq!(emails, vec!["carol@example.com".to_string()]);
        assert!(re.contains("BEGIN PGP PUBLIC KEY BLOCK"));
    }
}
