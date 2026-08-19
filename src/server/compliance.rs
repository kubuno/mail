//! Attachment and content compliance: the gate a message passes after it has
//! been authenticated and before it is filed, and the one an outgoing message
//! must clear before it is queued.
//!
//! Sender authentication (`authres`) answers "is this really from whom it
//! claims?". It says nothing about what the message *carries* — an executable
//! renamed `facture.exe`, a password-protected archive no scanner can open, a
//! phrase an organisation may not let out. That is this module's question, and
//! it is asked in both directions:
//!
//!   * on RECEPTION, after the authentication policy, so a message the operator
//!     forbids never reaches a mailbox;
//!   * on SENDING, before a message is handed to the outbound queue, so the
//!     same rule that keeps a file out also keeps it in.
//!
//! Everything here is pure: it takes the administrator's configuration and the
//! RFC 5322 bytes, and returns a verdict. The callers decide what a verdict
//! costs (an SMTP refusal, a spam filing, a validation error in the API).
//!
//! ## Two deliberate limits
//!
//! * The extension check reads the **declared filename**, not the file's magic
//!   bytes. A renamed executable therefore passes it. Sniffing every content
//!   type is a different (and much larger) piece of work; the setting's own
//!   description says so rather than implying a protection that is not there.
//! * "Encrypted archive" recognises the ZIP family, whose local file header
//!   states encryption in a flag bit. Other container formats are not claimed.

use mail_parser::{MessageParser, MimeHeaders};

use super::config::{PolicyAction, ServerConfig};

/// Characters kept when a filename or an expression is quoted back in a log
/// line or an SMTP reply. Anything else — control characters above all — would
/// let a crafted attachment name inject a second reply line.
const MAX_QUOTED: usize = 80;

/// What the compliance rules decided about one message.
///
/// `reason` is written for an administrator reading the logs and is safe to put
/// in an SMTP reply: it names what matched, never the message body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub action: PolicyAction,
    pub reason: String,
}

/// True when at least one compliance rule is configured. Checked before parsing
/// so an instance that uses none of them pays nothing per message.
pub fn is_armed(cfg: &ServerConfig) -> bool {
    attachment_rules_armed(cfg) || !cfg.content_blocked_expressions.is_empty()
}

fn attachment_rules_armed(cfg: &ServerConfig) -> bool {
    !cfg.attachment_blocked_extensions.is_empty()
        || cfg.attachment_max_bytes > 0
        || cfg.attachment_block_encrypted
}

/// Applies the attachment and content rules to one RFC 5322 message.
///
/// `None` = nothing matched, or nothing is configured. The attachment rules are
/// evaluated first: a forbidden file is a stronger, more specific finding than a
/// word appearing somewhere in a body.
pub fn scan(cfg: &ServerConfig, raw: &[u8]) -> Option<Verdict> {
    if !is_armed(cfg) {
        return None;
    }
    // A message we cannot parse is not silently cleared: it is simply not
    // judged here. The delivery path refuses an unparsable message on its own.
    let parsed = MessageParser::default().parse(raw)?;

    if attachment_rules_armed(cfg) {
        for part in parsed.attachments() {
            let name = part.attachment_name().unwrap_or("");
            if let Some(reason) = attachment_reason(cfg, name, part.contents()) {
                return Some(Verdict { action: cfg.attachment_action, reason });
            }
        }
    }

    if !cfg.content_blocked_expressions.is_empty() {
        let subject = parsed.subject().unwrap_or("").to_lowercase();
        let text = parsed.body_text(0).map(|b| b.to_lowercase()).unwrap_or_default();
        let html = parsed.body_html(0).map(|b| b.to_lowercase()).unwrap_or_default();
        for expression in &cfg.content_blocked_expressions {
            if subject.contains(expression.as_str())
                || text.contains(expression.as_str())
                || html.contains(expression.as_str())
            {
                return Some(Verdict {
                    action: cfg.content_action,
                    reason: format!("expression interdite « {} »", quotable(expression)),
                });
            }
        }
    }

    None
}

