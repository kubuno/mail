//! Core → module event delivery.
//!
//! The core pushes every event a module subscribed to (see `module.toml`) as an
//! adjacently-tagged `AppEvent`. A module's own event travels inside the
//! `Custom` variant, whose real name lives in `payload.event_type`.
//!
//! Mail consumes `calendar.invite`: when Calendar publishes an invitation
//! (METHOD:REQUEST) or a cancellation (METHOD:CANCEL), Mail sends the matching
//! e-mail — an `invite.ics` attachment plus a plain HTML body — to every
//! attendee, on behalf of the organizer's default sending account. Calendar is
//! the authority on the ICS payload, so it is forwarded verbatim, never rebuilt.

use axum::{extract::State, Json};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    models::{AttachmentInput, EmailAddress, SendMailDto},
    state::AppState,
};

/// The `AppEvent` envelope the core POSTs to `/ipc/events` (and `/events`).
#[derive(Deserialize)]
pub struct KubunoEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload:    Value,
}

/// One attendee of the invitation.
#[derive(Deserialize)]
struct InviteAttendee {
    email: String,
    #[serde(default)]
    name:  Option<String>,
    /// Answer links minted by Calendar for THIS guest (absent when the instance
    /// has no public URL configured, or for a cancellation).
    #[serde(default)]
    rsvp:  Option<RsvpLinks>,
}

/// One public link per answer, each carrying the guest's own token.
#[derive(Deserialize, Clone, Default)]
struct RsvpLinks {
    #[serde(default)]
    yes:   Option<String>,
    #[serde(default)]
    no:    Option<String>,
    #[serde(default)]
    maybe: Option<String>,
}

/// The inner `calendar.invite` payload published by Calendar. Kept permissive:
/// a field Calendar stops sending must not break delivery of the rest.
#[derive(Deserialize)]
struct CalendarInvite {
    /// "request" | "cancel".
    method:            String,
    organizer_user_id: Uuid,
    #[serde(default)]
    organizer_email:   Option<String>,
    #[serde(default)]
    organizer_name:    Option<String>,
    #[serde(default)]
    summary:           Option<String>,
    #[serde(default)]
    starts_at:         Option<String>,
    #[serde(default)]
    ends_at:           Option<String>,
    #[serde(default)]
    all_day:           bool,
    #[serde(default)]
    location:          Option<String>,
    #[serde(default)]
    description:       Option<String>,
    #[serde(default)]
    attendees:         Vec<InviteAttendee>,
    /// The complete VCALENDAR (METHOD:REQUEST or METHOD:CANCEL) — the authority
    /// on the invitation, attached verbatim as `invite.ics`.
    ics:               String,
}

/// Entry point for every core-delivered event. Always answers 200: a handler
/// that rejects an event makes the core replay it five times, and no replay can
/// fix a producer's mistake, so a malformed payload is logged and dropped.
pub async fn handle_event(
    State(state): State<AppState>,
    Json(event): Json<KubunoEvent>,
) -> Json<Value> {
    // Only `Custom` carries a module's own event; anything else is acknowledged
    // without side effects.
    if event.event_type != "Custom" {
        return Json(json!({ "ok": true }));
    }
    let inner_type = event
        .payload
        .get("event_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    if inner_type != "calendar.invite" {
        return Json(json!({ "ok": true }));
    }

    let body = match event.payload.get("payload") {
        Some(p) => p.clone(),
        None => return Json(json!({ "ok": true })),
    };
    let invite: CalendarInvite = match serde_json::from_value(body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "calendar.invite: charge utile illisible, ignorée");
            return Json(json!({ "ok": true }));
        }
    };

    if let Err(e) = send_invitation(&state, &invite).await {
        // Best-effort: log and still return 200 so the core does not replay.
        tracing::error!(error = %e, method = %invite.method, "Envoi des invitations échoué");
    }
    Json(json!({ "ok": true }))
}

