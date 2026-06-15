use std::{sync::Arc, time::Duration};
use crate::{services::crypto::MailCrypto, state::AppState};

pub async fn run(state: Arc<AppState>) {
    let interval_secs = state.settings.mail.sync_interval_secs;
    let crypto = match MailCrypto::new(&state.settings.mail.encryption_key) {
        Ok(c)  => c,
        Err(e) => {
            tracing::error!(error = %e, "Sync worker: clé de chiffrement invalide — worker arrêté");
            return;
        }
    };

    tracing::info!("Sync worker démarré (intervalle: {}s)", interval_secs);

    loop {
        sync_all_accounts(&state, &crypto).await;
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}

async fn sync_all_accounts(state: &AppState, crypto: &MailCrypto) {
    let accounts = match sqlx::query_as::<_, crate::models::EmailAccount>(
        r#"SELECT id, user_id, name, email_address,
                  incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  smtp_host, smtp_port, smtp_security, smtp_username,
                  is_default, is_active, last_sync_at, last_error,
                  created_at, updated_at
           FROM mail.accounts WHERE is_active = TRUE"#,
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(a)  => a,
        Err(e) => {
            tracing::error!(error = %e, "Sync: lecture comptes DB");
            return;
        }
    };

    for account in accounts {
        // Borne dure : une opération IMAP bloquée ne doit jamais figer le worker
        // (et donc tous les autres comptes) indéfiniment. L'échec est enregistré.
        let fut = crate::services::sync_service::sync_account(&state.db, &account, crypto);
        let outcome = tokio::time::timeout(Duration::from_secs(180), fut).await;
        let err: Option<String> = match outcome {
            Ok(Ok(()))  => None,
            Ok(Err(e))  => Some(e.to_string()),
            Err(_)      => Some("Délai de synchronisation dépassé (180 s)".to_string()),
        };
        if let Some(msg) = err {
            tracing::warn!(account_id = %account.id, error = %msg, "Sync compte échoué");
            let _ = sqlx::query("UPDATE mail.accounts SET last_error = $1 WHERE id = $2")
                .bind(msg)
                .bind(account.id)
                .execute(&state.db)
                .await;
        }
    }
}