/// Why this one attachment is refused, or `None` when it is acceptable.
fn attachment_reason(cfg: &ServerConfig, name: &str, bytes: &[u8]) -> Option<String> {
    let quoted = quotable(name);

    if cfg.attachment_max_bytes > 0 && bytes.len() > cfg.attachment_max_bytes {
        return Some(format!(
            "pièce jointe « {quoted} » de {} octets, au-dessus de la limite de {}",
            bytes.len(),
            cfg.attachment_max_bytes
        ));
    }

    if let Some(extension) = extension_of(name) {
        if cfg.attachment_blocked_extensions.iter().any(|blocked| *blocked == extension) {
            return Some(format!("pièce jointe « {quoted} » : extension « {extension} » interdite"));
        }
    }

    if cfg.attachment_block_encrypted && is_encrypted_archive(bytes) {
        return Some(format!("archive chiffrée « {quoted} » : contenu non analysable"));
    }

    None
}

/// The lower-cased extension of a filename, without its dot. `None` when the
/// name has none, or when what follows the last dot is not a plausible
/// extension (empty, or longer than any real one).
pub fn extension_of(name: &str) -> Option<String> {
    let base = name.rsplit(|c| c == '/' || c == '\\').next().unwrap_or(name).trim();
    let (_, extension) = base.rsplit_once('.')?;
    let extension = extension.trim().to_ascii_lowercase();
    if extension.is_empty() || extension.len() > 12 || !extension.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(extension)
}

/// True when the bytes are a ZIP archive whose first entry is encrypted.
///
/// Bit 0 of the local file header's general-purpose flag says so (APPNOTE
/// §4.4.4). A password-protected archive cannot be scanned by anything — which
/// is exactly why malware travels in one.
pub fn is_encrypted_archive(bytes: &[u8]) -> bool {
    if bytes.len() < 8 || &bytes[..4] != b"PK\x03\x04" {
        return false;
    }
    let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
    flags & 0x0001 != 0
}

/// Makes a filename or an expression safe to put in a log line or an SMTP
/// reply: control characters — CR and LF above all — become spaces, and the
/// result is bounded. Without this, an attachment named with a bare CRLF would
/// inject a second SMTP reply line.
fn quotable(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() || c == '«' || c == '»' { ' ' } else { c })
        .take(MAX_QUOTED)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Appends the instance's compliance footer to an HTML body.
