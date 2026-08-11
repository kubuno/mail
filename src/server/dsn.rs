//! Delivery Status Notifications (RFC 3464) — the bounce/delay reports sent back
//! to a message's sender when a recipient permanently fails or is delayed.
//!
//! The shape follows Postfix's `bounce_notify_util.c` exactly: a
//! `multipart/report; report-type=delivery-status` with three parts — a human
//! text, a machine `message/delivery-status`, and the returned headers — and,
//! critically, an empty envelope sender so a DSN that itself fails cannot
//! generate another DSN.

use chrono::Utc;

/// One recipient a DSN reports on.
pub struct FailedRecipient {
    pub recipient:  String,
    /// RFC 3463 status like "5.1.1" (permanent) or "4.4.1" (delayed).
    pub status:     String,
    /// The remote server's diagnostic text (its 5xx/4xx line), if any.
    pub diagnostic: Option<String>,
}

/// Whether the notice is a permanent failure or a "still trying" delay.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Failure,
    Delay,
}

/// Builds a complete RFC 5322 DSN message ready to enqueue. `original` is the
/// message that failed (its bytes); only its headers are returned, per the
/// common `RET=HDRS` behaviour, to keep the report small and avoid bouncing a
/// large body back.
///
/// The returned message's envelope sender MUST be `<>` (the caller enqueues it
/// so): a DSN is posted with the null return path (RFC 3464 §2.1.1) precisely so
/// its own delivery failure does not spawn a further DSN.
pub fn build(
    hostname: &str,
    report_to: &str,
    kind: Kind,
    recipients: &[FailedRecipient],
    original: &[u8],
) -> Vec<u8> {
    let boundary = format!("kubuno-dsn-{}", Utc::now().timestamp_micros());
    let date = Utc::now().format("%a, %d %b %Y %H:%M:%S %z");
    let (subject, action, intro) = match kind {
        Kind::Failure => (
            "Undelivered Mail Returned to Sender",
            "failed",
            "I'm sorry to have to inform you that your message could not be delivered \
             to one or more recipients. It is attached below.",
        ),
        Kind::Delay => (
            "Delivery Status Notification (Delay)",
            "delayed",
            "This is a warning only. You do NOT need to resend your message. Delivery \
             to the following recipients has been delayed and is still being retried.",
        ),
    };

    let mut out = String::with_capacity(2048);
    // Envelope/message headers. From is our MAILER-DAEMON; Auto-Submitted stops
    // vacation auto-responders from replying to the report (RFC 3834).
    push(&mut out, &format!("From: MAILER-DAEMON@{hostname} (Mail Delivery System)"));
    push(&mut out, &format!("To: <{}>", sanitize(report_to)));
    push(&mut out, &format!("Subject: {subject}"));
    push(&mut out, &format!("Date: {date}"));
    push(&mut out, "Auto-Submitted: auto-replied");
    push(&mut out, "MIME-Version: 1.0");
    push(&mut out, &format!(
        "Content-Type: multipart/report; report-type=delivery-status;\r\n\tboundary=\"{boundary}\""
    ));
    out.push_str("\r\n");

    // Part 1 — human-readable.
    push(&mut out, &format!("--{boundary}"));
    push(&mut out, "Content-Type: text/plain; charset=utf-8");
    out.push_str("\r\n");
    push(&mut out, &format!("This is the mail system at host {hostname}."));
    out.push_str("\r\n");
    push(&mut out, intro);
    out.push_str("\r\n");
    for r in recipients {
        let diag = r.diagnostic.as_deref().unwrap_or("delivery failed");
        push(&mut out, &format!("<{}>: {}", sanitize(&r.recipient), sanitize(diag)));
    }
    out.push_str("\r\n");

    // Part 2 — machine-readable delivery-status.
    push(&mut out, &format!("--{boundary}"));
    push(&mut out, "Content-Type: message/delivery-status");
    out.push_str("\r\n");
    push(&mut out, &format!("Reporting-MTA: dns; {hostname}"));
    push(&mut out, &format!("Arrival-Date: {date}"));
    for r in recipients {
        out.push_str("\r\n");
        push(&mut out, &format!("Final-Recipient: rfc822; {}", sanitize(&r.recipient)));
        push(&mut out, &format!("Action: {action}"));
        push(&mut out, &format!("Status: {}", sanitize(&r.status)));
        if let Some(diag) = &r.diagnostic {
            push(&mut out, &format!("Diagnostic-Code: smtp; {}", sanitize(diag)));
        }
    }
    out.push_str("\r\n");

    // Part 3 — the returned headers of the original message.
    push(&mut out, &format!("--{boundary}"));
    push(&mut out, "Content-Type: text/rfc822-headers");
    out.push_str("\r\n");
    out.push_str(&original_headers(original));
    out.push_str("\r\n");

    push(&mut out, &format!("--{boundary}--"));
    out.into_bytes()
}

fn push(out: &mut String, line: &str) {
    out.push_str(line);
    out.push_str("\r\n");
}

/// A header value can never carry a bare CR/LF — that is how a crafted address
/// would inject extra headers into the report.
fn sanitize(value: &str) -> String {
    value.chars().map(|c| if c == '\r' || c == '\n' { ' ' } else { c }).take(500).collect()
}

/// The header block of the original message (everything up to the first blank
/// line), CRLF-normalised. RET=HDRS: we return headers, not the whole body.
fn original_headers(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let end = text.find("\r\n\r\n").or_else(|| text.find("\n\n")).unwrap_or(text.len());
    text[..end].replace("\r\n", "\n").replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        build(
            "mail.example.com",
            "sender@source.example",
            Kind::Failure,
            &[FailedRecipient {
                recipient: "nobody@dest.example".into(),
                status: "5.1.1".into(),
                diagnostic: Some("550 5.1.1 User unknown".into()),
            }],
            b"From: sender@source.example\r\nTo: nobody@dest.example\r\nSubject: hi\r\n\r\nbody",
        )
    }

    #[test]
    fn dsn_has_the_three_report_parts() {
        let s = String::from_utf8(sample()).expect("utf8");
        assert!(s.contains("multipart/report; report-type=delivery-status"));
        assert!(s.contains("Content-Type: message/delivery-status"));
        assert!(s.contains("Content-Type: text/rfc822-headers"));
        assert!(s.contains("Final-Recipient: rfc822; nobody@dest.example"));
        assert!(s.contains("Action: failed"));
        assert!(s.contains("Status: 5.1.1"));
        assert!(s.contains("Auto-Submitted: auto-replied"));
        // The returned part carries the ORIGINAL headers but not its body.
        assert!(s.contains("Subject: hi"));
        assert!(!s.contains("\r\nbody"));
    }

    #[test]
    fn header_injection_via_recipient_is_neutralised() {
        let msg = build(
            "mail.example.com",
            "a@b.c\r\nBcc: victim@evil.example",
            Kind::Failure,
            &[],
            b"From: x\r\n\r\n",
        );
        let s = String::from_utf8(msg).expect("utf8");
        assert!(!s.lines().any(|l| l.starts_with("Bcc:")), "pas d'en-tête injecté");
    }
}
