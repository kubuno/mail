//! The SMTP service this module OFFERS (RFC 5321): how mail gets INTO Kubuno.
//!
//! Two kinds of client talk to this port, and the difference between them is
//! the whole security model:
//!
//! * a foreign mail server delivering to one of our users. It does not
//!   authenticate, and it may therefore only name recipients in one of OUR
//!   domains. Anything else is refused with `550 5.7.1 Relay access denied`.
//! * one of our own users, authenticated with a mailbox credential
//!   (`server::auth`), who is allowed to send anywhere — see `relay_decision`.
//!
//! An SMTP server that accepts mail for third-party domains from unauthenticated
//! clients is an open relay: within hours it is enumerated, within days it sends
//! spam and its address ends up on every blocklist. `relay_decision` is the
//! single place that call is made, it is total over the four cases, and it is
//! covered by tests. Nothing else in this file decides who may relay.
//!
//! TLS: the connection may arrive already encrypted (implicit TLS, port 465) or
//! be upgraded in place with STARTTLS (RFC 3207) when the listener offers it and
//! a certificate is configured. The anti-injection defence of the upgrade itself
//! lives in `server::tls`; this file only drives it.
//!
//! Who a recipient IS — a mailbox, an alias, a catch-all, a distribution list —
//! is answered by `server::resolve`, at RCPT time, because a recipient we
//! cannot serve must be refused while the sender is still being told which one.
//! One RCPT may therefore turn into several local deliveries and several
//! forwards; the limits that keep that expansion bounded live there.
//!
//! Not implemented on purpose: pipelining announcements.

use std::{net::IpAddr, sync::Arc, time::Duration};

use anyhow::Result;
use base64::Engine;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_rustls::TlsAcceptor;

use crate::server::limits::Tarpit;

use crate::server::{
    auth::{self, Mailbox},
    authres::{self, AuthVerdict},
    compliance,
    config::{PolicyAction, ServerConfig},
    deliver, greylist, hygiene, journal, log_session, queue,
    resolve::{self, LocalDelivery, Outcome},
    scram,
    tls::{MailStream, TlsMode, UpgradeError},
    Incoming,
};

/// A session with nothing happening is dropped. RFC 5321 §4.5.3.2 asks for at
/// least 5 minutes on most commands.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// RFC 5321 §4.5.3.1.4: a command line is at most 512 octets. 1000 is the text
/// line limit; using it here leaves room for the long parameter lists real
/// clients send while still bounding what we buffer.
const MAX_COMMAND_BYTES: usize = 1000;

/// DATA lines: the RFC's limit is 1000 octets, but receivers are expected to be
/// tolerant (§4.5.3.1.6) and real mailers do emit longer lines. Beyond this
/// bound the message is REFUSED, never silently truncated.
const MAX_DATA_LINE_BYTES: usize = 65_536;

// The recipient ceiling (RFC 5321 §4.5.3.1.8 asks for at least 100) and the
// tolerated number of negative replies are BOTH settings now — see
// `ServerConfig::max_recipients` and `max_protocol_errors`. Nothing in this
// file may re-introduce a compiled-in limit for them: a knob the admin panel
// shows and the server ignores is worse than no knob at all.

/// Where incoming attachments are written. Read once, from the same
/// configuration the rest of the module uses; the listener has no `Settings` of
/// its own because the supervisor only hands it the database and the server
/// configuration.
fn attachments_dir() -> &'static str {
    static DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        crate::config::settings::Settings::load()
            .map(|s| s.mail.attachments_dir)
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "Répertoire des pièces jointes illisible — repli sur le défaut");
                "/var/lib/kubuno/mail/attachments".to_string()
            })
    })
}

/// Entry point called by the supervisor for one accepted connection. `stream`
/// is plaintext OR already TLS (implicit-TLS ports); everything the handler
/// needs is carried by `inc`.
pub async fn handle(inc: Incoming, stream: MailStream) -> Result<()> {
    // A connection on an implicit-TLS port (465) arrives already encrypted; a
    // STARTTLS port starts in the clear and may be upgraded later.
    let encrypted = stream.is_tls();
    let peer_ip = inc.peer_ip();
    let mut session = Session {
        reader:     Some(BufReader::new(stream)),
        tls:        encrypted,
        submission: inc.submission,
        tarpit:     inc.tarpit.clone(),
        peer_ip,
        // Whether this connection is a declared internal relay. Computed once:
        // it drives the SPF-from-Received path and the greylist / block-list /
        // auth-penalty exemptions, and the configuration is fixed for the
        // session's whole life.
        trusted_upstream: inc.cfg.is_trusted_upstream(peer_ip),
        greeting: None,
        mailbox:  None,
        sender:   None,
        rcpts:    Vec::new(),
        remote_rcpts: Vec::new(),
        named_rcpts: 0,
        commands: 0,
        errors:   0,
    };

    let outcome = session.run(&inc).await;

    let error = match &outcome {
        Ok(Some(fault)) => Some(fault.clone()),
        Ok(None) => None,
        Err(e) => Some(e.to_string()),
    };
    log_session(&inc.db, "smtp", &inc.peer, session.mailbox.as_ref(), session.commands, error).await;

    outcome.map(|_| ())
}

// ── Session ──────────────────────────────────────────────────────────────────

struct Session {
    /// The live connection, read AND written through the SAME buffered reader
    /// (`BufReader` delegates `AsyncWrite` to its inner stream). It is an
    /// `Option` only so a STARTTLS upgrade can move the plaintext stream out and
    /// swap the encrypted one back in; it is `Some` at every observable point.
    reader: Option<BufReader<MailStream>>,
    /// Whether the connection is currently encrypted (implicit TLS, or after a
    /// successful STARTTLS). Gates the STARTTLS advertisement and a second upgrade.
    tls: bool,
    /// Shared per-IP auth tarpit and this client's IP, used to slow repeated
    /// authentication failures (Dovecot auth-penalty).
    tarpit: Arc<Tarpit>,
    peer_ip: IpAddr,
    /// True when `peer_ip` is a declared trusted upstream relay (`mynetworks`).
    /// A relay forwards a large volume of legitimate mail and its IP is not the
    /// sender's, so it is exempt from greylisting, the envelope block lists and
    /// the auth penalty, and its SPF is judged on the real client IP instead.
    trusted_upstream: bool,
    /// The listener's role (RFC 6409). `true` on a submission port (587/465):
    /// authentication is MANDATORY before any envelope command, cleartext AUTH is
    /// refused, the envelope sender must belong to the authenticated user, and the
    /// MSA fills in a missing Message-ID/Date. `false` on a reception port (25):
    /// an MX, which authenticates no one and rewrites nothing.
    submission: bool,
    /// The name given in HELO/EHLO — a session cannot start without one.
    greeting: Option<String>,
    /// Set once AUTH succeeded. This, and only this, unlocks relaying.
    mailbox: Option<Mailbox>,
    /// Envelope in progress.
    sender: Option<String>,
    /// Local deliveries this envelope has accumulated, already resolved and
    /// expanded (`server::resolve`): one RCPT may add several, because an alias
    /// or a list is one address the client names and many mailboxes we file
    /// into. Filed at end of DATA.
    rcpts:  Vec<LocalDelivery>,
    /// Remote addresses this envelope must forward to (queued for outbound
    /// delivery at end of DATA): a remote destination an authenticated
    /// submitter named, or one an alias of ours expands to.
    remote_rcpts: Vec<String>,
    /// How many recipients the CLIENT named, which is what `max_recipients`
    /// bounds. Counting the expanded addresses instead would refuse a legitimate
    /// second recipient because the first one happened to be a large list — and
    /// the expansion has limits of its own (`resolve::MAX_FANOUT`).
    named_rcpts: usize,
    commands: i32,
    errors:   u32,
}

/// What one read produced.
enum Line {
    /// `buf` holds the line, CRLF stripped.
    Read,
    /// The peer closed the connection.
    Eof,
    /// Longer than the caller allowed; the rest of it has been discarded.
    TooLong,
    /// Terminated by a bare LF (or containing a bare CR) instead of CRLF. The
    /// line has been discarded; the session must be torn down, never resumed —
    /// resuming is exactly what lets an attacker desynchronise us from a relay.
    BareNewline,
}

/// Outcome of processing a STARTTLS command.
enum Tls {
    /// Stay in the session — either the upgrade succeeded, or it was declined
    /// (already active / not offered) without tearing the connection down.
    KeepGoing,
    /// The connection must be closed; the string is the fault to log.
    Close(String),
}

impl Session {
    /// Runs the session. `Ok(Some(fault))` records something worth logging
    /// against the session without treating it as an I/O failure.
    async fn run(&mut self, inc: &Incoming) -> Result<Option<String>> {
        let db = &inc.db;
        let cfg: &ServerConfig = &inc.cfg;
        let peer = inc.peer.as_str();

        self.reply(&format!("220 {} Kubuno SMTP ready", sanitize_token(&cfg.hostname)))
            .await?;

        let mut buf: Vec<u8> = Vec::with_capacity(256);
        loop {
            match self.read_line(&mut buf, MAX_COMMAND_BYTES).await? {
                Line::Eof => return Ok(Some("Connexion fermée par le client".to_string())),
                Line::BareNewline => {
                    self.reply("500 5.5.2 Bare LF or CR not allowed").await?;
                    return Ok(Some("Fin de ligne non conforme (LF nu)".to_string()));
                }
                Line::TooLong => {
                    self.reply("500 5.5.2 Line too long").await?;
                    if self.too_many_errors(cfg) {
                        self.reply("421 4.7.0 Too many errors, closing connection").await?;
                        return Ok(Some("Trop d'erreurs de protocole".to_string()));
                    }
                    continue;
                }
                Line::Read => {}
            }

            // A command line is ASCII by definition; anything else is a client
            // error, not something to guess at.
            let line = String::from_utf8_lossy(&buf).to_string();
            let (verb, rest) = split_command(&line);
            if verb.is_empty() {
                self.reply("500 5.5.2 Command not recognised").await?;
                continue;
            }
            self.commands = self.commands.saturating_add(1);

            match verb.as_str() {
                "QUIT" => {
                    self.reply(&format!("221 2.0.0 {} closing connection", sanitize_token(&cfg.hostname)))
                        .await?;
                    return Ok(None);
                }
                // A HELO/EHLO the operator asked us to screen is refused here,
                // before the session has a greeting — so MAIL still answers
                // "send HELO first" rather than proceeding on a name we
                // rejected.
                "HELO" | "EHLO" if helo_rejected(cfg, self.submission, rest) => {
                    tracing::warn!(
                        peer = %peer, helo = %sanitize_token(rest),
                        "SMTP : HELO/EHLO refusé (nom non pleinement qualifié)"
                    );
                    self.reply("550 5.7.1 Helo command rejected: need fully-qualified hostname")
                        .await?;
                }
                "HELO" => {
                    self.greeting = Some(sanitize_token(rest));
                    self.reset_envelope();
                    self.reply(&format!("250 {}", sanitize_token(&cfg.hostname))).await?;
                }
                "EHLO" => {
                    self.greeting = Some(sanitize_token(rest));
                    self.reset_envelope();
                    let host = sanitize_token(&cfg.hostname);
                    self.reply(&format!("250-{host}")).await?;
                    self.reply(&format!("250-SIZE {}", cfg.max_message_bytes)).await?;
                    self.reply("250-8BITMIME").await?;
                    // Only offered on a plaintext listener that can actually
                    // upgrade — never advertise an encryption we cannot honour.
                    if advertises_starttls(inc.tls_mode, inc.acceptor.is_some(), self.tls) {
                        self.reply("250-STARTTLS").await?;
                    }
                    // AUTH is only offered where it may actually be used: a
                    // submission listener, and only once the channel is encrypted
                    // (RFC 6409 §4.3, RFC 4954). A reception MX advertises none.
                    if advertises_auth(self.submission, self.tls) {
                        self.reply(&format!("250-AUTH {}", auth_mechanisms())).await?;
                    }
                    self.reply("250 HELP").await?;
                }
                "STARTTLS" => match self.cmd_starttls(inc.acceptor.as_ref(), peer).await? {
                    Tls::KeepGoing => {}
                    Tls::Close(reason) => return Ok(Some(reason)),
                },
                "AUTH" => self.cmd_auth(db, rest).await?,
                // ── Submission checkpoint (RFC 6409 §4.3) ───────────────────
                // On a submission listener EVERY envelope command is refused
                // until the session has authenticated. This is the single place
                // that rule is enforced: MAIL/RCPT/DATA (and a hypothetical BDAT)
                // cannot be reached without passing it, so no handler can forget
                // it. On a reception listener the guard is inert.
                "MAIL" | "RCPT" | "DATA" | "BDAT"
                    if envelope_blocked_pending_auth(self.submission, self.mailbox.is_some()) =>
                {
                    self.reply("530 5.7.0 Authentication required").await?;
                }
                "MAIL" => self.cmd_mail(db, cfg, rest).await?,
                "RCPT" => self.cmd_rcpt(db, cfg, rest).await?,
                "DATA" => {
                    if let Some(fault) = self.cmd_data(db, cfg, peer).await? {
                        return Ok(Some(fault));
                    }
                }
                "RSET" => {
                    self.reset_envelope();
                    self.reply("250 2.0.0 Ok").await?;
                }
                "NOOP" => self.reply("250 2.0.0 Ok").await?,
                // VRFY is answered without ever confirming or denying an
                // address: address enumeration is exactly what it was abused
                // for, and RFC 5321 §3.5.3 blesses this answer.
                "VRFY" => {
                    self.reply("252 2.5.2 Cannot VRFY user, but will accept message and attempt delivery")
                        .await?
                }
                "EXPN" => self.reply("502 5.5.1 EXPN not supported").await?,
                "HELP" => {
                    self.reply("214 2.0.0 Commands: HELO EHLO AUTH MAIL RCPT DATA RSET NOOP VRFY QUIT")
                        .await?
                }
                _ => self.reply("500 5.5.2 Command not recognised").await?,
            }

            // Checked once, after any command: `reply` counted whatever the
            // handler refused, so this bounds probing across every verb —
            // including the RCPT and AUTH loops an attacker actually uses.
            if self.too_many_errors(cfg) {
                self.reply("421 4.7.0 Too many errors, closing connection").await?;
                return Ok(Some("Trop d'erreurs de protocole".to_string()));
            }
        }
    }

    // ── Commands ─────────────────────────────────────────────────────────────

