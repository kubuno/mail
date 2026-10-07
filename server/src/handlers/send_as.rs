//! "Send mail as" endpoints — manage the user's verified sender identities.
//!
//! Adding an address does not make it usable: the instance mails a confirmation
//! code TO the address, and only entering that code back marks it verified. This
//! is the ownership proof that stops the instance from being turned into an open
//! spoofer, and it mirrors Gmail's "send mail as" flow.
//!
//! The code is delivered through the instance's OWN send path, exactly like the
//! vacation responder: a code for a LOCAL address is filed straight into its
//! mailbox; a code for a REMOTE address is handed to the outbound queue (subject
//! to the administrator's `outbound_enabled` switch).
//!
//! No endpoint ever returns the code. `GET /send-as` selects the public columns
//! only, and verify/resend compare against it server-side.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use lettre::message::{header::ContentType, Mailbox, Message, MultiPart, SinglePart};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    server::{config, deliver, hygiene, queue, resolve},
    services::send_as,
    state::AppState,
};

// ── Wire shapes (camelCase, so the settings tab round-trips them directly) ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendAsDto {
    pub id:             Uuid,
    pub email:          String,
    pub display_name:   String,
    pub verified:       bool,
    pub treat_as_alias: bool,
    pub created_at:     DateTime<Utc>,
}

