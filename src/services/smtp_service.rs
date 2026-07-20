use anyhow::{Context, Result};
use base64::Engine;
use lettre::{
    message::{header::ContentType, Attachment, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};

use crate::models::{EmailAddress, SendMailDto};

pub struct SmtpConfig {
    pub host:     String,
    pub port:     u16,
    pub security: String,
    pub username: String,
    pub password: String,
    pub from_name:    String,
    pub from_email:   String,
}

pub async fn send_message(cfg: &SmtpConfig, dto: &SendMailDto, subject: &str, body_html: &str) -> Result<()> {
    let from = if cfg.from_name.is_empty() {
        cfg.from_email.parse().context("Adresse expéditeur invalide")?
    } else {
        format!("{} <{}>", cfg.from_name, cfg.from_email)
            .parse()
            .context("Adresse expéditeur invalide")?
    };

    let mut builder = Message::builder().from(from);

    for addr in &dto.to_addresses {
        builder = builder.to(format_addr(addr).parse().context("Adresse destinataire invalide")?);
    }
    for addr in dto.cc_addresses.as_deref().unwrap_or(&[]) {
        builder = builder.cc(format_addr(addr).parse().context("Adresse CC invalide")?);
    }
    // BCC: lettre adds these to the SMTP envelope then strips the Bcc header from
    // the formatted message (default behaviour) — hidden recipients stay hidden.
    for addr in dto.bcc_addresses.as_deref().unwrap_or(&[]) {
        builder = builder.bcc(format_addr(addr).parse().context("Adresse BCC invalide")?);
    }

    builder = builder.subject(subject);

    // Corps HTML + texte alternatif
    let body_text = html2text::from_read(body_html.as_bytes(), 80);
    let alternative = MultiPart::alternative()
        .singlepart(SinglePart::builder().header(ContentType::TEXT_PLAIN).body(body_text))
        .singlepart(SinglePart::builder().header(ContentType::TEXT_HTML).body(body_html.to_string()));

    // Pièces jointes (base64) → message « mixed » englobant le corps alternatif.
    let attachments = dto.attachments.as_deref().unwrap_or(&[]);
    let email = if attachments.is_empty() {
        builder.multipart(alternative).context("Construction du message")?
    } else {
        let mut mixed = MultiPart::mixed().multipart(alternative);
        for a in attachments {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(a.content.as_bytes())
                .context("Pièce jointe base64 invalide")?;
            let ct = a.mime.parse::<ContentType>().unwrap_or(ContentType::parse("application/octet-stream").unwrap());
            mixed = mixed.singlepart(Attachment::new(a.filename.clone()).body(bytes, ct));
        }
        builder.multipart(mixed).context("Construction du message")?
    };

    let creds = Credentials::new(cfg.username.clone(), cfg.password.clone());

    let transport: AsyncSmtpTransport<Tokio1Executor> = match cfg.security.as_str() {
        "ssl" => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)
            .context("Transport SMTPS")?
            .port(cfg.port)
            .credentials(creds)
            .build(),
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)
            .context("Transport SMTP+STARTTLS")?
            .port(cfg.port)
            .credentials(creds)
            .build(),
    };

    transport.send(email).await.context("Envoi SMTP")?;
    Ok(())
}

fn format_addr(addr: &EmailAddress) -> String {
    match &addr.name {
        Some(n) if !n.is_empty() => format!("{} <{}>", n, addr.email),
        _ => addr.email.clone(),
    }
}
