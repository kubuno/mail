//! The outbound queue worker: claims due recipients, attempts delivery, and
//! advances their state — with bounces turned into DSNs.
//!
//! It ties together `queue` (state), `outbound` (one delivery attempt) and
//! `dsn` (failure reports). The reliability contract is queue's; this loop just
//! maps a delivery outcome onto the right transition.

use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use super::{
    config::{self, ServerConfig},
    dsn,
    outbound::{self, DeliveryOutcome},
    queue::{self, Claimed},
    relay::{self, RelayTarget},
    signing,
};
use crate::{config::settings::Settings, services::crypto::MailCrypto};

/// How often the worker looks for due recipients. Deliveries are also naturally
/// paced by each recipient's backoff.
const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// A claimed recipient is leased this long; a worker that dies mid-delivery has
/// its rows reclaimed once this passes.
///
/// ⚠️ It MUST outlast the longest possible delivery attempt, or a row still
/// being delivered becomes claimable and a second worker sends the message
/// again. The worst case is connect + command + the wait for the reply to the
/// final `.` (the remote may fsync the whole message first): 30 + 120 + 600 s.
/// A 120 s lease — the previous value — was shorter than that single 600 s wait,
/// so a slow-but-healthy destination could produce duplicates. The margin below
/// keeps the lease safely above the sum.
const LEASE: Duration = Duration::from_secs(900);
/// Recipients handled per cycle. Keeps memory and connection use bounded.
const BATCH: i64 = 20;
/// Reply code used when the administrator requires a DKIM signature we cannot
/// produce. Temporary (4xx): publishing a key fixes it, and the message must
/// wait rather than go out in a shape the large providers will reject.
const DKIM_REQUIRED_CODE: u16 = 451;