/// Resolves the organizer's sending account and mails the invitation to each
/// attendee. Errors bubble up to the caller, which logs them and returns 200.
async fn send_invitation(state: &AppState, invite: &CalendarInvite) -> Result<(), anyhow::Error> {
    let is_cancel = invite.method.eq_ignore_ascii_case("cancel");

    // Resolve the organizer's default sending account: prefer a local mailbox of
    // this instance, then the account flagged default, then any active one.
    let account_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM mail.accounts
         WHERE user_id = $1
         ORDER BY (kind = 'local') DESC, is_default DESC, is_active DESC, created_at ASC
         LIMIT 1",
    )
    .bind(invite.organizer_user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, organizer = %invite.organizer_user_id, "Résolution du compte organisateur échouée");
        e
    })?;

    let account_id = match account_id {
        Some(id) => id,
        None => {
            tracing::warn!(
                organizer = %invite.organizer_user_id,
                "calendar.invite: aucun compte d'envoi pour l'organisateur, invitation ignorée"
            );
            return Ok(());
        }
    };

    let method_param = if is_cancel { "CANCEL" } else { "REQUEST" };
    let ics_b64 = base64::engine::general_purpose::STANDARD.encode(invite.ics.as_bytes());
    let subject = build_subject(invite, is_cancel);

    // No attendee is not an error — a solo event still updates the organizer's
    // own calendar; there is simply no e-mail to send.
    for attendee in &invite.attendees {
        let to = attendee.email.trim();
        if to.is_empty() {
            continue;
        }
        let dto = SendMailDto {
            account_id,
            to_addresses: vec![EmailAddress {
                name:  attendee.name.clone(),
                email: to.to_string(),
            }],
            cc_addresses:  None,
            bcc_addresses: None,
            subject:       subject.clone(),
            body_html:     build_body_html(invite, is_cancel, attendee.rsvp.as_ref()),
            reply_to_id:   None,
            draft_id:      None,
            scheduled_at:  None,
            attachments:   Some(vec![AttachmentInput {
                filename: "invite.ics".into(),
                mime:     format!("text/calendar; method={method_param}; charset=utf-8"),
                content:  ics_b64.clone(),
            }]),
            sign:      None,
            encrypt:   None,
            label_ids: None,
        };

        if let Err(e) =
            crate::handlers::messages::send_message_inner(state, invite.organizer_user_id, &dto, None)
                .await
        {
            // One failed recipient must not stop the others.
            tracing::error!(error = %e, attendee = %to, "Envoi de l'invitation à un invité échoué");
        }
    }

    Ok(())
}

/// Subject line, Google-style: "Invitation : {summary} @ {date} ({organizer})"
/// for a REQUEST, "Événement annulé : {summary}" for a CANCEL. When the event
/// carries a SEQUENCE > 0 the request is a rescheduling, flagged as such.
fn build_subject(invite: &CalendarInvite, is_cancel: bool) -> String {
    let summary = invite.summary.as_deref().unwrap_or("Événement");
    if is_cancel {
        return format!("Événement annulé : {summary}");
    }
    let who = invite
        .organizer_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or(invite.organizer_email.as_deref())
        .unwrap_or("l'organisateur");
    let updated = ics_sequence(&invite.ics) > 0;
    let prefix = if updated { "Invitation mise à jour" } else { "Invitation" };
    match invite.starts_at.as_deref().and_then(format_when_short) {
        Some(when) => format!("{prefix} : {summary} @ {when} ({who})"),
        None => format!("{prefix} : {summary} ({who})"),
    }
}

