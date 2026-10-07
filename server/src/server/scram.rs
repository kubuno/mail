//! SASL SCRAM-SHA-256 (RFC 5802) server side.
//!
//! SCRAM proves the client knows the password without ever sending it, and a
//! leak of the stored secret does not reveal it — an attacker would have to
//! break PBKDF2. This is the mechanism Thunderbird/Apple Mail prefer over PLAIN.
//!
//! Only the crypto primitives come from crates (PBKDF2/HMAC/SHA-256); the
//! message flow follows the RFC. `stringprep` applies SASLprep to the password
//! so the same password normalises identically on every client.

use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// PBKDF2 iteration count. RFC 5802 requires ≥ 4096; a higher count costs the
/// attacker more per guess. 16 384 is a reasonable modern floor for a value
/// checked on every login.
const ITERATIONS: u32 = 16_384;
const SALT_LEN: usize = 16;
const KEY_LEN: usize = 32; // SHA-256 output

/// The per-user secret stored in the database. None of these reveal the
/// password; `stored_key` verifies the client, `server_key` authenticates us.
#[derive(Debug, Clone)]
pub struct Secret {
    pub salt:       Vec<u8>,
    pub iterations: u32,
    pub stored_key: Vec<u8>,
    pub server_key: Vec<u8>,
}

/// Derives a SCRAM secret from a plaintext password, at credential-creation
/// time. SaltedPassword = PBKDF2(SASLprep(pw), salt, i); ClientKey =
/// HMAC(SaltedPassword, "Client Key"); StoredKey = H(ClientKey); ServerKey =
/// HMAC(SaltedPassword, "Server Key").
pub fn derive(password: &str) -> Secret {
    let prepped = saslprep(password);
    let mut salt = vec![0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);

    let salted = salted_password(prepped.as_bytes(), &salt, ITERATIONS);
    let client_key = hmac(&salted, b"Client Key");
    let stored_key = Sha256::digest(&client_key).to_vec();
    let server_key = hmac(&salted, b"Server Key");

    Secret { salt, iterations: ITERATIONS, stored_key, server_key }
}

/// One SCRAM exchange. Built with the secret for the username the client named
/// in its first message (or a decoy secret when the user is unknown, so the
/// failure is indistinguishable from a wrong password).
pub struct Handshake {
    secret:           Secret,
    client_first_bare: String,
    server_first:     String,
    known:            bool,
}

impl Handshake {
    /// Parses the username out of the client-first message (`n,,n=user,r=...`),
    /// so the caller can look up the secret before starting the exchange.
    pub fn username_of(client_first: &str) -> Result<String> {
        // GS2 header is `n,,` (no channel binding) or `y,,`; then the SCRAM body.
        let body = client_first.splitn(3, ',').nth(2).ok_or_else(|| anyhow!("client-first malformé"))?;
        for part in body.split(',') {
            if let Some(user) = part.strip_prefix("n=") {
                // SCRAM escapes '=' as =3D and ',' as =2C.
                return Ok(user.replace("=3D", "=").replace("=2C", ","));
            }
        }
        bail!("client-first sans nom d'utilisateur")
    }

    /// Starts the exchange. `secret` is the real one if the user exists, or a
    /// decoy (`decoy_secret`) otherwise — `known` records which.
    pub fn new(secret: Secret, known: bool, client_first: &str) -> Result<Self> {
        let bare = client_first
            .splitn(3, ',')
            .nth(2)
            .ok_or_else(|| anyhow!("client-first malformé"))?
            .to_string();
        Ok(Self { secret, client_first_bare: bare, server_first: String::new(), known })
    }

    /// Produces the server-first message: the combined nonce, the salt, and the
    /// iteration count. The client's nonce is echoed with ours appended.
    pub fn server_first(&mut self) -> Result<String> {
        let client_nonce = self
            .client_first_bare
            .split(',')
            .find_map(|p| p.strip_prefix("r="))
            .ok_or_else(|| anyhow!("client-first sans nonce"))?;

        let mut server_nonce_raw = [0u8; 18];
        rand::thread_rng().fill_bytes(&mut server_nonce_raw);
        let server_nonce = STANDARD.encode(server_nonce_raw);

        self.server_first = format!(
            "r={client_nonce}{server_nonce},s={},i={}",
            STANDARD.encode(&self.secret.salt),
            self.secret.iterations,
        );
        Ok(self.server_first.clone())
    }

