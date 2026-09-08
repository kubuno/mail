use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    models::{EmailAccount, SendMailDto},
    services::{
        crypto::MailCrypto,
        outgoing,
        smtp_service::{self, SmtpConfig},
    },
    state::AppState,
};

use super::pgp::{resolve_pgp, sender_autocrypt_key};

pub async fn send_message(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<crate::models::OnBehalfQuery>,
    headers: HeaderMap,
    Json(dto): Json<SendMailDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Validate up front so a malformed request never claims an idempotency key.
    if dto.to_addresses.is_empty() {
        return Err(MailError::Validation("Au moins un destinataire requis".into()));
    }
    if dto.subject.trim().is_empty() {
        return Err(MailError::Validation("Sujet requis".into()));
    }

    // Account delegation: a delegate may send FROM the grantor's mailbox. The
    // request then scopes to the grantor (account lookup + Sent copy), while the
    // delegate's OWN address is stamped as `Sender:` (RFC 5322) so the recipient
    // sees who materially sent it — the `From:` stays the grantor. Read access is
    // not enough: `can_send` must be set on the accepted delegation. Resolved
    // BEFORE claiming an idempotency key so an unauthorized delegate is refused
    // without side effects.
    let acting_id = crate::services::delegation::resolve_acting_user(&state.db, &user, q.on_behalf_of).await?;
    let sender_override: Option<String> = if acting_id != user.id {
        if !crate::services::delegation::send_authority(&state.db, acting_id, user.id).await? {
            return Err(MailError::Forbidden);
        }
        tracing::info!(delegate = %user.id, grantor = %acting_id, "Envoi délégué");
        Some(user.email.clone())
    } else {
        None
    };

    // Idempotency-Key (optional): a retried send with the same key must not send
    // the mail twice. The web client omits it and keeps the historical path.
    let key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 255)
        .map(str::to_string);

    if let Some(key) = &key {
        // Lazy TTL purge (~24 h) so the table cannot grow without bound.
        let _ = sqlx::query(
            "DELETE FROM mail.send_idempotency WHERE created_at < NOW() - INTERVAL '24 hours'",
        )
        .execute(&state.db)
        .await;

        // Claim the key. ON CONFLICT DO NOTHING makes two concurrent requests
        // race for a single winner: exactly one inserts the row and sends.
        let claimed = sqlx::query(
            "INSERT INTO mail.send_idempotency (user_id, key) VALUES ($1, $2) \
             ON CONFLICT (user_id, key) DO NOTHING",
        )
        .bind(user.id)
        .bind(key)
        .execute(&state.db)
        .await?
        .rows_affected()
            == 1;

        if !claimed {
            // The key is already held: replay the memorised response, or — when
            // the first send is still in flight (no response yet) — refuse,
            // rather than risk a second delivery.
            let stored: Option<serde_json::Value> =
                sqlx::query_scalar::<_, Option<serde_json::Value>>(
                    "SELECT response_json FROM mail.send_idempotency WHERE user_id = $1 AND key = $2",
                )
                .bind(user.id)
                .bind(key)
                .fetch_optional(&state.db)
                .await?
                .flatten();

            return match stored {
                Some(resp) => Ok(Json(resp)),
                None => Err(MailError::Conflict(
                    "Un envoi avec cette clé d'idempotence est déjà en cours.".into(),
                )),
            };
        }
    }

    // We hold the claim (or no key was supplied): perform the send, scoped to the
    // acting user (the grantor on a delegated send) and stamping the delegate as
    // `Sender:` when delegated.
    let result = send_message_inner(&state, acting_id, &dto, sender_override.as_deref()).await;

    if let Some(key) = &key {
        match &result {
            // Success → memorise the response so a replay returns it verbatim.
            Ok(response) => {
                if let Err(e) = sqlx::query(
                    "UPDATE mail.send_idempotency SET response_json = $3 WHERE user_id = $1 AND key = $2",
                )
                .bind(user.id)
                .bind(key)
                .bind(response)
                .execute(&state.db)
                .await
                {
                    tracing::error!(error = %e, "Mémorisation de la réponse idempotente échouée");
                }
            }
            // Failure → drop the claim so an honest retry can proceed.
            Err(_) => {
                let _ =
                    sqlx::query("DELETE FROM mail.send_idempotency WHERE user_id = $1 AND key = $2")
                        .bind(user.id)
                        .bind(key)
                        .execute(&state.db)
                        .await;
            }
        }
    }

    result.map(Json)
}

