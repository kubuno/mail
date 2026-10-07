//! Internal endpoints the core calls to migrate a mailbox from a third-party
//! IMAP server into a mailbox hosted here.
//!
//! Both are guarded by the shared internal secret (see the `internal` router):
//! they take a plaintext password in the body and copy mail into an arbitrary
//! user's mailbox, so no user session may ever reach them.
//!
//! They answer 200 with `ok: false` for anything the operator can act on — a
//! wrong password, an unreachable host, a destination with no mailbox — because
//! the core relays the message to an admin screen. Only a malformed request is
//! a 4xx.

use axum::{extract::State, http::StatusCode, response::{IntoResponse, Response}, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    services::import_service::{self, SourceSpec},
    state::AppState,
};

/// Deliberately NOT `Debug`: it carries the source mailbox password.
#[derive(Deserialize)]
struct SourceDto {
    host:     String,
    port:     u16,
    security: String,
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct ProbeRequest {
    source: SourceDto,
}

#[derive(Deserialize)]
struct RunRequest {
    source:          SourceDto,
    target_user_id:  Uuid,
    #[serde(default)]
    since:           Option<String>,
    #[serde(default)]
    exclude_folders: Vec<String>,
    #[serde(default)]
    budget_secs:     Option<u64>,
    /// Opaque blob returned by the previous call; absent or null on the first.
    #[serde(default)]
    cursor:          Value,
}

/// Longest folder name we accept in `exclude_folders`, and how many.
const MAX_EXCLUDE_ENTRIES: usize = 200;
const MAX_EXCLUDE_LEN:     usize = 500;
/// RFC 1035 caps a domain name at 255 octets.
const MAX_HOST_LEN:        usize = 255;

/// POST /internal/migration/probe — what the source mailbox contains.
pub async fn probe(Json(body): Json<Value>) -> Response {
    // Deserialised by hand so a malformed body gets OUR error shape. The serde
    // message itself is never echoed: a mistyped `password` field would come
    // back with its value quoted inside it.
    let req: ProbeRequest = match serde_json::from_value(body) {
        Ok(r)  => r,
        Err(_) => return bad_request("Requête invalide : champ manquant ou de type incorrect."),
    };

    let src = match validate_source(req.source) {
        Ok(s)  => s,
        Err(m) => return bad_request(&m),
    };

    match import_service::probe(&src).await {
        Ok(folders) => ok_json(json!({ "ok": true, "folders": folders })),
        Err(e) => {
            tracing::warn!(host = %src.host, error = %e, "Migration : sondage de la boîte source échoué");
            ok_json(json!({ "ok": false, "error": e.to_string() }))
        }
    }
}

/// POST /internal/migration/run — copy one bounded chunk, then report progress.
pub async fn run(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let req: RunRequest = match serde_json::from_value(body) {
        Ok(r)  => r,
        Err(_) => return bad_request("Requête invalide : champ manquant ou de type incorrect."),
    };

    // Echoed back untouched on the error path, so a failed chunk never costs
    // the core the progress already made.
    let cursor_in = req.cursor;

    let since = match req.since.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => match chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            Ok(d)  => Some(d),
            Err(_) => return bad_request("Date « since » invalide : format attendu AAAA-MM-JJ."),
        },
    };

    if req.exclude_folders.len() > MAX_EXCLUDE_ENTRIES {
        return bad_request("Trop de dossiers exclus.");
    }
    if req.exclude_folders.iter().any(|f| f.len() > MAX_EXCLUDE_LEN) {
        return bad_request("Nom de dossier exclu trop long.");
    }

    let budget_secs = req.budget_secs.unwrap_or(20).clamp(1, 120);

    let src = match validate_source(req.source) {
        Ok(s)  => s,
        Err(m) => return bad_request(&m),
    };

    match import_service::run_chunk(
        &state.db,
        &state.settings.mail,
        &src,
        req.target_user_id,
        since,
        &req.exclude_folders,
        budget_secs,
        cursor_in.clone(),
    )
    .await
    {
        Ok(outcome) => ok_json(json!({
            "ok":     true,
            "done":   outcome.done,
            "cursor": outcome.cursor,
            "copied": outcome.copied,
            "total":  outcome.total,
            "error":  Value::Null,
        })),
        Err(e) => {
            tracing::warn!(
                host = %src.host, target_user_id = %req.target_user_id, error = %e,
                "Migration : lot échoué"
            );
            ok_json(json!({
                "ok":     false,
                "done":   false,
                "cursor": cursor_in,
                "copied": 0,
                "total":  0,
                "error":  e.to_string(),
            }))
        }
    }
}

/// Checks the source before a single byte goes over the wire. The password is
/// only ever tested for emptiness — never measured, logged or echoed.
fn validate_source(dto: SourceDto) -> Result<SourceSpec, String> {
    let host = dto.host.trim().to_string();
    if host.is_empty() {
        return Err("Le serveur source est requis.".to_string());
    }
    if host.len() > MAX_HOST_LEN {
        return Err("Nom de serveur source trop long.".to_string());
    }
    if dto.port == 0 {
        return Err("Port source invalide.".to_string());
    }
    let security = dto.security.trim().to_lowercase();
    if !matches!(security.as_str(), "ssl" | "starttls" | "none") {
        return Err("Sécurité source invalide : ssl, starttls ou none.".to_string());
    }
    let username = dto.username.trim().to_string();
    if username.is_empty() {
        return Err("L'identifiant source est requis.".to_string());
    }
    if dto.password.is_empty() {
        return Err("Le mot de passe source est requis.".to_string());
    }

    Ok(SourceSpec {
        host,
        port: dto.port,
        security,
        username,
        password: dto.password,
    })
}

fn ok_json(body: Value) -> Response {
    (StatusCode::OK, Json(body)).into_response()
}

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}
