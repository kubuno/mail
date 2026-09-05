//! Extraction of schema.org structured data from incoming e-mail, the way Gmail
//! does it: JSON-LD annotations embedded as `<script type="application/ld+json">`
//! in the HTML body, plus `text/calendar` (ICS) invitations. Recognized nodes are
//! stored VERBATIM as a JSON array on the message (`structured_data` column); the
//! frontend maps each node to a rich card (event, flight, hotel, order…).
//!
//! Crucially this runs on the RAW HTML, BEFORE `html_sanitize` strips `<script>`
//! and microdata attributes — otherwise the annotations are gone before storage.

use serde_json::{json, Value};

/// schema.org `@type` values we turn into cards. Any `*Event` is also accepted.
const RECOGNIZED: &[&str] = &[
    "Event",
    "EventReservation",
    "FlightReservation",
    "LodgingReservation",
    "TrainReservation",
    "BusReservation",
    "FoodEstablishmentReservation",
    "RentalCarReservation",
    "Order",
    "ParcelDelivery",
];

// ── URL hardening ───────────────────────────────────────────────────────────
//
// Every value in these nodes is authored by the SENDER (JSON-LD embedded in the
// HTML body, or an attached .ics) and the frontend turns some of them into
// clickable links. A link whose scheme can run code in the page — `javascript:`,
// `data:text/html`, `vbscript:` — or hand the click to an arbitrary local
// handler must therefore never be stored in the first place: filtering at
// display time would leave the payload one framework change away from being
// honoured. Only the two schemes the cards actually need survive extraction.
// `mailto:` is deliberately NOT allowed: no card renders a mail link (the
// organizer's address travels in its own `organizerEmail` field, not as a URL).
const ALLOWED_URL_SCHEMES: &[&str] = &["http", "https"];

/// Fields the rich cards render as a link target (`url`, `checkinUrl`,
/// `trackingUrl`, …). Matching on the suffix keeps unknown vendor spellings
/// covered instead of relying on an allow-list we would have to maintain.
fn is_url_key(key: &str) -> bool {
    key.to_ascii_lowercase().ends_with("url")
}

/// True when the string carries an explicit, allowed scheme. A URL without a
/// scheme is rejected too: relative links are resolved against the webmail
/// itself, which is never what a remote sender meant.
fn is_safe_url(raw: &str) -> bool {
    // Browsers ignore ASCII whitespace and C0 controls while parsing the scheme,
    // so `java\tscript:` reaches the same handler as `javascript:`; normalize
    // the same way before comparing.
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .collect();
    let Some(colon) = cleaned.find(':') else { return false };
    let scheme = &cleaned[..colon];
    // A colon further down a path (`/a:b`) is not a scheme.
    if scheme.is_empty() || scheme.contains(['/', '?', '#']) {
        return false;
    }
    ALLOWED_URL_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str())
}

/// Drop every link field whose value is not a safe absolute URL, at any depth
/// (a reservation nests its `url` inside `reservationFor`). The key disappears
/// from the stored node, so a hostile link is simply not part of the data any
/// consumer — this webmail or another client — can ever read back.
fn strip_unsafe_urls(v: &mut Value) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(strip_unsafe_urls),
        Value::Object(o) => {
            o.retain(|k, val| match val {
                _ if !is_url_key(k) => true,
                // A non-string link (number, object, array) is meaningless to
                // the cards; drop it rather than store an unvalidated shape.
                Value::String(s) => is_safe_url(s),
                _ => false,
            });
            for (_, val) in o.iter_mut() {
                strip_unsafe_urls(val);
            }
        }
        _ => {}
    }
}

fn type_of(node: &Value) -> Option<String> {
    match node.get("@type") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(a)) => a.iter().find_map(|v| v.as_str().map(String::from)),
        _ => None,
    }
}

fn is_recognized(t: &str) -> bool {
    RECOGNIZED.contains(&t) || t.ends_with("Event")
}

