//! Web Key Directory (WKD) — serving THIS instance's public keys.
//!
//! Publishes the OpenPGP certificate of a local mailbox at the GnuPG/Sequoia
//! well-known location so other providers can discover it and encrypt to our
//! users without a manual key exchange. The address→URL mapping is in
//! `services::wkd`; here we resolve a served hash back to a local mailbox and
//! emit its key as a BINARY certificate.
//!
//! Reachability: a real WKD client queries the ROOT path
//! `/.well-known/openpgpkey/...`, which the core does NOT proxy to modules (its
//! module proxy only forwards `/api/v1/<module>/...`). These handlers are mounted
//! under the mail API (`/api/v1/mail/wkd/...`) and are reachable there today; to
//! answer real clients the root path must be routed here — see the module report
//! for the exact core/nginx glue. The `?d=` override lets that glue pass the
//! queried domain explicitly when the Host header is not the domain itself.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::services::wkd;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct WkdQuery {
    /// The original local-part (WKD `?l=`), advisory — the hash is authoritative.
    pub l: Option<String>,
    /// Domain override for the root-route glue / testing; falls back to Host.
    pub d: Option<String>,
}

/// WKD advanced-method policy file: its existence signals WKD support. We declare
/// no policy flags, so it is empty. Always public + CORS-open.
pub async fn policy() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        String::new(),
    )
        .into_response()
}

/// Serve the OpenPGP key for the local mailbox whose local-part hashes to `hash`
/// in the queried domain, as a binary certificate. 404 when nothing matches, when
/// the mailbox has no key, or when OpenPGP is disabled instance-wide.
pub async fn hu(
    State(state): State<AppState>,
    Path(hash): Path<String>,
    Query(q): Query<WkdQuery>,
    headers: HeaderMap,
) -> Response {
    // Only publish when OpenPGP is switched on for the instance.
    match crate::handlers::addresses::server_config(&state).await {
        Ok(cfg) if cfg.gpg_enabled => {}
        _ => return not_found(),
    }

    let Some(domain) = resolve_domain(&headers, q.d.as_deref()) else {
        return not_found();
    };
    let hash = hash.to_ascii_lowercase();

    // Match the served hash against the active local mailboxes of the domain.
    // `?l=` only narrows the scan; the hash is what must agree.
    let rows: Vec<(String, uuid::Uuid)> = sqlx::query_as(
        "SELECT address, user_id FROM mail.mailboxes WHERE domain = $1 AND is_active = TRUE",
    )
    .bind(&domain)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let matched = rows.into_iter().find(|(address, _)| {
        let local = address.split('@').next().unwrap_or("");
        if let Some(l) = &q.l {
            if !local.eq_ignore_ascii_case(l) {
                return false;
            }
        }
        wkd::local_part_hash(local) == hash
    });

    let Some((address, user_id)) = matched else {
        return not_found();
    };

    // The mailbox owner's key: prefer one whose User ID matches the address, then
    // the default identity.
    let public_armored: Option<String> = sqlx::query_scalar(
        r#"SELECT public_key FROM mail.pgp_keys WHERE user_id = $1
           ORDER BY (lower(email) = lower($2)) DESC, is_default DESC, created_at
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(&address)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some(public_armored) = public_armored else {
        return not_found();
    };
    let Ok(binary) = crate::services::pgp::public_armored_to_binary(&public_armored) else {
        return not_found();
    };

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        binary,
    )
        .into_response()
}

/// The queried domain: the `?d=` override, else the Host header with any port and
/// the advanced-method `openpgpkey.` prefix stripped, lower-cased.
fn resolve_domain(headers: &HeaderMap, d: Option<&str>) -> Option<String> {
    let raw = match d {
        Some(d) => d.to_string(),
        None => headers.get(header::HOST)?.to_str().ok()?.to_string(),
    };
    let host = raw.split(':').next().unwrap_or(&raw).to_ascii_lowercase();
    let domain = host.strip_prefix("openpgpkey.").unwrap_or(&host);
    (!domain.is_empty()).then(|| domain.to_string())
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        String::new(),
    )
        .into_response()
}
