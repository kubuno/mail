//! `GET /diagnostics` — the deliverability report shown at the top of the
//! module's admin page. Administrators only.
//!
//! Everything it reports is external to this instance (a DNS zone, a file on
//! disk), so nothing is cached: an operator reads this page right after editing
//! a zone, and a cached answer would tell them the edit did not take. The cost
//! is a handful of DNS queries per visit.

use axum::{extract::State, Json};

use crate::{
    errors::MailError,
    middleware::AuthUser,
    server::config,
    services::diagnostics::{self, DkimTarget, Report},
    state::AppState,
};

/// Instance-wide facts, and a map of a domain's DNS: administrators only.
fn require_admin(user: &AuthUser) -> Result<(), MailError> {
    if user.role == "admin" {
        Ok(())
    } else {
        Err(MailError::Forbidden)
    }
}

pub async fn report(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Report>, MailError> {
    require_admin(&user)?;

    // The same view of the configuration the listeners run on, read from the
    // core rather than from a copy kept here — a diagnostic that reports on a
    // stale snapshot of the settings is worse than none.
    let http = reqwest::Client::new();
    let Some(cfg) = config::fetch(&http, &state.settings).await else {
        return Err(MailError::Internal(anyhow::anyhow!(
            "Configuration du serveur de messagerie illisible"
        )));
    };

    // Public halves only. The private key is not selected, so it cannot leak
    // through this route by a later careless edit of the struct.
    let keys = sqlx::query_as::<_, (String, String, String, String, bool)>(
        "SELECT domain, selector, algorithm, public_key, is_active \
         FROM mail.dkim_keys_all ORDER BY domain, is_active DESC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "diagnostic : lecture des clés DKIM");
        MailError::Database(e)
    })?
    .into_iter()
    .map(|(domain, selector, algorithm, public_key, is_active)| DkimTarget {
        domain,
        selector,
        algorithm,
        public_key,
        is_active,
    })
    .collect::<Vec<_>>();

    // Whether an outbound relay is active decides the SPF the setup offers: with
    // a smarthost in front, mail leaves from the relay's IP, so `ip4:<own-ip>`
    // would be wrong. Only the flag is read — never the credentials.
    let relay_enabled = sqlx::query_as::<_, (bool,)>(
        "SELECT enabled FROM mail.outbound_relay WHERE id = TRUE",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "diagnostic : lecture de l'état du relais sortant");
        MailError::Database(e)
    })?
    .map(|(enabled,)| enabled)
    .unwrap_or(false);

    Ok(Json(diagnostics::run(&cfg, &keys, relay_enabled).await))
}
