//! Sending FROM a local account — a hosted mailbox of this instance.
//!
//! An external account relays through the provider's SMTP server
//! (`services::smtp_service`). A local account (`mail.accounts.kind = 'local'`,
//! created for every hosted mailbox by migration 000025) has no such relay: the
//! instance itself is the server. So an outgoing message is delivered by the
//! instance directly —
//!
//!   * a LOCAL recipient (another hosted address of this instance) is filed
//!     straight into its mailbox with `server::deliver::deliver_local`, exactly
//!     as an inbound SMTP message would be;
//!   * a REMOTE recipient is handed to the instance's outbound queue
//!     (`server::queue`), which the delivery worker signs (DKIM) and delivers to
//!     the destination MX with retries and DSNs.
//!
//! The split matters: the outbound worker resolves MX and speaks SMTP to a
//! remote server, so a local recipient dropped into that queue would either loop
//! back through our own MX or fail outright (a lab domain such as `kubuno.local`
//! has no MX at all). Local recipients therefore never enter the queue.
//!
//! Remote delivery obeys the administrator's `outbound_enabled` switch: with it
//! off and a remote recipient present, the whole send is refused with an explicit
//! message rather than the message being silently swallowed.

use crate::{
    errors::MailError,
    models::{EmailAccount, SendMailDto},
    server::{compliance, config, deliver, hygiene, journal, queue, resolve},
    services::smtp_service,
    state::AppState,
};