    /// Handles the STARTTLS command (RFC 3207). The caller has already matched
    /// the verb; `acceptor` is present only when an upgrade is actually possible.
    async fn cmd_starttls(&mut self, acceptor: Option<&Arc<TlsAcceptor>>, peer: &str) -> Result<Tls> {
        if self.tls {
            self.reply("454 4.7.0 TLS already active").await?;
            return Ok(Tls::KeepGoing);
        }
        let Some(acceptor) = acceptor else {
            self.reply("502 5.5.1 STARTTLS not available").await?;
            return Ok(Tls::KeepGoing);
        };

        // The "ready" reply MUST be written and flushed before the handshake
        // begins; `reply` flushes. `tls::upgrade` then enforces the
        // anti-injection rule and performs the handshake.
        self.reply("220 2.0.0 Ready to start TLS").await?;

        let reader = match self.reader.take() {
            Some(reader) => reader,
            None => return Ok(Tls::Close("STARTTLS: flux réseau indisponible".to_string())),
        };
        match crate::server::tls::upgrade(reader, acceptor).await {
            Ok(reader) => {
                self.reader = Some(reader);
                self.tls = true;
                // A successful STARTTLS discards EVERYTHING negotiated in the
                // clear (RFC 3207 §4.2): the client must re-issue EHLO inside the
                // tunnel before anything else is accepted.
                self.greeting = None;
                self.mailbox = None;
                self.reset_envelope();
                Ok(Tls::KeepGoing)
            }
            Err(UpgradeError::Pipelined) => {
                tracing::warn!(peer = %peer, "STARTTLS: données pipelinées en clair — connexion fermée");
                Ok(Tls::Close("STARTTLS: données pipelinées en clair".to_string()))
            }
            Err(UpgradeError::Handshake(e)) => {
                tracing::debug!(peer = %peer, error = %e, "STARTTLS: handshake TLS échoué");
                Ok(Tls::Close("STARTTLS: handshake TLS échoué".to_string()))
            }
        }
    }

    async fn cmd_auth(&mut self, db: &PgPool, rest: &str) -> Result<()> {
        if self.greeting.is_none() {
            return self.reply("503 5.5.1 Send HELO/EHLO first").await;
        }
        if self.mailbox.is_some() {
            return self.reply("503 5.5.1 Already authenticated").await;
        }
        // On submission, credentials MUST NOT travel in the clear (RFC 6409 §4.3,
        // RFC 4954 §4): the client is told to STARTTLS first rather than have its
        // password observed. A reception listener never needs this — it does not
        // advertise AUTH — so the rule is scoped to submission.
        if self.submission && !self.tls {
            return self
                .reply("538 5.7.11 Encryption required for requested authentication mechanism")
                .await;
        }

        let mut parts = rest.split_whitespace();
        let mechanism = parts.next().unwrap_or("").to_ascii_uppercase();
        let initial = parts.next().unwrap_or("");

        // SCRAM-SHA-256 is a multi-step challenge/response, not a single decoded
        // credential like PLAIN/LOGIN, so it runs its own exchange and returns.
        if mechanism == "SCRAM-SHA-256" {
            return self.cmd_auth_scram(db, initial).await;
        }

        // Credentials never reach the logs, at any level: what is decoded here
        // is a password in clear.
        let credentials = match mechanism.as_str() {
            "PLAIN" => {
                let payload = if initial.is_empty() {
                    self.reply("334 ").await?;
                    match self.read_secret_line().await? {
                        Some(value) => value,
                        None => return self.reply("501 5.7.0 Authentication aborted").await,
                    }
                } else {
                    initial.to_string()
                };
                decode_auth_plain(&payload)
            }
            "LOGIN" => {
                let user = if initial.is_empty() {
                    // "Username:" — the challenge clients expect verbatim.
                    self.reply("334 VXNlcm5hbWU6").await?;
                    match self.read_secret_line().await? {
                        Some(value) => value,
                        None => return self.reply("501 5.7.0 Authentication aborted").await,
                    }
                } else {
                    initial.to_string()
                };
                // "Password:"
                self.reply("334 UGFzc3dvcmQ6").await?;
                let pass = match self.read_secret_line().await? {
                    Some(value) => value,
                    None => return self.reply("501 5.7.0 Authentication aborted").await,
                };
                match (decode_base64_string(&user), decode_base64_string(&pass)) {
                    (Some(u), Some(p)) => Some((u, p)),
                    _ => None,
                }
            }
            _ => return self.reply("504 5.5.4 Authentication mechanism not supported").await,
        };

        let Some((username, password)) = credentials else {
            return self.reply("501 5.5.2 Cannot decode authentication").await;
        };
        if username.is_empty() || password.is_empty() {
            return self.reply("535 5.7.8 Authentication credentials invalid").await;
        }

        match auth::authenticate(db, &username, &password).await {
            Some(mailbox) => {
                tracing::info!(username = %mailbox.username, "SMTP : authentification réussie");
                self.tarpit.record_success(self.peer_ip);
                self.mailbox = Some(mailbox);
                // Authenticating restarts the mail transaction (RFC 4954 §4).
                self.reset_envelope();
                self.reply("235 2.7.0 Authentication successful").await
            }
            None => {
                // No hint about which half was wrong, and no password logged.
                // Hold the failure reply back by the growing tarpit delay, so
                // brute force is throttled per IP (Dovecot auth-penalty).
                tracing::warn!(username = %username, "SMTP : authentification refusée");
                // A trusted upstream relay is never tarpitted: it is not a
                // password-guessing client, and holding its replies back would
                // throttle legitimate forwarding.
                if !self.trusted_upstream {
                    self.tarpit.record_failure(self.peer_ip);
                    tokio::time::sleep(self.tarpit.delay_for(self.peer_ip)).await;
                }
                self.reply("535 5.7.8 Authentication credentials invalid").await
            }
        }
    }

