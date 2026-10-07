use anyhow::{Context, Result};
use base64::Engine;
use lettre::{
    message::{header::ContentType, Attachment, Mailbox, MultiPart, SinglePart},
    transport::smtp::authentication::{Credentials, Mechanism},
    Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};

use crate::models::SendMailDto;

pub struct SmtpConfig {
    pub host:     String,
    pub port:     u16,
    pub security: String,
    pub username: String,
    /// Account password, or an OAuth2 access token when `xoauth2` is true.
    pub password: String,
    /// Authenticate with the SASL XOAUTH2 mechanism (OAuth accounts).
    pub xoauth2:  bool,
    pub from_name:    String,
    pub from_email:   String,
}

/// Sends the message and returns its Message-ID (without angle brackets),
/// so the caller can store a matching local copy in the Sent folder.
pub async fn send_message(
    cfg: &SmtpConfig,
    dto: &SendMailDto,
    subject: &str,
    body_html: &str,
    pgp: Option<&crate::services::pgp_mime::PgpParams>,
    autocrypt_key: Option<&str>,
    sender: Option<&str>,
) -> Result<String> {
    let (email, message_id) = build_email(&cfg.from_name, &cfg.from_email, dto, subject, body_html, pgp, autocrypt_key, sender)?;
    let transport = build_transport(cfg)?;
    transport.send(email).await.context("Envoi SMTP")?;
    Ok(message_id)
}

/// Assembles the RFC 5322 message (alternative text/HTML body, attachments) and
/// returns it together with its Message-ID (without angle brackets). Kept apart
/// from [`send_message`] so a LOCAL account — which has no external SMTP relay —
/// can obtain the exact same bytes (`email.formatted()`) to hand to local
/// delivery and the instance's outbound queue.
#[allow(clippy::too_many_arguments)] // a message builder: from, body, PGP, autocrypt, delegated Sender…
pub fn build_email(
    from_name: &str,
    from_email: &str,
    dto: &SendMailDto,
    subject: &str,
    body_html: &str,
    pgp: Option<&crate::services::pgp_mime::PgpParams>,
    autocrypt_key: Option<&str>,
    sender: Option<&str>,
) -> Result<(Message, String)> {
    let mut builder = Message::builder()
        .from(mailbox(from_name, from_email).context("Adresse expéditeur invalide")?);

    // Account delegation (RFC 5322 §3.6.2): `From:` names the mailbox owner
    // (the grantor), and `Sender:` names the party who materially sent it (the
    // delegate). Only stamped for a delegated send; a self-send has none.
    if let Some(sender_email) = sender {
        builder = builder.sender(mailbox("", sender_email).context("Adresse Sender invalide")?);
    }

    // Autocrypt: advertise our public key in a top-level (always cleartext)
    // header. Built from the sender's armored certificate; a serialization
    // failure just drops the header rather than failing the send.
    if let Some(public_armored) = autocrypt_key {
        if let Ok(binary) = crate::services::pgp::public_armored_to_binary(public_armored) {
            let value = crate::services::autocrypt::header_value(from_email, &binary, true);
            builder = builder.header(crate::services::autocrypt::AutocryptMimeHeader(value));
        }
    }

    for addr in &dto.to_addresses {
        builder = builder.to(mailbox(addr.name.as_deref().unwrap_or(""), &addr.email)
            .context("Adresse destinataire invalide")?);
    }
    for addr in dto.cc_addresses.as_deref().unwrap_or(&[]) {
        builder = builder.cc(mailbox(addr.name.as_deref().unwrap_or(""), &addr.email)
            .context("Adresse CC invalide")?);
    }
    // BCC: lettre adds these to the SMTP envelope then strips the Bcc header from
    // the formatted message (default behaviour) — hidden recipients stay hidden.
    for addr in dto.bcc_addresses.as_deref().unwrap_or(&[]) {
        builder = builder.bcc(mailbox(addr.name.as_deref().unwrap_or(""), &addr.email)
            .context("Adresse BCC invalide")?);
    }

    builder = builder.subject(subject);

    // Corps HTML + texte alternatif. Under PGP the alternative is base64-encoded
    // so its exact bytes survive SMTP (a signature over a mutable body breaks).
    let body_text = html2text::from_read(body_html.as_bytes(), 80);
    let alternative = match pgp {
        Some(_) => crate::services::pgp_mime::alternative_body(&body_text, body_html),
        None => MultiPart::alternative()
            .singlepart(SinglePart::builder().header(ContentType::TEXT_PLAIN).body(body_text))
            .singlepart(SinglePart::builder().header(ContentType::TEXT_HTML).body(body_html.to_string())),
    };

    // Pièces jointes (base64) → message « mixed » englobant le corps alternatif.
    let attachments = dto.attachments.as_deref().unwrap_or(&[]);
    let content = if attachments.is_empty() {
        alternative
    } else {
        let mut mixed = MultiPart::mixed().multipart(alternative);
        for a in attachments {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(a.content.as_bytes())
                .context("Pièce jointe base64 invalide")?;
            let ct = a.mime.parse::<ContentType>().unwrap_or(ContentType::parse("application/octet-stream").unwrap());
            mixed = mixed.singlepart(Attachment::new(a.filename.clone()).body(bytes, ct));
        }
        mixed
    };

    // Sign and/or encrypt the whole content into a PGP/MIME part (RFC 3156).
    let content = match pgp {
        Some(p) => crate::services::pgp_mime::wrap(content, p).context("Protection PGP/MIME du message")?,
        None => content,
    };
    let email = builder.multipart(content).context("Construction du message")?;

    // lettre generates a Message-ID at build time; normalize it like mail-parser
    // does on the sync side (no angle brackets) so dedup comparisons match.
    let message_id = email
        .headers()
        .get_raw("Message-ID")
        .map(|v| v.trim().trim_matches(|c| c == '<' || c == '>').to_string())
        .unwrap_or_else(|| format!("{}@kubuno.generated", uuid::Uuid::new_v4()));

    Ok((email, message_id))
}