/// Sends `dto` from a local `account` and returns the message's Message-ID (no
/// angle brackets), so the caller can store a matching Sent copy just as it does
/// for an external send.
#[allow(clippy::too_many_arguments)] // mirrors build_email's builder-style signature (+ delegated Sender)
pub async fn send_from_local_account(
    state: &AppState,
    account: &EmailAccount,
    dto: &SendMailDto,
    subject: &str,
    body_html: &str,
    pgp: Option<&crate::services::pgp_mime::PgpParams>,
    autocrypt_key: Option<&str>,
    sender: Option<&str>,
) -> Result<String, MailError> {
    // The live server configuration: which domains are ours, whether outbound
    // delivery is switched on, and what the instance's compliance rules are.
    // Read from the core rather than a local copy — and read FIRST, because the
    // administrator's footer belongs inside the body we are about to build (and
    // therefore inside the PGP signature, when there is one).
    let http = reqwest::Client::new();
    let cfg = config::fetch(&http, &state.settings).await.ok_or_else(|| {
        MailError::Internal(anyhow::anyhow!(
            "Configuration du serveur de messagerie illisible : impossible d'envoyer depuis une boîte locale"
        ))
    })?;

    // The instance's footer (legal mention, disclaimer). No-op when none is
    // configured. The plain-text alternative is derived from this HTML by the
    // builder, so one insertion covers both parts of the message.
    let body_html = compliance::with_footer(body_html, &cfg.append_footer_html);

    // Build the RFC 5322 message once — the same bytes feed local delivery and
    // the outbound queue. `sender` (a delegate's address) is stamped as the
    // `Sender:` header for a delegated send; None for a self-send.
    let (email, message_id) =
        smtp_service::build_email(&account.name, &account.email_address, dto, subject, &body_html, pgp, autocrypt_key, sender)
            .map_err(|e| MailError::Smtp(e.to_string()))?;
    let raw = email.formatted();

    // Attachment and content compliance, applied to what the user is about to
    // send. The same rules that keep a forbidden file OUT must keep it IN; a
    // rule enforced only on reception is half a rule.
    //
    // Both non-trivial verdicts mean the same thing here — the message must not
    // leave — so the user is told now, while they can still fix it, rather than
    // discovering it from a bounce.
    if let Some(verdict) = compliance::scan(&cfg, &raw) {
        tracing::warn!(
            from = %account.email_address, reason = %verdict.reason,
            "Envoi refusé par la conformité du contenu"
        );
        return Err(MailError::Validation(format!(
            "Message refusé par la politique de contenu de l'instance : {}.",
            verdict.reason
        )));
    }

    let envelope_from = account.email_address.trim().to_ascii_lowercase();

    // Sort every recipient (To + Cc + Bcc) into local deliveries and remote
    // addresses. A local recipient is resolved the same way an inbound RCPT is
    // (aliases, lists and catch-alls expand, and a local alias may itself forward
    // to a remote address); a remote recipient is taken as-is.
    let directory = resolve::PgDirectory::new(&state.db);
    let mut local: Vec<resolve::LocalDelivery> = Vec::new();
    let mut remote: Vec<String> = Vec::new();

    let recipients = dto
        .to_addresses
        .iter()
        .chain(dto.cc_addresses.as_deref().unwrap_or(&[]))
        .chain(dto.bcc_addresses.as_deref().unwrap_or(&[]));

    for addr in recipients {
        let recipient = addr.email.trim().to_ascii_lowercase();
        if recipient.is_empty() {
            continue;
        }

        if cfg.is_local_domain(&recipient) {
            // Authenticated = true: this is the account owner sending their own
            // mail, so a list's "internal only" policy accepts them.
            match resolve::resolve(&directory, &cfg, &envelope_from, true, &recipient).await {
                Ok(resolve::Outcome::Accept(expansion)) => {
                    for delivery in expansion.local {
                        if !local.iter().any(|d| d.target == delivery.target) {
                            local.push(delivery);
                        }
                    }
                    for address in expansion.remote {
                        if !remote.contains(&address) {
                            remote.push(address);
                        }
                    }
                }
                Ok(resolve::Outcome::Refuse(refusal)) => {
                    return Err(MailError::Validation(format!(
                        "Destinataire « {recipient} » refusé : {}",
                        refusal.reply()
                    )));
                }
                Err(e) => {
                    tracing::error!(error = %e, recipient = %recipient, "Résolution d'un destinataire local (envoi depuis boîte locale)");
                    return Err(MailError::Internal(e));
                }
            }
        } else if !remote.contains(&recipient) {
            remote.push(recipient);
        }
    }

    // Refuse the whole send rather than swallow it: a remote recipient with
    // outbound delivery switched off can go nowhere.
    if !remote.is_empty() && !cfg.outbound_enabled {
        return Err(MailError::Validation(
            "L'envoi sortant est désactivé par l'administrateur : impossible d'envoyer vers des destinataires externes.".into(),
        ));
    }

    // The delivery restriction, if the operator declared one. Naming the refused
    // address is deliberate: the sender chose it and can change it, and the list
    // itself is not a secret — it is a policy they are subject to.
    if let Some(refused) = remote.iter().find(|address| !cfg.outbound_recipient_allowed(address)) {
        return Err(MailError::Validation(format!(
            "Le domaine du destinataire « {refused} » ne fait pas partie des domaines autorisés par l'administrateur."
        )));
    }

    // The sender's daily allowance, measured on what has actually been queued
    // for the internet over the last 24 hours. Checked before anything is
    // delivered, so a refusal never leaves half a send behind.
    if cfg.send_max_recipients_per_day > 0 && !remote.is_empty() {
        match queue::recipients_queued_last_24h(&state.db, account.user_id).await {
            Ok(used) if used.saturating_add(remote.len() as i64) > cfg.send_max_recipients_per_day => {
                return Err(MailError::Validation(format!(
                    "Quota d'envoi quotidien atteint : {used} destinataires externes sur les {} autorisés par 24 heures. Réessayez plus tard.",
                    cfg.send_max_recipients_per_day
                )));
            }
            Ok(_) => {}
            // A counting failure must not block legitimate mail; it is logged
            // and the send proceeds.
            Err(e) => tracing::error!(error = %e, "Quota d'envoi : comptage impossible — envoi autorisé"),
        }
    }

    // ── Local recipients: file straight into their mailboxes ────────────────
    let mut delivered = 0usize;
    for delivery in &local {
        match deliver::deliver_local(
            &state.db,
            &cfg,
            &envelope_from,
            &delivery.address,
            delivery.target,
            &raw,
            &state.settings.mail.attachments_dir,
            deliver::Disposition::Inbox,
            // Internal deposit: the message never crossed the network, and no
            // DMARC was evaluated on it.
            None,
            None,
        )
        .await
        {
            Ok(_) => delivered += 1,
            Err(e) => {
                tracing::error!(error = %e, recipient = %delivery.address, "Dépôt local (envoi depuis boîte locale) échoué");
            }
        }
    }

    // ── Remote recipients: hand to the outbound queue in one transaction ────
    let mut queued = 0usize;
    if !remote.is_empty() {
        // Stamp our own Received line, as the SMTP submission path does before
        // enqueueing, so the trace is honest and the hop-count guard has a hop.
        let outbound_raw = hygiene::prepend_received(&raw, "local", &cfg.hostname);
        let list = to_queue_list(&remote);
        match queue::enqueue_with_lifetime(
            &state.db,
            Some(account.user_id),
            Some(account.id),
            &envelope_from,
            &outbound_raw,
            false,
            &list,
            cfg.outbound_lifetime_hours,
        )
        .await
        {
            Ok(_) => queued = list.len(),
            Err(e) => {
                tracing::error!(error = %e, "Enfilement sortant (envoi depuis boîte locale) échoué");
                return Err(MailError::Internal(e));
            }
        }
    }

    if delivered == 0 && queued == 0 {
        return Err(MailError::Internal(anyhow::anyhow!(
            "Aucun destinataire n'a pu être servi"
        )));
    }

    // Journalling, once the send has actually happened. Best-effort: it never
    // fails a message that has already left.
    journal::archive(
        &state.db, &cfg, &state.settings.mail.attachments_dir, &envelope_from, &raw,
    )
    .await;

    Ok(message_id)
}

