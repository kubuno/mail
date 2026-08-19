//! The mail services this module OFFERS — SMTP, IMAP and POP3 — as opposed to
//! the client side, which connects to other people's servers.
//!
//! Each protocol is a plain TCP listener. Which ones run, on which ports, is
//! decided by the administrator in the console (`module.toml`'s `[[settings]]`,
//! read back through `/internal/modules/settings`), so the supervisor polls
//! that configuration and starts or stops listeners as it changes — no restart
//! needed to turn a service on.
//!
//! Nothing listens until an administrator switches it on: a mail server that
//! appears by itself the day a module is installed is a mail server nobody
//! decided to run.

pub mod auth;
pub mod authres;
pub mod compliance;
pub mod config;
pub mod deliver;
pub mod dkim;
pub mod dsn;
pub mod greylist;
pub mod hygiene;
pub mod imap;
pub mod journal;
pub mod limits;
pub mod outbound;
pub mod pop3;
pub mod queue;
pub mod relay;
pub mod resolve;
pub mod retention;
pub mod scram;
pub mod signing;
pub mod smtp;
pub mod store;
pub mod tls;
pub mod worker;

use std::{net::IpAddr, sync::Arc, time::Duration};

use sqlx::PgPool;
use tokio::{net::TcpListener, sync::RwLock, task::JoinHandle};
use tokio_rustls::TlsAcceptor;

use crate::config::settings::Settings;
use config::{Listener, Protocol, ServerConfig};
use limits::{ConnectionLimiter, SystemClock, Tarpit};
use tls::{MailStream, TlsMode};

/// The two shared, per-IP hardening facilities: a simultaneous-connection cap
/// (anvil) and an exponential auth tarpit (auth-penalty). One instance each,
/// shared by every listener, since the abuse they defend against is per-IP
/// across the whole server.
#[derive(Clone)]
struct Hardening {
    limiter: Arc<ConnectionLimiter>,
    tarpit:  Arc<Tarpit>,
}

/// How often the supervisor re-reads the administrator's choices.
const RECONFIGURE_EVERY: Duration = Duration::from_secs(30);

/// What is in force right now, read afresh by every accepted connection.
///
/// A listener task is spawned once and then runs for as long as its socket is
/// bound, so it must not carry a copy of the configuration: it would serve the
/// settings of the day it started for as long as the port stayed open. It holds
/// this handle instead and takes a snapshot per connection.
///
/// The configuration and the acceptor sit behind ONE lock so a session can never
/// pair a new configuration with the certificate of the old one.
struct Live {
    cfg:      Arc<ServerConfig>,
    acceptor: Option<Arc<TlsAcceptor>>,
}

type LiveConfig = Arc<RwLock<Live>>;

/// The part of the configuration a bound socket is *made of*. Everything else —
/// limits, policies, timeouts, the certificate — is read per connection and
/// therefore needs no rebind.
///
/// Comparing whole `ServerConfig`s to decide this would tear every socket down
/// and back up because an administrator changed, say, `spf_fail_action`: a
/// window where the ports are refused, for a setting no socket depends on.
fn bind_signature(cfg: &ServerConfig) -> (&str, &[Listener]) {
    (cfg.bind.as_str(), cfg.listeners.as_slice())
}

/// Everything a protocol handler needs, gathered once per accepted connection.
pub struct Incoming {
    pub db:         PgPool,
    pub cfg:        Arc<ServerConfig>,
    pub peer:       String,
    /// The listener's TLS mode — tells the handler whether to advertise
    /// STARTTLS/STLS and whether the connection is already encrypted.
    pub tls_mode:   TlsMode,
    /// SMTP only: submission (auth required) vs reception (MX).
    pub submission: bool,
    /// Present when STARTTLS is possible on this listener (a certificate is
    /// configured); `None` otherwise, so the handler never advertises an upgrade
    /// it cannot perform.
    pub acceptor:   Option<Arc<TlsAcceptor>>,
    /// Shared per-IP auth tarpit. Handlers slow their failure replies by
    /// `tarpit.delay_for(ip)` and call `record_failure`/`record_success` around
    /// each authentication, throttling brute force without penalising a
    /// legitimate user (Dovecot auth-penalty).
    pub tarpit:     Arc<Tarpit>,
}