/// The invitation body, laid out the way a calendar invitation reads: the event
/// name, then Date / Lieu / Invités as titled blocks separated by real vertical
/// space, rather than a cramped label-value table. Every sender-supplied value
/// is HTML-escaped before it lands in the markup: this HTML is produced
/// server-side and must not carry injected tags or script.
fn build_body_html(invite: &CalendarInvite, is_cancel: bool, rsvp: Option<&RsvpLinks>) -> String {
    let summary = esc(invite.summary.as_deref().unwrap_or("Événement"));

    let mut blocks = String::new();

    // The organiser's own words come first, as they do in a calendar invite.
    if let Some(desc) = invite.description.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        blocks.push_str(&format!(
            "<div style=\"margin:0 0 28px;font-size:14px;color:#3c4043\">{}</div>",
            esc(desc).replace('\n', "<br>")
        ));
    }

    blocks.push_str(&info_block("Date", &build_when_line(invite)));

    if let Some(loc) = invite.location.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        blocks.push_str(&info_block("Lieu", &esc(loc)));
    }

    // Guests: the organiser is named first and marked as such, then everyone
    // invited — one per line, so a long list stays readable.
    let mut guests = String::new();
    let organizer = match (
        invite.organizer_name.as_deref().filter(|s| !s.trim().is_empty()),
        invite.organizer_email.as_deref(),
    ) {
        (Some(name), Some(email)) => format!("{} &lt;{}&gt;", esc(name), esc(email)),
        (Some(name), None) => esc(name),
        (None, Some(email)) => esc(email),
        (None, None) => String::new(),
    };
    if !organizer.is_empty() {
        guests.push_str(&format!(
            "<div style=\"margin:0 0 4px\">{organizer}<span style=\"color:#5f6368\"> — organisateur</span></div>"
        ));
    }
    for a in &invite.attendees {
        let who = match a.name.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(n) => format!("{} &lt;{}&gt;", esc(n), esc(&a.email)),
            None => esc(&a.email),
        };
        guests.push_str(&format!("<div style=\"margin:0 0 4px\">{who}</div>"));
    }
    if !guests.is_empty() {
        blocks.push_str(&info_block("Invités", &guests));
    }

    let banner = if is_cancel {
        format!(
            "<div style=\"margin:0 0 8px;font-size:13px;color:#b3261e;font-weight:700\">Événement annulé</div>\
             <div style=\"margin:0 0 28px;font-size:22px;line-height:1.3;color:#202124;text-decoration:line-through\">{summary}</div>"
        )
    } else {
        format!("<div style=\"margin:0 0 28px;font-size:22px;line-height:1.3;color:#202124\">{summary}</div>")
    };

    // Answering: one button per answer when the instance publishes a public URL,
    // so a guest replies from any mail client without signing in. Without those
    // links the guest answers from their own calendar, through the attached file.
    let answer_block = if is_cancel {
        String::new()
    } else {
        let buttons = rsvp
            .map(|r| {
                [("Oui", r.yes.as_deref()), ("Non", r.no.as_deref()), ("Peut-être", r.maybe.as_deref())]
                    .iter()
                    .filter_map(|(label, url)| url.map(|u| button_link(label, u)))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        if buttons.is_empty() {
            "<div style=\"margin:32px 0 0;padding:16px 0 0;border-top:1px solid #e0e0e0;font-size:13px;color:#5f6368;line-height:1.6\">\
               Répondez à cette invitation depuis votre agenda : le fichier <strong>invite.ics</strong> joint à ce message \
               permet de l'accepter, de la refuser ou de répondre « peut-être ».\
             </div>".to_string()
        } else {
            format!(
                "<div style=\"margin:32px 0 0;padding:20px 0 0;border-top:1px solid #e0e0e0\">\
                   <div style=\"margin:0 0 12px;font-size:14px;font-weight:700;color:#202124\">Répondre</div>\
                   <div>{buttons}</div>\
                 </div>"
            )
        }
    };

    format!(
        "<div style=\"font-family:'Outfit',Arial,Helvetica,sans-serif;max-width:600px;color:#3c4043;line-height:1.6\">\
           {banner}{blocks}{answer_block}\
         </div>"
    )
}

/// One answer button. A styled `<a>` is the only shape every mail client
/// renders alike; the URL is minted by Calendar, never by sender-supplied text.
fn button_link(label: &str, url: &str) -> String {
    format!(
        "<a href=\"{}\" style=\"display:inline-block;margin:0 8px 8px 0;padding:10px 24px;\
           border:1px solid #dadce0;border-radius:20px;font-size:14px;color:#1a73e8;\
           text-decoration:none\">{}</a>",
        esc(url),
        esc(label)
    )
}

/// One titled block of the invitation: a bold label, the value under it, and
/// room to breathe before the next one. `value_html` is already escaped (or
/// intentional markup such as `<br>`), never raw sender text.
fn info_block(label: &str, value_html: &str) -> String {
    format!(
        "<div style=\"margin:0 0 24px\">\
           <div style=\"margin:0 0 6px;font-size:14px;font-weight:700;color:#202124\">{label}</div>\
           <div style=\"font-size:14px;color:#3c4043\">{value_html}</div>\
         </div>"
    )
}

/// The "Quand" line: "toute la journée" for an all-day event, otherwise a
/// readable start (and end when known).
fn build_when_line(invite: &CalendarInvite) -> String {
    if invite.all_day {
        return match invite.starts_at.as_deref().and_then(format_date_only) {
            Some(d) => format!("{} (toute la journée)", esc(&d)),
            None => "Toute la journée".to_string(),
        };
    }
    let start = invite.starts_at.as_deref().and_then(format_when_long);
    let end = invite.ends_at.as_deref().and_then(format_time_only);
    match (start, end) {
        (Some(s), Some(e)) => format!("{} – {}", esc(&s), esc(&e)),
        (Some(s), None) => esc(&s),
        (None, _) => "—".to_string(),
    }
}

// ── Date formatting (French, locale-free) ───────────────────────────────────

const FR_DAYS: [&str; 7] = [
    "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
];
const FR_MONTHS: [&str; 12] = [
    "janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août",
    "septembre", "octobre", "novembre", "décembre",
];

/// Parses an RFC3339 timestamp into local (as-sent) civil time. Calendar sends
/// the instant; we render its wall-clock components as delivered.
fn parse_rfc3339(s: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(s.trim()).ok()
}

/// "lundi 5 septembre 2026 à 14:30"
fn format_when_long(s: &str) -> Option<String> {
    use chrono::{Datelike, Timelike};
    let dt = parse_rfc3339(s)?;
    let day = FR_DAYS.get(dt.weekday().num_days_from_monday() as usize)?;
    let month = FR_MONTHS.get((dt.month() as usize).checked_sub(1)?)?;
    Some(format!(
        "{day} {} {month} {} à {:02}:{:02}",
        dt.day(),
        dt.year(),
        dt.hour(),
        dt.minute()
    ))
}

/// "5 sept. à 14:30" — compact form for the subject line.
fn format_when_short(s: &str) -> Option<String> {
    use chrono::{Datelike, Timelike};
    let dt = parse_rfc3339(s)?;
    let month = FR_MONTHS.get((dt.month() as usize).checked_sub(1)?)?;
    Some(format!(
        "{} {} à {:02}:{:02}",
        dt.day(),
        &month[..month.len().min(4)],
        dt.hour(),
        dt.minute()
    ))
}

/// "lundi 5 septembre 2026" — no time, for an all-day event.
fn format_date_only(s: &str) -> Option<String> {
    use chrono::Datelike;
    let dt = parse_rfc3339(s)?;
    let day = FR_DAYS.get(dt.weekday().num_days_from_monday() as usize)?;
    let month = FR_MONTHS.get((dt.month() as usize).checked_sub(1)?)?;
    Some(format!("{day} {} {month} {}", dt.day(), dt.year()))
}

/// "14:30" — the end time on the same day as the start.
fn format_time_only(s: &str) -> Option<String> {
    use chrono::Timelike;
    let dt = parse_rfc3339(s)?;
    Some(format!("{:02}:{:02}", dt.hour(), dt.minute()))
}

/// Reads SEQUENCE from the ICS (0 when absent): a request with SEQUENCE > 0 is
/// an update to an invitation already sent.
fn ics_sequence(ics: &str) -> i64 {
    for raw in ics.lines() {
        let line = raw.trim();
        if let Some(rest) = line
            .strip_prefix("SEQUENCE:")
            .or_else(|| line.strip_prefix("sequence:"))
        {
            return rest.trim().parse::<i64>().unwrap_or(0);
        }
    }
    0
}

/// Minimal HTML escaping for a value that goes into element text or an attribute
/// value. `&` first so the entities it introduces are not re-escaped.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