/// Builds the `(recipient, domain)` pairs the outbound queue expects, splitting
/// each address on its last `@`. A malformed address (no `@`) yields an empty
/// domain rather than being dropped — the queue records it and the delivery
/// worker bounces it, which is visible, whereas silently discarding it would
/// lose the recipient without a trace.
fn to_queue_list(remote: &[String]) -> Vec<(String, String)> {
    remote
        .iter()
        .map(|r| {
            let domain = r.rsplit_once('@').map(|(_, d)| d.to_string()).unwrap_or_default();
            (r.clone(), domain)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::config::ServerConfig;

    fn cfg_with_domains(domains: &[&str], outbound: bool) -> ServerConfig {
        ServerConfig {
            domains: domains.iter().map(|d| d.to_string()).collect(),
            outbound_enabled: outbound,
            ..ServerConfig::default()
        }
    }

    #[test]
    fn queue_list_extracts_the_domain_of_each_recipient() {
        let list = to_queue_list(&[
            "alice@example.com".to_string(),
            "bob@sub.example.org".to_string(),
        ]);
        assert_eq!(
            list,
            vec![
                ("alice@example.com".to_string(), "example.com".to_string()),
                ("bob@sub.example.org".to_string(), "sub.example.org".to_string()),
            ]
        );
    }

    #[test]
    fn a_malformed_recipient_keeps_an_empty_domain_rather_than_vanishing() {
        let list = to_queue_list(&["pas-une-adresse".to_string()]);
        assert_eq!(list, vec![("pas-une-adresse".to_string(), String::new())]);
    }

    /// The local/remote decision the send hinges on: an address in one of our
    /// domains is delivered locally, anything else is queued outbound.
    #[test]
    fn recipients_are_local_when_their_domain_is_ours() {
        let cfg = cfg_with_domains(&["kubuno.com", "kubuno.local"], true);
        assert!(cfg.is_local_domain("test001@kubuno.com"));
        assert!(cfg.is_local_domain("admin@KUBUNO.LOCAL"));
        assert!(!cfg.is_local_domain("someone@gmail.com"));
    }
}