impl Incoming {
    /// The client IP, parsed from `peer` (`ip:port`). Falls back to the
    /// unspecified address if the string is somehow unparseable, so tarpit and
    /// limits still key on *something* stable rather than panicking.
    pub fn peer_ip(&self) -> IpAddr {
        self.peer
            .rsplit_once(':')
            .and_then(|(host, _)| host.trim_start_matches('[').trim_end_matches(']').parse().ok())
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
    }
}

/// One running listener, kept so it can be dropped when the setting changes.
struct Running {
    listener: Listener,
    bind:     String,
    task:     JoinHandle<()>,
}

/// Runs forever: reads the configuration, reconciles the listeners with it,
/// sleeps, repeats.
pub async fn run(db: PgPool, settings: Settings, http: reqwest::Client) {
    let mut running: Vec<Running> = Vec::new();
    let mut current = ServerConfig::default();
    let mut announced = false;

    // Both live for the whole process and are RECONFIGURED, never replaced: the
    // strikes and the live connection counts they hold are about clients that
    // are still there, and an administrator saving an unrelated setting must not
    // wipe them.
    let hardening = Hardening {
        limiter: Arc::new(ConnectionLimiter::new(current.max_conn_per_ip)),
        tarpit:  Arc::new(Tarpit::new(Box::new(SystemClock::new()))),
    };
    hardening.tarpit.set_enabled(current.auth_penalty_enabled);

    let live: LiveConfig = Arc::new(RwLock::new(Live {
        cfg:      Arc::new(current.clone()),
        acceptor: None,
    }));

    loop {
        if let Some(next) = config::fetch(&http, &settings).await {
            if next != current || !announced {
                // Only a change to the bind address or to the set of listeners
                // justifies touching the sockets. A policy or limit change is
                // published below and picked up by the next connection.
                let rebind = !announced || bind_signature(&next) != bind_signature(&current);

                if next != current {
                    tracing::info!(
                        listeners = next.listeners.len(), tls = next.has_tls(),
                        bind = %next.bind, rebind,
                        "Serveur de messagerie : configuration mise à jour"
                    );
                    hardening.limiter.set_max_per_ip(next.max_conn_per_ip);
                    hardening.tarpit.set_enabled(next.auth_penalty_enabled);
                }

                // Rebuild the acceptor (certificate, TLS floor) and publish both
                // BEFORE reconciling: a listener started just below serves its
                // very first connection from this snapshot.
                let acceptor = build_acceptor(&next);
                {
                    let mut guard = live.write().await;
                    guard.cfg = Arc::new(next.clone());
                    guard.acceptor = acceptor;
                }

                current = next;
                if rebind {
                    reconcile(&mut running, &current, &hardening, &live, &db).await;
                }
                announced = true;
            }
        }
        tokio::time::sleep(RECONFIGURE_EVERY).await;
    }
}

/// Builds the shared TLS acceptor from the configuration, logging (not failing)
/// on a bad certificate — a broken cert must not take the whole module down,
/// only the TLS listeners.
fn build_acceptor(cfg: &ServerConfig) -> Option<Arc<TlsAcceptor>> {
    match tls::build_acceptor(&cfg.tls_cert_path, &cfg.tls_key_path, cfg.tls_min_version) {
        Ok(Some(acc)) => Some(Arc::new(acc)),
        Ok(None) => None,
        Err(e) => {
            tracing::error!(error = %e, "Certificat TLS illisible — services chiffrés indisponibles");
            None
        }
    }
}