/// Walk arrays and `@graph` containers, collecting every object that carries an
/// `@type` (recognized or not — filtering happens in the caller).
fn collect_nodes(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| collect_nodes(x, out)),
        Value::Object(o) => {
            if let Some(g) = o.get("@graph") {
                collect_nodes(g, out);
            }
            if o.contains_key("@type") {
                out.push(v.clone());
            }
        }
        _ => {}
    }
}

/// Extract JSON-LD nodes with a recognized `@type` from raw HTML. Dependency-free
/// scan for `<script …ld+json…>…</script>` blocks (a full HTML parser is overkill
/// and the sanitizer would have removed these tags anyway).
fn from_jsonld(html: &str) -> Vec<Value> {
    let mut cards = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while let Some(rel) = lower[i..].find("<script") {
        let tag_start = i + rel;
        let Some(gt_rel) = lower[tag_start..].find('>') else { break };
        let open_end = tag_start + gt_rel + 1;
        let open_tag = &lower[tag_start..open_end];
        if open_tag.contains("ld+json") {
            if let Some(close_rel) = lower[open_end..].find("</script") {
                let content = &html[open_end..open_end + close_rel];
                if let Ok(v) = serde_json::from_str::<Value>(content.trim()) {
                    let mut nodes = Vec::new();
                    collect_nodes(&v, &mut nodes);
                    for n in nodes {
                        if type_of(&n).map(|t| is_recognized(&t)).unwrap_or(false) {
                            cards.push(n);
                        }
                    }
                }
                i = open_end + close_rel;
                continue;
            }
        }
        i = open_end;
    }
    cards
}

// ── ICS (text/calendar) ─────────────────────────────────────────────────────

/// Unfold RFC 5545 lines: a line beginning with a space or tab continues the
/// previous one.
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

/// Split `NAME;PARAM=x:VALUE` into (`NAME`, params-string, `VALUE`).
fn split_prop(line: &str) -> Option<(String, String, String)> {
    let colon = line.find(':')?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let (name, params) = match head.find(';') {
        Some(sc) => (&head[..sc], &head[sc + 1..]),
        None => (head, ""),
    };
    Some((name.to_ascii_uppercase(), params.to_string(), value.to_string()))
}

