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

/// Organizer display name: `CN` param if present, else the mailto address.
fn organizer_name(params: &str, value: &str) -> Option<String> {
    for p in params.split(';') {
        if let Some(cn) = p.strip_prefix("CN=").or_else(|| p.strip_prefix("cn=")) {
            return Some(cn.trim_matches('"').to_string());
        }
    }
    let v = value.trim();
    v.strip_prefix("mailto:")
        .or_else(|| v.strip_prefix("MAILTO:"))
        .map(str::to_string)
        .or_else(|| (!v.is_empty()).then(|| v.to_string()))
}

/// First VEVENT of an ICS document → an `Event`-shaped JSON-LD node, tagged with
/// `_source:"ics"` and `_invite` (true when METHOD is REQUEST — the RSVP case).
fn event_from_ics(ics: &str) -> Option<Value> {
    let lines = unfold(ics);
    let method = lines
        .iter()
        .find_map(|l| split_prop(l).filter(|(n, ..)| n == "METHOD").map(|(_, _, v)| v.to_ascii_uppercase()));

    let mut in_event = false;
    let (mut summary, mut start, mut end, mut location, mut description, mut organizer, mut url) =
        (None, None, None, None, None, None, None);

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
            "ORGANIZER" => organizer = organizer_name(&params, &value),
            _ => {}
        }
    }

    if !in_event && start.is_none() {
        return None;
    }
    let start = start?;

    let mut node = json!({
        "@type": "Event",
        "name": summary.unwrap_or_else(|| "Événement".to_string()),
        "startDate": start,
        "_source": "ics",
        "_invite": method.as_deref() == Some("REQUEST"),
    });
    let obj = node.as_object_mut().unwrap();
    if let Some(e) = end { obj.insert("endDate".into(), json!(e)); }
    if let Some(l) = location { obj.insert("location".into(), json!(l)); }
    if let Some(d) = description { obj.insert("description".into(), json!(d)); }
    if let Some(u) = url { obj.insert("url".into(), json!(u)); }
    if let Some(o) = organizer { obj.insert("organizer".into(), json!({ "name": o })); }
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
    (!cards.is_empty()).then_some(Value::Array(cards))
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
    fn nothing_found_is_none() {
        assert!(extract(Some("<p>plain mail</p>"), &[]).is_none());
    }
}
