use uuid::Uuid;

use crate::{
    errors::MailError,
    models::{EmailMessage, SendMailDto},
    services::crypto::MailCrypto,
    state::AppState,
};

/// Decrypt / verify a message IN PLACE when it was stored as PGP/MIME (its raw
/// MIME preserved by the sync path) and the reader has OpenPGP enabled with a
/// usable key. On any failure the stored (opaque) body is left untouched — a mail
/// you cannot open must still open, showing the ciphertext, not an error.
///
/// Shared by the single-message and thread endpoints, since a message card is
/// populated from either. Skips the whole path — including the config lookup —
/// for ordinary mail, whose `pgp_raw` is NULL.
pub async fn decode_pgp_in_place(state: &AppState, user_id: Uuid, msg: &mut EmailMessage) {
    // Preserved raw MIME? Only PGP messages have it; everything else exits here.
    let raw: Option<Vec<u8>> = sqlx::query_scalar::<_, Option<Vec<u8>>>(
        "SELECT pgp_raw FROM mail.messages WHERE id = $1",
    )
    .bind(msg.id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .flatten();
    let Some(raw) = raw else { return };

    // Honour the instance switch: OpenPGP off ⇒ leave the message as delivered.
    match crate::handlers::addresses::server_config(state).await {
        Ok(cfg) if cfg.gpg_enabled => {}
        _ => return,
    }

    let Ok(crypto) = MailCrypto::new(&state.settings.mail.encryption_key) else { return };

    // Every one of the reader's secret keys — the message may be encrypted to any
    // of them; decryption tries each until one opens it.
    let key_rows = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
        "SELECT private_key, private_key_nonce FROM mail.pgp_keys WHERE user_id = $1 ORDER BY is_default DESC, created_at",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let secrets: Vec<String> = key_rows
        .iter()
        .filter_map(|(enc, nonce)| crypto.decrypt(enc, nonce).ok())
        .collect();

    // The sender's public key (for signature verification), by their From address.
    let sender_public: Option<String> = sqlx::query_scalar(
        "SELECT public_key FROM mail.pgp_contacts WHERE user_id = $1 AND lower(email) = lower($2) LIMIT 1",
    )
    .bind(user_id)
    .bind(&msg.from_email)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let parsed = match crate::services::pgp_mime::parse_incoming(&raw, &secrets, sender_public.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(msg = %msg.id, error = %e, "Déchiffrement PGP entrant échoué");
            return;
        }
    };

    // Surface the decrypted body when we recovered one; keep the stored body
    // otherwise (e.g. encrypted with a key we do not hold).
    if parsed.body_html.is_some() {
        msg.body_html = parsed.body_html;
    }
    if parsed.body_text.is_some() {
        msg.body_text = parsed.body_text;
    }
    msg.pgp_encrypted = parsed.encrypted;
    if let Some(verdict) = parsed.signed {
        msg.pgp_signature_valid = Some(verdict.valid);
        msg.pgp_signer_fingerprint = verdict.fingerprint;
    }
}