/// ICS escaping in TEXT values (`\,` `\;` `\n` …) → plain text.
fn unescape_text(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `20260909T140000Z` → `2026-09-09T14:00:00Z`; `20260909` (VALUE=DATE) →
/// `2026-09-09`. A local (no-Z) datetime is returned without a zone; the client
/// renders it in its own locale, which is what an invite without VTIMEZONE means.
fn ics_datetime(value: &str, params: &str) -> Option<String> {
    let v = value.trim();
    let date_only = params.to_ascii_uppercase().contains("VALUE=DATE") || v.len() == 8;
    if v.len() < 8 || !v[..8].bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let date = format!("{}-{}-{}", &v[0..4], &v[4..6], &v[6..8]);
    if date_only {
        return Some(date);
    }
    // Expect `T` then HHMMSS, optional trailing `Z`.
    let rest = &v[8..];
    let rest = rest.strip_prefix('T').unwrap_or(rest);
    if rest.len() < 6 || !rest[..6].bytes().all(|b| b.is_ascii_digit()) {
        return Some(date);
    }
    let time = format!("{}:{}:{}", &rest[0..2], &rest[2..4], &rest[4..6]);
    let zulu = if rest.ends_with('Z') { "Z" } else { "" };
    Some(format!("{date}T{time}{zulu}"))
}

/// Organizer display name (`CN` param) and email (the `mailto:` address). Either
/// may be absent; the name falls back to the address when there is no `CN`.
fn organizer_parts(params: &str, value: &str) -> (Option<String>, Option<String>) {
    let cn = params.split(';').find_map(|p| {
        p.strip_prefix("CN=")
            .or_else(|| p.strip_prefix("cn="))
            .map(|c| c.trim_matches('"').to_string())
    });
    let v = value.trim();
    let email = v
        .strip_prefix("mailto:")
        .or_else(|| v.strip_prefix("MAILTO:"))
        .map(str::to_string);
    let name = cn.or_else(|| email.clone());
    (name, email)
}

/// First VEVENT of an ICS document → an `Event`-shaped JSON-LD node, tagged with
/// `_source:"ics"` and `_invite` (true when METHOD is REQUEST — the RSVP case).
fn event_from_ics(ics: &str) -> Option<Value> {
    let lines = unfold(ics);
    let method = lines
        .iter()
        .find_map(|l| split_prop(l).filter(|(n, ..)| n == "METHOD").map(|(_, _, v)| v.to_ascii_uppercase()));

    let mut in_event = false;
    let (mut summary, mut start, mut end, mut location, mut description, mut url) =
        (None, None, None, None, None, None);
    let (mut organizer, mut organizer_email, mut uid, mut sequence) =
        (None, None, None, None);
    // REPLY only: who answered and with which participation status.
    let (mut reply_from, mut reply_name, mut reply_partstat) = (None, None, None);

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
        let Some((name, params, value)) = split_prop(line) else { continue };
        match name.as_str() {
            "SUMMARY" => summary = Some(unescape_text(&value)),
            "DTSTART" => start = ics_datetime(&value, &params),
            "DTEND" => end = ics_datetime(&value, &params),
            "LOCATION" => location = Some(unescape_text(&value)),
            "DESCRIPTION" => description = Some(unescape_text(&value)),
            "URL" => url = Some(value),
            "UID" => uid = Some(value),
            "SEQUENCE" => sequence = value.trim().parse::<i64>().ok(),
            "ORGANIZER" => {
                let (n, e) = organizer_parts(&params, &value);
                organizer = n;
                organizer_email = e;
            }
            // In a REPLY there is exactly one ATTENDEE: the person answering.
            "ATTENDEE" => {
                let (n, e) = organizer_parts(&params, &value);
                if reply_from.is_none() {
                    reply_name = n;
                    reply_from = e;
                    reply_partstat = params.split(';').find_map(|p| {
                        p.strip_prefix("PARTSTAT=")
                            .or_else(|| p.strip_prefix("partstat="))
                            .map(|v| v.trim().to_ascii_uppercase())
                    });
                }
            }
            _ => {}
        }
    }

    if !in_event && start.is_none() {
        return None;
    }
    // RFC 5546: DTSTART is required in a REQUEST/PUBLISH but optional in a
    // REPLY or CANCEL, which only need to identify the event (UID/SEQUENCE).
    // A reply without timing must still surface its partstat and UID so the
    // RSVP reaches the organizer's calendar; an invitation without timing is
    // meaningless and is dropped.
    let timing_optional = matches!(method.as_deref(), Some("REPLY") | Some("CANCEL"));
    if start.is_none() && !timing_optional {
        return None;
    }

    let mut node = json!({
        "@type": "Event",
        "name": summary.unwrap_or_else(|| "Événement".to_string()),
        "_source": "ics",
        "_invite": method.as_deref() == Some("REQUEST"),
        // The iTIP method drives what this message MEANS: a request to attend,
        // someone's answer to ours, or a cancellation.
        "_method": method.as_deref().unwrap_or("PUBLISH").to_ascii_lowercase(),
    });
    let Some(obj) = node.as_object_mut() else { return None };
    if let Some(s) = start { obj.insert("startDate".into(), json!(s)); }
    if let Some(e) = end { obj.insert("endDate".into(), json!(e)); }
    if let Some(l) = location { obj.insert("location".into(), json!(l)); }
    if let Some(d) = description { obj.insert("description".into(), json!(d)); }
    // The invitation file comes from the sender; its URL property is checked
    // here rather than trusted and cleaned up downstream.
    if let Some(u) = url.filter(|u| is_safe_url(u)) { obj.insert("url".into(), json!(u)); }
    if let Some(o) = organizer { obj.insert("organizer".into(), json!({ "name": o })); }
    // Fields an iMIP REPLY needs: organizer address, event UID/SEQUENCE, and the
    // raw ICS so the reply can echo the request's timing exactly.
    if let Some(e) = organizer_email { obj.insert("organizerEmail".into(), json!(e)); }
    if let Some(u) = uid { obj.insert("uid".into(), json!(u)); }
    if let Some(sq) = sequence { obj.insert("sequence".into(), json!(sq)); }
    obj.insert("_ics".into(), json!(ics));
    if method.as_deref() == Some("REPLY") {
        if let Some(p) = reply_partstat { obj.insert("_replyPartstat".into(), json!(p)); }
        if let Some(e) = reply_from { obj.insert("_replyFrom".into(), json!(e)); }
        if let Some(n) = reply_name { obj.insert("_replyName".into(), json!(n)); }
    }
    Some(node)
}