    /// Drives one SASL SCRAM-SHA-256 exchange (RFC 4954 + RFC 5802). The password
    /// never crosses the wire: the client proves knowledge of it against the
    /// stored secret, and an unknown user is served a decoy so the failure is
    /// indistinguishable from a wrong password (no address-enumeration oracle).
    ///
    /// The base64-wrapped SASL data is never logged: even the intermediate
    /// messages carry material an attacker could use.
    async fn cmd_auth_scram(&mut self, db: &PgPool, initial: &str) -> Result<()> {
        // client-first: either the SASL initial response, or read after an empty
        // `334` challenge (RFC 4954 §4). `*` / EOF cancels the exchange.
        let client_first_b64 = if initial.is_empty() {
            self.reply("334 ").await?;
            match self.read_secret_line().await? {
                Some(value) => value,
                None => return self.reply("501 5.7.0 Authentication cancelled").await,
            }
        } else {
            initial.to_string()
        };
        let client_first = match decode_base64_string(&client_first_b64) {
            Some(value) => value,
            None => return self.reply("501 5.5.2 Cannot decode authentication").await,
        };

        // The username names which secret to prove against; a malformed
        // client-first is a client error, not an auth failure.
        let username = match scram::Handshake::username_of(&client_first) {
            Ok(value) => value,
            Err(_) => return self.reply("501 5.5.2 Cannot decode authentication").await,
        };

        // Real secret for a known user, decoy otherwise — the exchange proceeds
        // identically and fails at the proof either way.
        let (secret, known) = match auth::scram_secret(db, &username).await {
            Some(secret) => (secret, true),
            None => (scram::decoy_secret(), false),
        };

        let mut handshake = match scram::Handshake::new(secret, known, &client_first) {
            Ok(handshake) => handshake,
            Err(_) => return self.reply("501 5.5.2 Cannot decode authentication").await,
        };
        let server_first = match handshake.server_first() {
            Ok(message) => message,
            Err(_) => return self.reply("501 5.5.2 Cannot decode authentication").await,
        };
        self.reply(&format!("334 {}", encode_base64_string(&server_first))).await?;

        // client-final carries the proof; `*` / EOF cancels.
        let client_final_b64 = match self.read_secret_line().await? {
            Some(value) => value,
            None => return self.reply("501 5.7.0 Authentication cancelled").await,
        };
        let client_final = match decode_base64_string(&client_final_b64) {
            Some(value) => value,
            None => return self.reply("501 5.5.2 Cannot decode authentication").await,
        };

        match handshake.server_final(&client_final) {
            Ok(server_final) => {
                // The proof verified. Send our server-signature so the client can
                // authenticate us in turn, then read its (empty) acknowledgement.
                self.reply(&format!("334 {}", encode_base64_string(&server_final))).await?;
                let _ = self.read_secret_line().await?;

                // The secret came from a real row (known == true), so the mailbox
                // exists; fetch the user_id it belongs to.
                let user_id = match sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT user_id FROM mail.mailbox_credentials WHERE username = $1",
                )
                .bind(username.trim().to_ascii_lowercase())
                .fetch_optional(db)
                .await
                {
                    Ok(Some(id)) => id,
                    Ok(None) => {
                        // The credential vanished between the two lookups: refuse
                        // temporarily rather than authenticate a phantom mailbox.
                        tracing::error!(username = %username, "SMTP : identifiant SCRAM introuvable après vérification");
                        return self
                            .reply("454 4.7.0 Temporary authentication failure")
                            .await;
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Lecture de l'identifiant de boîte après SCRAM");
                        return self
                            .reply("454 4.7.0 Temporary authentication failure")
                            .await;
                    }
                };

                tracing::info!(username = %username, "SMTP : authentification SCRAM réussie");
                self.tarpit.record_success(self.peer_ip);
                self.mailbox = Some(Mailbox { user_id, username });
                // Authenticating restarts the mail transaction (RFC 4954 §4).
                self.reset_envelope();
                self.reply("235 2.7.0 Authentication successful").await
            }
            Err(_) => {
                // Wrong password OR unknown user: same reply, no password logged,
                // held back by the growing per-IP tarpit delay (Dovecot penalty).
                tracing::warn!(username = %username, "SMTP : authentification SCRAM refusée");
                // A trusted upstream relay is never tarpitted (see cmd_auth).
                if !self.trusted_upstream {
                    self.tarpit.record_failure(self.peer_ip);
                    tokio::time::sleep(self.tarpit.delay_for(self.peer_ip)).await;
                }
                self.reply("535 5.7.8 Authentication credentials invalid").await
            }
        }
    }

    async fn cmd_mail(&mut self, db: &PgPool, cfg: &ServerConfig, rest: &str) -> Result<()> {
        if self.greeting.is_none() {
            return self.reply("503 5.5.1 Send HELO/EHLO first").await;
        }
        if self.sender.is_some() {
            return self.reply("503 5.5.1 Sender already specified").await;
        }

        let envelope = match parse_mail_from(rest) {
            Ok(value) => value,
            Err(message) => return self.reply(message).await,
        };
        // The announced size is checked BEFORE the message is transferred; the
        // real count is enforced again during DATA.
        if let Some(size) = envelope.size {
            if size > cfg.max_message_bytes {
                return self.reply("552 5.3.4 Message too big for system").await;
            }
        }

        // Instance-wide block list, the operator's own `check_sender_access`.
        // It only ever applies to FOREIGN mail: a session that authenticated is
        // one of our users, and the address they may use is settled by the
        // ownership check below, not by a spam list. The allow-list wins, which
        // is its documented purpose — it buys a bypass of the block lists and
        // of greylisting, and of nothing else. A trusted upstream relay is
        // exempt too: it has already filtered, and it must not be treated as a
        // hostile client.
        if !self.submission && self.mailbox.is_none() && !self.trusted_upstream {
            let sender = envelope.address.as_str();
            if !sender.is_empty() && !cfg.is_allowlisted(sender) && cfg.is_blocklisted(sender) {
                tracing::warn!(
                    sender = %sanitize_token(sender),
                    "SMTP : expéditeur d'enveloppe refusé (liste de blocage de l'instance)"
                );
                return self.reply("550 5.7.1 Sender address rejected by policy").await;
            }

            // Delivery restriction: when the operator has declared the only
            // domains this instance exchanges mail with, anything else is
            // refused here — before a byte of the message is transferred, and
            // permanently, so the sending server tells its user why. Our own
            // domains and the null return path are never restricted (see
            // `inbound_sender_allowed`).
            if !cfg.inbound_sender_allowed(sender) {
                tracing::warn!(
                    sender = %sanitize_token(sender),
                    "SMTP : expéditeur hors des domaines autorisés (restriction de remise)"
                );
                return self
                    .reply("550 5.7.1 Sender domain not authorised by this instance's delivery restriction")
                    .await;
            }
        }

        // Anti-spoofing (RFC 6409): an authenticated user may only send AS
        // themselves. Kubuno has no downstream MTA to enforce this, so it is done
        // here — and ONLY on submission. Authentication is already guaranteed by
        // the dispatch-loop checkpoint; the `else` is defence in depth.
        if self.submission {
            match self.mailbox.clone() {
                Some(mailbox) => match verify_sender_ownership(db, &envelope.address, &mailbox).await {
                    SenderCheck::Accept => {}
                    SenderCheck::Reject(reply) => return self.reply(reply).await,
                },
                None => return self.reply("530 5.7.0 Authentication required").await,
            }
        }

        self.sender = Some(envelope.address);
        // BOTH recipient lists start empty: leaving `remote_rcpts` behind would
        // attach the previous transaction's remote addresses to this one.
        self.rcpts.clear();
        self.remote_rcpts.clear();
        self.named_rcpts = 0;
        self.reply("250 2.1.0 Sender ok").await
    }

    async fn cmd_rcpt(&mut self, db: &PgPool, cfg: &ServerConfig, rest: &str) -> Result<()> {
        if self.sender.is_none() {
            return self.reply("503 5.5.1 Need MAIL before RCPT").await;
        }
        // What the CLIENT named, local and remote alike — the case this limit
        // exists for is an authenticated submitter fanning one message out to
        // thousands of remote addresses, and counting only the local list left
        // exactly that unbounded.
        if self.named_rcpts >= cfg.max_recipients {
            return self.reply("452 4.5.3 Too many recipients").await;
        }
        // …and what those names EXPAND to, which the client does not choose.
        // Each recipient is bounded on its own by `resolve::MAX_FANOUT`; this
        // is the same ceiling applied to the whole envelope, so a handful of
        // large lists cannot multiply into a delivery run nobody asked for.
        if self.rcpts.len() + self.remote_rcpts.len() >= resolve::MAX_FANOUT {
            tracing::warn!(
                expanded = self.rcpts.len() + self.remote_rcpts.len(),
                limit = resolve::MAX_FANOUT,
                "SMTP : enveloppe trop développée — destinataire supplémentaire refusé"
            );
            return self.reply("452 4.5.3 Too many recipients").await;
        }

        let rest = match strip_keyword(rest, "TO:") {
            Some(value) => value,
            None => return self.reply("501 5.5.4 Syntax: RCPT TO:<address>").await,
        };
        let recipient = match extract_path(rest) {
            Some(value) if is_valid_address(&value) => value.to_ascii_lowercase(),
            // A null path is a valid SENDER, never a valid recipient.
            _ => return self.reply("501 5.1.3 Bad recipient address syntax").await,
        };

        // ── The relay decision. Everything above is parsing. ────────────────
        let sender = self.sender.clone().unwrap_or_default();
        match relay_decision(cfg, self.mailbox.is_some(), &recipient) {
            RelayDecision::Local => {
                // One address the client named; possibly many mailboxes and
                // forwards once aliases and lists are expanded. The expansion
                // happens HERE, at RCPT, because a recipient we cannot serve
                // must be refused while the sender can still be told which one.
                let directory = resolve::PgDirectory::new(db);
                let outcome = resolve::resolve(
                    &directory, cfg, &sender, self.mailbox.is_some(), &recipient,
                )
                .await;

                let expansion = match outcome {
                    Ok(Outcome::Accept(expansion)) => expansion,
                    Ok(Outcome::Refuse(refusal)) => return self.reply(refusal.reply()).await,
                    Err(e) => {
                        // A database problem is OURS: answering 550 would make a
                        // legitimate sender bounce the message for good.
                        tracing::error!(error = %e, recipient = %recipient, "Vérification du destinataire impossible");
                        return self
                            .reply("451 4.3.0 Recipient verification temporarily unavailable")
                            .await;
                    }
                };

                // Greylisting (RFC 6647), last and only on a real recipient of
                // ours: there is no point remembering a triplet aimed at a
                // mailbox that does not exist, and deferring one would hide the
                // 550 the sender is owed.
                if greylist::applies(
                    cfg.greylisting_enabled,
                    self.submission,
                    self.mailbox.is_some(),
                    cfg.is_allowlisted(&sender),
                    self.trusted_upstream,
                ) && greylist::check(db, cfg, self.peer_ip, &sender, &recipient).await
                    == greylist::Verdict::Defer
                {
                    // 4xx on purpose: the message is not refused, it is
                    // postponed. A real MTA queues it and comes back.
                    return self.reply("450 4.7.1 Greylisted, please retry").await;
                }

                if expansion.len() > 1 {
                    tracing::info!(
                        recipient = %recipient, local = expansion.local.len(),
                        remote = expansion.remote.len(),
                        "SMTP : destinataire développé (alias ou liste)"
                    );
                }
                self.named_rcpts += 1;
                // Deduplicated across the WHOLE envelope, not just within one
                // expansion: naming both a mailbox and an alias that leads to
                // it is a client's prerogative, and it must still deliver one
                // copy. Postfix's duplicate recipient elimination.
                for delivery in expansion.local {
                    if !self.rcpts.iter().any(|d| d.target == delivery.target) {
                        self.rcpts.push(delivery);
                    }
                }
                for address in expansion.remote {
                    // An alias of ours that forwards outside is an OUTGOING hop,
                    // and the delivery restriction governs it. The local
                    // deliveries of the same expansion still happen: the rule
                    // stops mail from leaving, it does not make an address
                    // undeliverable.
                    if !cfg.outbound_recipient_allowed(&address) {
                        tracing::warn!(
                            forward_to = %sanitize_token(&address),
                            "SMTP : réexpédition d'alias hors des domaines autorisés — abandonnée"
                        );
                        continue;
                    }
                    if !self.remote_rcpts.contains(&address) {
                        self.remote_rcpts.push(address);
                    }
                }
                self.reply("250 2.1.5 Recipient ok").await
            }
            RelayDecision::Denied => {
                tracing::warn!(
                    recipient = %recipient, sender = %sender,
                    "SMTP : relais refusé (client non authentifié)"
                );
                self.reply("550 5.7.1 Relay access denied").await
            }
            RelayDecision::ForwardUnsupported => {
                // An authenticated submitter sending to a remote domain: accept
                // the recipient and queue it for outbound delivery at end of DATA
                // (the worker delivers with retries + DSN once outbound is on).
                //
                // …unless the operator restricted whom this instance may write
                // to. Refusing at RCPT is what lets the sender's own client show
                // which address was refused, instead of a bounce arriving later.
                if !cfg.outbound_recipient_allowed(&recipient) {
                    tracing::warn!(
                        recipient = %sanitize_token(&recipient),
                        "SMTP : destinataire hors des domaines autorisés (restriction de remise)"
                    );
                    return self
                        .reply("550 5.7.1 Recipient domain not authorised by this instance's delivery restriction")
                        .await;
                }
                self.named_rcpts += 1;
                if !self.remote_rcpts.contains(&recipient) {
                    self.remote_rcpts.push(recipient);
                }
                self.reply("250 2.1.5 Recipient ok").await
            }
        }
    }

    /// Reads the message and delivers it. Returns `Some(fault)` when the session
    /// must end.
    async fn cmd_data(
        &mut self,
        db: &PgPool,
        cfg: &ServerConfig,
        peer: &str,
    ) -> Result<Option<String>> {
        if self.sender.is_none() {
            self.reply("503 5.5.1 Need MAIL before DATA").await?;
            return Ok(None);
        }
        if self.rcpts.is_empty() && self.remote_rcpts.is_empty() {
            self.reply("503 5.5.1 Need RCPT before DATA").await?;
            return Ok(None);
        }

        self.reply("354 End data with <CR><LF>.<CR><LF>").await?;

        let mut message: Vec<u8> = Vec::with_capacity(8192);
        let mut buf: Vec<u8> = Vec::with_capacity(1024);
        let mut too_big = false;
        let mut damaged = false;
        // Once the message is over the limit we stop keeping it but must keep
        // reading to find the terminator — up to a bound, past which the peer
        // is simply flooding us.
        let drain_limit = cfg.max_message_bytes.saturating_add(1024 * 1024);
        let mut drained: usize = 0;

        loop {
            match self.read_line(&mut buf, MAX_DATA_LINE_BYTES).await? {
                Line::Eof => return Ok(Some("Connexion coupée pendant DATA".to_string())),
                Line::BareNewline => {
                    // Inside DATA this is the smuggling vector itself: the peer
                    // is trying to make us disagree with the next hop about
                    // where the message ends. Nothing is delivered.
                    self.reply("500 5.5.2 Bare LF or CR not allowed").await?;
                    return Ok(Some("Fin de ligne non conforme pendant DATA".to_string()));
                }
                Line::TooLong => {
                    // Refusing beats storing a truncated message.
                    damaged = true;
                    continue;
                }
                Line::Read => {}
            }

            if buf == b"." {
                break;
            }

            let content = unstuff_dot(&buf);
            drained = drained.saturating_add(content.len() + 2);
            if drained > drain_limit {
                self.reply("552 5.3.4 Message too big for system").await?;
                return Ok(Some("Message hors limite pendant DATA".to_string()));
            }

            if !too_big && !damaged {
                if message.len() + content.len() + 2 > cfg.max_message_bytes {
                    too_big = true;
                    message = Vec::new(); // release what was buffered
                } else {
                    message.extend_from_slice(content);
                    message.extend_from_slice(b"\r\n");
                }
            }
        }

        let rcpts = std::mem::take(&mut self.rcpts);
        // Mutable because a content-policy quarantine drops the outward hop: a
        // message we judge unfit for one of our own inboxes must not be
        // forwarded on to somebody else's.
        let mut remote_rcpts = std::mem::take(&mut self.remote_rcpts);
        let sender = self.sender.clone().unwrap_or_default();
        let user_id = self.mailbox.as_ref().map(|m| m.user_id);
        self.reset_envelope();

        if too_big {
            self.reply("552 5.3.4 Message too big for system").await?;
            return Ok(None);
        }
        if damaged {
            self.reply("500 5.5.2 Line too long, message refused").await?;
            return Ok(None);
        }
        if message.is_empty() {
            self.reply("554 5.6.0 Empty message refused").await?;
            return Ok(None);
        }
        // Loop guard, at the session level. `deliver_local` refuses a looping
        // message too, but that refusal arrives after we would have answered
        // 451 — and a 4xx tells the loop to try again, which is the one thing
        // it must not hear. A hop count is never transient: answer 5xx.
        //
        // The ceiling is the administrator's `hopcount_limit`, capped by the
        // compiled floor: the setting may only make the check STRICTER.
        // `deliver_local` keeps enforcing the floor on its own, and it must
        // never be the one to fire — its refusal turns into a 451.
        let limit = effective_hopcount_limit(cfg);
        let hops = hygiene::hop_count(&message);
        if hops >= limit {
            tracing::error!(hops, limit, peer = %peer,
                "SMTP : boucle de courrier détectée — message refusé définitivement");
            self.reply("554 5.4.6 Routing loop detected, too many Received headers").await?;
            return Ok(None);
        }

        // The sender's daily allowance, checked BEFORE anything is delivered or
        // queued. A refusal that came after the local copies were filed would
        // make the client retry a message it had already half-delivered, so the
        // one safe place for this check is here — nothing has happened yet.
        //
        // Temporary (4xx) on purpose: an allowance is a rolling window, so the
        // message becomes sendable again on its own. A 5xx would bounce for good
        // something that is merely early.
        if cfg.send_max_recipients_per_day > 0 && !remote_rcpts.is_empty() {
            if let Some(sender_id) = user_id {
                match queue::recipients_queued_last_24h(db, sender_id).await {
                    Ok(used) if used.saturating_add(remote_rcpts.len() as i64)
                        > cfg.send_max_recipients_per_day =>
                    {
                        tracing::warn!(
                            used, asked = remote_rcpts.len(), limit = cfg.send_max_recipients_per_day,
                            "SMTP : quota d'envoi quotidien atteint — message différé"
                        );
                        self.reply("452 4.5.3 Daily sending quota exceeded, try again later").await?;
                        return Ok(None);
                    }
                    Ok(_) => {}
                    // The count is a protection, not a gate: a database hiccup
                    // must not stop legitimate mail. It is logged and the send
                    // proceeds.
                    Err(e) => tracing::error!(error = %e, "Quota d'envoi : comptage impossible — envoi autorisé"),
                }
            }
        }

        // A submission client (an end-user's mail app) may legitimately leave out
        // its Message-ID and Date; the MSA supplies them (RFC 6409 §8.1). A
        // reception MX must NOT rewrite a message it merely relays, so this is
        // scoped to submission and applied once, before any trace header.
        // Where the authentication policy says this message belongs. Decided
        // before storing, because the classifier downstream must not get a
        // second opinion on a message DMARC already condemned.
        let mut disposition = deliver::Disposition::Inbox;
        // Kept beyond the block below so the stored message can carry it: the
        // reader needs it to decide whether a brand logo may be shown.
        let mut auth_dmarc: Option<String> = None;
        let message = if self.submission {
            add_submission_headers(&message, &cfg.hostname)
        } else {
            // Reception (MX): authenticate the sender with SPF/DKIM/DMARC and
            // stamp the result. Any Authentication-Results already in the message
            // is stripped first — otherwise a spammer forges `dmarc=pass`.
            let helo = self.greeting.clone().unwrap_or_default();
            let verdict = authres::verify(
                self.peer_ip, &cfg.trusted_upstreams, &helo, &sender, &message, &cfg.hostname,
            )
            .await;
            auth_dmarc = verdict.dmarc.clone();
            let stamped = stamp_auth_results(&message, &verdict.header);

            // …and ACT on it. Computing a verdict and throwing it away is the
            // worst of both worlds: the administrator sees an anti-spoofing
            // policy in the panel and every forgery is delivered anyway.
            //
            // ⚠️ Only on reception. An authenticated submitter is never judged
            // by these policies — their own domain's SPF record does not list
            // our IP, so every user of the instance would be rejected.
            let policy = published_dmarc_policy(cfg, &verdict, &stamped).await;
            match message_policy_action(cfg, &verdict, policy) {
                PolicyAction::Reject => {
                    // Refused at SMTP time, never accepted-then-bounced: the
                    // return path of a forged message belongs to the victim,
                    // and a bounce sent there is backscatter. The sending
                    // server now owns the notification.
                    tracing::warn!(
                        peer = %peer, sender = %sanitize_token(&sender),
                        spf = ?verdict.spf, dkim = ?verdict.dkim, dmarc = ?verdict.dmarc,
                        "SMTP : message refusé par la politique d'authentification"
                    );
                    self.reply("550 5.7.1 Message rejected by sender authentication policy")
                        .await?;
                    return Ok(None);
                }
                action @ (PolicyAction::Mark | PolicyAction::Quarantine) => {
                    if action == PolicyAction::Quarantine {
                        disposition = deliver::Disposition::Spam;
                    }
                    tracing::warn!(
                        peer = %peer, sender = %sanitize_token(&sender),
                        spf = ?verdict.spf, dkim = ?verdict.dkim, dmarc = ?verdict.dmarc,
                        quarantined = action == PolicyAction::Quarantine,
                        "SMTP : message marqué par la politique d'authentification"
                    );
                    policy_marker(&stamped, action, &verdict)
                }
                PolicyAction::Ignore => stamped,
            }
        };

        // Attachment and content compliance, in BOTH directions. These rules are
        // the operator's, not the sending domain's, so an authenticated
        // submitter is judged by them exactly like a foreign MX — that is the
        // whole point of a rule that forbids a file type: it must stop the file
        // coming in *and* going out.
        //
        // The reason is logged, never put in the reply: it can quote a filename,
        // and an SMTP reply line is neither the place for attacker-chosen text
        // nor for the non-ASCII a filename may carry.
        // Set when a quarantine emptied the outward hop, so a message with no
        // local recipient left is refused permanently (550) rather than deferred
        // (451): the sender must stop retrying something we will never relay.
        let mut relay_quarantined = false;
        if let Some(verdict) = compliance::scan(cfg, &message) {
            let refuse = verdict.action == PolicyAction::Reject
                // On submission there is no Spam folder to quarantine into: the
                // message is the sender's own and it must simply not leave.
                || (verdict.action == PolicyAction::Quarantine && self.submission);
            if refuse {
                tracing::warn!(
                    peer = %peer, sender = %sanitize_token(&sender), reason = %verdict.reason,
                    "SMTP : message refusé par la conformité du contenu"
                );
                self.reply("550 5.7.0 Message rejected by the instance content policy").await?;
                return Ok(None);
            }
            if verdict.action == PolicyAction::Quarantine {
                disposition = deliver::Disposition::Spam;
                if !remote_rcpts.is_empty() {
                    // An alias of ours that forwards outside would carry the
                    // message to a third party we have just judged unfit for our
                    // own inboxes. Quarantine means quarantine.
                    tracing::warn!(
                        dropped = remote_rcpts.len(),
                        "SMTP : réexpédition abandonnée — message mis en quarantaine par la conformité"
                    );
                    remote_rcpts.clear();
                    relay_quarantined = true;
                }
                tracing::warn!(
                    peer = %peer, sender = %sanitize_token(&sender), reason = %verdict.reason,
                    "SMTP : message classé indésirable par la conformité du contenu"
                );
            }
        }

        // Deliver to each LOCAL recipient (into our own store). These are the
        // ADDRESSES THE MESSAGE RESOLVED TO, not the ones the client named: an
        // alias or a list expanded at RCPT time, and each mailbox is filed
        // under the address that actually reached it, so the trace headers and
        // the logs name a real destination.
        let mut delivered = 0usize;
        let mut last_id = None;
        for delivery in &rcpts {
            let raw = with_trace_headers(
                &message, cfg, peer, self.greeting.as_deref(), &sender, &delivery.address,
            );
            match deliver::deliver_local(
                db, cfg, &sender, &delivery.address, delivery.target, &raw,
                attachments_dir(), disposition,
                // What the sending hop actually used, so the reader can see it.
                Some(if self.tls { "tls" } else { "none" }),
                auth_dmarc.as_deref(),
            )
            .await
            {
                Ok(id) => {
                    delivered += 1;
                    last_id = Some(id);
                }
                Err(e) => {
                    tracing::error!(error = %e, recipient = %delivery.address, "Dépôt local échoué");
                }
            }
        }

        // Queue REMOTE recipients for outbound delivery: the ones an
        // authenticated submitter named, and the ones an alias of OURS forwards
        // to. One message, all remote recipients, enqueued in a single
        // transaction — durable before we answer 250. The worker signs (DKIM)
        // and delivers with retries + DSN.
        //
        // A forwarded message keeps the sender's return path, which is what
        // makes forwarding correct (a failure is reported to whoever wrote the
        // message, not to the alias's owner) and what makes it dangerous: two
        // instances forwarding to each other would trade the same message for
        // ever, and every bounce along the way is backscatter aimed at a third
        // party. The hop-count check above is the guard, and it runs BEFORE
        // this block on purpose: each pass through a server adds a `Received:`
        // header — ours included, prepended just below — so a routing loop dies
        // at `effective_hopcount_limit` with a permanent 554 instead of being
        // queued again. Nothing here may be moved above that check.
        let mut queued = 0usize;
        if !remote_rcpts.is_empty() {
            let outbound_raw = hygiene::prepend_received(&message, peer, &cfg.hostname);
            let list: Vec<(String, String)> = remote_rcpts
                .iter()
                .map(|r| (r.clone(), r.rsplit_once('@').map(|(_, d)| d.to_string()).unwrap_or_default()))
                .collect();
            match queue::enqueue_with_lifetime(
                db, user_id, None, &sender, &outbound_raw, false, &list,
                cfg.outbound_lifetime_hours,
            )
            .await
            {
                Ok(id) => {
                    queued = list.len();
                    last_id = Some(id);
                }
                Err(e) => {
                    tracing::error!(error = %e, "Enfilement sortant échoué");
                }
            }
        }

        if delivered == 0 && queued == 0 {
            if relay_quarantined {
                // Nothing was stored and nothing will be relayed: the only
                // recipients were outward hops we deliberately dropped. A 4xx
                // here would have the sender retry a message we will never pass
                // on, for as long as its queue lifetime lasts.
                self.reply("550 5.7.0 Message rejected by the instance content policy").await?;
                return Ok(None);
            }
            // Temporary on purpose: a sender that retries is far better than a
            // message we have quietly lost.
            self.reply("451 4.3.0 Message could not be stored, try again later").await?;
            return Ok(Some("Aucun destinataire servi".to_string()));
        }

        // Journalling, once per accepted message and only once it IS accepted:
        // an archive of messages we refused would be a record of things that
        // never happened. Best-effort — it never changes the reply below.
        journal::archive(db, cfg, attachments_dir(), &sender, &message).await;

        let id = last_id.map(|id| id.to_string()).unwrap_or_default();
        tracing::info!(peer = %peer, sender = %sender, delivered, queued, "SMTP : message accepté");
        self.reply(&format!("250 2.0.0 Ok: queued as {id}")).await?;
        Ok(None)
    }

    // ── Plumbing ─────────────────────────────────────────────────────────────

    fn reset_envelope(&mut self) {
        self.sender = None;
        self.rcpts.clear();
        self.remote_rcpts.clear();
        self.named_rcpts = 0;
    }

    /// True once the client has had enough chances. The counter itself is fed by
    /// `reply`, so no handler can forget to declare its own rejection; the
    /// ceiling is the administrator's `max_protocol_errors`.
    fn too_many_errors(&self, cfg: &ServerConfig) -> bool {
        self.errors >= cfg.max_protocol_errors
    }

    /// The live buffered connection. Absent only for the instant a STARTTLS
    /// upgrade swaps the stream — a read or write outside that window means the
    /// session is being driven after it was torn down, and is a hard error.
    fn conn(&mut self) -> Result<&mut BufReader<MailStream>> {
        self.reader
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("flux réseau indisponible"))
    }

    async fn reply(&mut self, line: &str) -> Result<()> {
        // Every negative reply counts as a protocol error, the way Postfix's
        // error counter works. Counting HERE rather than at each rejection is
        // what makes the limit real: previously only two of the thirty-odd
        // rejection sites called the counter, so a client could be refused
        // indefinitely — a free address-enumeration and password-probing
        // channel. `421` is our own goodbye and never counts.
        if (line.starts_with('4') || line.starts_with('5')) && !line.starts_with("421") {
            self.errors = self.errors.saturating_add(1);
        }
        let conn = self.conn()?;
        conn.write_all(line.as_bytes()).await?;
        conn.write_all(b"\r\n").await?;
        conn.flush().await?;
        Ok(())
    }

    /// Reads one line of a SASL exchange. `None` when the client cancelled with
    /// `*` or hung up. The value is base64 and is never logged.
    async fn read_secret_line(&mut self) -> Result<Option<String>> {
        let mut buf: Vec<u8> = Vec::with_capacity(256);
        match self.read_line(&mut buf, MAX_COMMAND_BYTES).await? {
            Line::Read => {}
            _ => return Ok(None),
        }
        let value = String::from_utf8_lossy(&buf).trim().to_string();
        if value == "*" || value.is_empty() {
            return Ok(None);
        }
        Ok(Some(value))
    }

    /// Reads one CRLF-terminated line into `buf`, bounded by `limit`.
    ///
    /// Reads through the buffer rather than `read_until`, which would let a peer
    /// that never sends a newline grow our memory without bound. An overlong
    /// line is consumed to its end and reported as `TooLong`, so the next read
    /// starts on a real line boundary instead of on the tail of that one.
    async fn read_line(&mut self, buf: &mut Vec<u8>, limit: usize) -> Result<Line> {
        buf.clear();
        let mut overflow = false;

        loop {
            let conn = self.conn()?;
            let filled = match tokio::time::timeout(IDLE_TIMEOUT, conn.fill_buf()).await {
                Ok(result) => result?,
                Err(_) => {
                    // Say why before dropping the connection.
                    let _ = conn.write_all(b"421 4.4.2 Idle timeout, closing connection\r\n").await;
                    let _ = conn.flush().await;
                    anyhow::bail!("Session inactive plus de {}s", IDLE_TIMEOUT.as_secs());
                }
            };

            if filled.is_empty() {
                return Ok(if overflow {
                    Line::TooLong
                } else if buf.is_empty() {
                    Line::Eof
                } else {
                    Line::Read
                });
            }

            match filled.iter().position(|b| *b == b'\n') {
                Some(index) => {
                    if !overflow {
                        buf.extend_from_slice(&filled[..index]);
                    }
                    conn.consume(index + 1);
                    // The length is checked HERE too: a line that arrives whole
                    // in one read never went through the branch below, and
                    // would otherwise slip past the limit.
                    if overflow || buf.len() > limit {
                        buf.clear();
                        return Ok(Line::TooLong);
                    }
                    // A line MUST end with CRLF (RFC 5321 §2.3.8). Accepting a
                    // bare LF is what makes SMTP smuggling possible: a frontend
                    // and a backend that disagree on where the DATA terminator
                    // "\r\n.\r\n" is let an attacker append a second, forged
                    // message with a sender of their choosing. Postfix closed
                    // this with `smtpd_forbid_bare_newline`; we refuse outright.
                    if buf.last() != Some(&b'\r') {
                        buf.clear();
                        return Ok(Line::BareNewline);
                    }
                    buf.pop();
                    // A bare CR inside the line is equally ambiguous.
                    if buf.contains(&b'\r') {
                        buf.clear();
                        return Ok(Line::BareNewline);
                    }
                    return Ok(Line::Read);
                }
                None => {
                    let taken = filled.len();
                    if !overflow {
                        buf.extend_from_slice(filled);
                    }
                    conn.consume(taken);
                    if !overflow && buf.len() > limit {
                        overflow = true;
                        buf.clear();
                    }
                }
            }
        }
    }
}