/// Brings the set of listeners in line with the configuration: stops what is no
/// longer wanted, starts what is missing, leaves the rest running so open
/// client sessions are not cut for an unrelated change.
async fn reconcile(
    running: &mut Vec<Running>,
    cfg: &ServerConfig,
    hardening: &Hardening,
    live: &LiveConfig,
    db: &PgPool,
) {
    // Stop the listeners that are no longer wanted, and WAIT for each aborted
    // task to unwind before moving on: its TcpListener is dropped only when the
    // task's future is, so rebinding the same port immediately (a TLS mode
    // change keeps the port but replaces the Listener) would otherwise hit
    // EADDRINUSE against the socket we just told to close.
    let mut kept = Vec::with_capacity(running.len());
    for r in running.drain(..) {
        if cfg.bind == r.bind && cfg.listeners.contains(&r.listener) {
            kept.push(r);
        } else {
            tracing::info!(protocol = r.listener.protocol.as_str(), port = r.listener.port,
                "Service de messagerie arrêté");
            r.task.abort();
            let _ = r.task.await;
        }
    }
    *running = kept;

    for listener in &cfg.listeners {
        if running.iter().any(|r| r.bind == cfg.bind && r.listener == *listener) {
            continue;
        }
        match start(*listener, &cfg.bind, live.clone(), hardening.clone(), db.clone()).await {
            Ok(task) => {
                tracing::info!(
                    protocol = listener.protocol.as_str(), port = listener.port,
                    tls = ?listener.tls, submission = listener.submission, bind = %cfg.bind,
                    "Service de messagerie démarré"
                );
                running.push(Running { listener: *listener, bind: cfg.bind.clone(), task });
            }
            Err(e) => {
                // The usual cause is a port below 1024: the service runs
                // unprivileged, so binding one needs CAP_NET_BIND_SERVICE.
                tracing::error!(protocol = listener.protocol.as_str(), port = listener.port,
                    error = %e, "Écoute impossible");
            }
        }
    }
}

async fn start(
    listener: Listener,
    bind: &str,
    live: LiveConfig,
    hardening: Hardening,
    db: PgPool,
) -> std::io::Result<JoinHandle<()>> {
    let socket = TcpListener::bind((bind, listener.port)).await?;

    Ok(tokio::spawn(async move {
        loop {
            let (stream, peer_addr) = match socket.accept().await {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::error!(protocol = listener.protocol.as_str(), error = %e,
                        "Connexion entrante refusée");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            };

            // One snapshot per connection: the session keeps a consistent view
            // for its whole life, while the NEXT connection already sees an
            // administrator's change — no rebind, no cut session. Read BEFORE
            // the connection cap so the trusted-upstream exemption below can
            // consult it.
            let (cfg, acceptor) = {
                let current = live.read().await;
                (current.cfg.clone(), current.acceptor.clone())
            };

            // Per-IP simultaneous-connection cap (anvil): refuse a host already
            // at its limit before spending any work on it. The guard is moved
            // into the session task and frees the slot when it ends.
            //
            // A declared trusted upstream relay is exempt: it legitimately opens
            // many simultaneous connections to forward mail, and capping it at
            // the per-IP limit meant for a hostile client would throttle the
            // whole instance. It gets no guard (`None`) and is never counted.
            let guard = if cfg.is_trusted_upstream(peer_addr.ip()) {
                None
            } else {
                match hardening.limiter.acquire(peer_addr.ip()) {
                    Some(g) => Some(g),
                    None => {
                        tracing::warn!(peer = %peer_addr, protocol = listener.protocol.as_str(),
                            "Trop de connexions simultanées pour cette IP — refusée");
                        drop(stream);
                        continue;
                    }
                }
            };

            let _ = stream.set_nodelay(true);
            let db = db.clone();
            let tarpit = hardening.tarpit.clone();
            let peer = peer_addr.to_string();

            tokio::spawn(async move {
                let _guard = guard; // held for the whole session (None for a trusted upstream); frees the slot on drop
                // Implicit-TLS ports (465/993/995) hand the handler an already
                // encrypted stream; the handshake happens here, before a single
                // protocol byte. STARTTLS ports start plaintext and upgrade later.
                let mail_stream = if listener.tls == TlsMode::Implicit {
                    match &acceptor {
                        Some(acc) => match acc.accept(stream).await {
                            Ok(tls) => MailStream::Tls(Box::new(tls)),
                            Err(e) => {
                                tracing::debug!(peer = %peer, error = %e, "Handshake TLS implicite échoué");
                                return;
                            }
                        },
                        None => {
                            tracing::error!(port = listener.port, "Port TLS implicite sans certificat — connexion fermée");
                            return;
                        }
                    }
                } else {
                    MailStream::Plain(stream)
                };

                // STARTTLS is only offered when a certificate exists AND the
                // listener asked for it — never advertise an upgrade we cannot do.
                let starttls_acceptor = if listener.tls == TlsMode::StartTls { acceptor.clone() } else { None };
                let inc = Incoming {
                    db, cfg, peer: peer.clone(),
                    tls_mode: listener.tls,
                    submission: listener.submission,
                    acceptor: starttls_acceptor,
                    tarpit,
                };

                let outcome = match listener.protocol {
                    Protocol::Smtp => smtp::handle(inc, mail_stream).await,
                    Protocol::Imap => imap::handle(inc, mail_stream).await,
                    Protocol::Pop3 => pop3::handle(inc, mail_stream).await,
                };
                if let Err(e) = outcome {
                    // Clients disconnect mid-session all the time; this is
                    // diagnostic material, not a fault of the server.
                    tracing::debug!(protocol = listener.protocol.as_str(), peer = %peer,
                        error = %e, "Session terminée sur erreur");
                }
            });
        }
    }))
}