/// Extract all rich-card nodes from a message: JSON-LD from the raw HTML body plus
/// one Event per ICS part. Returns `None` when nothing was found (so the column
/// stays NULL for ordinary mail).
pub fn extract(html: Option<&str>, ics_parts: &[String]) -> Option<Value> {
    let mut cards: Vec<Value> = Vec::new();
    if let Some(h) = html {
        cards.extend(from_jsonld(h));
    }
    for ics in ics_parts {
        if let Some(ev) = event_from_ics(ics) {
            cards.push(ev);
        }
    }
    // Nodes are kept verbatim, so this is the single point where sender-authored
    // link fields are vetted before they reach the database.
    cards.iter_mut().for_each(strip_unsafe_urls);
    (!cards.is_empty()).then_some(Value::Array(cards))
}

// ── Invitation notices (for push notifications) ─────────────────────────────

/// What an incoming calendar message means for the recipient.
#[derive(Debug, PartialEq)]
pub enum InviteNotice {
    /// Someone invites us to an event.
    Invitation { summary: String, organizer: Option<String> },
    /// Someone answered an invitation WE sent.
    Reply { summary: String, who: Option<String>, partstat: String },
    /// The organizer cancelled the event.
    Cancelled { summary: String, organizer: Option<String> },
}

/// Reads the extracted nodes and reports the first calendar message that
/// deserves its own notification (invitation / reply / cancellation).
pub fn invite_notice(nodes: &Value) -> Option<InviteNotice> {
    let arr = nodes.as_array()?;
    for n in arr {
        if n.get("_source").and_then(|v| v.as_str()) != Some("ics") {
            continue;
        }
        let summary = n
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Événement")
            .to_string();
        let organizer = n
            .get("organizer")
            .and_then(|o| o.get("name"))
            .and_then(|v| v.as_str())
            .or_else(|| n.get("organizerEmail").and_then(|v| v.as_str()))
            .map(str::to_string);
        return match n.get("_method").and_then(|v| v.as_str()) {
            Some("request") => Some(InviteNotice::Invitation { summary, organizer }),
            Some("cancel") => Some(InviteNotice::Cancelled { summary, organizer }),
            Some("reply") => Some(InviteNotice::Reply {
                summary,
                who: n
                    .get("_replyName")
                    .and_then(|v| v.as_str())
                    .or_else(|| n.get("_replyFrom").and_then(|v| v.as_str()))
                    .map(str::to_string),
                partstat: n
                    .get("_replyPartstat")
                    .and_then(|v| v.as_str())
                    .unwrap_or("NEEDS-ACTION")
                    .to_string(),
            }),
            _ => None,
        };
    }
    None
}

/// The raw fields an iMIP REPLY carries, kept for republishing the RSVP to
/// Calendar (which `InviteNotice::Reply` deliberately drops, since it only
/// exists to word a push notification).
#[derive(Debug, PartialEq)]
pub struct InviteReplyDetails {
    /// The event's iCalendar UID — matches the invitation Calendar sent.
    pub uid:       String,
    /// The address that answered (the REPLY's single ATTENDEE).
    pub from:      String,
    /// ACCEPTED | DECLINED | TENTATIVE | NEEDS-ACTION.
    pub partstat:  String,
    /// The event SEQUENCE the reply answers (0 when absent).
    pub sequence:  i64,
    /// The organizer address the reply is addressed to, when present.
    pub organizer: Option<String>,
}