// ── Relay policy ─────────────────────────────────────────────────────────────

/// What may be done with one recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayDecision {
    /// One of our domains: deliver into the local store.
    Local,
    /// A third-party domain named by a client that has not authenticated —
    /// the open relay case, always refused.
    Denied,
    /// A third-party domain named by one of our own users. Legitimate, but this
    /// server has no outbound relay, so it is refused rather than accepted and
    /// dropped.
    ForwardUnsupported,
}

/// The only place relaying is decided. Total over (authenticated, local).
fn relay_decision(cfg: &ServerConfig, authenticated: bool, recipient: &str) -> RelayDecision {
    if cfg.is_local_domain(recipient) {
        RelayDecision::Local
    } else if authenticated {
        RelayDecision::ForwardUnsupported
    } else {
        RelayDecision::Denied
    }
}

/// Whether the EHLO response should advertise STARTTLS: only on a plaintext
/// listener that can actually upgrade (a certificate is configured) and only
/// while the connection is still in the clear.
fn advertises_starttls(tls_mode: TlsMode, has_acceptor: bool, already_tls: bool) -> bool {
    tls_mode == TlsMode::StartTls && has_acceptor && !already_tls
}

// ── Submission policy (RFC 6409) ─────────────────────────────────────────────

/// Whether the EHLO response should advertise AUTH: only on a submission
/// listener, and only once the channel is encrypted — offering a mechanism whose
/// use we would then refuse (see the `538` in `cmd_auth`) only misleads clients.
fn advertises_auth(submission: bool, encrypted: bool) -> bool {
    submission && encrypted
}

/// The SASL mechanisms named in the EHLO `AUTH` line, in preference order.
/// SCRAM-SHA-256 comes first: it is the strongest — the password never crosses
/// the wire and a stolen stored secret does not reveal it (RFC 5802) — so a
/// capable client picks it over the cleartext-carrying PLAIN/LOGIN.
fn auth_mechanisms() -> &'static str {
    "SCRAM-SHA-256 PLAIN LOGIN"
}

/// The one predicate behind the submission checkpoint: on a submission listener,
/// an unauthenticated session may issue no envelope command. Inert on reception.
fn envelope_blocked_pending_auth(submission: bool, authenticated: bool) -> bool {
    submission && !authenticated
}

/// Outcome of the sender-ownership check.
enum SenderCheck {
    /// The envelope sender belongs to the authenticated user: proceed.
    Accept,
    /// Refuse with this protocol reply — either a permanent `553` (not owned) or
    /// a temporary `451` (the lookup itself failed, must not bounce for good).
    Reject(&'static str),
}

/// Whether `envelope_from` (already lowercased) is one of the addresses the
/// authenticated mailbox owns. Kept pure — the caller assembles the set from the
/// mailbox login and the user's account addresses. The null sender `<>` is never
/// owned: a submission client sends real mail, not a bounce.
fn sender_is_owned(envelope_from: &str, owned_addresses: &[&str]) -> bool {
    let from = envelope_from.trim().to_ascii_lowercase();
    if from.is_empty() {
        return false;
    }
    owned_addresses
        .iter()
        .any(|owned| owned.trim().eq_ignore_ascii_case(&from))
}

/// Confirms the envelope sender belongs to the authenticated user. The mailbox
/// login is owned by definition; anything else must be one of the addresses on
/// the user's configured accounts, which only the database knows.
async fn verify_sender_ownership(db: &PgPool, address: &str, mailbox: &Mailbox) -> SenderCheck {
    if sender_is_owned(address, &[mailbox.username.as_str()]) {
        return SenderCheck::Accept;
    }
    // A null or empty sender is never owned; do not even query for it.
    if address.trim().is_empty() {
        return SenderCheck::Reject("553 5.7.1 Sender address not owned by authenticated user");
    }

    let owned: bool = match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM mail.accounts WHERE user_id = $1 AND LOWER(email_address) = $2)",
    )
    .bind(mailbox.user_id)
    .bind(address.trim().to_ascii_lowercase())
    .fetch_one(db)
    .await
    {
        Ok(value) => value,
        Err(e) => {
            tracing::error!(error = %e, "Vérification de la propriété de l'expéditeur impossible");
            return SenderCheck::Reject("451 4.3.0 Sender verification temporarily unavailable");
        }
    };

    if owned {
        SenderCheck::Accept
    } else {
        tracing::warn!(
            user = %mailbox.username,
            "SMTP : expéditeur d'enveloppe non possédé par l'utilisateur authentifié"
        );
        SenderCheck::Reject("553 5.7.1 Sender address not owned by authenticated user")
    }
}

/// Prepends the two headers an MSA supplies when the client omitted them
/// (RFC 6409 §8.1): a Date and a Message-ID. Nothing is added when the message
/// already carries one, and the originals are never touched. Prepending is safe:
/// header order carries no meaning, and the block boundary is unchanged.
fn add_submission_headers(message: &[u8], hostname: &str) -> Vec<u8> {
    let mut prefix = Vec::new();
    if !has_header(message, b"Date") {
        let now = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S %z");
        prefix.extend_from_slice(format!("Date: {now}\r\n").as_bytes());
    }
    if !has_header(message, b"Message-ID") {
        let id = uuid::Uuid::new_v4();
        prefix.extend_from_slice(
            format!("Message-ID: <{id}@{}>\r\n", sanitize_token(hostname)).as_bytes(),
        );
    }
    if prefix.is_empty() {
        return message.to_vec();
    }
    let mut out = Vec::with_capacity(prefix.len() + message.len());
    out.extend_from_slice(&prefix);
    out.extend_from_slice(message);
    out
}