/// The actual send, factored out so the idempotency wrapper above can memorise
/// its JSON response and replay it on a retried request. Returns the response
/// body as a bare `Value`.
pub(crate) async fn send_message_inner(
    state: &AppState,
    user_id: Uuid,
    dto: &SendMailDto,
    sender_override: Option<&str>,
) -> Result<serde_json::Value, MailError> {
    // Envoi PROGRAMMÉ : on stocke comme brouillon planifié ; le worker scheduler
    // l'enverra quand l'heure sera venue. (Voir workers::scheduler_worker.)
    if let Some(when) = dto.scheduled_at {
        sqlx::query(
            r#"INSERT INTO mail.drafts
               (id, account_id, user_id, to_addresses, cc_addresses, bcc_addresses, subject, body_html, reply_to_id, scheduled_at)
               VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        )
        .bind(dto.account_id)
        .bind(user_id)
        .bind(serde_json::to_value(&dto.to_addresses).unwrap_or_else(|_| serde_json::json!([])))
        .bind(serde_json::to_value(dto.cc_addresses.clone().unwrap_or_default()).unwrap_or_else(|_| serde_json::json!([])))
        .bind(serde_json::to_value(dto.bcc_addresses.clone().unwrap_or_default()).unwrap_or_else(|_| serde_json::json!([])))
        .bind(&dto.subject)
        .bind(ammonia::clean(&dto.body_html))
        .bind(dto.reply_to_id)
        .bind(when)
        .execute(&state.db)
        .await?;
        if let Some(draft_id) = dto.draft_id {
            let _ = sqlx::query("DELETE FROM mail.drafts WHERE id = $1 AND user_id = $2")
                .bind(draft_id).bind(user_id).execute(&state.db).await;
        }
        return Ok(serde_json::json!({ "message": "Envoi programmé", "scheduled_at": when }));
    }

    let account = sqlx::query_as::<_, EmailAccount>(
        r#"SELECT id, user_id, name, email_address, kind, mailbox_id,
                  incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  smtp_host, smtp_port, smtp_security, smtp_username, auth_kind,
                  is_default, is_active, last_sync_at, last_error,
                  created_at, updated_at
           FROM mail.accounts WHERE id = $1 AND user_id = $2"#,
    )
    .bind(dto.account_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Compte {}", dto.account_id)))?;

    // The module's own hardened policy, the one the reader applies too — not
    // bare `ammonia::clean`, which strips every `style` attribute and so sent
    // each message, invitations included, stripped of its formatting.
    let body_html = crate::services::html_sanitize::sanitize_email_html(&dto.body_html);

    // OpenPGP: resolve the sender's secret key and recipients' public keys up front
    // (None unless the message asks to be signed/encrypted). Both send paths hand
    // this to build_email, which wraps the body in PGP/MIME.
    let pgp = resolve_pgp(state, user_id, &account.email_address, dto).await?;

    // Autocrypt: advertise our public key on EVERY outgoing message (not only
    // encrypted ones) so correspondents learn it passively. `None` when OpenPGP
    // is off instance-wide or we hold no key for this address.
    let autocrypt = sender_autocrypt_key(state, user_id, &account.email_address).await;

    // A local account (a hosted mailbox of this instance) has no external SMTP
    // relay: the instance itself delivers. Local recipients are filed straight
    // into the store, remote recipients go through the instance's outbound queue.
    // An external account keeps the historical SMTP path. See migration 000025.
    let message_id = if account.kind == "local" {
        outgoing::send_from_local_account(state, &account, dto, &dto.subject, &body_html, pgp.as_ref(), autocrypt.as_deref(), sender_override).await?
    } else {
        let crypto = MailCrypto::new(&state.settings.mail.encryption_key)
            .map_err(|_| MailError::Crypto)?;

        // OAuth accounts send with XOAUTH2 (access token); password accounts keep
        // the historical credentials path.
        let (smtp_secret, xoauth2) = if account.auth_kind.starts_with("oauth") {
            let token = crate::services::oauth::valid_access_token(
                &state.db, &crypto, &state.settings.mail, account.id,
            )
            .await
            .map_err(|e| MailError::Smtp(e.to_string()))?;
            (token, true)
        } else {
            let (smtp_pass_enc, smtp_nonce): (Vec<u8>, Vec<u8>) = sqlx::query_as(
                "SELECT smtp_password, smtp_password_nonce FROM mail.accounts WHERE id = $1"
            )
            .bind(account.id)
            .fetch_one(&state.db)
            .await?;
            let smtp_pass = crate::services::app_password_normalize(
                &account.smtp_host,
                &crypto.decrypt(&smtp_pass_enc, &smtp_nonce).map_err(|_| MailError::Crypto)?,
            );
            (smtp_pass, false)
        };

        let smtp_cfg = SmtpConfig {
            host:       account.smtp_host.clone(),
            port:       account.smtp_port as u16,
            security:   account.smtp_security.clone(),
            username:   account.smtp_username.clone(),
            password:   smtp_secret,
            xoauth2,
            from_name:  account.name.clone(),
            from_email: account.email_address.clone(),
        };

        smtp_service::send_message(&smtp_cfg, dto, &dto.subject, &body_html, pgp.as_ref(), autocrypt.as_deref(), sender_override)
            .await
            .map_err(|e| MailError::Smtp(e.to_string()))?
    };

    // Local Sent copy — the message is already sent, so a storage failure must
    // not fail the request; log it instead. On success we get the thread the copy
    // was filed under, so the composer's labels can be attached to it.
    match crate::services::sent_copy::store_sent_copy(
        &state.db, &account, dto, &body_html, &message_id,
        &state.settings.mail.attachments_dir,
    ).await {
        Ok(thread_id) => {
            if let Some(requested) = dto.label_ids.as_deref() {
                apply_sent_labels(&state.db, user_id, thread_id, requested).await;
            }
        }
        Err(e) => {
            tracing::error!(error = %e, account_id = %account.id, "Copie locale « envoyés » échouée");
        }
    }

    if let Some(draft_id) = dto.draft_id {
        let _ = sqlx::query("DELETE FROM mail.drafts WHERE id = $1 AND user_id = $2")
            .bind(draft_id)
            .bind(user_id)
            .execute(&state.db)
            .await;
    }

    // Feed the recipient-autocomplete index — people the user WRITES TO rank
    // highest (weight 3 vs 1 for synced mail).
    let mut sent_to: Vec<(String, Option<String>)> = Vec::new();
    for a in dto.to_addresses.iter()
        .chain(dto.cc_addresses.as_deref().unwrap_or(&[]))
        .chain(dto.bcc_addresses.as_deref().unwrap_or(&[]))
    {
        sent_to.push((a.email.clone(), a.name.clone()));
    }
    crate::services::address_index::upsert(&state.db, user_id, &sent_to, 3).await;

    Ok(serde_json::json!({ "message": "Message envoyé" }))
}

/// Attach the composer-picked labels to the Sent copy's thread. Only labels the
/// user actually OWNS are applied (filtered in SQL by `user_id`), so a foreign
/// or stale id is silently dropped rather than turning a successful send into an
/// error. Best-effort: a labelling failure only gets logged — the mail already
/// left. Uses `mail.thread_labels`, the same link table every other labelling
/// path writes to (handlers::labels, filters, sync).
async fn apply_sent_labels(db: &sqlx::PgPool, user_id: Uuid, thread_id: Uuid, requested: &[Uuid]) {
    if requested.is_empty() {
        return;
    }
    // Which of the requested ids does this user own? Ownership is enforced here.
    let owned: Vec<Uuid> = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM mail.labels WHERE id = ANY($1) AND user_id = $2",
    )
    .bind(requested)
    .bind(user_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let to_apply = retain_owned_labels(requested, &owned);
    if to_apply.is_empty() {
        return;
    }

    if let Err(e) = sqlx::query(
        "INSERT INTO mail.thread_labels (thread_id, label_id) \
         SELECT $1, unnest($2::uuid[]) ON CONFLICT DO NOTHING",
    )
    .bind(thread_id)
    .bind(&to_apply)
    .execute(db)
    .await
    {
        tracing::error!(error = %e, %thread_id, "Application des libellés à la copie « envoyés » échouée");
    }
}

/// Keep only the requested label ids the user owns, preserving request order and
/// dropping duplicates. Pure companion to `apply_sent_labels`, isolated so the
/// ownership-filtering decision is unit-testable without a database.
fn retain_owned_labels(requested: &[Uuid], owned: &[Uuid]) -> Vec<Uuid> {
    let owned: std::collections::HashSet<Uuid> = owned.iter().copied().collect();
    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    requested
        .iter()
        .copied()
        .filter(|id| owned.contains(id) && seen.insert(*id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::retain_owned_labels;
    use uuid::Uuid;

    #[test]
    fn foreign_labels_are_refused() {
        let mine_a = Uuid::new_v4();
        let mine_b = Uuid::new_v4();
        let someone_else = Uuid::new_v4();

        // The user asked for two of their own labels plus one belonging to
        // another user; only the owned pair survives, foreign id is dropped.
        let requested = [mine_a, someone_else, mine_b];
        let owned = [mine_a, mine_b];
        let applied = retain_owned_labels(&requested, &owned);

        assert_eq!(applied, vec![mine_a, mine_b]);
        assert!(!applied.contains(&someone_else));
    }

    #[test]
    fn duplicates_and_unknown_ids_are_dropped() {
        let mine = Uuid::new_v4();
        let requested = [mine, mine, Uuid::new_v4()];
        let owned = [mine];
        // Deduped to a single entry; the unknown/unowned id never appears.
        assert_eq!(retain_owned_labels(&requested, &owned), vec![mine]);
    }

    #[test]
    fn empty_request_applies_nothing() {
        let owned = [Uuid::new_v4()];
        assert!(retain_owned_labels(&[], &owned).is_empty());
    }
}