/// Builds the SMTP transport for the account: password (PLAIN/LOGIN) or
/// XOAUTH2 with an access token. Also used by the connection test.
pub fn build_transport(cfg: &SmtpConfig) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let creds = Credentials::new(cfg.username.clone(), cfg.password.clone());

    let mut builder = match cfg.security.as_str() {
        "ssl" => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)
            .context("Transport SMTPS")?,
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)
            .context("Transport SMTP+STARTTLS")?,
    };
    builder = builder.port(cfg.port).credentials(creds);
    if cfg.xoauth2 {
        builder = builder.authentication(vec![Mechanism::Xoauth2]);
    }
    Ok(builder.build())
}

/// Builds a `Mailbox` from a free-text display name and an address, letting
/// lettre quote/encode the name (RFC 5322 / 2047) rather than interpolating it
/// into `"name <addr>"` and re-parsing — a name carrying `@`, a comma or an
/// accent would otherwise fail the parse and take the whole send down with it
/// (a local mailbox whose display name defaults to its own address did exactly
/// that). A display name that is empty, or equal to the address, is dropped so a
/// bare `x@y` never renders as `x@y <x@y>`.
fn mailbox(name: &str, email: &str) -> Result<Mailbox> {
    let email = email.trim();
    let address: Address = email.parse().with_context(|| format!("Adresse « {email} » invalide"))?;
    let name = name.trim();
    let display = if name.is_empty() || name.eq_ignore_ascii_case(email) {
        None
    } else {
        Some(name.to_string())
    };
    Ok(Mailbox::new(display, address))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EmailAddress, SendMailDto};
    use uuid::Uuid;

    fn dto(to: &str) -> SendMailDto {
        SendMailDto {
            account_id:   Uuid::nil(),
            to_addresses: vec![EmailAddress { name: None, email: to.into() }],
            cc_addresses: None,
            bcc_addresses: None,
            subject:      "Bonjour".into(),
            body_html:    "<p>Bonjour</p>".into(),
            reply_to_id:  None,
            draft_id:     None,
            scheduled_at: None,
            attachments:  None,
            sign:         None,
            encrypt:      None,
            label_ids:    None,
        }
    }

    /// A DELEGATED send: `From:` names the grantor (the mailbox owner) while
    /// `Sender:` names the delegate who materially sent it (RFC 5322 §3.6.2).
    #[test]
    fn delegated_send_stamps_sender_distinct_from_from() {
        let (email, _mid) = build_email(
            "Grantor",
            "grantor@kubuno.local",
            &dto("dest@example.com"),
            "Bonjour",
            "<p>Bonjour</p>",
            None,
            None,
            Some("delegate@kubuno.local"),
        )
        .expect("build");
        let raw = String::from_utf8_lossy(&email.formatted()).to_string();
        assert!(raw.contains("From:") && raw.contains("grantor@kubuno.local"));
        assert!(
            raw.contains("Sender: delegate@kubuno.local")
                || raw.contains("Sender:") && raw.contains("delegate@kubuno.local"),
            "the delegate must appear in a Sender header:\n{raw}"
        );
    }

    /// A self-send (no delegation) carries NO `Sender:` header — `From:` alone.
    #[test]
    fn self_send_has_no_sender_header() {
        let (email, _mid) = build_email(
            "Me",
            "me@kubuno.local",
            &dto("dest@example.com"),
            "Bonjour",
            "<p>Bonjour</p>",
            None,
            None,
            None,
        )
        .expect("build");
        let raw = String::from_utf8_lossy(&email.formatted()).to_string();
        assert!(!raw.contains("Sender:"), "a self-send must not carry a Sender header:\n{raw}");
    }
}