///
/// Returns the body untouched when no footer is configured, so the common case
/// costs one comparison. The footer is administrator-supplied and inserted as
/// written: it is meant to carry a legal mention, which needs a link and a line
/// break to read properly. The plain-text alternative is derived from this HTML
/// by the message builder, so the footer reaches both parts.
pub fn with_footer(body_html: &str, footer: &str) -> String {
    let footer = footer.trim();
    if footer.is_empty() {
        return body_html.to_string();
    }
    format!(
        "{body_html}<div data-kubuno-footer=\"1\" style=\"margin-top:16px;padding-top:8px;border-top:1px solid #ddd;font-size:12px;color:#666\">{footer}</div>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(subject: &str, body: &str) -> Vec<u8> {
        format!("From: a@b.test\r\nTo: c@d.test\r\nSubject: {subject}\r\n\r\n{body}\r\n").into_bytes()
    }

    /// A message with an attachment of the named type and content.
    fn with_attachment(filename: &str, content_b64: &str) -> Vec<u8> {
        format!(
            "From: a@b.test\r\nTo: c@d.test\r\nSubject: pj\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: multipart/mixed; boundary=\"XX\"\r\n\r\n\
             --XX\r\nContent-Type: text/plain\r\n\r\nbonjour\r\n\
             --XX\r\nContent-Type: application/octet-stream\r\n\
             Content-Transfer-Encoding: base64\r\n\
             Content-Disposition: attachment; filename=\"{filename}\"\r\n\r\n\
             {content_b64}\r\n--XX--\r\n"
        )
        .into_bytes()
    }

    fn cfg() -> ServerConfig {
        ServerConfig::default()
    }

    #[test]
    fn nothing_configured_means_nothing_is_scanned() {
        let c = cfg();
        assert!(!is_armed(&c));
        assert_eq!(scan(&c, &message("bonjour", "texte")), None);
    }

    #[test]
    fn a_blocked_extension_is_caught_and_carries_the_configured_action() {
        let c = ServerConfig {
            attachment_blocked_extensions: vec!["exe".to_string()],
            attachment_action: PolicyAction::Reject,
            ..cfg()
        };
        // "AAAA" decodes to four bytes, enough for a real attachment part.
        let verdict = scan(&c, &with_attachment("facture.EXE", "AAAAAA==")).expect("refusée");
        assert_eq!(verdict.action, PolicyAction::Reject);
        assert!(verdict.reason.contains("exe"), "{}", verdict.reason);

        // A name whose extension is not listed passes.
        assert_eq!(scan(&c, &with_attachment("facture.pdf", "AAAAAA==")), None);
    }

    #[test]
    fn an_oversized_attachment_is_caught() {
        let c = ServerConfig { attachment_max_bytes: 2, attachment_action: PolicyAction::Quarantine, ..cfg() };
        let verdict = scan(&c, &with_attachment("gros.bin", "AAAAAAAAAAAA")).expect("trop gros");
        assert_eq!(verdict.action, PolicyAction::Quarantine);
    }

    #[test]
    fn an_encrypted_zip_is_recognised_by_its_flag_bit() {
        // PK\x03\x04, version, then the general purpose flag with bit 0 set.
        let encrypted = [b'P', b'K', 0x03, 0x04, 0x14, 0x00, 0x01, 0x00];
        let plain = [b'P', b'K', 0x03, 0x04, 0x14, 0x00, 0x00, 0x00];
        assert!(is_encrypted_archive(&encrypted));
        assert!(!is_encrypted_archive(&plain));
        // Anything that is not a ZIP is not claimed to be one.
        assert!(!is_encrypted_archive(b"%PDF-1.7"));
        assert!(!is_encrypted_archive(b"PK"));
    }

    #[test]
    fn a_blocked_expression_matches_the_subject_and_the_body_case_insensitively() {
        let c = ServerConfig {
            content_blocked_expressions: vec!["confidentiel défense".to_string()],
            content_action: PolicyAction::Quarantine,
            ..cfg()
        };
        assert!(scan(&c, &message("Confidentiel Défense", "rien")).is_some());
        assert!(scan(&c, &message("banal", "mention CONFIDENTIEL DÉFENSE en bas")).is_some());
        assert_eq!(scan(&c, &message("banal", "rien de spécial")), None);
    }

    #[test]
    fn extensions_are_read_from_the_last_dot_only() {
        assert_eq!(extension_of("facture.pdf").as_deref(), Some("pdf"));
        assert_eq!(extension_of("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension_of("FACTURE.EXE").as_deref(), Some("exe"));
        assert_eq!(extension_of("sans-extension"), None);
        assert_eq!(extension_of("fin."), None);
        // Not an extension: too long, or not alphanumeric.
        assert_eq!(extension_of("nom.avec espace"), None);
        assert_eq!(extension_of("x.tropluuuuuuuungue"), None);
    }

    /// The reply-injection guard: an attachment name carrying a CRLF must not
    /// be able to add a second SMTP reply line.
    #[test]
    fn a_quoted_name_can_never_carry_a_line_break() {
        let quoted = quotable("innocent.txt\r\n550 5.7.1 fake");
        assert!(!quoted.contains('\r') && !quoted.contains('\n'), "{quoted}");
    }

    #[test]
    fn the_footer_is_appended_only_when_one_is_configured() {
        assert_eq!(with_footer("<p>bonjour</p>", ""), "<p>bonjour</p>");
        assert_eq!(with_footer("<p>bonjour</p>", "   "), "<p>bonjour</p>");
        let with = with_footer("<p>bonjour</p>", "Mentions légales");
        assert!(with.starts_with("<p>bonjour</p>"));
        assert!(with.contains("Mentions légales"));
    }
}