/// Runs forever. Delivery only happens when the administrator has enabled
/// outbound in the console; otherwise the loop idles (mail stays queued).
pub async fn run(db: PgPool, settings: Settings, http: reqwest::Client) {
    let worker_id = Uuid::new_v4();
    // The DKIM signing key is decrypted with the module's crypto; a bad key just
    // means outbound mail goes unsigned (logged), never that the worker stops.
    let crypto = match MailCrypto::new(&settings.mail.encryption_key) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::error!(error = %e, "File sortante : clé de chiffrement invalide — signature DKIM indisponible");
            None
        }
    };
    tracing::info!(%worker_id, "File d'envoi sortant : worker démarré");

    loop {
        if let Some(cfg) = config::fetch(&http, &settings).await {
            if cfg.outbound_enabled {
                // Read the relay configuration once per cycle, exactly like the
                // rest of the config — not once per message. `None` means no
                // relay: deliver direct-to-MX as before. When a relay is set,
                // EVERY remote recipient below goes through it instead of the MX.
                let relay = relay::fetch(&db, crypto.as_ref()).await;
                if let Err(e) = process_cycle(&db, &cfg, crypto.as_ref(), relay.as_ref(), worker_id).await {
                    tracing::error!(error = %e, "File sortante : cycle en erreur");
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn process_cycle(
    db: &PgPool,
    cfg: &ServerConfig,
    crypto: Option<&MailCrypto>,
    relay: Option<&RelayTarget>,
    worker_id: Uuid,
) -> anyhow::Result<()> {
    let batch = queue::claim_batch(db, worker_id, LEASE, BATCH).await?;
    for claimed in batch {
        deliver_one(db, cfg, crypto, relay, claimed).await;
    }
    Ok(())
}

/// One recipient: attempt delivery and record the outcome. An expired message
/// is bounced even on a temporary failure — we do not retry forever.
async fn deliver_one(
    db: &PgPool,
    cfg: &ServerConfig,
    crypto: Option<&MailCrypto>,
    relay: Option<&RelayTarget>,
    c: Claimed,
) {
    // The exact bytes to transmit, DKIM-signed when the administrator asked for
    // it. `Err` means a signature is REQUIRED and we could not produce one:
    // nothing goes on the wire.
    let to_send = match prepare(db, cfg, crypto, &c).await {
        Ok(bytes) => bytes,
        Err(reason) => {
            tracing::warn!(recipient = %c.recipient, %reason, "Sortant : signature DKIM exigée mais indisponible");
            let reason = format!("Signature DKIM exigée mais indisponible : {reason}");
            if let Err(e) = defer_or_bounce(db, cfg, &c, DKIM_REQUIRED_CODE, &reason).await {
                tracing::error!(error = %e, recipient = %c.recipient, "Sortant : transition d'état échouée");
            }
            return;
        }
    };

    // A configured relay takes over every remote delivery: connect to its
    // host:port instead of resolving the recipient's MX. `to_send` is the SAME
    // DKIM-signed payload either way — the relay does not re-sign.
    let outcome = if let Some(relay) = relay {
        outbound::deliver_via_relay(&c.recipient, &c.envelope_from, &to_send, relay, &cfg.hostname).await
    } else {
        // Per-destination policy: a domain the operator listed as
        // encryption-only is never delivered to in the clear, whatever the
        // general level says.
        let policy = outbound::Policy::for_domain(cfg, &c.domain);
        outbound::deliver(&c.recipient, &c.domain, &c.envelope_from, &to_send, &policy).await
    };

    let result = match outcome {
        DeliveryOutcome::Delivered => {
            tracing::info!(recipient = %c.recipient, "Sortant : livré");
            queue::mark_sent(db, c.recipient_id).await
        }
        DeliveryOutcome::Deferred { code, reason } => {
            tracing::info!(recipient = %c.recipient, code, "Sortant : différé");
            defer_or_bounce(db, cfg, &c, code, &reason).await
        }
        DeliveryOutcome::Bounced { code, reason } => {
            tracing::warn!(recipient = %c.recipient, code, "Sortant : rejet définitif");
            bounce(db, cfg, &c, code, &reason).await
        }
    };
    if let Err(e) = result {
        // The row stays leased; its lease will expire and it will be retried.
        // Better a duplicate attempt than a lost state transition.
        tracing::error!(error = %e, recipient = %c.recipient, "Sortant : transition d'état échouée");
    }
}

/// Produces the bytes to transmit, applying the two DKIM settings.
///
/// `Err(reason)` means the administrator requires a signature (
/// `dkim_require_signature`) that cannot be produced — the message must wait,
/// not leave unsigned, since unsigned mail is refused outright by the large
/// providers past a few thousand messages a day.
async fn prepare(
    db: &PgPool,
    cfg: &ServerConfig,
    crypto: Option<&MailCrypto>,
    c: &Claimed,
) -> Result<Vec<u8>, String> {
    // A DSN travels with the null return path and is not aligned by DMARC; it is
    // never signed, and requiring a signature must not strand bounce reports.
    if c.is_dsn {
        return Ok(c.raw.clone());
    }

    if !cfg.dkim_signing_enabled {
        if cfg.dkim_require_signature {
            // A contradictory configuration. Deferring says so plainly instead of
            // quietly picking one of the two settings.
            return Err("la signature DKIM est désactivée alors qu'elle est exigée".to_string());
        }
        return Ok(c.raw.clone());
    }

    let Some(crypto) = crypto else {
        if cfg.dkim_require_signature {
            return Err("clé de chiffrement du module indisponible".to_string());
        }
        return Ok(c.raw.clone());
    };

    match signing::sign(db, crypto, &c.raw).await {
        signing::Signed::Signed(bytes) => Ok(bytes),
        signing::Signed::Unsigned { raw, reason } => {
            if cfg.dkim_require_signature {
                Err(reason)
            } else {
                tracing::info!(recipient = %c.recipient, %reason, "Sortant : message envoyé NON SIGNÉ");
                Ok(raw)
            }
        }
    }
}

/// A temporary failure: retry later, unless the message has outlived the
/// configured queue lifetime, in which case we give up and report it.
async fn defer_or_bounce(
    db: &PgPool,
    cfg: &ServerConfig,
    c: &Claimed,
    code: u16,
    reason: &str,
) -> anyhow::Result<()> {
    if c.expired {
        tracing::warn!(recipient = %c.recipient, "Sortant : expiré en file, abandon");
        return bounce(db, cfg, c, code, &format!("Message expiré en file : {reason}")).await;
    }
    queue::mark_deferred(
        db,
        c.recipient_id,
        c.attempts,
        code,
        reason,
        queue::Backoff::from_config(cfg),
    )
    .await
}

/// Marks the recipient bounced and queues a DSN back to the sender — unless the
/// failed message is itself a DSN or has the null return path, in which case the
/// notice is dropped (RFC 3464: no DSN of a DSN, no infinite bounce loop).
async fn bounce(db: &PgPool, cfg: &ServerConfig, c: &Claimed, code: u16, reason: &str) -> anyhow::Result<()> {
    queue::mark_bounced(db, c.recipient_id, code, reason).await?;

    if c.is_dsn || c.envelope_from.trim().is_empty() {
        tracing::debug!(recipient = %c.recipient, "Avis de non-remise supprimé (DSN ou chemin de retour nul)");
        return Ok(());
    }

    let report = dsn::build(
        &cfg.hostname,
        &c.envelope_from,
        dsn::Kind::Failure,
        &[dsn::FailedRecipient {
            recipient:  c.recipient.clone(),
            status:     status_code(code),
            diagnostic: Some(format!("{code} {reason}")),
        }],
        &c.raw,
    );

    let sender_domain = c
        .envelope_from
        .rsplit_once('@')
        .map(|(_, d)| d.to_string())
        .unwrap_or_default();

    // The DSN is posted with the null return path (envelope_from empty) and
    // flagged is_dsn, so its own failure will not spawn another report. It gets
    // the same queue lifetime as any other message.
    queue::enqueue_with_lifetime(
        db,
        None,
        None,
        "",
        &report,
        true,
        &[(c.envelope_from.clone(), sender_domain)],
        cfg.outbound_lifetime_hours,
    )
    .await?;
    Ok(())
}

/// Maps an SMTP reply code onto an RFC 3463 status class: 5xx → 5.0.0,
/// otherwise 4.0.0. Good enough for the report; the exact detail is in the
/// Diagnostic-Code line.
fn status_code(code: u16) -> String {
    if code >= 500 { "5.0.0".to_string() } else { "4.0.0".to_string() }
}

#[cfg(test)]
mod tests {
    use super::status_code;

    #[test]
    fn status_class_follows_the_code() {
        assert_eq!(status_code(550), "5.0.0");
        assert_eq!(status_code(451), "4.0.0");
    }
}
