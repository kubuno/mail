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

/// An alternative time the invitee proposes instead of the one asked for
/// (iTIP COUNTER). Values are ISO 8601 as the client sends them.
pub struct Proposal {
    pub start: String,
    pub end: Option<String>,
}

/// `2026-09-18T14:00:00Z` / `…T14:00:00+02:00` / `2026-09-18` → an ICS value.
/// A zoned or naive datetime keeps its wall time (`…T140000`), a UTC one keeps
/// its `Z`, and a date-only value becomes a `VALUE=DATE` day.
fn ics_value(iso: &str) -> (String, bool) {
    let t = iso.trim();
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 8 {
        return (t.to_string(), false);
    }
    let date = &digits[..8];
    // Date only (no time part in the source).
    if !t.contains('T') {
        return (date.to_string(), true);
    }
    let time = if digits.len() >= 14 { &digits[8..14] } else { "000000" };
    let zulu = if t.ends_with('Z') || t.ends_with('z') { "Z" } else { "" };
    (format!("{date}T{time}{zulu}"), false)
}

/// Build the REPLY VCALENDAR from the ORIGINAL request ICS, echoing the fields an
/// organizer's scheduler needs (UID, DTSTART/DTEND, ORGANIZER, SUMMARY, SEQUENCE)
/// and appending our single ATTENDEE line with the chosen PARTSTAT. Timing lines
/// are reused VERBATIM so any TZID survives. `attendee_email` is our address.
///
/// `comment` is the invitee's optional note (RFC 5545 COMMENT) — typically the
/// reason for a decline. `proposal` turns the message into an iTIP COUNTER
/// (RFC 5546 §3.2.7): same event, but with the time the invitee suggests
/// instead, which the organizer can accept or refuse.
pub fn build_reply(
    request_ics: &str,
    attendee_email: &str,
    attendee_name: Option<&str>,
    rsvp: Rsvp,
    comment: Option<&str>,
    proposal: Option<&Proposal>,
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

    // Suggesting another time makes this a COUNTER, not a plain REPLY.
    let method = if proposal.is_some() { "COUNTER" } else { "REPLY" };
    let mut out: Vec<String> = vec![
        "BEGIN:VCALENDAR".into(),
        "PRODID:-//Kubuno//Mail//EN".into(),
        "VERSION:2.0".into(),
        format!("METHOD:{method}"),
        "BEGIN:VEVENT".into(),
    ];
    if let Some(l) = uid_line { out.push(l); }
    match proposal {
        // The proposed time REPLACES the requested one (that is the counter).
        Some(p) => {
            let (v, date_only) = ics_value(&p.start);
            out.push(format!("DTSTART{}:{v}", if date_only { ";VALUE=DATE" } else { "" }));
            if let Some(e) = p.end.as_deref().filter(|e| !e.trim().is_empty()) {
                let (v, date_only) = ics_value(e);
                out.push(format!("DTEND{}:{v}", if date_only { ";VALUE=DATE" } else { "" }));
            }
        }
        None => {
            if let Some(l) = dtstart { out.push(l); }
            if let Some(l) = dtend { out.push(l); }
        }
    }
    if let Some(l) = organizer { out.push(l); }
    out.push(attendee);
    if let Some(l) = summary { out.push(l); }
    if let Some(c) = comment.map(str::trim).filter(|c| !c.is_empty()) {
        out.push(format!("COMMENT:{}", escape_text(c)));
    }
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
        let out = build_reply(req, "bob@kubuno.com", Some("Bob"), Rsvp::Accepted, None, None);
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
        let out = build_reply(req, "me@x", None, Rsvp::Declined, None, None);
        assert!(out.contains("ATTENDEE;PARTSTAT=DECLINED:mailto:me@x"));
        assert!(out.contains("SEQUENCE:0"));
    }

    #[test]
    fn decline_with_comment() {
        let req = "BEGIN:VEVENT\r\nUID:u1\r\nORGANIZER:mailto:a@x\r\nEND:VEVENT";
        let out = build_reply(req, "me@x", None, Rsvp::Declined, Some("Je suis en congé, désolé"), None);
        assert!(out.contains("METHOD:REPLY"));
        assert!(out.contains("COMMENT:Je suis en congé\\, désolé"));
    }

    #[test]
    fn counter_proposes_another_time() {
        let req = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:u9\r\nSUMMARY:Atelier\r\nDTSTART;TZID=Europe/Paris:20260918T140000\r\nDTEND;TZID=Europe/Paris:20260918T160000\r\nORGANIZER:mailto:a@x\r\nEND:VEVENT\r\nEND:VCALENDAR";
        let p = Proposal { start: "2026-09-19T10:00:00".into(), end: Some("2026-09-19T12:00:00".into()) };
        let out = build_reply(req, "me@x", Some("Moi"), Rsvp::Declined, Some("Plutôt vendredi ?"), Some(&p));
        assert!(out.contains("METHOD:COUNTER"));
        assert!(out.contains("UID:u9"));
        assert!(out.contains("DTSTART:20260919T100000"));
        assert!(out.contains("DTEND:20260919T120000"));
        // The original time must NOT survive a counter.
        assert!(!out.contains("20260918T140000"));
        assert!(out.contains("COMMENT:Plutôt vendredi ?"));
    }
}