/// Removes every existing `Authentication-Results:` header (and its folded
/// continuation lines) from the message, then prepends our freshly computed one.
///
/// Stripping the inbound ones is the security point: a sender can put
/// `Authentication-Results: our.host; dmarc=pass` in their message, and a
/// downstream reader that trusts the topmost such header would be fooled. Only a
/// header WE add, above everything, may be trusted — so we drop theirs first.
fn stamp_auth_results(message: &[u8], header_value: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(message.len() + header_value.len() + 32);
    if !header_value.trim().is_empty() {
        out.extend_from_slice(b"Authentication-Results: ");
        // A header value can never carry a bare CR/LF.
        let clean: String = header_value.chars().filter(|c| *c != '\r' && *c != '\n').collect();
        out.extend_from_slice(clean.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(&strip_headers(message, "Authentication-Results"));
    out
}

/// Returns the message with every header field named `name` — and its folded
/// continuation lines — removed from the header block.
///
/// This is what makes a header WE add trustworthy: whatever the sender wrote
/// under that name is gone before ours goes on top, so the topmost occurrence
/// is always the one we vouch for. Only the header block is scanned; the body
/// is copied through untouched.
fn strip_headers(message: &[u8], name: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(message);
    let head_end = text.find("\r\n\r\n").map(|i| i + 2)
        .or_else(|| text.find("\n\n").map(|i| i + 1))
        .unwrap_or(text.len());
    let (headers, body) = text.split_at(head_end);

    let mut kept = String::with_capacity(headers.len());
    let mut dropping = false;
    for line in headers.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let is_continuation = trimmed.starts_with(' ') || trimmed.starts_with('\t');
        if dropping && is_continuation {
            continue; // folded line of a dropped header
        }
        dropping = header_name_is(trimmed, name);
        if dropping {
            continue;
        }
        kept.push_str(line);
    }

    let mut out = Vec::with_capacity(message.len());
    out.extend_from_slice(kept.as_bytes());
    out.extend_from_slice(body.as_bytes());
    out
}

// ── Reception policy ─────────────────────────────────────────────────────────

/// Whether this HELO/EHLO argument must be refused.
///
/// Only on a reception listener: `require_fqdn_helo` is a setting about foreign
/// servers, and an end user's mail client routinely announces its laptop's
/// name. Refusing that would break submission for the sake of a check that
/// buys nothing there — the client has already authenticated.
fn helo_rejected(cfg: &ServerConfig, submission: bool, argument: &str) -> bool {
    cfg.require_fqdn_helo && !submission && !hygiene::is_fqdn_helo(argument)
}

/// The hop-count ceiling actually applied at DATA time: the administrator's
/// value, never above the compiled floor `deliver_local` also enforces.
fn effective_hopcount_limit(cfg: &ServerConfig) -> usize {
    cfg.hopcount_limit.clamp(1, hygiene::HOPCOUNT_LIMIT)
}

/// The disposition a sending domain published in its DMARC record (`p=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DmarcPolicy {
    /// `p=none` — the domain asks to be told, not to be protected.
    None,
    Quarantine,
    Reject,
}

/// What to do with a received message, given every authentication verdict and
/// the policy its `From:` domain published.
///
/// Every check that fired contributes an action and the STRICTEST wins
/// (`PolicyAction` is ordered by severity). A verdict that is not a plain
/// failure contributes nothing: `temperror` above all means our own resolver
/// had a bad minute, and turning that into a rejection would drop real mail
/// every time DNS hiccups. Same for `permerror`, `neutral` and `none` — a
/// domain that publishes nothing has said nothing.
fn message_policy_action(
    cfg: &ServerConfig,
    verdict: &AuthVerdict,
    dmarc_policy: Option<DmarcPolicy>,
) -> PolicyAction {
    let mut action = PolicyAction::Ignore;
    let is = |value: &Option<String>, token: &str| value.as_deref() == Some(token);

    if is(&verdict.spf, "fail") {
        action = action.max(cfg.spf_fail_action);
    }
    if is(&verdict.spf, "softfail") {
        action = action.max(cfg.spf_softfail_action);
    }
    if is(&verdict.dkim, "fail") {
        action = action.max(cfg.dkim_fail_action);
    }
    if cfg.dmarc_honor_policy && is(&verdict.dmarc, "fail") {
        action = action.max(match dmarc_policy {
            Some(DmarcPolicy::Reject) => cfg.dmarc_reject_action,
            Some(DmarcPolicy::Quarantine) => cfg.dmarc_quarantine_action,
            // `p=none`, or a policy we could not read back (a DNS failure is
            // never a reason to refuse mail): record the failure, deliver it.
            Some(DmarcPolicy::None) | Option::None => PolicyAction::Mark,
        });
    }
    action
}

/// Reads back the `p=` the `From:` domain publishes, when — and only when —
/// something is going to be decided with it.
///
/// `mail-auth` computes the applicable policy inside `verify_dmarc` but does
/// not hand it back through `AuthVerdict`, and its tree walk is private, so
/// this is a second, deliberately narrow lookup: it runs on a `dmarc=fail`
/// with the policy honoured, which is a rare path, and never otherwise.
///
/// Returns `None` when the policy cannot be established — the caller treats
/// that as "mark only", never as a licence to reject.
async fn published_dmarc_policy(
    cfg: &ServerConfig,
    verdict: &AuthVerdict,
    message: &[u8],
) -> Option<DmarcPolicy> {
    if !cfg.dmarc_honor_policy || verdict.dmarc.as_deref() != Some("fail") {
        return None;
    }
    let domain = header_from_domain(message)?;
    lookup_dmarc_policy(&domain).await
}

/// One `_dmarc.<domain>` TXT lookup, then the parent domain's record — RFC 7489
/// §6.6.3's tree walk, minus the public-suffix step: without a suffix list the
/// only safe stopping point is the last label, and a query for `_dmarc.com`
/// returns nothing anyway.
async fn lookup_dmarc_policy(domain: &str) -> Option<DmarcPolicy> {
    use mail_auth::{common::cache::NoCache, dmarc::Dmarc, MessageAuthenticator, Txt};

    let resolver = MessageAuthenticator::new_system_conf()
        .map_err(|e| tracing::warn!(error = %e, "Résolution DMARC : resolveur indisponible"))
        .ok()?;
    let none = Option::<&NoCache<String, Txt>>::None;

    if let Ok(record) = resolver
        .txt_lookup::<Dmarc>(format!("_dmarc.{domain}."), none)
        .await
    {
        return dmarc_policy_of(record.p);
    }

    // A subdomain with no record of its own inherits the organizational
    // domain's, where `sp=` — when present — is what governs it.
    let parent = domain.split_once('.').map(|(_, rest)| rest)?;
    if !parent.contains('.') {
        return None;
    }
    let record = resolver
        .txt_lookup::<Dmarc>(format!("_dmarc.{parent}."), none)
        .await
        .ok()?;
    dmarc_policy_of(if record.sp == mail_auth::dmarc::Policy::Unspecified {
        record.p
    } else {
        record.sp
    })
}

/// Maps `mail-auth`'s policy tag onto ours. `Unspecified` — a record with no
/// `p=` — is not a policy and yields `None`, so nothing is enforced from it.
fn dmarc_policy_of(policy: mail_auth::dmarc::Policy) -> Option<DmarcPolicy> {
    match policy {
        mail_auth::dmarc::Policy::None => Some(DmarcPolicy::None),
        mail_auth::dmarc::Policy::Quarantine => Some(DmarcPolicy::Quarantine),
        mail_auth::dmarc::Policy::Reject => Some(DmarcPolicy::Reject),
        mail_auth::dmarc::Policy::Unspecified => Option::None,
    }
}

/// The domain of the `From:` header — the identity DMARC is about, and the only
/// one a human ever sees.
///
/// Deliberately hand-rolled and bounded rather than a full RFC 5322 parse: this
/// runs on the reception path, before the message is trusted enough to be
/// handed to a parser, and it needs exactly one field. When the value holds an
/// angle-addr the LAST one wins, which is what defeats
/// `From: "victim@bank.example <x>" <attacker@evil.example>`.
fn header_from_domain(message: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(message);
    let mut value: Option<String> = None;

    for raw in text.split('\n') {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            break; // end of the header block
        }
        match &mut value {
            // Already collecting: keep folding continuation lines in.
            Some(collected) if line.starts_with(' ') || line.starts_with('\t') => {
                collected.push(' ');
                collected.push_str(line.trim());
                continue;
            }
            Some(_) => break, // the From header is complete
            None => {}
        }
        // Sliced through `get` so a multi-byte first character cannot panic on
        // a byte index that is not a character boundary.
        if line.get(..5).is_some_and(|name| name.eq_ignore_ascii_case("From:")) {
            value = line.get(5..).map(|rest| rest.trim().to_string());
        }
    }

    let value = value?;
    let address = match (value.rfind('<'), value.rfind('>')) {
        (Some(open), Some(close)) if close > open => value[open + 1..close].to_string(),
        _ => value,
    };
    let domain = address.rsplit_once('@')?.1;
    let domain = domain
        .trim()
        .trim_end_matches('>')
        .trim_matches('"')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    // Bounded, and a real domain: nothing else reaches a DNS query.
    if domain.is_empty() || domain.len() > 255 || !hygiene::is_fqdn_helo(&domain) {
        return None;
    }
    Some(domain)
}

/// Stamps the message with the action the authentication policy took, so the
/// store, the user's rules and the interface can all see it.
///
/// `Quarantine` gets the same header AND is filed into Spam by way of
/// `deliver::Disposition` — the header alone would only be a mark under
/// another name.
fn policy_marker(message: &[u8], action: PolicyAction, verdict: &AuthVerdict) -> Vec<u8> {
    let label = match action {
        PolicyAction::Quarantine => "quarantine",
        _ => "mark",
    };
    // Every value spliced in is sanitised: a verdict token comes from
    // `mail-auth` and is safe, but nothing in this file splices an unsanitised
    // string into a header.
    let header = format!(
        "X-Kubuno-Auth-Policy: {}; spf={}; dkim={}; dmarc={}\r\n",
        hygiene::sanitize_header_value(label),
        hygiene::sanitize_header_value(verdict.spf.as_deref().unwrap_or("none")),
        hygiene::sanitize_header_value(verdict.dkim.as_deref().unwrap_or("none")),
        hygiene::sanitize_header_value(verdict.dmarc.as_deref().unwrap_or("none")),
    );
    // Anything the SENDER wrote under this name is dropped first. A forged
    // marker could otherwise sit alongside ours and let a reader that takes the
    // last occurrence — or a naive filter — read the wrong verdict.
    let stripped = strip_headers(message, "X-Kubuno-Auth-Policy");
    let mut out = Vec::with_capacity(header.len() + stripped.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(&stripped);
    out
}

/// Whether this header line's field name is exactly `name`.
///
/// The name must END where it ends: matching on the prefix alone would drop
/// `Authentication-Results-Original:` along with `Authentication-Results:`, and
/// — the other way round — let `X-Kubuno-Auth-Policy-Note:` masquerade as ours.
fn header_name_is(line: &str, name: &str) -> bool {
    match line.get(..name.len()) {
        Some(candidate) if candidate.eq_ignore_ascii_case(name) => {
            // A colon, possibly after the stray space non-conforming senders
            // put before it.
            line[name.len()..].trim_start().starts_with(':')
        }
        _ => false,
    }
}

/// Whether the header block (everything up to the first blank line) carries a
/// field named `name`, case-insensitively. Folded continuation lines — those
/// starting with a space or tab — belong to the previous header and are skipped,
/// so a `name:` appearing inside a folded value is never mistaken for a field.
fn has_header(message: &[u8], name: &[u8]) -> bool {
    for raw in message.split(|b| *b == b'\n') {
        // Drop the CR left by splitting a CRLF stream on LF.
        let line = match raw.split_last() {
            Some((b'\r', rest)) => rest,
            _ => raw,
        };
        if line.is_empty() {
            // The blank line ends the header block.
            return false;
        }
        if matches!(line.first(), Some(b' ') | Some(b'\t')) {
            continue; // folded continuation of the previous header
        }
        if let Some(colon) = line.iter().position(|b| *b == b':') {
            if trim_ascii_end(&line[..colon]).eq_ignore_ascii_case(name) {
                return true;
            }
        }
    }
    false
}

/// Trims trailing ASCII whitespace from a byte slice (a header name may carry a
/// stray space before its colon on non-conforming input).
fn trim_ascii_end(mut bytes: &[u8]) -> &[u8] {
    while let Some((last, rest)) = bytes.split_last() {
        if last.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

// ── Parsing helpers (kept pure so they can be tested) ────────────────────────

/// The verb and the rest of a command line.
fn split_command(line: &str) -> (String, &str) {
    let trimmed = line.trim_start();
    let end = trimmed
        .find(|c: char| c.is_whitespace() || c == ':')
        .unwrap_or(trimmed.len());
    let verb = trimmed[..end].to_ascii_uppercase();
    (verb, &trimmed[end..])
}

/// What `MAIL FROM` carried.
#[derive(Debug, PartialEq, Eq)]
struct Envelope {
    /// The reverse path; empty for the null sender `<>` (bounces).
    address: String,
    /// The `SIZE=` parameter, when the client announced one.
    size: Option<usize>,
}

/// Strips a leading keyword such as `TO:` or `FROM:`, case-insensitively,
/// tolerating the space some clients put before the colon.
fn strip_keyword<'a>(rest: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = rest.trim_start();
    let name = keyword.trim_end_matches(':');
    let candidate = rest.get(..name.len())?;
    if !candidate.eq_ignore_ascii_case(name) {
        return None;
    }
    let after = rest[name.len()..].trim_start();
    after.strip_prefix(':').map(str::trim_start)
}

/// Parses everything after the `MAIL` verb: `FROM:<a@b> SIZE=123 BODY=8BITMIME`.
fn parse_mail_from(rest: &str) -> Result<Envelope, &'static str> {
    let rest = strip_keyword(rest, "FROM:").ok_or("501 5.5.4 Syntax: MAIL FROM:<address>")?;

    // The path first, then the ESMTP parameters that follow it.
    let (path, params) = if let Some(stripped) = rest.strip_prefix('<') {
        let end = stripped.find('>').ok_or("501 5.1.7 Bad sender address syntax")?;
        (&stripped[..end], &stripped[end + 1..])
    } else {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        (&rest[..end], &rest[end..])
    };

    let address = normalize_path(path);
    if !address.is_empty() && !is_valid_address(&address) {
        return Err("501 5.1.7 Bad sender address syntax");
    }

    let mut size = None;
    for param in params.split_whitespace() {
        let Some((key, value)) = param.split_once('=') else { continue };
        if key.eq_ignore_ascii_case("SIZE") {
            size = Some(value.parse::<usize>().map_err(|_| "501 5.5.4 Bad SIZE parameter")?);
        }
        // Other parameters (BODY, AUTH, …) are accepted and ignored: refusing
        // an unknown one only breaks clients without protecting anything.
    }

    Ok(Envelope { address: address.to_ascii_lowercase(), size })
}

/// Extracts the address of a `RCPT TO:` path.
fn extract_path(rest: &str) -> Option<String> {
    let rest = rest.trim();
    let inner = if let Some(stripped) = rest.strip_prefix('<') {
        stripped.split('>').next()?
    } else {
        rest.split_whitespace().next()?
    };
    Some(normalize_path(inner))
}

/// Drops the obsolete source route of a path (`@relay1,@relay2:user@host` →
/// `user@host`, RFC 5321 §4.1.1.3) and trims what is left.
fn normalize_path(path: &str) -> String {
    let path = path.trim();
    match path.starts_with('@').then(|| path.rfind(':')).flatten() {
        Some(index) => path[index + 1..].trim().to_string(),
        None => path.to_string(),
    }
}

/// Is this something we can safely put in a database column and an envelope?
fn is_valid_address(address: &str) -> bool {
    if address.is_empty() || address.len() > 320 {
        return false;
    }
    if address.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    match address.rsplit_once('@') {
        Some((local, domain)) => {
            !local.is_empty() && !domain.is_empty() && domain.contains('.') && !domain.starts_with('.')
        }
        None => false,
    }
}

/// Decodes a SASL PLAIN payload: `authzid\0authcid\0password`.
/// Returns the login and the password; both are secret and never logged.
fn decode_auth_plain(payload: &str) -> Option<(String, String)> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload.trim().as_bytes())
        .ok()?;
    let mut fields = decoded.split(|b| *b == 0);
    let authzid = fields.next()?;
    let authcid = fields.next()?;
    let password = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    // The login is the authentication identity, falling back to the
    // authorization identity when the client left it empty.
    let login = if authcid.is_empty() { authzid } else { authcid };
    Some((
        String::from_utf8(login.to_vec()).ok()?,
        String::from_utf8(password.to_vec()).ok()?,
    ))
}

