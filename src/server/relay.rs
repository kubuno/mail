//! The outbound relay (smarthost): the runtime side of the configuration the
//! admin API stores in `mail.outbound_relay`.
//!
//! Why this exists — an instance on a residential line cannot deliver
//! direct-to-MX (port 25 blocked outbound). Its only route out is a trusted
//! SMTP host (the VPS's Postfix over a private tunnel), so when a relay is
//! configured the outbound worker hands EVERY remote message to `host:port`
//! instead of resolving each recipient's MX. See migration 000026.
//!
//! This module is the read side: load the singleton row once per worker cycle,
//! decrypt the password with the module's `MailCrypto`, and hand the worker a
//! ready-to-use target — or `None`, meaning "deliver direct-to-MX as before".

use sqlx::PgPool;

use crate::services::crypto::MailCrypto;

/// How TLS is used TO the relay. Distinct from `OutboundTls` (the MX policy):
/// a relay is one known host, so the vocabulary is a mail client's — clear,
/// STARTTLS, or implicit TLS — not Postfix's opportunistic MX levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaySecurity {
    /// Cleartext. Legitimate ONLY over a trusted private link (the VPS tunnel).
    None,
    /// Upgrade an initially-cleartext connection with STARTTLS.
    StartTls,
    /// TLS from the first byte (implicit, the 465-style port).
    Tls,
}

impl RelaySecurity {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "starttls" => Some(Self::StartTls),
            "tls" => Some(Self::Tls),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::StartTls => "starttls",
            Self::Tls => "tls",
        }
    }
}

/// A relay the worker should deliver through, with its password already
/// decrypted. Built only when the row is enabled and has a host; the worker
/// treats `Some` as "relay everything" and `None` as "direct-to-MX".
#[derive(Debug, Clone)]
pub struct RelayTarget {
    pub host: String,
    pub port: u16,
    pub security: RelaySecurity,
    /// Empty = the relay wants no authentication (a trusted-network Postfix).
    pub username: String,
    /// Decrypted password; empty when there is none. NEVER logged.
    pub password: String,
}

/// One row of `mail.outbound_relay` as the worker reads it.
#[derive(sqlx::FromRow)]
struct RelayRow {
    enabled:        bool,
    host:           String,
    port:           i32,
    security:       String,
    username:       String,
    password_enc:   Option<Vec<u8>>,
    password_nonce: Option<Vec<u8>>,
}

/// Loads the relay configuration once, for a worker cycle.
///
/// Returns `None` — deliver direct-to-MX — when the relay is disabled, has no
/// host, or the row cannot be read (a transient DB error must not turn into a
/// bounce; the worker simply keeps the previous path and retries next cycle).
///
/// A password that is set but cannot be decrypted (missing/broken module key)
/// yields a target with an EMPTY password rather than no target: the connection
/// is still attempted, AUTH then fails, and the message is deferred — visible in
/// the log, never bounced, and self-heals once the key is fixed.
pub async fn fetch(db: &PgPool, crypto: Option<&MailCrypto>) -> Option<RelayTarget> {
    let row: Option<RelayRow> = match sqlx::query_as(
        "SELECT enabled, host, port, security, username, password_enc, password_nonce \
         FROM mail.outbound_relay WHERE id = TRUE",
    )
    .fetch_optional(db)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(error = %e, "Relais sortant : lecture de la configuration impossible");
            return None;
        }
    };

    let row = row?;
    if !row.enabled {
        return None;
    }
    let host = row.host.trim().to_string();
    if host.is_empty() {
        tracing::warn!("Relais sortant : activé mais sans hôte — livraison directe au MX");
        return None;
    }
    let security = RelaySecurity::parse(&row.security).unwrap_or(RelaySecurity::None);
    let port = u16::try_from(row.port).unwrap_or(25);

    // Decrypt the password only when authentication is configured.
    let username = row.username.trim().to_string();
    let password = if username.is_empty() {
        String::new()
    } else {
        decrypt_password(crypto, &row)
    };

    Some(RelayTarget { host, port, security, username, password })
}

/// Decrypts the stored password, or returns an empty string (never a secret in a
/// log) when there is nothing to decrypt or the module key is unavailable.
fn decrypt_password(crypto: Option<&MailCrypto>, row: &RelayRow) -> String {
    let (Some(enc), Some(nonce)) = (row.password_enc.as_ref(), row.password_nonce.as_ref()) else {
        return String::new();
    };
    let Some(crypto) = crypto else {
        tracing::error!("Relais sortant : clé de chiffrement du module indisponible — authentification impossible");
        return String::new();
    };
    match crypto.decrypt(enc, nonce) {
        Ok(pw) => pw,
        Err(e) => {
            // The error type only — the value is a credential.
            tracing::error!(error = %e, "Relais sortant : déchiffrement du mot de passe impossible");
            String::new()
        }
    }
}

/// The delivery target the worker picks for this cycle: the relay host:port when
/// a relay is active, otherwise direct-to-MX. Pure, so the decision the whole
/// feature hinges on is unit-tested on its own.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// Connect to this host:port for every remote recipient.
    Relay { host: String, port: u16 },
    /// Resolve each recipient's MX, as before.
    Mx,
}

pub fn choose_target(relay: Option<&RelayTarget>) -> Target {
    match relay {
        Some(r) => Target::Relay { host: r.host.clone(), port: r.port },
        None => Target::Mx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_parses_and_round_trips() {
        assert_eq!(RelaySecurity::parse(" STARTTLS "), Some(RelaySecurity::StartTls));
        assert_eq!(RelaySecurity::parse("tls"), Some(RelaySecurity::Tls));
        assert_eq!(RelaySecurity::parse("none"), Some(RelaySecurity::None));
        assert_eq!(RelaySecurity::parse("dane"), None);
        assert_eq!(RelaySecurity::StartTls.as_str(), "starttls");
    }

    fn target(host: &str, port: u16) -> RelayTarget {
        RelayTarget {
            host:     host.to_string(),
            port,
            security: RelaySecurity::None,
            username: String::new(),
            password: String::new(),
        }
    }

    /// The decision the feature hinges on: with a relay active every remote
    /// recipient is sent to host:port; without one, delivery resolves the MX.
    #[test]
    fn an_active_relay_targets_host_port_instead_of_the_mx() {
        let relay = target("15.100.1.1", 587);
        assert_eq!(
            choose_target(Some(&relay)),
            Target::Relay { host: "15.100.1.1".to_string(), port: 587 }
        );
        assert_eq!(choose_target(None), Target::Mx);
    }
}