impl From<send_as::SendAsAddress> for SendAsDto {
    fn from(a: send_as::SendAsAddress) -> Self {
        SendAsDto {
            id:             a.id,
            email:          a.email,
            display_name:   a.display_name,
            verified:       a.verified,
            treat_as_alias: a.treat_as_alias,
            created_at:     a.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddSendAsDto {
    pub email:        String,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifyDto {
    #[serde(default)]
    pub code: String,
}

// ── Endpoints ───────────────────────────────────────────────────────────────

/// GET /send-as — the user's sender identities (never the pending code).
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<SendAsDto>>, MailError> {
    let rows = send_as::list(&state.db, user.id).await.map_err(MailError::Internal)?;
    Ok(Json(rows.into_iter().map(SendAsDto::from).collect()))
}

/// POST /send-as — add an address and mail it a confirmation code.
///
/// The address is created unverified with its pending code BEFORE the code is
/// sent, so a delivery failure still leaves a row the user can resend from — the
/// error is surfaced, nothing is silently lost.
pub async fn add(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<AddSendAsDto>,
) -> Result<Json<SendAsDto>, MailError> {
    let email = send_as::normalize_email(&body.email);
    if !send_as::is_valid_email(&email) {
        return Err(MailError::Validation("Adresse e-mail invalide.".into()));
    }
    let display_name = body.display_name.trim().to_string();

    if send_as::exists(&state.db, user.id, &email)
        .await
        .map_err(MailError::Internal)?
    {
        return Err(MailError::Conflict("Cette adresse est déjà dans la liste.".into()));
    }

    let issued = send_as::issue_code(Utc::now());
    let row = send_as::insert(
        &state.db,
        user.id,
        &email,
        &display_name,
        &issued.code,
        issued.expires_at,
    )
    .await
    .map_err(MailError::Internal)?;

    // Best-effort semantics: the row exists now; a send failure is reported but
    // the address stays, verifiable through a later resend.
    deliver_confirmation(&state, &user.email, &email, &issued.code).await?;

    Ok(Json(SendAsDto::from(row)))
}

/// POST /send-as/:id/resend — regenerate the code and mail it again.
pub async fn resend(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let secret = send_as::load_secret(&state.db, user.id, id)
        .await
        .map_err(MailError::Internal)?
        .ok_or_else(|| MailError::NotFound("Adresse d'envoi introuvable".into()))?;

    // Never mail a code to an address that is already confirmed.
    if secret.verified {
        return Err(MailError::Conflict("Cette adresse est déjà vérifiée.".into()));
    }

    // Anti-abuse: bound how often a stranger's inbox can be hit.
    if !send_as::can_resend(secret.updated_at, Utc::now(), send_as::RESEND_MIN_INTERVAL_SECS) {
        return Err(MailError::Conflict(
            "Un code vient d'être envoyé. Patientez une minute avant de réessayer.".into(),
        ));
    }

    let issued = send_as::issue_code(Utc::now());
    send_as::regenerate_code(&state.db, user.id, id, &issued.code, issued.expires_at)
        .await
        .map_err(MailError::Internal)?;

    deliver_confirmation(&state, &user.email, &secret.email, &issued.code).await?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /send-as/:id/verify — confirm ownership with the mailed code.
///
/// No information leakage: a wrong code and an expired code return the same
/// generic message, so a caller cannot tell them apart.
pub async fn verify(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<VerifyDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    let secret = send_as::load_secret(&state.db, user.id, id)
        .await
        .map_err(MailError::Internal)?
        .ok_or_else(|| MailError::NotFound("Adresse d'envoi introuvable".into()))?;

    // Already verified: idempotent success rather than a confusing "invalid code".
    if secret.verified {
        return Ok(Json(serde_json::json!({ "verified": true })));
    }

    let ok = send_as::code_is_valid(
        secret.code.as_deref(),
        secret.expires_at,
        Utc::now(),
        &body.code,
    );
    if !ok {
        return Err(MailError::Validation("Code invalide ou expiré.".into()));
    }

    send_as::mark_verified(&state.db, user.id, id)
        .await
        .map_err(MailError::Internal)?;

    Ok(Json(serde_json::json!({ "verified": true })))
}

/// DELETE /send-as/:id — remove a sender identity.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let affected = send_as::delete(&state.db, user.id, id)
        .await
        .map_err(MailError::Internal)?;
    if affected == 0 {
        return Err(MailError::NotFound("Adresse d'envoi introuvable".into()));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Confirmation-code delivery ──────────────────────────────────────────────

/// Mails the confirmation code to `to_email` through the instance's own send
/// path: local recipients are filed directly into their mailbox, remote ones are
/// enqueued for outbound delivery (when the administrator allows it).
///
/// `from_hint` is the requesting user's own address; it is used as the sender
/// when it is a valid address, falling back to `postmaster@<hostname>` so the
/// message always has a sane envelope.
async fn deliver_confirmation(
    state: &AppState,
    from_hint: &str,
    to_email: &str,
    code: &str,
) -> Result<(), MailError> {
    // The live server configuration: which domains are ours, and whether
    // outbound delivery is switched on.
    let http = reqwest::Client::new();
    let cfg = config::fetch(&http, &state.settings).await.ok_or_else(|| {
        MailError::Internal(anyhow::anyhow!(
            "Configuration du serveur de messagerie illisible : impossible d'envoyer le code de confirmation"
        ))
    })?;

    // Sender: the user's own address when it is usable, else a system address on
    // this instance so the envelope is always well-formed.
    let from_email = {
        let candidate = from_hint.trim().to_ascii_lowercase();
        if send_as::is_valid_email(&candidate) {
            candidate
        } else {
            format!("postmaster@{}", cfg.hostname)
        }
    };

    let to_lc = to_email.trim().to_ascii_lowercase();
    let raw = build_confirmation(&from_email, &to_lc, code)
        .map_err(|e| MailError::Smtp(e.to_string()))?;

    if cfg.is_local_domain(&to_lc) {
        // A local address: file the confirmation straight into its mailbox, the
        // same way an inbound message is delivered.
        let directory = resolve::PgDirectory::new(&state.db);
        match resolve::resolve(&directory, &cfg, &from_email, true, &to_lc).await {
            Ok(resolve::Outcome::Accept(expansion)) => {
                let mut delivered = false;
                for delivery in expansion.local {
                    match deliver::deliver_local(
                        &state.db,
                        &cfg,
                        &from_email,
                        &delivery.address,
                        delivery.target,
                        &raw,
                        &state.settings.mail.attachments_dir,
                        deliver::Disposition::Inbox,
                        None,
                        None,
                    )
                    .await
                    {
                        Ok(_) => delivered = true,
                        Err(e) => tracing::error!(error = %e, "Code d'envoi : dépôt local échoué"),
                    }
                }
                // A local alias may forward to remote addresses; honour those too.
                if !expansion.remote.is_empty()
                    && cfg.outbound_enabled
                    && enqueue_remote(state, &cfg, &from_email, &raw, &expansion.remote).await?
                {
                    delivered = true;
                }
                if delivered {
                    Ok(())
                } else {
                    Err(MailError::Internal(anyhow::anyhow!(
                        "Le code de confirmation n'a pu être remis à aucune boîte"
                    )))
                }
            }
            Ok(resolve::Outcome::Refuse(_)) => Err(MailError::Validation(
                "Adresse locale introuvable sur cette instance.".into(),
            )),
            Err(e) => Err(MailError::Internal(e)),
        }
    } else if cfg.outbound_enabled {
        // A remote address: hand it to the outbound queue.
        let stamped = hygiene::prepend_received(&raw, "local", &cfg.hostname);
        let domain = to_lc.rsplit_once('@').map(|(_, d)| d.to_string()).unwrap_or_default();
        queue::enqueue_with_lifetime(
            &state.db,
            None,
            None,
            &from_email,
            &stamped,
            false,
            &[(to_lc, domain)],
            cfg.outbound_lifetime_hours,
        )
        .await
        .map_err(MailError::Internal)?;
        Ok(())
    } else {
        Err(MailError::Validation(
            "L'envoi sortant est désactivé par l'administrateur : impossible d'envoyer le code à une adresse externe.".into(),
        ))
    }
}

/// Enqueues the confirmation for a list of remote recipients (alias forwarding).
/// Returns whether anything was enqueued.
async fn enqueue_remote(
    state: &AppState,
    cfg: &config::ServerConfig,
    from_email: &str,
    raw: &[u8],
    remote: &[String],
) -> Result<bool, MailError> {
    let stamped = hygiene::prepend_received(raw, "local", &cfg.hostname);
    let list: Vec<(String, String)> = remote
        .iter()
        .map(|r| {
            let domain = r.rsplit_once('@').map(|(_, d)| d.to_string()).unwrap_or_default();
            (r.clone(), domain)
        })
        .collect();
    if list.is_empty() {
        return Ok(false);
    }
    queue::enqueue_with_lifetime(
        &state.db,
        None,
        None,
        from_email,
        &stamped,
        false,
        &list,
        cfg.outbound_lifetime_hours,
    )
    .await
    .map_err(MailError::Internal)?;
    Ok(true)
}

/// Builds the RFC 5322 confirmation message (text + HTML alternative).
fn build_confirmation(from_email: &str, to_email: &str, code: &str) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context;
    let from_mb: Mailbox = from_email.parse().context("Adresse expéditeur du code invalide")?;
    let to_mb: Mailbox = to_email.parse().context("Adresse destinataire du code invalide")?;

    let subject = "Confirmez votre adresse d'envoi Kubuno";

    let text = format!(
        "Vous (ou quelqu'un) avez demandé à ajouter cette adresse comme expéditeur \
         dans Kubuno.\n\n\
         Votre code de confirmation : {code}\n\n\
         Saisissez-le dans Kubuno Mail → Paramètres → Comptes → « Envoyer des \
         e-mails en tant que » pour valider l'adresse. Le code expire dans 24 heures.\n\n\
         Si vous n'êtes pas à l'origine de cette demande, ignorez ce message : \
         l'adresse ne pourra pas être utilisée sans ce code.\n"
    );

    // Minimal, self-contained HTML — the code stands out, no external assets.
    let html = format!(
        "<div style=\"font-family:system-ui,Arial,sans-serif;font-size:14px;color:#202124;line-height:1.6\">\
           <p>Vous (ou quelqu'un) avez demandé à ajouter cette adresse comme expéditeur dans <strong>Kubuno</strong>.</p>\
           <p>Votre code de confirmation&nbsp;:</p>\
           <p style=\"font-size:26px;font-weight:700;letter-spacing:4px;margin:16px 0\">{code}</p>\
           <p>Saisissez-le dans <em>Kubuno Mail → Paramètres → Comptes → « Envoyer des e-mails en tant que »</em> pour valider l'adresse. Le code expire dans 24&nbsp;heures.</p>\
           <p style=\"color:#5f6368\">Si vous n'êtes pas à l'origine de cette demande, ignorez ce message&nbsp;: l'adresse ne pourra pas être utilisée sans ce code.</p>\
         </div>"
    );

    let alternative = MultiPart::alternative()
        .singlepart(SinglePart::builder().header(ContentType::TEXT_PLAIN).body(text))
        .singlepart(SinglePart::builder().header(ContentType::TEXT_HTML).body(html));

    let email = Message::builder()
        .from(from_mb)
        .to(to_mb)
        .subject(subject)
        .multipart(alternative)
        .context("Construction du message de confirmation")?;
    Ok(email.formatted())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_message_carries_the_code_and_headers() {
        let raw = build_confirmation("me@kubuno.com", "someone@example.com", "ABCD2345")
            .expect("build");
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("Subject: Confirmez"));
        assert!(text.contains("someone@example.com"));
        // The code appears in both the text and HTML alternatives.
        assert!(text.contains("ABCD2345"));
    }
}