/// Decodes one base64 field of a SASL LOGIN exchange.
fn decode_base64_string(value: &str) -> Option<String> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value.trim().as_bytes())
        .ok()?;
    String::from_utf8(decoded).ok()
}

/// Wraps one SASL SCRAM message as the base64 a `334` challenge carries.
fn encode_base64_string(value: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

/// Undoes the transparency dot of RFC 5321 §4.5.2: a line the client started
/// with `.` had one added, and the original line is what must be stored.
fn unstuff_dot(line: &[u8]) -> &[u8] {
    match line.first() {
        Some(b'.') => &line[1..],
        _ => line,
    }
}

/// Removes anything that could break out of a header value or a log line.
///
/// Control characters become spaces rather than disappearing: dropping them
/// would splice `client.example\r\nBcc: victim@…` into one unreadable token,
/// whereas a space keeps the whole thing visible on the single line it belongs
/// to — which is what stops it from becoming a header of its own.
fn sanitize_token(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(255)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

/// Prepends the trace headers a receiving server owes the message: who handed
/// it over, to whom it was addressed, and when we took it.
fn with_trace_headers(
    message: &[u8],
    cfg: &ServerConfig,
    peer: &str,
    greeting: Option<&str>,
    sender: &str,
    recipient: &str,
) -> Vec<u8> {
    let now = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S %z");
    let helo = greeting.map(sanitize_token).unwrap_or_else(|| "unknown".to_string());
    let header = format!(
        "Return-Path: <{}>\r\n\
         Delivered-To: {}\r\n\
         Received: from {} ({})\r\n\tby {} with SMTP;\r\n\t{}\r\n",
        sanitize_token(sender),
        sanitize_token(recipient),
        helo,
        sanitize_token(peer),
        sanitize_token(&cfg.hostname),
        now,
    );

    let mut out = Vec::with_capacity(header.len() + message.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(message);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    };

    fn config() -> ServerConfig {
        ServerConfig {
            hostname: "mail.kubuno.local".into(),
            domains: vec!["kubuno.local".into(), "exemple.fr".into()],
            ..ServerConfig::default()
        }
    }

    // ── Relay policy: the four cases, one test ──────────────────────────────

    #[test]
    fn an_unauthenticated_client_may_only_write_to_local_domains() {
        let cfg = config();
        assert_eq!(relay_decision(&cfg, false, "alice@kubuno.local"), RelayDecision::Local);
        assert_eq!(relay_decision(&cfg, false, "victime@ailleurs.net"), RelayDecision::Denied);
    }

    #[test]
    fn an_authenticated_client_may_address_the_outside() {
        let cfg = config();
        assert_eq!(relay_decision(&cfg, true, "bob@exemple.fr"), RelayDecision::Local);
        assert_eq!(
            relay_decision(&cfg, true, "ami@ailleurs.net"),
            RelayDecision::ForwardUnsupported,
            "légitime, mais aucun relais sortant n'est configuré"
        );
    }

    #[test]
    fn domain_matching_is_case_insensitive_and_never_suffix_based() {
        let cfg = config();
        assert_eq!(relay_decision(&cfg, false, "alice@KUBUNO.local"), RelayDecision::Local);
        // "notkubuno.local" merely ends with our domain: it is NOT ours.
        assert_eq!(relay_decision(&cfg, false, "eve@notkubuno.local"), RelayDecision::Denied);
        assert_eq!(relay_decision(&cfg, false, "eve@kubuno.local.evil.net"), RelayDecision::Denied);
    }

    #[test]
    fn a_server_with_no_domain_relays_nothing() {
        let cfg = ServerConfig::default();
        assert_eq!(relay_decision(&cfg, false, "alice@kubuno.local"), RelayDecision::Denied);
    }

    // ── STARTTLS advertisement ──────────────────────────────────────────────

    #[test]
    fn advertises_starttls_only_when_upgradable_and_still_cleartext() {
        assert!(advertises_starttls(TlsMode::StartTls, true, false));
        assert!(!advertises_starttls(TlsMode::StartTls, false, false), "aucun certificat configuré");
        assert!(!advertises_starttls(TlsMode::StartTls, true, true), "connexion déjà chiffrée");
        assert!(!advertises_starttls(TlsMode::None, true, false), "écouteur en clair pur");
        assert!(!advertises_starttls(TlsMode::Implicit, true, true), "TLS implicite : rien à annoncer");
    }

    // ── MAIL FROM ───────────────────────────────────────────────────────────

    #[test]
    fn parses_mail_from_with_a_size_parameter() {
        let envelope = parse_mail_from(" FROM:<Alice@Example.com> SIZE=123").expect("parse");
        assert_eq!(envelope.address, "alice@example.com");
        assert_eq!(envelope.size, Some(123));
    }

    #[test]
    fn parses_mail_from_without_parameters_and_the_null_sender() {
        assert_eq!(
            parse_mail_from(" FROM:<a@b.example>").expect("parse"),
            Envelope { address: "a@b.example".into(), size: None }
        );
        assert_eq!(
            parse_mail_from("FROM:<>").expect("parse"),
            Envelope { address: String::new(), size: None },
            "le chemin nul est l'expéditeur des rapports de non-remise"
        );
    }

    #[test]
    fn parses_mail_from_with_other_esmtp_parameters() {
        let envelope = parse_mail_from("FROM:<a@b.example> BODY=8BITMIME SIZE=4096").expect("parse");
        assert_eq!(envelope.size, Some(4096));
    }

    #[test]
    fn rejects_malformed_mail_from() {
        assert!(parse_mail_from("").is_err());
        assert!(parse_mail_from(" TO:<a@b.example>").is_err());
        assert!(parse_mail_from(" FROM:<a@b.example").is_err(), "chevron non fermé");
        assert!(parse_mail_from(" FROM:<pas-une-adresse>").is_err());
        assert!(parse_mail_from(" FROM:<a@b.example> SIZE=beaucoup").is_err());
    }

    // ── Address extraction ──────────────────────────────────────────────────

    #[test]
    fn extracts_the_address_between_angle_brackets() {
        assert_eq!(extract_path("<bob@exemple.fr>").as_deref(), Some("bob@exemple.fr"));
        assert_eq!(extract_path(" <bob@exemple.fr> NOTIFY=NEVER").as_deref(), Some("bob@exemple.fr"));
        assert_eq!(extract_path("bob@exemple.fr").as_deref(), Some("bob@exemple.fr"));
        assert_eq!(extract_path("<>").as_deref(), Some(""));
    }

    #[test]
    fn drops_the_obsolete_source_route() {
        assert_eq!(
            extract_path("<@relais1.net,@relais2.net:bob@exemple.fr>").as_deref(),
            Some("bob@exemple.fr")
        );
    }

    #[test]
    fn validates_addresses_before_they_reach_the_database() {
        assert!(is_valid_address("bob@exemple.fr"));
        assert!(!is_valid_address(""));
        assert!(!is_valid_address("bob"));
        assert!(!is_valid_address("bob@localhost"), "un domaine sans point n'est pas routable");
        assert!(!is_valid_address("bob@exemple.fr\r\nRCPT TO:<eve@exemple.fr>"));
        assert!(!is_valid_address(&format!("{}@exemple.fr", "a".repeat(400))));
    }

    #[test]
    fn splits_the_verb_from_the_rest() {
        assert_eq!(split_command("ehlo client.example").0, "EHLO");
        assert_eq!(split_command("  MAIL FROM:<a@b.example>").1.trim(), "FROM:<a@b.example>");
        assert_eq!(split_command("rcpt to:<a@b.example>").0, "RCPT");
        assert_eq!(split_command("").0, "");
    }

    // ── AUTH ────────────────────────────────────────────────────────────────

    #[test]
    fn decodes_auth_plain() {
        // "\0alice@kubuno.local\0secret"
        let payload = base64::engine::general_purpose::STANDARD.encode(b"\0alice@kubuno.local\0secret");
        assert_eq!(
            decode_auth_plain(&payload),
            Some(("alice@kubuno.local".into(), "secret".into()))
        );
    }

    #[test]
    fn auth_plain_falls_back_to_the_authorization_identity() {
        let payload = base64::engine::general_purpose::STANDARD.encode(b"alice@kubuno.local\0\0secret");
        assert_eq!(
            decode_auth_plain(&payload),
            Some(("alice@kubuno.local".into(), "secret".into()))
        );
    }

    #[test]
    fn rejects_malformed_auth_plain() {
        assert_eq!(decode_auth_plain("pas du base64 !!"), None);
        let two_fields = base64::engine::general_purpose::STANDARD.encode(b"alice\0secret");
        assert_eq!(decode_auth_plain(&two_fields), None, "il faut trois champs");
        let four_fields = base64::engine::general_purpose::STANDARD.encode(b"a\0b\0c\0d");
        assert_eq!(decode_auth_plain(&four_fields), None);
    }

    #[test]
    fn decodes_auth_login_fields() {
        let user = base64::engine::general_purpose::STANDARD.encode(b"alice@kubuno.local");
        assert_eq!(decode_base64_string(&user).as_deref(), Some("alice@kubuno.local"));
        assert_eq!(decode_base64_string("???"), None);
    }

    // ── DATA ────────────────────────────────────────────────────────────────

    #[test]
    fn undoes_dot_stuffing() {
        assert_eq!(unstuff_dot(b".."), b".");
        assert_eq!(unstuff_dot(b"..texte"), b".texte");
        assert_eq!(unstuff_dot(b".From nobody"), b"From nobody");
        assert_eq!(unstuff_dot(b"texte normal"), b"texte normal");
        assert_eq!(unstuff_dot(b""), b"");
    }

    #[test]
    fn trace_headers_cannot_be_used_to_inject_more_headers() {
        let raw = with_trace_headers(
            b"Subject: bonjour\r\n\r\ncorps\r\n",
            &config(),
            "10.0.0.9:40000",
            Some("client.example\r\nBcc: victime@exemple.fr"),
            "alice@exemple.fr",
            "bob@kubuno.local",
        );
        let text = String::from_utf8_lossy(&raw);
        // Ce qui compte : le HELO ne peut PAS ouvrir une ligne d'en-tête ; son
        // contenu reste à l'intérieur de la ligne Received.
        assert!(
            !text.lines().any(|l| l.starts_with("Bcc:")),
            "aucun en-tête injecté par le HELO"
        );
        assert!(text.contains("client.example  Bcc: victime@exemple.fr"), "replié dans Received");
        assert!(text.starts_with("Return-Path: <alice@exemple.fr>"));
        assert!(text.contains("Delivered-To: bob@kubuno.local"));
        assert!(text.contains("by mail.kubuno.local with SMTP"));
        assert!(text.ends_with("corps\r\n"));
    }

    // ── A real session, over a real socket ──────────────────────────────────
    //
    // The database handle points nowhere on purpose: refusing to relay must
    // need no query at all, and a database that is down must yield a TEMPORARY
    // refusal — a 550 there would bounce legitimate mail for good.

    fn unreachable_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(200))
            .connect_lazy("postgres://nobody:nobody@127.0.0.1:1/none")
            .expect("pool paresseux")
    }

    async fn expect_line(
        reader: &mut BufReader<OwnedReadHalf>,
        prefix: &str,
    ) -> String {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
            .await
            .expect("réponse du serveur")
            .expect("lecture");
        assert!(line.starts_with(prefix), "attendu {prefix:?}, reçu {line:?}");
        line
    }

    /// Starts one plaintext session on a loopback port and hands back the client
    /// side. The listener has no TLS (`TlsMode::None`, no acceptor), matching a
    /// reception port behind a terminator.
    async fn connected(cfg: ServerConfig) -> (BufReader<OwnedReadHalf>, OwnedWriteHalf) {
        connected_role(cfg, false).await
    }

    /// Like `connected`, but choosing the listener role: `submission = true`
    /// exercises the MSA path (auth required), `false` the reception MX path.
    async fn connected_role(
        cfg: ServerConfig,
        submission: bool,
    ) -> (BufReader<OwnedReadHalf>, OwnedWriteHalf) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("écoute");
        let addr = listener.local_addr().expect("adresse");
        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.expect("connexion entrante");
            let inc = Incoming {
                db: unreachable_pool(),
                cfg: Arc::new(cfg),
                peer: peer.to_string(),
                tls_mode: TlsMode::None,
                submission,
                acceptor: None,
                tarpit: Arc::new(Tarpit::new(Box::new(crate::server::limits::SystemClock::new()))),
            };
            let _ = handle(inc, MailStream::Plain(stream)).await;
        });

        let stream = TcpStream::connect(addr).await.expect("connexion");
        let (read_half, write_half) = stream.into_split();
        (BufReader::new(read_half), write_half)
    }

    #[tokio::test]
    async fn a_live_session_refuses_to_relay() {
        let cfg = ServerConfig {
            hostname: "mail.kubuno.test".into(),
            domains: vec!["kubuno.test".into()],
            max_message_bytes: 1024,
            ..ServerConfig::default()
        };
        let (mut reader, mut writer) = connected(cfg).await;

        expect_line(&mut reader, "220 mail.kubuno.test Kubuno SMTP ready").await;

        writer.write_all(b"EHLO client.example\r\n").await.expect("EHLO");
        expect_line(&mut reader, "250-mail.kubuno.test").await;
        expect_line(&mut reader, "250-SIZE 1024").await;
        expect_line(&mut reader, "250-8BITMIME").await;
        // Réception en clair : ni STARTTLS (aucun certificat) ni AUTH (un MX
        // n'authentifie personne) ne sont annoncés.
        expect_line(&mut reader, "250 HELP").await;

        // Taille annoncée hors limite : refusée avant tout transfert.
        writer.write_all(b"MAIL FROM:<a@ailleurs.net> SIZE=999999\r\n").await.expect("MAIL");
        expect_line(&mut reader, "552 5.3.4").await;

        writer.write_all(b"MAIL FROM:<spammeur@ailleurs.net>\r\n").await.expect("MAIL");
        expect_line(&mut reader, "250 2.1.0").await;

        // Le test qui compte : destinataire distant, client non authentifié.
        writer.write_all(b"RCPT TO:<victime@autre.net>\r\n").await.expect("RCPT");
        expect_line(&mut reader, "550 5.7.1 Relay access denied").await;

        // Destinataire local, base injoignable : temporaire, jamais définitif.
        writer.write_all(b"RCPT TO:<alice@kubuno.test>\r\n").await.expect("RCPT");
        expect_line(&mut reader, "451 4.3.0").await;

        writer.write_all(b"DATA\r\n").await.expect("DATA");
        expect_line(&mut reader, "503 5.5.1").await;

        writer.write_all(b"RSET\r\n").await.expect("RSET");
        expect_line(&mut reader, "250 2.0.0").await;
        writer.write_all(b"VRFY alice\r\n").await.expect("VRFY");
        expect_line(&mut reader, "252 2.5.2").await;
        writer.write_all(b"NOOP\r\n").await.expect("NOOP");
        expect_line(&mut reader, "250 2.0.0").await;
        writer.write_all(b"BLABLA\r\n").await.expect("commande inconnue");
        expect_line(&mut reader, "500 5.5.2").await;
        // STARTTLS sans certificat offert : non disponible.
        writer.write_all(b"STARTTLS\r\n").await.expect("STARTTLS");
        expect_line(&mut reader, "502 5.5.1").await;

        // AUTH LOGIN complet : la base est injoignable, donc refus.
        writer.write_all(b"AUTH LOGIN\r\n").await.expect("AUTH");
        expect_line(&mut reader, "334 VXNlcm5hbWU6").await;
        writer.write_all(b"YWxpY2VAa3VidW5vLnRlc3Q=\r\n").await.expect("login");
        expect_line(&mut reader, "334 UGFzc3dvcmQ6").await;
        writer.write_all(b"bW90ZGVwYXNzZQ==\r\n").await.expect("mot de passe");
        expect_line(&mut reader, "535 5.7.8").await;

        // Toujours non authentifié : le relais reste fermé.
        writer.write_all(b"MAIL FROM:<spammeur@ailleurs.net>\r\n").await.expect("MAIL");
        expect_line(&mut reader, "250 2.1.0").await;
        writer.write_all(b"RCPT TO:<victime@autre.net>\r\n").await.expect("RCPT");
        expect_line(&mut reader, "550 5.7.1").await;

        writer.write_all(b"QUIT\r\n").await.expect("QUIT");
        expect_line(&mut reader, "221 2.0.0").await;
    }

    /// End to end on a real socket: with the screen on, a non-qualified HELO is
    /// refused and the session never acquires a greeting — while a proper name
    /// goes through on the very same listener.
    #[tokio::test]
    async fn a_live_session_refuses_a_non_qualified_helo() {
        let cfg = ServerConfig { require_fqdn_helo: true, ..config() };
        let (mut reader, mut writer) = connected(cfg).await;
        expect_line(&mut reader, "220").await;

        writer.write_all(b"EHLO localhost\r\n").await.expect("écriture");
        expect_line(&mut reader, "550 5.7.1").await;

        // The envelope is still unreachable: no greeting was recorded.
        writer.write_all(b"MAIL FROM:<a@ailleurs.fr>\r\n").await.expect("écriture");
        expect_line(&mut reader, "503 5.5.1").await;

        // A qualified name on the same session is accepted.
        writer.write_all(b"EHLO mail.exemple.fr\r\n").await.expect("écriture");
        expect_line(&mut reader, "250-").await;
    }

    /// The instance block list refuses the envelope sender outright, and the
    /// allow-list is what lifts it — that is the only thing it lifts.
    #[tokio::test]
    async fn a_live_session_refuses_a_blocklisted_sender() {
        let cfg = ServerConfig {
            blocklist_domains: vec!["casino.example".into()],
            blocklist_senders: vec!["spammeur@exemple.fr".into()],
            allowlist_senders: vec!["ami@casino.example".into()],
            ..config()
        };
        let (mut reader, mut writer) = connected(cfg).await;
        expect_line(&mut reader, "220").await;
        writer.write_all(b"HELO mail.exemple.fr\r\n").await.expect("écriture");
        expect_line(&mut reader, "250").await;

        writer.write_all(b"MAIL FROM:<n-importe-qui@casino.example>\r\n").await.expect("écriture");
        expect_line(&mut reader, "550 5.7.1").await;

        writer.write_all(b"MAIL FROM:<spammeur@exemple.fr>\r\n").await.expect("écriture");
        expect_line(&mut reader, "550 5.7.1").await;

        // Allow-listed, on a blocked domain: it goes through.
        writer.write_all(b"MAIL FROM:<ami@casino.example>\r\n").await.expect("écriture");
        expect_line(&mut reader, "250 2.1.0").await;
    }

    /// The error ceiling is the administrator's now: set it to two and the
    /// session really is closed on the second refusal.
    #[tokio::test]
    async fn a_live_session_closes_after_the_configured_number_of_errors() {
        let cfg = ServerConfig { max_protocol_errors: 2, ..config() };
        let (mut reader, mut writer) = connected(cfg).await;
        expect_line(&mut reader, "220").await;

        writer.write_all(b"N-IMPORTE-QUOI\r\n").await.expect("écriture");
        expect_line(&mut reader, "500").await;
        writer.write_all(b"ENCORE\r\n").await.expect("écriture");
        expect_line(&mut reader, "500").await;
        expect_line(&mut reader, "421 4.7.0").await;
    }

    #[tokio::test]
    async fn a_session_that_skips_the_greeting_gets_nowhere() {
        let cfg = ServerConfig {
            hostname: "mail.kubuno.test".into(),
            domains: vec!["kubuno.test".into()],
            ..ServerConfig::default()
        };
        let (mut reader, mut writer) = connected(cfg).await;
        expect_line(&mut reader, "220 ").await;

        writer.write_all(b"MAIL FROM:<a@ailleurs.net>\r\n").await.expect("MAIL");
        expect_line(&mut reader, "503 5.5.1").await;
        writer.write_all(b"AUTH PLAIN AGFsaWNlAHNlY3JldA==\r\n").await.expect("AUTH");
        expect_line(&mut reader, "503 5.5.1").await;
        writer.write_all(b"RCPT TO:<alice@kubuno.test>\r\n").await.expect("RCPT");
        expect_line(&mut reader, "503 5.5.1").await;
        writer.write_all(b"QUIT\r\n").await.expect("QUIT");
        expect_line(&mut reader, "221 ").await;
    }

    // ── Submission policy (RFC 6409) ────────────────────────────────────────

    #[test]
    fn only_submission_requires_authentication_for_the_envelope() {
        // Reception (MX): the envelope is never gated on auth.
        assert!(!envelope_blocked_pending_auth(false, false));
        assert!(!envelope_blocked_pending_auth(false, true));
        // Submission (MSA): gated until authenticated.
        assert!(envelope_blocked_pending_auth(true, false), "530 attendu tant que non authentifié");
        assert!(!envelope_blocked_pending_auth(true, true));
    }

    #[test]
    fn auth_is_advertised_only_on_an_encrypted_submission_channel() {
        assert!(advertises_auth(true, true), "soumission chiffrée");
        assert!(!advertises_auth(true, false), "soumission en clair : STARTTLS d'abord");
        assert!(!advertises_auth(false, true), "réception : aucun AUTH");
        assert!(!advertises_auth(false, false));
    }

    #[test]
    fn scram_is_the_first_advertised_mechanism() {
        // Announced (via `250-AUTH <auth_mechanisms()>`) exactly when
        // `advertises_auth` holds — i.e. on an encrypted submission channel.
        let advertised = auth_mechanisms();
        assert!(advertised.starts_with("SCRAM-SHA-256"), "SCRAM annoncé en premier (préféré)");
        assert!(advertised.contains("PLAIN"));
        assert!(advertised.contains("LOGIN"));
    }

    #[test]
    fn sender_ownership_matches_case_insensitively_and_refuses_the_null_sender() {
        let owned = ["alice@kubuno.local", "alice.pro@exemple.fr"];
        assert!(sender_is_owned("alice@kubuno.local", &owned));
        assert!(sender_is_owned("Alice.Pro@Exemple.FR", &owned), "comparaison insensible à la casse");
        assert!(!sender_is_owned("eve@kubuno.local", &owned), "adresse non possédée");
        assert!(!sender_is_owned("", &owned), "l'expéditeur nul <> n'est jamais possédé");
        assert!(!sender_is_owned("alice@kubuno.local", &[]), "aucune adresse possédée");
    }

    // ── Missing Message-ID / Date detection ─────────────────────────────────

    // ── Reception policy ────────────────────────────────────────────────────

    fn verdict(spf: Option<&str>, dkim: Option<&str>, dmarc: Option<&str>) -> AuthVerdict {
        AuthVerdict {
            spf: spf.map(str::to_string),
            dkim: dkim.map(str::to_string),
            dmarc: dmarc.map(str::to_string),
            ..AuthVerdict::default()
        }
    }

    #[test]
    fn the_fqdn_helo_screen_is_off_until_the_operator_turns_it_on() {
        let cfg = config();
        assert!(!cfg.require_fqdn_helo);
        assert!(!helo_rejected(&cfg, false, "localhost"));
    }

    #[test]
    fn a_screened_helo_must_be_fully_qualified() {
        let cfg = ServerConfig { require_fqdn_helo: true, ..config() };
        assert!(helo_rejected(&cfg, false, "localhost"));
        assert!(!helo_rejected(&cfg, false, "mail.example.org"));
        assert!(!helo_rejected(&cfg, false, "[192.0.2.1]"));
    }

    /// A user's mail client announces the name of their laptop. Screening that
    /// on the submission port would refuse every one of our own users.
    #[test]
    fn the_fqdn_helo_screen_never_applies_to_submission() {
        let cfg = ServerConfig { require_fqdn_helo: true, ..config() };
        assert!(!helo_rejected(&cfg, true, "PC-DE-JEAN"));
    }

    /// The setting may tighten the loop guard, never loosen it past the floor
    /// `deliver_local` enforces on its own.
    #[test]
    fn the_hopcount_setting_may_only_be_stricter_than_the_floor() {
        assert_eq!(effective_hopcount_limit(&ServerConfig { hopcount_limit: 10, ..config() }), 10);
        assert_eq!(
            effective_hopcount_limit(&ServerConfig { hopcount_limit: 500, ..config() }),
            hygiene::HOPCOUNT_LIMIT
        );
    }

    #[test]
    fn a_fully_authenticated_message_is_left_alone() {
        let cfg = config();
        let v = verdict(Some("pass"), Some("pass"), Some("pass"));
        assert_eq!(message_policy_action(&cfg, &v, None), PolicyAction::Ignore);
    }

    #[test]
    fn each_failure_triggers_its_own_configured_action() {
        let cfg = ServerConfig {
            spf_fail_action: PolicyAction::Quarantine,
            spf_softfail_action: PolicyAction::Mark,
            dkim_fail_action: PolicyAction::Mark,
            dmarc_honor_policy: false,
            ..config()
        };
        let spf_fail = verdict(Some("fail"), Some("pass"), Some("pass"));
        assert_eq!(message_policy_action(&cfg, &spf_fail, None), PolicyAction::Quarantine);

        let softfail = verdict(Some("softfail"), Some("pass"), Some("pass"));
        assert_eq!(message_policy_action(&cfg, &softfail, None), PolicyAction::Mark);

        let dkim_fail = verdict(Some("pass"), Some("fail"), Some("pass"));
        assert_eq!(message_policy_action(&cfg, &dkim_fail, None), PolicyAction::Mark);
    }

    /// Several checks can fail at once; the answer is the strictest of them,
    /// never the average and never the last one evaluated.
    #[test]
    fn the_strictest_triggered_action_wins() {
        let cfg = ServerConfig {
            spf_fail_action: PolicyAction::Mark,
            dkim_fail_action: PolicyAction::Quarantine,
            dmarc_honor_policy: true,
            dmarc_reject_action: PolicyAction::Reject,
            ..config()
        };
        let v = verdict(Some("fail"), Some("fail"), Some("fail"));
        assert_eq!(
            message_policy_action(&cfg, &v, Some(DmarcPolicy::Reject)),
            PolicyAction::Reject
        );
        // Without the DMARC leg, the strictest of the two remaining ones.
        let v = verdict(Some("fail"), Some("fail"), Some("pass"));
        assert_eq!(message_policy_action(&cfg, &v, None), PolicyAction::Quarantine);
    }

    /// A resolver hiccup is not a forgery. `temperror` (and `permerror`,
    /// `neutral`, `none`) must never cost a message its delivery.
    #[test]
    fn a_temporary_verification_error_never_rejects() {
        let cfg = ServerConfig {
            spf_fail_action: PolicyAction::Reject,
            dkim_fail_action: PolicyAction::Reject,
            dmarc_reject_action: PolicyAction::Reject,
            ..config()
        };
        for token in ["temperror", "permerror", "neutral", "none"] {
            let v = verdict(Some(token), Some(token), Some(token));
            assert_eq!(
                message_policy_action(&cfg, &v, Some(DmarcPolicy::Reject)),
                PolicyAction::Ignore,
                "{token} ne doit rien déclencher"
            );
        }
        // An unevaluated check (the message would not parse) is not a failure.
        let v = verdict(None, None, None);
        assert_eq!(message_policy_action(&cfg, &v, None), PolicyAction::Ignore);
    }

    #[test]
    fn the_published_dmarc_policy_selects_the_action() {
        let cfg = ServerConfig {
            dmarc_honor_policy: true,
            dmarc_reject_action: PolicyAction::Reject,
            dmarc_quarantine_action: PolicyAction::Quarantine,
            ..config()
        };
        let v = verdict(Some("pass"), Some("pass"), Some("fail"));
        assert_eq!(
            message_policy_action(&cfg, &v, Some(DmarcPolicy::Reject)),
            PolicyAction::Reject
        );
        assert_eq!(
            message_policy_action(&cfg, &v, Some(DmarcPolicy::Quarantine)),
            PolicyAction::Quarantine
        );
        // `p=none` asks for reports, not for protection: mark and deliver.
        assert_eq!(
            message_policy_action(&cfg, &v, Some(DmarcPolicy::None)),
            PolicyAction::Mark
        );
        // Policy unreadable (DNS down): mark, never reject.
        assert_eq!(message_policy_action(&cfg, &v, None), PolicyAction::Mark);
    }

    /// An operator can soften a sender's policy — that is what the two
    /// `dmarc_*_action` settings are for — and turning DMARC off entirely must
    /// really turn it off.
    #[test]
    fn honouring_the_dmarc_policy_can_be_switched_off() {
        let cfg = ServerConfig {
            dmarc_honor_policy: false,
            dmarc_reject_action: PolicyAction::Reject,
            ..config()
        };
        let v = verdict(Some("pass"), Some("pass"), Some("fail"));
        assert_eq!(
            message_policy_action(&cfg, &v, Some(DmarcPolicy::Reject)),
            PolicyAction::Ignore
        );

        let softened = ServerConfig {
            dmarc_honor_policy: true,
            dmarc_reject_action: PolicyAction::Quarantine,
            ..config()
        };
        assert_eq!(
            message_policy_action(&softened, &v, Some(DmarcPolicy::Reject)),
            PolicyAction::Quarantine
        );
    }

    #[test]
    fn reads_the_domain_of_the_from_header() {
        let raw = b"From: Alice <alice@Example.ORG>\r\nTo: bob@kubuno.local\r\n\r\nbody";
        assert_eq!(header_from_domain(raw).as_deref(), Some("example.org"));

        let bare = b"From: alice@example.org\r\n\r\nbody";
        assert_eq!(header_from_domain(bare).as_deref(), Some("example.org"));

        let folded = b"From: Alice Example\r\n <alice@example.org>\r\nSubject: x\r\n\r\nbody";
        assert_eq!(header_from_domain(folded).as_deref(), Some("example.org"));
    }

    /// The angle-addr is the real identity: a display name holding a decoy
    /// address must not be what we look the DMARC policy up for.
    #[test]
    fn a_decoy_in_the_display_name_does_not_win() {
        let raw = b"From: \"service@banque.example <x@y.example>\" <attaquant@mechant.example>\r\n\r\nbody";
        assert_eq!(header_from_domain(raw).as_deref(), Some("mechant.example"));
    }

    #[test]
    fn refuses_a_from_domain_that_could_not_be_queried() {
        // No From at all, a From in the body only, and a nonsense domain.
        assert_eq!(header_from_domain(b"To: bob@kubuno.local\r\n\r\nFrom: a@b.c\r\n"), None);
        assert_eq!(header_from_domain(b"From: not-an-address\r\n\r\nbody"), None);
        assert_eq!(header_from_domain(b"From: alice@localhost\r\n\r\nbody"), None);
        assert_eq!(header_from_domain(b"From: alice@\r\n\r\nbody"), None);
    }

    /// Marking must never be able to open a header of its own, and it must
    /// leave the message it wraps untouched.
    #[test]
    fn the_policy_marker_is_one_header_above_an_intact_message() {
        let raw = b"From: a@b.example\r\nSubject: hi\r\n\r\nbody\r\n";
        let out = policy_marker(raw, PolicyAction::Quarantine, &verdict(Some("fail"), None, Some("fail")));
        let text = String::from_utf8_lossy(&out);
        assert!(text.starts_with("X-Kubuno-Auth-Policy: quarantine; spf=fail; dkim=none; dmarc=fail\r\n"));
        assert!(out.windows(raw.len()).any(|w| w == raw), "le message est intact");
        assert!(has_header(&out, b"X-Kubuno-Auth-Policy"));
        // Exactly one such header, whatever the verdict carried.
        assert_eq!(text.matches("X-Kubuno-Auth-Policy").count(), 1);
    }

    /// The whole value of a header WE add is that the sender's own copy is
    /// gone: a spammer who writes `Authentication-Results: … dmarc=pass` into
    /// their message must not leave it in the delivered mail.
    #[test]
    fn a_forged_authentication_results_is_stripped_before_ours_is_added() {
        let raw = b"Authentication-Results: mx.kubuno.local; dmarc=pass\r\n\
                    \t(forged continuation)\r\n\
                    From: a@b.example\r\n\
                    \r\n\
                    Authentication-Results: this one is body text\r\n";
        let out = stamp_auth_results(raw, "mx.kubuno.local; spf=fail");
        let text = String::from_utf8_lossy(&out);
        assert!(text.starts_with("Authentication-Results: mx.kubuno.local; spf=fail\r\n"));
        assert!(!text.contains("dmarc=pass"), "l'en-tête forgé doit disparaître");
        assert!(!text.contains("forged continuation"), "sa ligne repliée aussi");
        assert!(text.contains("From: a@b.example"), "le reste des en-têtes est intact");
        // The body is never touched, even when it looks like a header.
        assert!(text.contains("Authentication-Results: this one is body text"));
    }

    /// A near-miss field name is a different header and must survive.
    #[test]
    fn stripping_a_header_matches_the_whole_field_name() {
        assert!(header_name_is("Authentication-Results: x", "Authentication-Results"));
        assert!(header_name_is("authentication-results : x", "Authentication-Results"));
        assert!(!header_name_is("Authentication-Results-Original: x", "Authentication-Results"));
        assert!(!header_name_is("X-Authentication-Results: x", "Authentication-Results"));
        assert!(!header_name_is("Auth", "Authentication-Results"));
    }

    /// Same reasoning for our own marker: the sender does not get to write it.
    #[test]
    fn a_forged_policy_marker_is_replaced_by_ours() {
        let raw = b"X-Kubuno-Auth-Policy: mark; spf=pass; dkim=pass; dmarc=pass\r\n\
                    From: a@b.example\r\n\r\nbody";
        let out = policy_marker(raw, PolicyAction::Mark, &verdict(Some("fail"), None, None));
        let text = String::from_utf8_lossy(&out);
        assert_eq!(text.matches("X-Kubuno-Auth-Policy").count(), 1);
        assert!(text.starts_with("X-Kubuno-Auth-Policy: mark; spf=fail;"));
    }

    #[test]
    fn detects_present_and_absent_headers() {
        let msg = b"From: a@b.example\r\nMessage-ID: <x@h>\r\nSubject: hi\r\n\r\nbody\r\n";
        assert!(has_header(msg, b"Message-ID"));
        assert!(has_header(msg, b"message-id"), "insensible à la casse");
        assert!(has_header(msg, b"Subject"));
        assert!(!has_header(msg, b"Date"));
        // A body line that merely looks like a header must not count: the scan
        // stops at the blank line.
        let tricky = b"From: a@b.example\r\n\r\nDate: not a header, this is the body\r\n";
        assert!(!has_header(tricky, b"Date"));
        // A folded continuation carrying "Date:" is part of the previous value.
        let folded = b"Subject: about the\r\n Date: of the meeting\r\n\r\nbody";
        assert!(!has_header(folded, b"Date"));
        // Tolerate a stray space before the colon.
        assert!(has_header(b"Message-ID : <x@h>\r\n\r\nbody", b"Message-ID"));
    }

    #[test]
    fn submission_headers_are_added_only_when_missing() {
        let bare = b"From: a@b.example\r\nSubject: hi\r\n\r\nbody\r\n";
        let out = add_submission_headers(bare, "mail.kubuno.local");
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("Message-ID: <"));
        assert!(text.contains("@mail.kubuno.local>"));
        assert!(text.starts_with("Date: "), "les en-têtes ajoutés précèdent le bloc");
        assert!(text.contains("From: a@b.example"), "l'original est conservé intact");
        assert!(text.ends_with("body\r\n"));

        // Both already present: the message is returned untouched.
        let complete = b"Date: Wed, 01 Jan 2025 00:00:00 +0000\r\nMessage-ID: <keep@me>\r\n\r\nbody";
        let out = add_submission_headers(complete, "mail.kubuno.local");
        assert_eq!(out, complete, "aucune réécriture quand tout est présent");
    }

    // ── A submission session over a real socket ─────────────────────────────

    #[tokio::test]
    async fn submission_requires_auth_and_refuses_cleartext_auth() {
        let cfg = ServerConfig {
            hostname: "mail.kubuno.test".into(),
            domains: vec!["kubuno.test".into()],
            ..ServerConfig::default()
        };
        let (mut reader, mut writer) = connected_role(cfg, true).await;
        expect_line(&mut reader, "220 ").await;

        writer.write_all(b"EHLO client.example\r\n").await.expect("EHLO");
        expect_line(&mut reader, "250-mail.kubuno.test").await;
        expect_line(&mut reader, "250-SIZE").await;
        expect_line(&mut reader, "250-8BITMIME").await;
        // Écouteur de soumission en clair : ni STARTTLS ni AUTH annoncés.
        expect_line(&mut reader, "250 HELP").await;

        // L'enveloppe est refusée tant que la session n'est pas authentifiée.
        writer.write_all(b"MAIL FROM:<alice@kubuno.test>\r\n").await.expect("MAIL");
        expect_line(&mut reader, "530 5.7.0 Authentication required").await;
        writer.write_all(b"RCPT TO:<bob@kubuno.test>\r\n").await.expect("RCPT");
        expect_line(&mut reader, "530 5.7.0").await;
        writer.write_all(b"DATA\r\n").await.expect("DATA");
        expect_line(&mut reader, "530 5.7.0").await;

        // AUTH en clair sur soumission : chiffrement requis d'abord.
        writer.write_all(b"AUTH LOGIN\r\n").await.expect("AUTH");
        expect_line(&mut reader, "538 5.7.11").await;

        // Idem pour SCRAM : le mécanisme fort ne dispense pas du chiffrement.
        writer.write_all(b"AUTH SCRAM-SHA-256\r\n").await.expect("AUTH");
        expect_line(&mut reader, "538 5.7.11").await;

        writer.write_all(b"QUIT\r\n").await.expect("QUIT");
        expect_line(&mut reader, "221 ").await;
    }

    // ── A full SCRAM-SHA-256 exchange over a real socket ─────────────────────
    //
    // On a reception listener AUTH needs no encryption, so the wire flow
    // (334 empty challenge → 334 server-first → 535) can be exercised without a
    // certificate. The database is unreachable, so the username is unknown and a
    // decoy secret is used: the exchange proceeds exactly as a real one and
    // fails at the proof, proving the failure is indistinguishable and that the
    // base64 framing at each step is driven correctly.
    #[tokio::test]
    async fn a_scram_exchange_runs_its_full_wire_flow_and_fails_closed() {
        let cfg = ServerConfig {
            hostname: "mail.kubuno.test".into(),
            domains: vec!["kubuno.test".into()],
            ..ServerConfig::default()
        };
        let (mut reader, mut writer) = connected(cfg).await;
        expect_line(&mut reader, "220 ").await;

        writer.write_all(b"EHLO client.example\r\n").await.expect("EHLO");
        expect_line(&mut reader, "250-mail.kubuno.test").await;
        expect_line(&mut reader, "250-SIZE").await;
        expect_line(&mut reader, "250-8BITMIME").await;
        expect_line(&mut reader, "250 HELP").await;

        // AUTH without an initial response: the server challenges with `334 `.
        writer.write_all(b"AUTH SCRAM-SHA-256\r\n").await.expect("AUTH");
        expect_line(&mut reader, "334 ").await;

        // client-first, base64-wrapped.
        let client_first = base64::engine::general_purpose::STANDARD
            .encode(b"n,,n=ghost@kubuno.test,r=clientNONCE");
        writer.write_all(format!("{client_first}\r\n").as_bytes()).await.expect("client-first");

        // server-first arrives base64 inside a `334`; it must decode to `r=..,s=..,i=..`.
        let line = expect_line(&mut reader, "334 ").await;
        let payload = line.trim_start_matches("334 ").trim();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .expect("server-first base64");
        let server_first = String::from_utf8(decoded).expect("server-first utf8");
        assert!(server_first.starts_with("r=clientNONCE"), "le nonce client est réverbéré");
        assert!(server_first.contains(",s="));
        assert!(server_first.contains(",i="));

        // client-final with a bogus proof: an unknown user never verifies.
        let client_final = base64::engine::general_purpose::STANDARD
            .encode(b"c=biws,r=clientNONCE,p=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        writer.write_all(format!("{client_final}\r\n").as_bytes()).await.expect("client-final");
        expect_line(&mut reader, "535 5.7.8").await;

        writer.write_all(b"QUIT\r\n").await.expect("QUIT");
        expect_line(&mut reader, "221 ").await;
    }

    #[tokio::test]
    async fn a_scram_exchange_can_be_cancelled_with_a_star() {
        let cfg = ServerConfig {
            hostname: "mail.kubuno.test".into(),
            domains: vec!["kubuno.test".into()],
            ..ServerConfig::default()
        };
        let (mut reader, mut writer) = connected(cfg).await;
        expect_line(&mut reader, "220 ").await;
        writer.write_all(b"EHLO client.example\r\n").await.expect("EHLO");
        expect_line(&mut reader, "250-mail.kubuno.test").await;
        expect_line(&mut reader, "250-SIZE").await;
        expect_line(&mut reader, "250-8BITMIME").await;
        expect_line(&mut reader, "250 HELP").await;

        writer.write_all(b"AUTH SCRAM-SHA-256\r\n").await.expect("AUTH");
        expect_line(&mut reader, "334 ").await;
        // `*` cancels the SASL exchange (RFC 4954 §4).
        writer.write_all(b"*\r\n").await.expect("cancel");
        expect_line(&mut reader, "501 5.7.0 Authentication cancelled").await;

        writer.write_all(b"QUIT\r\n").await.expect("QUIT");
        expect_line(&mut reader, "221 ").await;
    }

    #[tokio::test]
    async fn an_overlong_command_line_is_refused_without_desynchronising() {
        let cfg = ServerConfig {
            hostname: "mail.kubuno.test".into(),
            domains: vec!["kubuno.test".into()],
            ..ServerConfig::default()
        };
        let (mut reader, mut writer) = connected(cfg).await;
        expect_line(&mut reader, "220 ").await;

        let mut flood = b"EHLO ".to_vec();
        flood.extend(std::iter::repeat_n(b'x', 4000));
        flood.extend_from_slice(b"\r\n");
        writer.write_all(&flood).await.expect("ligne trop longue");
        expect_line(&mut reader, "500 5.5.2 Line too long").await;

        // La commande suivante doit être lue comme une commande, pas comme la
        // queue de la précédente.
        writer.write_all(b"NOOP\r\n").await.expect("NOOP");
        expect_line(&mut reader, "250 2.0.0").await;
        writer.write_all(b"QUIT\r\n").await.expect("QUIT");
        expect_line(&mut reader, "221 ").await;
    }
}
