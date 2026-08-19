//! Journalling: an unconditional copy of every message the instance's SMTP
//! services receive or send, filed into one designated mailbox.
//!
//! An organisation under a retention obligation cannot rely on its users
//! keeping their own mail: a deleted message is gone, and a mailbox emptied
//! before an audit takes its evidence with it. A journal solves that by
//! capturing the message at the moment it crosses the instance, in a mailbox
//! nobody else writes to.
//!
//! ## What is captured, and what is not
//!
//! Capture happens at the two points where a message enters or leaves through
//! the SMTP services:
//!
//!   * everything an SMTP session accepts — foreign mail on the MX port, and
//!     everything an authenticated user submits from a mail client;
//!   * everything sent from the web interface by a hosted mailbox.
//!
//! It does NOT capture the mail of EXTERNAL accounts a user polls over IMAP
//! (that mail belongs to another provider and never crosses our SMTP), nor the
//! instance's own automatic messages (vacation replies, delivery reports),
//! which are notifications rather than correspondence. The setting's own text
//! says so: a journal whose limits are undocumented is worse than none, because
//! its gaps are only discovered when the archive is needed.
//!
//! ## Why it cannot loop
//!
//! The copy is filed with [`deliver::deliver_local`], the same call an ordinary
//! delivery uses — and this module is never called FROM that path. A journalled
//! copy therefore produces no second copy of itself.

use sqlx::PgPool;

use super::{
    config::ServerConfig,
    deliver::{self, Disposition},
    resolve::{self, Outcome},
};

/// Files a copy of `raw` into the configured journal mailbox.
///
/// Best-effort from end to end: journalling must never fail, delay or alter the
/// delivery that has already happened. Every problem is logged — loudly, since
/// a journal silently not recording is the one failure mode that matters — and
/// swallowed.
pub async fn archive(
    db: &PgPool,
    cfg: &ServerConfig,
    attachments_dir: &str,
    envelope_from: &str,
    raw: &[u8],
) {
    let address = cfg.archive_address.trim().to_ascii_lowercase();
    if address.is_empty() {
        return;
    }

    // The journal mailbox must be one of ours. A remote address would turn every
    // message the instance handles into an outbound copy to a third party — the
    // opposite of what an archive is for.
    if !cfg.is_local_domain(&address) {
        tracing::error!(
            archive = %address,
            "Archivage : l'adresse d'archivage n'est pas une adresse de cette instance — aucune copie conservée"
        );
        return;
    }

    let directory = resolve::PgDirectory::new(db);
    // Authenticated = true: the instance itself is depositing, so a distribution
    // list's "internal senders only" policy accepts it.
    let expansion = match resolve::resolve(&directory, cfg, envelope_from, true, &address).await {
        Ok(Outcome::Accept(expansion)) => expansion,
        Ok(Outcome::Refuse(refusal)) => {
            tracing::error!(
                archive = %address, reason = %refusal.reply(),
                "Archivage : l'adresse d'archivage est refusée — aucune copie conservée"
            );
            return;
        }
        Err(e) => {
            tracing::error!(error = %e, archive = %address, "Archivage : résolution de l'adresse impossible");
            return;
        }
    };

    if expansion.local.is_empty() {
        tracing::error!(
            archive = %address,
            "Archivage : l'adresse d'archivage ne mène à aucune boîte — aucune copie conservée"
        );
        return;
    }

    for delivery in &expansion.local {
        match deliver::deliver_local(
            db,
            cfg,
            envelope_from,
            &delivery.address,
            delivery.target,
            raw,
            attachments_dir,
            // A journal copy is filed in the inbox whatever the original's fate:
            // a message quarantined for a user is exactly the one an auditor
            // will look for, and burying it in the archive's spam folder would
            // hide it. Authentication verdicts are left out for the same reason
            // — this copy is evidence, not correspondence to be judged again.
            Disposition::Inbox,
            None,
            None,
        )
        .await
        {
            Ok(_) => {}
            Err(e) => tracing::error!(
                error = %e, archive = %delivery.address,
                "Archivage : copie non conservée"
            ),
        }
    }
}
