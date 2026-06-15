use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use anyhow::{Context, Result};

pub struct MailCrypto {
    cipher: Aes256Gcm,
}

impl MailCrypto {
    pub fn new(key_hex: &str) -> Result<Self> {
        let key_bytes = hex::decode(key_hex).context("Clé de chiffrement invalide (hex requis)")?;
        anyhow::ensure!(key_bytes.len() == 32, "Clé de chiffrement : 32 octets requis");
        let cipher = Aes256Gcm::new_from_slice(&key_bytes)
            .map_err(|_| anyhow::anyhow!("Initialisation AES-256-GCM échouée"))?;
        Ok(Self { cipher })
    }

    pub fn encrypt(&self, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>)> {
        use aes_gcm::aead::rand_core::RngCore;
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self.cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| anyhow::anyhow!("Chiffrement échoué"))?;
        Ok((ciphertext, nonce_bytes.to_vec()))
    }

    pub fn decrypt(&self, ciphertext: &[u8], nonce_bytes: &[u8]) -> Result<String> {
        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = self.cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| anyhow::anyhow!("Déchiffrement échoué"))?;
        String::from_utf8(plaintext).context("Données déchiffrées invalides (UTF-8)")
    }
}