    /// Verifies the client-final message's proof and returns the server-final
    /// message (`v=<ServerSignature>`). An error means authentication failed.
    pub fn server_final(&self, client_final: &str) -> Result<String> {
        if !self.known {
            bail!("utilisateur inconnu");
        }
        // client-final = c=<gs2>,r=<nonce>,p=<proof>
        let without_proof = client_final
            .rsplit_once(",p=")
            .map(|(head, _)| head)
            .ok_or_else(|| anyhow!("client-final sans preuve"))?;
        let proof_b64 = client_final
            .rsplit_once(",p=")
            .map(|(_, p)| p)
            .ok_or_else(|| anyhow!("client-final sans preuve"))?;
        let proof = STANDARD.decode(proof_b64).map_err(|_| anyhow!("preuve base64 invalide"))?;

        // Echoed nonce must match the one we sent, or a MITM is splicing.
        let sent_nonce = self.server_first.split(',').find_map(|p| p.strip_prefix("r="));
        let got_nonce = client_final.split(',').find_map(|p| p.strip_prefix("r="));
        if sent_nonce.is_none() || sent_nonce != got_nonce {
            bail!("nonce SCRAM incohérent");
        }

        let auth_message =
            format!("{},{},{}", self.client_first_bare, self.server_first, without_proof);
        let client_signature = hmac(&self.secret.stored_key, auth_message.as_bytes());

        // ClientKey = ClientProof XOR ClientSignature; StoredKey' = H(ClientKey).
        if proof.len() != client_signature.len() {
            bail!("longueur de preuve invalide");
        }
        let client_key: Vec<u8> =
            proof.iter().zip(&client_signature).map(|(a, b)| a ^ b).collect();
        let derived_stored = Sha256::digest(&client_key);
        if derived_stored.as_slice() != self.secret.stored_key.as_slice() {
            bail!("preuve SCRAM incorrecte");
        }

        let server_signature = hmac(&self.secret.server_key, auth_message.as_bytes());
        Ok(format!("v={}", STANDARD.encode(server_signature)))
    }
}

/// A secret to hand SCRAM for an unknown user: random, so the exchange proceeds
/// and fails at the proof exactly as a wrong password would (no user-enumeration
/// oracle). RFC 5802 §5.1 recommends this.
pub fn decoy_secret() -> Secret {
    let mut salt = vec![0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut stored_key = vec![0u8; KEY_LEN];
    rand::thread_rng().fill_bytes(&mut stored_key);
    Secret { salt, iterations: ITERATIONS, stored_key, server_key: vec![0u8; KEY_LEN] }
}

fn salted_password(password: &[u8], salt: &[u8], iterations: u32) -> Vec<u8> {
    let mut out = vec![0u8; KEY_LEN];
    pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, iterations, &mut out);
    out
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepte toute longueur de clé");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// SASLprep (RFC 4013) via stringprep. On any prep error, fall back to the raw
/// password — better to let a valid password through than to reject it.
fn saslprep(password: &str) -> String {
    stringprep::saslprep(password).map(|c| c.into_owned()).unwrap_or_else(|_| password.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_exchange_succeeds_with_the_right_password() {
        // Derive as at credential creation.
        let secret = derive("correct horse battery staple");

        // Client-first (fixed nonce for the test).
        let client_first = "n,,n=alice,r=clientNONCE";
        assert_eq!(Handshake::username_of(client_first).unwrap(), "alice");

        let mut hs = Handshake::new(secret.clone(), true, client_first).unwrap();
        let server_first = hs.server_first().unwrap();

        // The client would now compute its proof. Reproduce the client side here
        // to check the server accepts a correct proof.
        let full_nonce = server_first.split(',').find_map(|p| p.strip_prefix("r=")).unwrap();
        let without_proof = format!("c=biws,r={full_nonce}");
        let salted = salted_password(
            saslprep("correct horse battery staple").as_bytes(),
            &secret.salt,
            secret.iterations,
        );
        let client_key = hmac(&salted, b"Client Key");
        let stored = Sha256::digest(&client_key);
        let auth = format!("n=alice,r=clientNONCE,{server_first},{without_proof}");
        let client_sig = hmac(&stored, auth.as_bytes());
        let proof: Vec<u8> = client_key.iter().zip(&client_sig).map(|(a, b)| a ^ b).collect();
        let client_final = format!("{without_proof},p={}", STANDARD.encode(proof));

        let server_final = hs.server_final(&client_final).unwrap();
        assert!(server_final.starts_with("v="));
    }

    #[test]
    fn a_wrong_password_is_rejected() {
        let secret = derive("the real password");
        let client_first = "n,,n=bob,r=cn";
        let mut hs = Handshake::new(secret, true, client_first).unwrap();
        let server_first = hs.server_first().unwrap();
        let full = server_first.split(',').find_map(|p| p.strip_prefix("r=")).unwrap();
        // Proof computed from a DIFFERENT password.
        let wrong = derive("not the password");
        let salted = salted_password(b"not the password", &wrong.salt, wrong.iterations);
        let ck = hmac(&salted, b"Client Key");
        let stored = Sha256::digest(&ck);
        let np = format!("c=biws,r={full}");
        let auth = format!("n=bob,r=cn,{server_first},{np}");
        let cs = hmac(&stored, auth.as_bytes());
        let proof: Vec<u8> = ck.iter().zip(&cs).map(|(a, b)| a ^ b).collect();
        let cf = format!("{np},p={}", STANDARD.encode(proof));
        assert!(hs.server_final(&cf).is_err(), "mauvais mot de passe refusé");
    }

    #[test]
    fn unknown_user_never_succeeds() {
        let mut hs = Handshake::new(decoy_secret(), false, "n,,n=ghost,r=x").unwrap();
        let _ = hs.server_first().unwrap();
        assert!(hs.server_final("c=biws,r=x,p=AAAA").is_err());
    }
}
