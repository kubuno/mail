//! Automatic mailbox attribution.
//!
//! ## What it does, and why it is a reconcile rather than an event handler
//!
//! When a primary domain is verified, every account that has no mailbox on it
//! should receive one, built from an administrator's rule; every new account
//! after that should be served the same way. The obvious wiring — react to a
//! `domain.mail_ready` event and to `UserCreated` — is not the one used here,
//! for a plain reason: this module does not ingest core events (it has no
//! `/ipc/events` route), and even a module that did would miss the ones that
//! arrived while it was down. So the mechanism is a **reconcile**: read the
//! world, create what is missing, and be safe to run again.
//!
//! [`reconcile`] is therefore idempotent by construction. It runs on a timer
//! (the scheduler worker), so a new account gets its address within one tick,
//! and a domain that becomes ready is back-filled on the next pass — no event,
//! no delivery to miss.
//!
//! ## What it never does
//!
//! It never overwrites. An account that already holds a mailbox on the domain is
//! skipped whole — a manually chosen address, or one from a previous run, is
//! left untouched. That is enforced in
//! [`crate::handlers::addresses::mailboxes::provision_mailbox`], not here.

use crate::state::AppState;

/// One account, as the core's provisioning endpoint returns it.
#[derive(serde::Deserialize)]
struct ProvUser {
    id: uuid::Uuid,
    username: String,
    display_name: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
}

#[derive(serde::Deserialize)]
struct ProvList {
    users: Vec<ProvUser>,
}

/// Reads the world and provisions what is missing. Cheap and silent when there
/// is nothing to do — off by setting, or no verified primary domain.
pub async fn reconcile(state: &AppState) {
    let http = reqwest::Client::new();

    let cfg = match crate::server::config::fetch(&http, &state.settings).await {
        Some(c) => c,
        None => return, // core unreachable; try again next tick.
    };

    if !cfg.autoprovision {
        return;
    }

    // The one domain a mailbox is attributed on: the primary, ownership proven.
    // A secondary or an alias is not a home for a new account's address.
    let domain = match cfg
        .instance_domains
        .iter()
        .find(|d| d.kind == "primary" && d.verified)
    {
        Some(d) => d.name.clone(),
        None => return,
    };

    let url = format!("{}/internal/mail/provisioning/users", state.settings.core.url);
    let list: ProvList = match http
        .get(&url)
        .header("X-Internal-Secret", state.settings.core.internal_secret.as_str())
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "provisioning : réponse illisible");
                return;
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "provisioning : liste des comptes injoignable");
            return;
        }
    };

    let mut created = 0usize;
    for u in &list.users {
        let base = local_part(&cfg.address_format, u);
        if base.is_empty() {
            continue; // nothing usable, not even a username — skip rather than guess.
        }
        match crate::handlers::addresses::mailboxes::provision_mailbox(
            state,
            &domain,
            u.id,
            &base,
            u.display_name.as_deref(),
        )
        .await
        {
            Ok(Some(address)) => {
                created += 1;
                tracing::info!(user_id = %u.id, %address, "provisioning : adresse attribuée");
            }
            Ok(None) => {} // already served, or no free address — nothing to say each tick.
            Err(e) => tracing::warn!(user_id = %u.id, error = %e, "provisioning : échec"),
        }
    }

    if created > 0 {
        tracing::info!(created, domain = %domain, "provisioning : adresses attribuées ce cycle");
    }
}

/// Builds the local part from the rule and one account.
///
/// Tokens: `{prenom}`, `{nom}`, `{p}`, `{n}`, `{username}`. A missing name part
/// leaves its token empty; if the whole thing comes out empty (no names at all),
/// the caller falls back to the username, because an account with no address is
/// worse than one whose address is its login.
fn local_part(format: &str, u: &ProvUser) -> String {
    let first = u.first_name.as_deref().unwrap_or("").trim();
    let last = u.last_name.as_deref().unwrap_or("").trim();
    let initial = |s: &str| s.chars().next().map(|c| c.to_string()).unwrap_or_default();

    let rendered = format
        .replace("{prenom}", first)
        .replace("{nom}", last)
        .replace("{p}", &initial(first))
        .replace("{n}", &initial(last))
        .replace("{username}", &u.username);

    let sanitised = sanitize(&rendered);
    if sanitised.is_empty() {
        sanitize(&u.username)
    } else {
        sanitised
    }
}

/// Reduces free text to a valid mailbox local part.
///
/// Lower-cased, accents folded to ASCII, anything not `[a-z0-9._-]` turned into a
/// dot, runs of dots collapsed, and no leading or trailing dot — the shape an
/// address parser will accept without a fight, and the one a human reads as a
/// name rather than as an escape sequence.
fn sanitize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.trim().chars() {
        match fold_ascii(ch) {
            // A known Latin accent: emit its ASCII base (already lower-case).
            Some(base) => out.push_str(base),
            // Anything else: keep it if it is an allowed character, else a dot.
            None => {
                let c = ch.to_ascii_lowercase();
                if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                    out.push(c);
                } else {
                    out.push('.');
                }
            }
        }
    }
    // Collapse dot runs and trim dots from the ends.
    let mut collapsed = String::with_capacity(out.len());
    let mut last_dot = false;
    for c in out.chars() {
        if c == '.' {
            if !last_dot {
                collapsed.push('.');
            }
            last_dot = true;
        } else {
            collapsed.push(c);
            last_dot = false;
        }
    }
    collapsed.trim_matches('.').to_string()
}

/// Folds the Latin accents that turn up in real names to their ASCII base, or
/// `None` when the character is not one this table knows — the caller then keeps
/// the character if it is allowed and turns it into a dot otherwise. Not a full
/// Unicode transliteration: a name in a script with no ASCII form collapses to
/// dots and the rule falls back to the username, which is the honest outcome.
fn fold_ascii(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => "a",
        'ç' | 'Ç' => "c",
        'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => "i",
        'ñ' | 'Ñ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' => "o",
        'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => "u",
        'ý' | 'ÿ' | 'Ý' => "y",
        'æ' | 'Æ' => "ae",
        'œ' | 'Œ' => "oe",
        'ß' => "ss",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn folds_accents_and_case() {
        assert_eq!(sanitize("Marie"), "marie");
        assert_eq!(sanitize("Hélène"), "helene");
        assert_eq!(sanitize("Léa Fauré"), "lea.faure");
    }

    #[test]
    fn collapses_and_trims_dots() {
        assert_eq!(sanitize("  Jean--Marie  "), "jean--marie");
        assert_eq!(sanitize("O'Neill"), "o.neill");
        assert_eq!(sanitize(".x."), "x");
        assert_eq!(sanitize("a  b"), "a.b");
    }

    #[test]
    fn non_latin_collapses_to_empty() {
        assert_eq!(sanitize("日本語"), "");
    }
}