/// Resolve the OpenPGP material for a send: the sender's secret key (for signing)
/// and every recipient's public key (for encryption), or `None` when the message
/// asked for neither. Fails early with a clear message when a required key is
/// missing, rather than silently sending in clear.
pub(super) async fn resolve_pgp(
    state: &AppState,
    user_id: Uuid,
    from_email: &str,
    dto: &SendMailDto,
) -> Result<Option<crate::services::pgp_mime::PgpParams>, MailError> {
    let sign = dto.sign.unwrap_or(false);
    let encrypt = dto.encrypt.unwrap_or(false);
    if !sign && !encrypt {
        return Ok(None);
    }
    if !crate::handlers::addresses::server_config(state).await?.gpg_enabled {
        return Err(MailError::Forbidden);
    }

    let crypto = MailCrypto::new(&state.settings.mail.encryption_key).map_err(|_| MailError::Crypto)?;

    // The sender's own key: prefer one whose address matches the From, then default.
    let own = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, String)>(
        r#"SELECT private_key, private_key_nonce, public_key
           FROM mail.pgp_keys WHERE user_id = $1
           ORDER BY (lower(email) = lower($2)) DESC, is_default DESC, created_at
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(from_email)
    .fetch_optional(&state.db)
    .await?;

    if sign && own.is_none() {
        return Err(MailError::Validation(
            "Aucune clé OpenPGP : générez-en une dans Réglages ▸ Chiffrement.".into(),
        ));
    }

    let (sender_secret_armored, sender_public) = match &own {
        Some((enc, nonce, public)) => (
            crypto.decrypt(enc, nonce).map_err(|_| MailError::Crypto)?,
            Some(public.clone()),
        ),
        None => (String::new(), None),
    };

    let mut recipient_public_armored = Vec::new();
    if encrypt {
        let mut recipients: Vec<String> = dto.to_addresses.iter().map(|a| a.email.clone()).collect();
        recipients.extend(dto.cc_addresses.as_deref().unwrap_or(&[]).iter().map(|a| a.email.clone()));
        recipients.extend(dto.bcc_addresses.as_deref().unwrap_or(&[]).iter().map(|a| a.email.clone()));

        let wkd_http = reqwest::Client::new();
        for email in &recipients {
            let key: Option<String> = sqlx::query_scalar(
                "SELECT public_key FROM mail.pgp_contacts WHERE user_id = $1 AND lower(email) = lower($2) LIMIT 1",
            )
            .bind(user_id)
            .bind(email)
            .fetch_optional(&state.db)
            .await?;
            match key {
                Some(k) => recipient_public_armored.push(k),
                None => {
                    // No stored key: try Web Key Directory before giving up. On a
                    // hit we cache it as a contact (source='wkd') so the next send
                    // is offline, then encrypt to it.
                    match crate::services::wkd::discover(&wkd_http, email).await {
                        Some((armored, fingerprint)) => {
                            store_discovered_contact(state, user_id, email, &armored, &fingerprint, "wkd").await;
                            recipient_public_armored.push(armored);
                        }
                        None => {
                            return Err(MailError::Validation(format!(
                                "Pas de clé publique pour {email} — introuvable via WKD ; importez-la dans Réglages ▸ Chiffrement."
                            )))
                        }
                    }
                }
            }
        }
        // Encrypt to ourselves too, so the Sent copy stays readable.
        if let Some(pub_own) = sender_public {
            recipient_public_armored.push(pub_own);
        }
    }

    Ok(Some(crate::services::pgp_mime::PgpParams {
        sign,
        encrypt,
        sender_secret_armored,
        recipient_public_armored,
    }))
}

/// Cache a public key learned out-of-band (WKD or Autocrypt) as a correspondent
/// contact. A rotated key replaces the stored one (upsert on `lower(email)`).
/// Best-effort: a storage failure must not fail the send that discovered it.
async fn store_discovered_contact(
    state: &AppState,
    user_id: Uuid,
    email: &str,
    public_armored: &str,
    fingerprint: &str,
    source: &str,
) {
    let res = sqlx::query(
        r#"INSERT INTO mail.pgp_contacts (user_id, email, fingerprint, public_key, source)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (user_id, lower(email))
           DO UPDATE SET fingerprint = EXCLUDED.fingerprint,
                         public_key  = EXCLUDED.public_key,
                         source      = EXCLUDED.source"#,
    )
    .bind(user_id)
    .bind(email.to_lowercase())
    .bind(fingerprint)
    .bind(public_armored)
    .bind(source)
    .execute(&state.db)
    .await;
    if let Err(e) = res {
        tracing::warn!(email, source, error = %e, "Stockage clé publique découverte échoué");
    }
}

/// The armored public key to advertise in an outgoing `Autocrypt:` header for
/// `from_email`, or `None` when OpenPGP is off instance-wide or the sender holds
/// no key. Prefers a key whose User ID matches the From address, else the default.
pub(crate) async fn sender_autocrypt_key(
    state: &AppState,
    user_id: Uuid,
    from_email: &str,
) -> Option<String> {
    // Query the local key FIRST: the common sender holds none, and this avoids an
    // internal HTTP round-trip (server_config) on every send. Only a sender who
    // actually has a key pays the switch check.
    let key: String = sqlx::query_scalar(
        r#"SELECT public_key FROM mail.pgp_keys WHERE user_id = $1
           ORDER BY (lower(email) = lower($2)) DESC, is_default DESC, created_at
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(from_email)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()?;

    if !crate::handlers::addresses::server_config(state).await.ok()?.gpg_enabled {
        return None;
    }
    Some(key)
}