/// Records a finished session. Best effort: failing to write the log must never
/// fail the session that just ended.
pub async fn log_session(
    db: &PgPool,
    protocol: &str,
    peer: &str,
    mailbox: Option<&auth::Mailbox>,
    commands: i32,
    error: Option<String>,
) {
    let result = sqlx::query(
        r#"INSERT INTO mail.server_sessions
             (protocol, user_id, username, peer, authed, commands, error, ended_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())"#,
    )
    .bind(protocol)
    .bind(mailbox.map(|m| m.user_id))
    .bind(mailbox.map(|m| m.username.clone()))
    .bind(peer)
    .bind(mailbox.is_some())
    .bind(commands)
    .bind(error)
    .execute(db)
    .await;

    if let Err(e) = result {
        tracing::error!(error = %e, protocol, "Journalisation de session impossible");
    }
}

#[cfg(test)]
mod tests {
    use super::config::{PolicyAction, TlsFloor};
    use super::*;

    fn base() -> ServerConfig {
        ServerConfig {
            bind: "127.0.0.1".into(),
            listeners: vec![Listener {
                protocol:   Protocol::Imap,
                port:       1143,
                tls:        TlsMode::StartTls,
                submission: false,
            }],
            ..ServerConfig::default()
        }
    }

    fn same_sockets(a: &ServerConfig, b: &ServerConfig) -> bool {
        bind_signature(a) == bind_signature(b)
    }

    /// The reason this function exists: a policy knob must not close and reopen
    /// every port. The whole-struct comparison that gates the reconfiguration
    /// says "something changed"; only this says "the sockets changed".
    #[test]
    fn a_policy_change_does_not_rebind() {
        let before = base();
        let mut after = before.clone();
        after.spf_fail_action = PolicyAction::Reject;
        after.max_message_bytes = 50 * 1024 * 1024;
        after.imap_idle_minutes = 5;
        after.auth_penalty_enabled = false;
        after.max_conn_per_ip = 40;

        assert_ne!(before, after, "la configuration a bien changé");
        assert!(same_sockets(&before, &after), "mais aucune socket n'est concernée");
    }

    /// The certificate and the TLS floor are read per connection from the shared
    /// snapshot, so they take effect on the next handshake without unbinding a
    /// port — no window during which the service is refused.
    #[test]
    fn a_tls_floor_change_does_not_rebind_either() {
        let before = base();
        let mut after = before.clone();
        after.tls_min_version = TlsFloor::Tls13;
        assert_ne!(before, after);
        assert!(same_sockets(&before, &after));
    }

    #[test]
    fn a_port_a_bind_or_a_tls_mode_change_does_rebind() {
        let before = base();

        let mut port = before.clone();
        port.listeners[0].port = 143;
        assert!(!same_sockets(&before, &port), "changement de port");

        let mut bind = before.clone();
        bind.bind = "0.0.0.0".into();
        assert!(!same_sockets(&before, &bind), "changement d'adresse d'écoute");

        // A certificate appearing or disappearing turns a StartTls listener into
        // a plaintext one (see `config::from_settings`), which IS a new socket.
        let mut mode = before.clone();
        mode.listeners[0].tls = TlsMode::None;
        assert!(!same_sockets(&before, &mode), "changement de mode TLS");

        let mut added = before.clone();
        added.listeners.push(Listener {
            protocol:   Protocol::Pop3,
            port:       1110,
            tls:        TlsMode::None,
            submission: false,
        });
        assert!(!same_sockets(&before, &added), "listener ajouté");

        let removed = ServerConfig { listeners: Vec::new(), ..before.clone() };
        assert!(!same_sockets(&before, &removed), "listener retiré");
    }
}
