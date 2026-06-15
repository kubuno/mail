// Worker d'ENVOI PROGRAMMÉ : envoie les brouillons dont `scheduled_at` est échu.
use std::{sync::Arc, time::Duration};
use uuid::Uuid;
use serde_json::Value;

use crate::{
    models::{EmailAccount, EmailAddress, SendMailDto},
    services::{crypto::MailCrypto, smtp_service::{self, SmtpConfig}},
    state::AppState,
};

pub async fn run(state: Arc<AppState>) {
    let crypto = match MailCrypto::new(&state.settings.mail.encryption_key) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "Scheduler: clé de chiffrement invalide — worker arrêté");
            return;
        }
    };
    tracing::info!("Worker d'envoi programmé démarré (vérif toutes les 30 s)");
    loop {
        send_due(&state, &crypto).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

#[allow(clippy::type_complexity)]
async fn send_due(state: &AppState, crypto: &MailCrypto) {
    let due: Vec<(Uuid, Uuid, Value, Value, Value, String, String, Option<Uuid>)> = match sqlx::query_as(
        r#"SELECT id, account_id, to_addresses, cc_addresses, bcc_addresses, subject, body_html, reply_to_id
           FROM mail.drafts
           WHERE scheduled_at IS NOT NULL AND scheduled_at <= NOW()
           LIMIT 20"#,
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => { tracing::warn!(error = %e, "Scheduler: lecture des brouillons"); return; }
    };

    for (id, account_id, to_v, cc_v, bcc_v, subject, body_html, reply_to_id) in due {
        match send_one(state, crypto, account_id, &to_v, &cc_v, &bcc_v, &subject, &body_html, reply_to_id).await {
            Ok(()) => {
                let _ = sqlx::query("DELETE FROM mail.drafts WHERE id = $1").bind(id).execute(&state.db).await;
                tracing::info!(draft_id = %id, "Envoi programmé effectué");
            }
            Err(e) => {
                // Éviter une boucle d'échecs : reporter d'une heure.
                tracing::warn!(draft_id = %id, error = %e, "Envoi programmé échoué — report d'1 h");
                let _ = sqlx::query("UPDATE mail.drafts SET scheduled_at = NOW() + INTERVAL '1 hour' WHERE id = $1")
                    .bind(id).execute(&state.db).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_one(
    state: &AppState, crypto: &MailCrypto, account_id: Uuid,
    to_v: &Value, cc_v: &Value, bcc_v: &Value,
    subject: &str, body_html: &str, reply_to_id: Option<Uuid>,
) -> anyhow::Result<()> {
    let account = sqlx::query_as::<_, EmailAccount>(
        r#"SELECT id, user_id, name, email_address, incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  smtp_host, smtp_port, smtp_security, smtp_username,
                  is_default, is_active, last_sync_at, last_error, created_at, updated_at
           FROM mail.accounts WHERE id = $1"#,
    )
    .bind(account_id)
    .fetch_one(&state.db)
    .await?;

    let (enc, nonce): (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT smtp_password, smtp_password_nonce FROM mail.accounts WHERE id = $1",
    )
    .bind(account_id)
    .fetch_one(&state.db)
    .await?;
    let pass = crypto.decrypt(&enc, &nonce)?;

    let cfg = SmtpConfig {
        host:       account.smtp_host.clone(),
        port:       account.smtp_port as u16,
        security:   account.smtp_security.clone(),
        username:   account.smtp_username.clone(),
        password:   pass,
        from_name:  account.name.clone(),
        from_email: account.email_address.clone(),
    };

    let to:  Vec<EmailAddress> = serde_json::from_value(to_v.clone()).unwrap_or_default();
    let cc:  Vec<EmailAddress> = serde_json::from_value(cc_v.clone()).unwrap_or_default();
    let bcc: Vec<EmailAddress> = serde_json::from_value(bcc_v.clone()).unwrap_or_default();

    let dto = SendMailDto {
        account_id,
        to_addresses:  to,
        cc_addresses:  if cc.is_empty()  { None } else { Some(cc) },
        bcc_addresses: if bcc.is_empty() { None } else { Some(bcc) },
        subject:       subject.to_string(),
        body_html:     body_html.to_string(),
        reply_to_id,
        draft_id:      None,
        scheduled_at:  None,
        attachments:   None,
    };

    smtp_service::send_message(&cfg, &dto, subject, body_html).await?;
    Ok(())
}
