//! iMIP (RFC 6047 / iTIP RFC 5546) REPLY construction: when the user answers a
//! calendar invitation (Yes / Maybe / No), we email the organizer a VCALENDAR
//! with `METHOD:REPLY` carrying our ATTENDEE line with the chosen PARTSTAT —
//! exactly what Gmail and other clients emit so the organizer's calendar updates.

use chrono::Utc;

/// The RSVP a user can give, and its iCalendar PARTSTAT.
#[derive(Clone, Copy)]
pub enum Rsvp {
    Accepted,
    Tentative,
    Declined,
}

impl Rsvp {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "accepted" => Some(Self::Accepted),
            "tentative" => Some(Self::Tentative),
            "declined" => Some(Self::Declined),
            _ => None,
        }
    }
    pub fn partstat(self) -> &'static str {
        match self {
            Self::Accepted => "ACCEPTED",
            Self::Tentative => "TENTATIVE",
            Self::Declined => "DECLINED",
        }
    }
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Tentative => "tentative",
            Self::Declined => "declined",
        }
    }
}

/// Unfold RFC 5545 lines (a leading space/tab continues the previous line).
fn unfold(ics: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in ics.lines() {
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            if let Some(last) = out.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        out.push(line.to_string());
    }
    out
}

/// The property name of an ICS line (`DTSTART;TZID=…:…` → `DTSTART`).
fn prop_name(line: &str) -> &str {
    let head = line.split(':').next().unwrap_or(line);
    head.split(';').next().unwrap_or(head)
}

fn escape_text(v: &str) -> String {
    v.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\n', "\\n")
}

/// Build the REPLY VCALENDAR from the ORIGINAL request ICS, echoing the fields an
/// organizer's scheduler needs (UID, DTSTART/DTEND, ORGANIZER, SUMMARY, SEQUENCE)
/// and appending our single ATTENDEE line with the chosen PARTSTAT. Timing lines
/// are reused VERBATIM so any TZID survives. `attendee_email` is our address.
pub fn build_reply(
    request_ics: &str,
    attendee_email: &str,
    attendee_name: Option<&str>,
    rsvp: Rsvp,
) -> String {
    let lines = unfold(request_ics);
    let mut in_event = false;
    let (mut uid_line, mut dtstart, mut dtend, mut organizer, mut summary, mut sequence) =
        (None, None, None, None, None, None);
    for line in &lines {
        let up = line.to_ascii_uppercase();
        if up.starts_with("BEGIN:VEVENT") {
            in_event = true;
            continue;
        }
        if up.starts_with("END:VEVENT") {
            break;
        }
        if !in_event {
            continue;
        }
        match prop_name(line).to_ascii_uppercase().as_str() {
            "UID" => uid_line = Some(line.clone()),
            "DTSTART" => dtstart = Some(line.clone()),
            "DTEND" => dtend = Some(line.clone()),
            "ORGANIZER" => organizer = Some(line.clone()),
            "SUMMARY" => summary = Some(line.clone()),
            "SEQUENCE" => sequence = Some(line.clone()),
            _ => {}
        }
    }

    let dtstamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let cn = attendee_name
        .filter(|n| !n.trim().is_empty())
        .map(|n| format!(";CN={}", escape_text(n)))
        .unwrap_or_default();
    let attendee = format!(
        "ATTENDEE{cn};PARTSTAT={}:mailto:{attendee_email}",
        rsvp.partstat()
    );

    let mut out: Vec<String> = vec![
        "BEGIN:VCALENDAR".into(),
        "PRODID:-//Kubuno//Mail//EN".into(),
        "VERSION:2.0".into(),
        "METHOD:REPLY".into(),
        "BEGIN:VEVENT".into(),
    ];
    if let Some(l) = uid_line { out.push(l); }
    if let Some(l) = dtstart { out.push(l); }
    if let Some(l) = dtend { out.push(l); }
    if let Some(l) = organizer { out.push(l); }
    out.push(attendee);
    if let Some(l) = summary { out.push(l); }
    out.push(sequence.unwrap_or_else(|| "SEQUENCE:0".to_string()));
    out.push(format!("DTSTAMP:{dtstamp}"));
    out.push("END:VEVENT".into());
    out.push("END:VCALENDAR".into());
    // RFC 5545 mandates CRLF line endings.
    format!("{}\r\n", out.join("\r\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_echoes_request_fields() {
        let req = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:evt-42@x\r\nSUMMARY:Réunion\r\nDTSTART;TZID=Europe/Paris:20260909T140000\r\nDTEND;TZID=Europe/Paris:20260909T150000\r\nORGANIZER;CN=Alice:mailto:alice@example.com\r\nSEQUENCE:2\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = build_reply(req, "bob@kubuno.com", Some("Bob"), Rsvp::Accepted);
        assert!(out.contains("METHOD:REPLY"));
        assert!(out.contains("UID:evt-42@x"));
        assert!(out.contains("DTSTART;TZID=Europe/Paris:20260909T140000"));
        assert!(out.contains("ORGANIZER;CN=Alice:mailto:alice@example.com"));
        assert!(out.contains("ATTENDEE;CN=Bob;PARTSTAT=ACCEPTED:mailto:bob@kubuno.com"));
        assert!(out.contains("SEQUENCE:2"));
        assert!(out.contains("DTSTAMP:"));
        assert!(out.ends_with("\r\n"));
    }

    #[test]
    fn declined_partstat() {
        let req = "BEGIN:VEVENT\r\nUID:u1\r\nORGANIZER:mailto:a@x\r\nEND:VEVENT";
        let out = build_reply(req, "me@x", None, Rsvp::Declined);
        assert!(out.contains("ATTENDEE;PARTSTAT=DECLINED:mailto:me@x"));
        assert!(out.contains("SEQUENCE:0"));
    }
}