/// Reads the extracted nodes and, for the first iMIP REPLY, returns the fields
/// needed to forward the RSVP to Calendar. `None` for anything but a REPLY, or
/// when the REPLY lacks the UID / answering address that make it actionable.
pub fn invite_reply_details(nodes: &Value) -> Option<InviteReplyDetails> {
    let arr = nodes.as_array()?;
    for n in arr {
        if n.get("_source").and_then(Value::as_str) != Some("ics") {
            continue;
        }
        if n.get("_method").and_then(Value::as_str) != Some("reply") {
            continue;
        }
        let uid = n.get("uid").and_then(Value::as_str)?.to_string();
        let from = n.get("_replyFrom").and_then(Value::as_str)?.to_string();
        return Some(InviteReplyDetails {
            uid,
            from,
            partstat: n
                .get("_replyPartstat")
                .and_then(Value::as_str)
                .unwrap_or("NEEDS-ACTION")
                .to_string(),
            sequence: n.get("sequence").and_then(Value::as_i64).unwrap_or(0),
            organizer: n
                .get("organizerEmail")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flight_reservation_jsonld() {
        let html = r#"<html><head><script type="application/ld+json">
        {"@context":"http://schema.org","@type":"FlightReservation",
         "reservationId":"XY7Z9",
         "reservationFor":{"@type":"Flight","flightNumber":"1234",
            "airline":{"@type":"Airline","name":"Air France","iataCode":"AF"},
            "departureAirport":{"@type":"Airport","name":"Paris CDG","iataCode":"CDG"},
            "arrivalAirport":{"@type":"Airport","name":"Lisbonne","iataCode":"LIS"},
            "departureTime":"2026-09-09T10:05:00+02:00",
            "arrivalTime":"2026-09-09T12:10:00+01:00"}}
        </script></head><body>Bon vol</body></html>"#;
        let out = extract(Some(html), &[]).expect("some cards");
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["@type"], "FlightReservation");
        assert_eq!(arr[0]["reservationFor"]["flightNumber"], "1234");
    }

    #[test]
    fn graph_and_multiple_scripts() {
        let html = r#"
        <script type="application/ld+json">{"@graph":[
            {"@type":"WebPage","name":"ignore me"},
            {"@type":"EventReservation","reservationFor":{"@type":"Event","name":"Concert","startDate":"2026-10-01T20:00:00Z"}}
        ]}</script>
        <script type="application/ld+json">{"@type":"Order","orderNumber":"A-42"}</script>"#;
        let out = extract(Some(html), &[]).unwrap();
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["@type"], "EventReservation");
        assert_eq!(arr[1]["@type"], "Order");
    }

    #[test]
    fn ics_invite_event() {
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nSUMMARY:Réunion projet\r\nDTSTART;TZID=Europe/Paris:20260909T140000\r\nDTEND;TZID=Europe/Paris:20260909T150000\r\nLOCATION:Salle B\r\nORGANIZER;CN=Alice:mailto:alice@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = extract(None, &[ics.to_string()]).unwrap();
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["@type"], "Event");
        assert_eq!(arr[0]["name"], "Réunion projet");
        assert_eq!(arr[0]["startDate"], "2026-09-09T14:00:00");
        assert_eq!(arr[0]["_invite"], true);
        assert_eq!(arr[0]["organizer"]["name"], "Alice");
    }

    #[test]
    fn reply_ics_carries_partstat_and_notice() {
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nBEGIN:VEVENT\r\nUID:u1\r\nSUMMARY:Comité\r\nDTSTART:20260915T080000Z\r\nORGANIZER;CN=Moi:mailto:moi@x\r\nATTENDEE;CN=Alice;PARTSTAT=ACCEPTED:mailto:alice@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = extract(None, &[ics.to_string()]).unwrap();
        let n = &out.as_array().unwrap()[0];
        assert_eq!(n["_method"], "reply");
        assert_eq!(n["_replyPartstat"], "ACCEPTED");
        assert_eq!(n["_replyName"], "Alice");
        assert_eq!(
            invite_notice(&out),
            Some(InviteNotice::Reply {
                summary: "Comité".into(),
                who: Some("Alice".into()),
                partstat: "ACCEPTED".into()
            })
        );
    }

    #[test]
    fn reply_details_carry_uid_and_answering_address() {
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nBEGIN:VEVENT\r\nUID:evt-42\r\nSEQUENCE:3\r\nSUMMARY:Comité\r\nDTSTART:20260915T080000Z\r\nORGANIZER;CN=Moi:mailto:moi@x\r\nATTENDEE;CN=Alice;PARTSTAT=DECLINED:mailto:alice@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = extract(None, &[ics.to_string()]).unwrap();
        assert_eq!(
            invite_reply_details(&out),
            Some(InviteReplyDetails {
                uid:       "evt-42".into(),
                from:      "alice@example.com".into(),
                partstat:  "DECLINED".into(),
                sequence:  3,
                organizer: Some("moi@x".into()),
            })
        );
    }

    #[test]
    fn reply_without_dtstart_still_yields_details() {
        // RFC 5546 makes DTSTART optional in a REPLY: a minimal, compliant
        // answer must still reach the organizer's calendar.
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nBEGIN:VEVENT\r\nUID:evt-7\r\nSEQUENCE:0\r\nDTSTAMP:20260905T120000Z\r\nORGANIZER:mailto:moi@x\r\nATTENDEE;CN=Bob;PARTSTAT=ACCEPTED:mailto:bob@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = extract(None, &[ics.to_string()]).expect("a timing-less REPLY is still an event node");
        assert_eq!(out[0]["_method"], "reply");
        assert!(out[0].get("startDate").is_none());
        let d = invite_reply_details(&out).expect("reply details");
        assert_eq!(d.uid, "evt-7");
        assert_eq!(d.from, "bob@example.org");
        assert_eq!(d.partstat, "ACCEPTED");
    }

    #[test]
    fn request_without_dtstart_is_dropped() {
        // An invitation with no timing is meaningless and must not surface a card.
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:evt-8\r\nSUMMARY:Sans horaire\r\nORGANIZER:mailto:moi@x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert!(extract(None, &[ics.to_string()]).is_none());
    }

    #[test]
    fn request_ics_has_no_reply_details() {
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:evt-1\r\nSUMMARY:Atelier\r\nDTSTART:20260918T120000Z\r\nORGANIZER;CN=Bob:mailto:bob@x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = extract(None, &[ics.to_string()]).unwrap();
        assert!(invite_reply_details(&out).is_none());
    }

    #[test]
    fn request_ics_is_an_invitation_notice() {
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nSUMMARY:Atelier\r\nDTSTART:20260918T120000Z\r\nORGANIZER;CN=Bob:mailto:bob@x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = extract(None, &[ics.to_string()]).unwrap();
        assert_eq!(
            invite_notice(&out),
            Some(InviteNotice::Invitation { summary: "Atelier".into(), organizer: Some("Bob".into()) })
        );
    }

    #[test]
    fn nothing_found_is_none() {
        assert!(extract(Some("<p>plain mail</p>"), &[]).is_none());
    }

    /// Build a one-node JSON-LD mail carrying `url` set to `raw`.
    fn jsonld_with_url(raw: &str) -> Value {
        let html = format!(
            r#"<script type="application/ld+json">{{"@type":"Event","name":"X","startDate":"2026-10-01T20:00:00Z","url":{}}}</script>"#,
            serde_json::to_string(raw).expect("url encodable en JSON")
        );
        extract(Some(&html), &[]).expect("une carte")
    }

    #[test]
    fn jsonld_url_javascript_est_supprimee() {
        let out = jsonld_with_url("javascript:alert(1)");
        let n = &out.as_array().expect("tableau de cartes")[0];
        assert_eq!(n["name"], "X", "la carte doit rester extraite");
        assert!(n.get("url").is_none(), "une URL javascript: ne doit pas être stockée");
    }

    #[test]
    fn jsonld_url_data_html_est_supprimee() {
        let out = jsonld_with_url("data:text/html;base64,PHNjcmlwdD4=");
        let n = &out.as_array().expect("tableau de cartes")[0];
        assert!(n.get("url").is_none(), "une URL data: ne doit pas être stockée");
    }

    #[test]
    fn jsonld_url_https_est_conservee() {
        let out = jsonld_with_url("https://exemple.test/billet?id=42");
        let n = &out.as_array().expect("tableau de cartes")[0];
        assert_eq!(
            n["url"], "https://exemple.test/billet?id=42",
            "une URL https normale doit passer intacte"
        );
    }

    #[test]
    fn schemas_exotiques_et_url_relatives_sont_rejetes() {
        for raw in [
            "JavaScript:alert(1)",       // scheme comparison is case-insensitive
            "java\tscript:alert(1)",     // controls ignored by the browser parser
            "  javascript:alert(1)",     // leading whitespace ignored too
            "vbscript:msgbox(1)",
            "file:///etc/passwd",
            "/relatif/vers/le/webmail",
            "sans-schema.example.com",
        ] {
            let out = jsonld_with_url(raw);
            let n = &out.as_array().expect("tableau de cartes")[0];
            assert!(n.get("url").is_none(), "URL refusée attendue pour {raw:?}");
        }
        assert!(
            jsonld_with_url("http://exemple.test/x").as_array().expect("tableau")[0]
                .get("url")
                .is_some(),
            "http doit rester autorisé"
        );
    }

    #[test]
    fn url_imbriquee_et_champ_tracking_sont_valides() {
        let html = r#"<script type="application/ld+json">
        {"@type":"FlightReservation","checkinUrl":"javascript:alert(1)",
         "reservationFor":{"@type":"Flight","flightNumber":"7","url":"data:text/html,x"}}
        </script>
        <script type="application/ld+json">
        {"@type":"ParcelDelivery","trackingNumber":"T1","trackingUrl":"https://suivi.test/T1"}
        </script>"#;
        let out = extract(Some(html), &[]).expect("des cartes");
        let arr = out.as_array().expect("tableau de cartes");
        assert!(arr[0].get("checkinUrl").is_none(), "checkinUrl hostile supprimée");
        assert!(
            arr[0]["reservationFor"].get("url").is_none(),
            "une URL imbriquée doit être vérifiée elle aussi"
        );
        assert_eq!(arr[0]["reservationFor"]["flightNumber"], "7", "le reste du nœud est intact");
        assert_eq!(arr[1]["trackingUrl"], "https://suivi.test/T1", "trackingUrl https conservée");
    }

    #[test]
    fn url_ics_hostile_est_supprimee() {
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nSUMMARY:Piégé\r\nDTSTART:20260918T120000Z\r\nURL:javascript:alert(document.cookie)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = extract(None, &[ics.to_string()]).expect("un événement");
        let n = &out.as_array().expect("tableau de cartes")[0];
        assert_eq!(n["name"], "Piégé", "l'événement reste affiché");
        assert!(n.get("url").is_none(), "une URL javascript: d'un .ics ne doit pas être stockée");

        let ok = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Ok\r\nDTSTART:20260918T120000Z\r\nURL:https://exemple.test/evt\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = extract(None, &[ok.to_string()]).expect("un événement");
        assert_eq!(
            out.as_array().expect("tableau")[0]["url"], "https://exemple.test/evt",
            "une URL https d'un .ics doit être conservée"
        );
    }
}
