//! Publishing events to the core's internal event bus.
//!
//! The core deserialises the body as its `AppEvent` enum, which is adjacently
//! tagged (`#[serde(tag = "type", content = "payload")]`). For the `Custom`
//! variant — the only one a module may emit — the wire shape is therefore:
//!
//! ```jsonc
//! { "type": "Custom",
//!   "payload": {                 // the Custom struct
//!     "event_type": "mail.received",
//!     "module_id":  "mail",
//!     "payload": { … }           // the free-form Value the push mapper reads
//!   } }
//! ```
//!
//! The push mapper (`core/.../push/mapping.rs`) reads `recipient_user_ids`,
//! `title`, `body` and `resource_id` out of that innermost `payload`.

use std::sync::OnceLock;

use reqwest::Client;
use serde_json::Value;
use uuid::Uuid;

/// A process-wide HTTP client for best-effort event publishing. Reused so a
/// burst of incoming mail does not build a fresh connection pool per message.
fn http() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(Client::new)
}

/// POSTs a `Custom` `AppEvent` to `{core}/internal/events/publish`.
///
/// `payload` is the innermost free-form object the core's push mapper reads.
/// Returns an error the caller may log; publishing must never be load-bearing.
pub async fn publish_custom(
    core_url: &str,
    internal_secret: &str,
    event_type: &str,
    payload: Value,
) -> anyhow::Result<()> {
    let url = format!("{core_url}/internal/events/publish");
    let body = serde_json::json!({
        "type": "Custom",
        "payload": {
            "event_type": event_type,
            "module_id":  "mail",
            "payload":    payload,
        },
    });

    http()
        .post(&url)
        .header("X-Internal-Secret", internal_secret)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// Notifies the core that a message just landed in a local recipient's inbox,
/// so it can fan out a push notification to that user's registered devices.
///
/// Best-effort: a failure is logged and swallowed — a mail that was stored must
/// never be lost because the notification could not be sent. No body content
/// beyond the subject ever leaves this module.
#[allow(clippy::too_many_arguments)]
pub async fn notify_incoming_mail(
    core_url: &str,
    internal_secret: &str,
    recipient: Uuid,
    thread_id: Uuid,
    sender_name: Option<&str>,
    sender_email: &str,
    subject: &str,
) {
    // A module started before the core answered, or a misconfigured deployment,
    // leaves these empty: there is nowhere to publish to, so do nothing.
    if core_url.trim().is_empty() || internal_secret.trim().is_empty() {
        return;
    }

    // Title = a human name when we have one, otherwise the bare address. Body =
    // the subject, truncated so a very long subject cannot bloat the push.
    let title = sender_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(sender_email)
        .to_string();
    let body: String = {
        let subject = subject.trim();
        let s = if subject.is_empty() { "(sans objet)" } else { subject };
        s.chars().take(140).collect()
    };

    let payload = serde_json::json!({
        "recipient_user_ids": [recipient],
        "title":              title,
        "body":               body,
        "resource_id":        thread_id.to_string(),
    });

    if let Err(e) = publish_custom(core_url, internal_secret, "mail.received", payload).await {
        tracing::warn!(error = %e, %recipient, "Publication de l'event « mail reçu » échouée (push ignoré)");
    }

    // Tell whoever keeps an address book that this recipient just dealt with
    // this correspondent. Published on the CORE's bus, not sent to a module:
    // mail knows nothing about contacts, and an instance without an address book
    // simply has no subscriber. Best-effort like everything else here.
    notify_interlocutor(core_url, internal_secret, recipient, sender_email, sender_name).await;
}

/// Reports one person the user exchanged with, for the "Other contacts" list of
/// whichever module collects them.
///
/// The address is the identity: a display name is a nicety that may be missing,
/// spoofed or change from one message to the next, so it travels as a hint and
/// the subscriber decides what to do with it.
pub async fn notify_interlocutor(
    core_url: &str,
    internal_secret: &str,
    owner: Uuid,
    email: &str,
    display_name: Option<&str>,
) {
    let email = email.trim();
    if core_url.trim().is_empty() || internal_secret.trim().is_empty() || email.is_empty() {
        return;
    }
    let name = display_name.map(str::trim).filter(|s| !s.is_empty());
    let payload = serde_json::json!({
        "owner_id": owner.to_string(),
        "interlocutors": [{
            "kind":         "email",
            "value":        email,
            "display_name": name,
        }],
    });
    if let Err(e) = publish_custom(core_url, internal_secret, "mail.interlocutor", payload).await {
        tracing::warn!(error = %e, %owner, "Publication de l'event « interlocuteur » échouée (ignorée)");
    }
}
