//! Who may connect to the served protocols.
//!
//! A mailbox credential is a dedicated secret, not the Kubuno password: mail
//! clients keep what they are given for years and hand it to whatever host they
//! are pointed at. Revoking one costs the user their mail client, nothing else.

use anyhow::{Context, Result};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use kubuno_db::dialect::Assign;
use kubuno_db::DbPool;
use uuid::Uuid;

/// An authenticated mailbox: the Kubuno user behind it and the address used.
#[derive(Debug, Clone)]
pub struct Mailbox {
    pub user_id:  Uuid,
    pub username: String,
}

/// Hashes a mailbox password (Argon2id, per-credential salt).
pub fn hash_password(plain: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("Hachage du mot de passe: {e}"))
}

/// Verifies a login. Returns `None` for both "no such mailbox" and "wrong
/// password": the protocols must not let a caller tell those apart, or the
/// login prompt becomes a way to enumerate addresses.
pub async fn authenticate(db: &DbPool, username: &str, password: &str) -> Option<Mailbox> {
    let username = username.trim().to_ascii_lowercase();
    if username.is_empty() || password.is_empty() {
        return None;
    }

    let (credential_id, user_id, hash) = crate::db::query_as::<(Uuid, Uuid, String)>(
        "SELECT id, user_id, password_hash FROM mail.mailbox_credentials WHERE username = $1",
    )
    .bind(&username)
    .fetch_optional(db)
    .await
    .map_err(|e| tracing::error!(error = %e, "Lecture des identifiants de boîte"))
    .ok()
    .flatten()?;

    let parsed = PasswordHash::new(&hash)
        .map_err(|e| tracing::error!(error = %e, "Empreinte de mot de passe illisible"))
        .ok()?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .ok()?;

    // Best effort: knowing when a credential was last used is what tells an
    // administrator which ones are dead weight.
    if let Err(e) = crate::db::query("UPDATE mail.mailbox_credentials SET last_used_at = $1 WHERE id = $2")
        .bind(chrono::Utc::now())
        .bind(credential_id)
        .execute(db)
        .await
    {
        tracing::error!(error = %e, "Horodatage d'utilisation de l'identifiant");
    }

    Some(Mailbox { user_id, username })
}

/// Creates (or replaces) the credential for one address. Returns its id; the
/// plaintext is the caller's to show once and forget.
pub async fn upsert_credential(
    db: &DbPool,
    user_id: Uuid,
    username: &str,
    password: &str,
    label: Option<&str>,
) -> Result<Uuid> {
    let username = username.trim().to_ascii_lowercase();
    if !username.contains('@') {
        anyhow::bail!("L'identifiant doit être une adresse e-mail");
    }
    if password.chars().count() < 12 {
        anyhow::bail!("Le mot de passe de boîte doit faire au moins 12 caractères");
    }

    let hash = hash_password(password)?;
    // Also derive the SCRAM-SHA-256 secret now, while the plaintext is in hand:
    // SCRAM auth (preferred by modern clients) never sees the password again.
    let scram = super::scram::derive(password);
    // The id is generated here and only used when the row is new; on a
    // conflict the existing row keeps its id, which is re-read below (no
    // RETURNING on MySQL).
    let upsert = db.backend().upsert(
        "mail.mailbox_credentials",
        &["username"],
        &[
            Assign::Incoming("password_hash"),
            Assign::Incoming("label"),
            Assign::Incoming("user_id"),
            Assign::Incoming("scram_salt"),
            Assign::Incoming("scram_iterations"),
            Assign::Incoming("scram_stored_key"),
            Assign::Incoming("scram_server_key"),
            Assign::Expr { col: "last_used_at", expr: "NULL" },
        ],
    );
    crate::db::query(format!(
        "INSERT INTO mail.mailbox_credentials
             (id, user_id, username, password_hash, label,
              scram_salt, scram_iterations, scram_stored_key, scram_server_key)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9){upsert}"
    ))
    .bind(kubuno_db::new_id())
    .bind(user_id)
    .bind(&username)
    .bind(&hash)
    .bind(label)
    .bind(&scram.salt)
    .bind(scram.iterations as i32)
    .bind(&scram.stored_key)
    .bind(&scram.server_key)
    .execute(db)
    .await
    .context("Enregistrement de l'identifiant de boîte")?;

    let id: Uuid =
        crate::db::query_scalar("SELECT id FROM mail.mailbox_credentials WHERE username = $1")
            .bind(&username)
            .fetch_one(db)
            .await
            .context("Relecture de l'identifiant de boîte")?;

    Ok(id)
}

/// The four nullable SCRAM columns, as read from the row.
type ScramColumns = (Option<Vec<u8>>, Option<i32>, Option<Vec<u8>>, Option<Vec<u8>>);

/// Loads the SCRAM secret for a username, for the SCRAM-SHA-256 auth path.
/// Returns `None` when the mailbox has no SCRAM secret (created before SCRAM
/// existed, or unknown user) — the caller then uses a decoy so the failure is
/// indistinguishable.
pub async fn scram_secret(db: &DbPool, username: &str) -> Option<super::scram::Secret> {
    let username = username.trim().to_ascii_lowercase();
    let row: Option<ScramColumns> = crate::db::query_as(
        "SELECT scram_salt, scram_iterations, scram_stored_key, scram_server_key \
         FROM mail.mailbox_credentials WHERE username = $1",
    )
    .bind(&username)
    .fetch_optional(db)
    .await
    .map_err(|e| tracing::error!(error = %e, "Lecture du secret SCRAM"))
    .ok()
    .flatten();

    match row {
        Some((Some(salt), Some(iter), Some(stored), Some(server))) => Some(super::scram::Secret {
            salt,
            iterations: iter as u32,
            stored_key: stored,
            server_key: server,
        }),
        _ => None,
    }
}

/// The mailbox behind a username, once SCRAM has proven the client knows the
/// password. A separate lookup because SCRAM verifies against the stored keys,
/// not through `authenticate` (which needs the plaintext).
pub async fn mailbox_of(db: &DbPool, username: &str) -> Option<Mailbox> {
    let username = username.trim().to_ascii_lowercase();
    let user_id: Option<Uuid> = crate::db::query_scalar(
        "SELECT user_id FROM mail.mailbox_credentials WHERE username = $1",
    )
    .bind(&username)
    .fetch_optional(db)
    .await
    .map_err(|e| tracing::error!(error = %e, "Lecture de la boîte authentifiée"))
    .ok()
    .flatten();

    user_id.map(|user_id| Mailbox { user_id, username })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_round_trips() {
        let hash = hash_password("correct horse battery").expect("hash");
        let parsed = PasswordHash::new(&hash).expect("parse");
        assert!(Argon2::default()
            .verify_password(b"correct horse battery", &parsed)
            .is_ok());
        assert!(Argon2::default().verify_password(b"wrong", &parsed).is_err());
    }

    #[test]
    fn each_hash_carries_its_own_salt() {
        let a = hash_password("same password").expect("hash");
        let b = hash_password("same password").expect("hash");
        assert_ne!(a, b);
    }
}
