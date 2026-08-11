//! The IMAP4rev1 service this module offers (RFC 3501).
//!
//! It serves the module's own store, not an upstream account: the five folders
//! of `store::FOLDERS`, with `local_uid` as the IMAP UID — that column exists
//! precisely so a mail client can keep a stable handle on a message across
//! sessions.
//!
//! Scope is "enough that a real client can read its mailbox, manage flags, move
//! and copy messages, upload drafts, and be pushed changes as they happen": IDLE
//! (RFC 2177), MOVE (RFC 6851), COPY/APPEND with UIDPLUS (RFC 4315), ENABLE
//! (RFC 5161), CONDSTORE/QRESYNC (RFC 7162) and non-synchronising literals
//! (LITERAL+, RFC 7888) are supported; arbitrary folders are not. Anything
//! outside that is answered with NO or BAD, never by hanging up: a client that
//! gets a clean refusal degrades gracefully, a client whose socket disappears
//! retries forever.
//!
//! Every read is bounded (line length, literal size, idle time) because a
//! listener open to the network is a listener open to a client that says
//! nothing, or says far too much.

use std::{collections::HashSet, net::IpAddr, sync::Arc, time::Duration};

use anyhow::{bail, Context, Result};
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    time::timeout,
};
use tokio_rustls::TlsAcceptor;
use uuid::Uuid;

use crate::server::{
    auth::{self, Mailbox},
    config::ServerConfig,
    limits::Tarpit,
    store::{self, StoredMessage},
    tls::{upgrade, MailStream, TlsMode, UpgradeError},
};
use crate::services::pop_imap::{self, ImapExpungeMode, ImapPurgeMode, PopImapSettings};

// The grammar lives beside this file so it can be exercised without a socket.
// The path is spelled out because `imap.rs` is not a `mod.rs`: a bare
// `mod imap_parse;` would be looked for under `server/imap/`.
#[path = "imap_parse.rs"]
mod imap_parse;

use imap_parse::{
    compute_resync, decode_plain, display_name, fetch_response, flags_of, is_modifier_group,
    matches_criteria, parse_fetch_modifiers, parse_qresync, parse_sequence_set, parse_unchangedsince,
    seq_contains, special_use, split_fetch_items, tokenize_line, uid_set_string, vanished_earlier_line,
    FlagMode, Token,
};

/// Longest command line accepted. Most commands are tens of bytes, but a
/// `UID FETCH` over a few thousand UIDs is a single long line — Dovecot's
/// `imap_max_line_length` is 64 KiB for exactly that reason, and 8 KiB
/// disconnected clients doing legitimate bulk work.
const MAX_LINE: u64 = 64 * 1024;
/// Longest literal accepted. APPEND uploads a whole message as a literal, so
/// this must not undercut the size a client is allowed to send: capping it at
/// 1 MiB while `server_max_message_mb` allowed 25 made every larger APPEND
/// fail. Kept a little above the 25 MiB default so the two never disagree.
const MAX_LITERAL: usize = 32 * 1024 * 1024;
/// How long a single IDLE may last before the server ends it. RFC 2177 lets a
/// server drop an IDLE after 30 minutes of inactivity; ending just under that
/// keeps the session honest without surprising a well-behaved client.
const IDLE_MAX: Duration = Duration::from_secs(29 * 60);
/// How often IDLE emits an untagged keepalive, to hold NAT mappings and dead
/// connections open long enough to notice they died.
/// Dovecot sends one every 2 minutes; NAT mappings commonly expire between 2
/// and 5, so 4 minutes was on the wrong side of that window.
const IDLE_KEEPALIVE: Duration = Duration::from_secs(2 * 60);
/// Flags this store can actually represent.
const FLAGS: &str = "\\Seen \\Flagged \\Deleted";

/// The single buffered stream a session reads commands from and writes replies
/// to. One stream, not a split read/write pair, because STARTTLS upgrades the
/// connection in place and a split cannot be reassembled around the new TLS
/// layer.
type Reader = BufReader<MailStream>;

/// Builds the advertised capability list for the connection's current state.
///
/// `STARTTLS` (and, per RFC 2595, `LOGINDISABLED` to steer conformant clients
/// away from sending credentials in the clear) is offered only on a plaintext
/// connection of a STARTTLS listener that actually holds a certificate. Once the
/// connection is encrypted — an implicit-TLS port, or after a successful
/// STARTTLS — neither is advertised and `AUTH=PLAIN` stands on its own.
fn capability_line(tls_mode: TlsMode, has_acceptor: bool, tls: bool) -> String {
    // The modern extensions are advertised unconditionally: they are stateless
    // for the client to discover and cost nothing to a client that ignores them.
    let mut caps =
        String::from("IMAP4rev1 LITERAL+ ENABLE IDLE MOVE UIDPLUS CONDSTORE QRESYNC");
    let upgradable = tls_mode == TlsMode::StartTls && has_acceptor && !tls;
    if upgradable {
        caps.push_str(" STARTTLS LOGINDISABLED");
    }
    // SCRAM-SHA-256 never sends the password, so it is safe on any channel.
    caps.push_str(" AUTH=SCRAM-SHA-256");
    // PLAIN sends the password: advertise it only where it cannot be read off
    // the wire. Announcing LOGINDISABLED while still offering AUTH=PLAIN in the
    // clear was contradictory — a client that trusts the advertisement would
    // hand over the password anyway.
    if !upgradable {
        caps.push_str(" AUTH=PLAIN");
    }
    caps
}

/// The mailbox a client has SELECTed, as it looked when it did.
///
/// Sequence numbers are positions in this snapshot; IMAP requires them to stay
/// put for the whole session, so the list is only reloaded at points where the
/// protocol allows the view to change (NOOP, EXPUNGE, a new SELECT).
struct Selected {
    folder:    &'static str,
    messages:  Vec<StoredMessage>,
    /// `\Deleted` is session state until EXPUNGE: marking is not deleting, and
    /// a client that marks then disconnects must not lose mail.
    deleted:   HashSet<Uuid>,
    read_only: bool,
}

struct Session {
    db:       PgPool,
    cfg:      Arc<ServerConfig>,
    /// One buffered stream for both directions. `None` only for the instant a
    /// STARTTLS upgrade owns it, which returns before any further I/O.
    conn:     Option<Reader>,
    /// Present when this listener can upgrade to TLS (a certificate is loaded).
    acceptor: Option<Arc<TlsAcceptor>>,
    /// The listener's TLS mode, fixed for the whole connection.
    tls_mode: TlsMode,
    /// Whether the connection is encrypted right now (implicit TLS, or upgraded).
    tls:      bool,
    peer:     String,
    /// Shared per-IP auth tarpit and this client's IP (Dovecot auth-penalty).
    tarpit:   Arc<Tarpit>,
    peer_ip:  IpAddr,
    mailbox:  Option<Mailbox>,
    /// This mailbox's POP/IMAP policy, read once at login. Governs whether the
    /// session is allowed at all, the per-folder message cap, and how `\Deleted`
    /// / EXPUNGE are committed. Meaningful only once `mailbox` is set.
    settings: PopImapSettings,
    selected: Option<Selected>,
    commands: i32,
    /// CONDSTORE (RFC 7162) is on for this session: once enabled — by `ENABLE
    /// CONDSTORE/QRESYNC`, or a `SELECT (CONDSTORE)`/`(QRESYNC ...)` — every FETCH
    /// response carries a MODSEQ item for the rest of the session.
    condstore: bool,
    /// QRESYNC (RFC 7162) is on: the client asked for `ENABLE QRESYNC`, which is
    /// what unlocks VANISHED responses (VANISHED is only sent to clients that
    /// enabled it). Implies CONDSTORE.
    qresync:   bool,
}

pub async fn handle(
    inc: crate::server::Incoming,
    stream: crate::server::tls::MailStream,
) -> anyhow::Result<()> {
    // `submission` is an SMTP concern; IMAP ignores it.
    let peer_ip = inc.peer_ip();
    let crate::server::Incoming { db, cfg, peer, tls_mode, acceptor, tarpit, .. } = inc;
    let tls = stream.is_tls();

    let mut session = Session {
        db: db.clone(),
        cfg,
        conn: Some(BufReader::new(stream)),
        acceptor,
        tls_mode,
        tls,
        peer: peer.clone(),
        tarpit,
        peer_ip,
        mailbox: None,
        settings: PopImapSettings::default(),
        selected: None,
        commands: 0,
        condstore: false,
        qresync: false,
    };

    let outcome = session.run().await;
    let error = outcome.as_ref().err().map(std::string::ToString::to_string);
    super::log_session(&db, "imap", &peer, session.mailbox.as_ref(), session.commands, error).await;
    outcome
}

impl Session {
    async fn run(&mut self) -> Result<()> {
        let greeting = format!(
            "* OK [CAPABILITY {}] {} Kubuno IMAP ready\r\n",
            self.capability(),
            self.cfg.hostname
        );
        self.write(&greeting).await?;

        loop {
            let Some(tokens) = self.read_command().await? else { return Ok(()) };
            if tokens.is_empty() {
                continue;
            }
            self.commands += 1;

            let tag = tokens[0].text.clone();
            let Some(name) = tokens.get(1).map(Token::upper) else {
                self.write(&format!("{tag} BAD Missing command\r\n")).await?;
                continue;
            };

            if !self.dispatch(&tag, &name, &tokens).await? {
                return Ok(());
            }
        }
    }

    /// Runs one command. `false` means the session is over (LOGOUT).
    async fn dispatch(&mut self, tag: &str, name: &str, tokens: &[Token]) -> Result<bool> {
        match name {
            "LOGOUT" => {
                self.write("* BYE Kubuno IMAP signing off\r\n").await?;
                self.write(&format!("{tag} OK LOGOUT completed\r\n")).await?;
                return Ok(false);
            }
            "CAPABILITY" => {
                self.write(&format!("* CAPABILITY {}\r\n", self.capability())).await?;
                self.ok(tag, "CAPABILITY completed").await?;
            }
            // A successful upgrade continues the loop in the new TLS session; a
            // failure (injection attempt or broken handshake) ends it.
            "STARTTLS" => return self.cmd_starttls(tag).await,
            "NOOP" | "CHECK" => self.cmd_noop(tag, name).await?,
            "LOGIN" => self.cmd_login(tag, tokens).await?,
            "AUTHENTICATE" => self.cmd_authenticate(tag, tokens).await?,
            "LIST" | "LSUB" => self.cmd_list(tag, name).await?,
            "SELECT" | "EXAMINE" => self.cmd_select(tag, tokens, name == "EXAMINE").await?,
            "STATUS" => self.cmd_status(tag, tokens).await?,
            "FETCH" => self.cmd_fetch(tag, tokens, 2, false).await?,
            "STORE" => self.cmd_store(tag, tokens, 2, false).await?,
            "SEARCH" => self.cmd_search(tag, tokens, 2, false).await?,
            "EXPUNGE" => self.cmd_expunge(tag, true).await?,
            "CLOSE" => self.cmd_close(tag).await?,
            "MOVE" => self.cmd_move(tag, tokens, 2, false).await?,
            "COPY" => self.cmd_copy(tag, tokens, 2, false).await?,
            "UID" => self.cmd_uid(tag, tokens).await?,
            "ENABLE" => self.cmd_enable(tag, tokens).await?,
            // IDLE may end the session if the client disappears mid-idle.
            "IDLE" => return self.cmd_idle(tag).await,
            "SUBSCRIBE" | "UNSUBSCRIBE" => self.cmd_subscribe(tag, tokens).await?,
            "APPEND" => self.cmd_append(tag, tokens).await?,
            // Known commands this server refuses: the folder set is fixed and
            // clients cannot make their own folders.
            "CREATE" | "DELETE" | "RENAME" => {
                self.no(tag, &format!("{name} is not supported by this server")).await?;
            }
            _ => self.write(&format!("{tag} BAD Unknown command\r\n")).await?,
        }
        Ok(true)
    }

    /// `UID FETCH/STORE/SEARCH`: same commands, sequence sets read as UIDs.
    async fn cmd_uid(&mut self, tag: &str, tokens: &[Token]) -> Result<()> {
        let Some(sub) = tokens.get(2).map(Token::upper) else {
            self.write(&format!("{tag} BAD Missing UID subcommand\r\n")).await?;
            return Ok(());
        };
        match sub.as_str() {
            "FETCH" => self.cmd_fetch(tag, tokens, 3, true).await,
            "STORE" => self.cmd_store(tag, tokens, 3, true).await,
            "SEARCH" => self.cmd_search(tag, tokens, 3, true).await,
            "EXPUNGE" => self.cmd_expunge(tag, true).await,
            "MOVE" => self.cmd_move(tag, tokens, 3, true).await,
            "COPY" => self.cmd_copy(tag, tokens, 3, true).await,
            _ => {
                self.write(&format!("{tag} BAD Unknown UID subcommand\r\n")).await?;
                Ok(())
            }
        }
    }

    // ── Authentication ───────────────────────────────────────────────────────

    async fn cmd_login(&mut self, tag: &str, tokens: &[Token]) -> Result<()> {
        // LOGIN carries the password in the clear; honour LOGINDISABLED rather
        // than merely advertising it.
        if self.cleartext_login_refused() {
            return self.no(tag, "LOGIN disabled on a cleartext connection, use STARTTLS").await;
        }
        let (Some(user), Some(password)) = (tokens.get(2), tokens.get(3)) else {
            self.write(&format!("{tag} BAD LOGIN requires a user and a password\r\n")).await?;
            return Ok(());
        };
        self.finish_login(tag, &user.text, &password.text).await
    }

    async fn cmd_authenticate(&mut self, tag: &str, tokens: &[Token]) -> Result<()> {
        let Some(mechanism) = tokens.get(2).map(Token::upper) else {
            self.write(&format!("{tag} BAD AUTHENTICATE requires a mechanism\r\n")).await?;
            return Ok(());
        };
        match mechanism.as_str() {
            // Refusing, not just hiding: a client that ignores LOGINDISABLED and
            // asks for PLAIN in the clear must not be allowed to send the
            // password anyway. SCRAM is offered instead, on any channel.
            "PLAIN" if self.cleartext_login_refused() => {
                self.no(tag, "Encryption required for this authentication mechanism").await
            }
            "PLAIN" => self.cmd_authenticate_plain(tag, tokens).await,
            "SCRAM-SHA-256" => self.cmd_authenticate_scram(tag, tokens).await,
            _ => self.no(tag, "Unsupported authentication mechanism").await,
        }
    }

    /// True when credentials must not travel in the clear: the channel is
    /// unencrypted AND this listener could have upgraded it (STARTTLS with a
    /// certificate). A plaintext-only listener — a LAN deployment with no
    /// certificate — keeps working, since refusing there would lock everyone out.
    fn cleartext_login_refused(&self) -> bool {
        !self.tls && self.tls_mode == TlsMode::StartTls && self.acceptor.is_some()
    }

    async fn cmd_authenticate_plain(&mut self, tag: &str, tokens: &[Token]) -> Result<()> {
        // The payload may ride along on the command line (SASL-IR) or follow a
        // continuation; both are accepted.
        let payload = match tokens.get(3) {
            Some(inline) => inline.text.clone(),
            None => {
                self.write("+ \r\n").await?;
                match self.read_line().await? {
                    Some(line) => line,
                    None => return Ok(()),
                }
            }
        };

        if payload.trim() == "*" {
            self.write(&format!("{tag} BAD Authentication cancelled\r\n")).await?;
            return Ok(());
        }

        let Some((user, password)) = decode_plain(&payload) else {
            self.no(tag, "Invalid credentials").await?;
            return Ok(());
        };
        self.finish_login(tag, &user, &password).await
    }

    /// SASL SCRAM-SHA-256 (RFC 5802). The password is never sent; the client
    /// proves it knows it against the salted keys stored at credential creation.
    async fn cmd_authenticate_scram(&mut self, tag: &str, tokens: &[Token]) -> Result<()> {
        use base64::{engine::general_purpose::STANDARD, Engine};

        // client-first: SASL-IR on the line, or a continuation.
        let client_first_b64 = match tokens.get(3) {
            Some(inline) => inline.text.clone(),
            None => {
                self.write("+ \r\n").await?;
                match self.read_line().await? {
                    Some(l) => l,
                    None => return Ok(()),
                }
            }
        };
        if client_first_b64.trim() == "*" {
            return self.bad(tag, "Authentication cancelled").await;
        }
        let Ok(client_first) = STANDARD.decode(client_first_b64.trim()).map(|b| String::from_utf8_lossy(&b).into_owned()) else {
            return self.bad(tag, "Invalid base64").await;
        };

        // Look up the user's secret; an unknown user gets a decoy so the failure
        // is indistinguishable from a wrong password.
        let username = match crate::server::scram::Handshake::username_of(&client_first) {
            Ok(u) => u,
            Err(_) => return self.bad(tag, "Malformed SCRAM message").await,
        };
        let (secret, known) = match auth::scram_secret(&self.db, &username).await {
            Some(s) => (s, true),
            None => (crate::server::scram::decoy_secret(), false),
        };

        let mut handshake = match crate::server::scram::Handshake::new(secret, known, &client_first) {
            Ok(h) => h,
            Err(_) => return self.bad(tag, "Malformed SCRAM message").await,
        };
        let server_first = match handshake.server_first() {
            Ok(m) => m,
            Err(_) => return self.bad(tag, "Malformed SCRAM message").await,
        };

        // Challenge with server-first, read client-final.
        self.write(&format!("+ {}\r\n", STANDARD.encode(server_first.as_bytes()))).await?;
        let client_final_b64 = match self.read_line().await? {
            Some(l) => l,
            None => return Ok(()),
        };
        if client_final_b64.trim() == "*" {
            return self.bad(tag, "Authentication cancelled").await;
        }
        let Ok(client_final) = STANDARD.decode(client_final_b64.trim()).map(|b| String::from_utf8_lossy(&b).into_owned()) else {
            return self.bad(tag, "Invalid base64").await;
        };

        match handshake.server_final(&client_final) {
            Ok(server_final) => {
                // Send server-final, read (and ignore) the client's empty ack.
                self.write(&format!("+ {}\r\n", STANDARD.encode(server_final.as_bytes()))).await?;
                let _ = self.read_line().await?;
                match auth::mailbox_of(&self.db, &username).await {
                    Some(mailbox) => {
                        let username = mailbox.username.clone();
                        self.tarpit.record_success(self.peer_ip);
                        if !self.admit_imap(tag, mailbox).await? {
                            return Ok(());
                        }
                        tracing::info!(user = %username, "Session IMAP authentifiée (SCRAM)");
                        self.write(&format!(
                            "{tag} OK [CAPABILITY {}] Authentication successful\r\n",
                            self.capability()
                        ))
                        .await
                    }
                    None => self.no(tag, "Internal error").await,
                }
            }
            Err(_) => {
                tracing::warn!(user = %username, "Échec d'authentification IMAP (SCRAM)");
                self.tarpit.record_failure(self.peer_ip);
                tokio::time::sleep(self.tarpit.delay_for(self.peer_ip)).await;
                self.no(tag, "Invalid credentials").await
            }
        }
    }

    /// Loads the mailbox's policy and, if IMAP is disabled for it, refuses the
    /// session with a tagged `NO [ALERT]` (Gmail's wording) — the auth WAS valid,
    /// so no tarpit — leaving the session unauthenticated. On success the policy
    /// is stashed and the mailbox becomes the session's, and the caller sends its
    /// own OK. Returns `true` when the login may proceed.
    ///
    /// A settings-read failure falls back to the compatibility default (IMAP
    /// open) so a bad row never locks a client out.
    async fn admit_imap(&mut self, tag: &str, mailbox: Mailbox) -> Result<bool> {
        let settings = pop_imap::load(&self.db, mailbox.user_id).await.unwrap_or_else(|e| {
            tracing::error!(error = %e, "IMAP : lecture de la politique POP/IMAP");
            PopImapSettings::default()
        });
        if !settings.imap_enabled {
            tracing::info!(user = %mailbox.username, "IMAP refusé : accès IMAP désactivé");
            self.no(tag, "[ALERT] IMAP access is disabled for this mailbox").await?;
            return Ok(false);
        }
        self.settings = settings;
        self.mailbox = Some(mailbox);
        Ok(true)
    }

    async fn finish_login(&mut self, tag: &str, user: &str, password: &str) -> Result<()> {
        match auth::authenticate(&self.db, user, password).await {
            Some(mailbox) => {
                let username = mailbox.username.clone();
                self.tarpit.record_success(self.peer_ip);
                if !self.admit_imap(tag, mailbox).await? {
                    return Ok(());
                }
                tracing::info!(user = %username, "Session IMAP authentifiée");
                self.write(&format!("{tag} OK [CAPABILITY {}] LOGIN completed\r\n", self.capability()))
                    .await
            }
            None => {
                // One message for both "no such mailbox" and "wrong password":
                // the difference is what turns a login prompt into an address
                // enumeration oracle. The reply is held back by the growing
                // tarpit delay to throttle brute force (Dovecot auth-penalty).
                tracing::warn!(user = %user, "Échec d'authentification IMAP");
                self.tarpit.record_failure(self.peer_ip);
                tokio::time::sleep(self.tarpit.delay_for(self.peer_ip)).await;
                self.no(tag, "Invalid credentials").await
            }
        }
    }

    // ── TLS ──────────────────────────────────────────────────────────────────

    fn capability(&self) -> String {
        capability_line(self.tls_mode, self.acceptor.is_some(), self.tls)
    }

    /// Handles `STARTTLS` (RFC 2595). Returns `Ok(false)` when the session must
    /// end: a pipelined-command injection attempt and a failed handshake both
    /// leave the connection unusable, so it is dropped rather than continued.
    async fn cmd_starttls(&mut self, tag: &str) -> Result<bool> {
        if self.tls {
            self.write(&format!("{tag} BAD TLS already active\r\n")).await?;
            return Ok(true);
        }
        let Some(acceptor) = self.acceptor.clone() else {
            self.write(&format!("{tag} BAD STARTTLS not available\r\n")).await?;
            return Ok(true);
        };

        // The client must see this reply, in the clear and flushed, before the
        // handshake begins.
        self.write(&format!("{tag} OK Begin TLS negotiation now\r\n")).await?;

        let Some(conn) = self.conn.take() else { return Ok(false) };
        match upgrade(conn, &acceptor).await {
            Ok(secured) => {
                self.conn = Some(secured);
                self.tls = true;
                // A STARTTLS upgrade returns the session to the Non-authenticated
                // state: everything learned in the clear (the logged-in mailbox,
                // any selected folder) is discarded so a network attacker cannot
                // have it carried into the tunnel.
                self.mailbox = None;
                self.selected = None;
                self.condstore = false;
                self.qresync = false;
                Ok(true)
            }
            Err(UpgradeError::Pipelined) => {
                tracing::warn!(peer = %self.peer,
                    "STARTTLS IMAP : octets envoyés avant le handshake — connexion fermée");
                Ok(false)
            }
            Err(UpgradeError::Handshake(e)) => {
                tracing::debug!(peer = %self.peer, error = %e, "Handshake STARTTLS IMAP échoué");
                Ok(false)
            }
        }
    }

    // ── Mailboxes ────────────────────────────────────────────────────────────

    async fn cmd_list(&mut self, tag: &str, name: &str) -> Result<()> {
        if self.mailbox.is_none() {
            return self.no(tag, "Please log in first").await;
        }
        for folder in store::FOLDERS {
            let attributes = if folder == "INBOX" { "" } else { special_use(folder) };
            self.write(&format!("* {name} ({attributes}) \"/\" \"{folder}\"\r\n")).await?;
        }
        self.ok(tag, &format!("{name} completed")).await
    }

    async fn cmd_subscribe(&mut self, tag: &str, tokens: &[Token]) -> Result<()> {
        if self.mailbox.is_none() {
            return self.no(tag, "Please log in first").await;
        }
        // The folder set is fixed, so subscription has nothing to store; saying
        // OK for a folder that exists keeps clients that subscribe on first run
        // from showing an error the user cannot act on.
        match tokens.get(2).and_then(|t| store::folder_of(&t.text)) {
            Some(_) => self.ok(tag, "SUBSCRIBE completed").await,
            None => self.no(tag, "Mailbox does not exist").await,
        }
    }

    // ── APPEND (RFC 3501, APPENDUID RFC 4315) ────────────────────────────────

    /// `APPEND <mailbox> [(<flags>)] [<date-time>] {<literal>}`: file a
    /// client-supplied message into a folder. Used mostly to keep a local copy of
    /// a draft or a sent message. The literal has already been read by
    /// `read_command`, so it is simply the final argument here.
    async fn cmd_append(&mut self, tag: &str, tokens: &[Token]) -> Result<()> {
        let Some(user_id) = self.mailbox.as_ref().map(|m| m.user_id) else {
            return self.no(tag, "Please log in first").await;
        };
        // Minimal form is `<tag> APPEND <mailbox> {<literal>}` — four tokens.
        let (Some(mailbox), true) = (tokens.get(2), tokens.len() >= 4) else {
            self.write(&format!("{tag} BAD APPEND requires a mailbox and a message\r\n")).await?;
            return Ok(());
        };
        // The message literal is always the last argument; the optional flag list
        // and date-time sit between the mailbox and it.
        let message = &tokens[tokens.len() - 1];
        let flag_group = tokens[3..tokens.len() - 1].iter().find(|t| t.text.starts_with('('));
        let (seen, flagged) = match flag_group {
            Some(group) => {
                let lower = group.text.to_ascii_lowercase();
                (lower.contains("\\seen"), lower.contains("\\flagged"))
            }
            None => (false, false),
        };

        let Some(folder) = store::folder_of(&mailbox.text) else {
            // UIDPLUS TRYCREATE: the upload would work if the mailbox existed, but
            // the folder set is fixed and the client cannot create one.
            return self.no(tag, "[TRYCREATE] Mailbox does not exist").await;
        };

        match store::append(&self.db, user_id, folder, message.text.as_bytes(), seen, flagged).await {
            Ok(local_uid) => {
                // If the destination is the selected folder, the message becomes
                // visible on the next poll; APPENDUID lets the client find it now.
                let uidvalidity = store::uidvalidity(folder);
                self.ok(tag, &format!("[APPENDUID {uidvalidity} {local_uid}] APPEND completed")).await
            }
            Err(e) => {
                tracing::error!(error = %e, "APPEND IMAP impossible");
                self.no(tag, "Internal error appending the message").await
            }
        }
    }

    async fn cmd_select(&mut self, tag: &str, tokens: &[Token], read_only: bool) -> Result<()> {
        if self.mailbox.is_none() {
            return self.no(tag, "Please log in first").await;
        }
        let Some(requested) = tokens.get(2) else {
            self.write(&format!("{tag} BAD SELECT requires a mailbox\r\n")).await?;
            return Ok(());
        };
        let Some(folder) = store::folder_of(&requested.text) else {
            // Selecting the wrong mailbox must leave the previous one selected
            // per RFC 3501; it does not here — the client is told plainly.
            self.selected = None;
            return self.no(tag, "[NONEXISTENT] Mailbox does not exist").await;
        };
        let Some(user_id) = self.mailbox.as_ref().map(|m| m.user_id) else {
            return self.no(tag, "Please log in first").await;
        };

        // Optional CONDSTORE/QRESYNC select parameters (RFC 7162): `(CONDSTORE)`
        // turns MODSEQ on for the session; `(QRESYNC (<uidvalidity> <modseq>
        // [<uids>]))` additionally asks for a flag/vanished catch-up. Presence of
        // either enables the extension for the rest of the session.
        let param = tokens.get(3).map(|t| t.text.clone()).unwrap_or_default();
        let qresync = parse_qresync(&param);
        if qresync.is_some() {
            self.condstore = true;
            self.qresync = true;
        } else if param.to_ascii_uppercase().contains("CONDSTORE") {
            self.condstore = true;
        }

        let messages = match self.load(folder).await {
            Ok(messages) => messages,
            Err(()) => return self.no(tag, "Internal error reading the mailbox").await,
        };

        let exists = messages.len();
        let uidnext = messages.last().map(|m| m.local_uid + 1).unwrap_or(1);
        let first_unseen = messages.iter().position(|m| !m.is_read).map(|i| i + 1);

        self.write(&format!("* {exists} EXISTS\r\n")).await?;
        // Nothing is ever \Recent: the store has no notion of "arrived since
        // the last session", and claiming otherwise makes clients chirp.
        self.write("* 0 RECENT\r\n").await?;
        self.write(&format!("* FLAGS ({FLAGS})\r\n")).await?;
        self.write(&format!(
            "* OK [UIDVALIDITY {}] UIDs are stable\r\n",
            store::uidvalidity(folder)
        ))
        .await?;
        self.write(&format!("* OK [UIDNEXT {uidnext}] Predicted next UID\r\n")).await?;
        if let Some(seq) = first_unseen {
            self.write(&format!("* OK [UNSEEN {seq}] First unseen message\r\n")).await?;
        }
        let permanent = if read_only { String::new() } else { format!("{FLAGS} ") };
        self.write(&format!("* OK [PERMANENTFLAGS ({permanent}\\*)] Limited by the store\r\n"))
            .await?;

        // CONDSTORE: announce the folder's HIGHESTMODSEQ so the client can sync
        // incrementally from this point. A read error must not fail the SELECT —
        // it is logged and the line simply omitted.
        if self.condstore {
            match store::highest_modseq(&self.db, user_id, folder).await {
                Ok(hm) => {
                    self.write(&format!("* OK [HIGHESTMODSEQ {hm}] Highest modseq\r\n")).await?;
                }
                Err(e) => tracing::error!(error = %e, "Lecture HIGHESTMODSEQ au SELECT impossible"),
            }
        }

        self.selected = Some(Selected {
            folder,
            messages,
            deleted: HashSet::new(),
            read_only,
        });

        // QRESYNC reconnection catch-up (RFC 7162): the client remembers a folder
        // by its UIDVALIDITY and modseq. If the UIDVALIDITY still matches, push
        // what changed since — VANISHED (EARLIER) for departures, FETCH for flag
        // changes. A mismatch means the client's cache is void, so QRESYNC is
        // skipped and it will resync in full (not an error, RFC 7162 3.2.5.2).
        if let Some(q) = qresync {
            if q.uidvalidity == store::uidvalidity(folder) {
                self.qresync_catch_up(user_id, folder, &q).await?;
            }
        }

        let access = if read_only { "READ-ONLY" } else { "READ-WRITE" };
        let command = if read_only { "EXAMINE" } else { "SELECT" };
        self.write(&format!("{tag} OK [{access}] {command} completed\r\n")).await
    }

    /// Pushes the QRESYNC catch-up for a just-selected folder: the UIDs that
    /// vanished since the client's modseq (VANISHED EARLIER), then a FETCH of the
    /// flags of every message changed since. Both are narrowed to the client's
    /// declared `known_uids` when it supplied a set.
    async fn qresync_catch_up(
        &mut self,
        user_id: Uuid,
        folder: &'static str,
        q: &imap_parse::QResyncParams,
    ) -> Result<()> {
        let known = |uid: i64| q.known_uids.is_empty() || seq_contains(&q.known_uids, uid.max(0) as u64);

        // VANISHED (EARLIER): departures read straight from the tombstones.
        match store::vanished_since(&self.db, user_id, folder, q.modseq).await {
            Ok(uids) => {
                let filtered: Vec<i64> = uids.into_iter().filter(|u| known(*u)).collect();
                if let Some(line) = vanished_earlier_line(&filtered) {
                    self.write(&line).await?;
                }
            }
            Err(e) => tracing::error!(error = %e, "Lecture VANISHED au SELECT QRESYNC impossible"),
        }

        // Flag changes: every message whose modseq advanced past the client's,
        // reported with UID, FLAGS and MODSEQ so the client can move its own
        // HIGHESTMODSEQ forward.
        let changed: Vec<(u64, StoredMessage)> = {
            let Some(selected) = self.selected.as_ref() else { return Ok(()) };
            selected
                .messages
                .iter()
                .enumerate()
                .filter(|(_, m)| m.modseq > q.modseq && known(m.local_uid))
                .map(|(index, m)| (index as u64 + 1, m.clone()))
                .collect()
        };
        for (seq, message) in &changed {
            let items = ["UID".to_string(), "FLAGS".to_string(), "MODSEQ".to_string()];
            let (response, _) = fetch_response(*seq, message, false, &items, true);
            self.write_bytes(&response).await?;
        }
        Ok(())
    }

    async fn cmd_status(&mut self, tag: &str, tokens: &[Token]) -> Result<()> {
        if self.mailbox.is_none() {
            return self.no(tag, "Please log in first").await;
        }
        let Some(requested) = tokens.get(2) else {
            self.write(&format!("{tag} BAD STATUS requires a mailbox\r\n")).await?;
            return Ok(());
        };
        let Some(folder) = store::folder_of(&requested.text) else {
            return self.no(tag, "[NONEXISTENT] Mailbox does not exist").await;
        };
        let messages = match self.load(folder).await {
            Ok(messages) => messages,
            Err(()) => return self.no(tag, "Internal error reading the mailbox").await,
        };

        let name = display_name(folder);
        let asked = tokens
            .get(3)
            .map(|t| t.text.trim_matches(|c| c == '(' || c == ')').to_ascii_uppercase())
            .unwrap_or_default();
        let wanted: Vec<&str> = asked.split_whitespace().collect();

        let mut items = Vec::new();
        for item in &wanted {
            let value = match *item {
                "MESSAGES" => messages.len() as i64,
                "UNSEEN" => messages.iter().filter(|m| !m.is_read).count() as i64,
                "RECENT" => 0,
                "UIDNEXT" => messages.last().map(|m| m.local_uid + 1).unwrap_or(1),
                "UIDVALIDITY" => i64::from(store::uidvalidity(folder)),
                _ => continue,
            };
            items.push(format!("{item} {value}"));
        }

        self.write(&format!("* STATUS \"{name}\" ({})\r\n", items.join(" "))).await?;
        self.ok(tag, "STATUS completed").await
    }

    async fn cmd_noop(&mut self, tag: &str, name: &str) -> Result<()> {
        // A poll is the one moment the protocol allows the view to grow, so it
        // is where newly delivered mail becomes visible to a connected client.
        if let Some(folder) = self.selected.as_ref().map(|s| s.folder) {
            if let Ok(messages) = self.load(folder).await {
                let grew = self.selected.as_ref().is_some_and(|s| messages.len() > s.messages.len());
                let count = messages.len();
                if let Some(selected) = self.selected.as_mut() {
                    selected.deleted.retain(|id| messages.iter().any(|m| m.id == *id));
                    selected.messages = messages;
                }
                if grew {
                    self.write(&format!("* {count} EXISTS\r\n")).await?;
                    self.write("* 0 RECENT\r\n").await?;
                }
            }
        }
        self.ok(tag, &format!("{name} completed")).await
    }

    // ── FETCH ────────────────────────────────────────────────────────────────

    async fn cmd_fetch(
        &mut self,
        tag: &str,
        tokens: &[Token],
        at: usize,
        by_uid: bool,
    ) -> Result<()> {
        let (Some(spec), Some(items)) = (tokens.get(at), tokens.get(at + 1)) else {
            self.write(&format!("{tag} BAD FETCH requires a set and a list of items\r\n")).await?;
            return Ok(());
        };
        // CONDSTORE/QRESYNC modifier group, when present: `(CHANGEDSINCE <n>
        // [VANISHED])` trailing the fetch items (RFC 7162).
        let mods = tokens
            .get(at + 2)
            .map(|t| parse_fetch_modifiers(&t.text))
            .unwrap_or_default();

        let Some(selected) = self.selected.as_ref() else {
            return self.no(tag, "No mailbox selected").await;
        };

        let star = star_of(selected, by_uid);
        let ranges = parse_sequence_set(&spec.text, star);
        let targets: Vec<(u64, StoredMessage, bool)> = selected
            .messages
            .iter()
            .enumerate()
            .filter(|(index, message)| {
                let key = if by_uid { message.local_uid.max(0) as u64 } else { *index as u64 + 1 };
                seq_contains(&ranges, key)
            })
            // CHANGEDSINCE narrows the fetch to what changed since the given
            // modseq: only messages whose modseq is strictly greater are served.
            .filter(|(_, message)| mods.changed_since.is_none_or(|n| message.modseq > n))
            .map(|(index, message)| {
                (index as u64 + 1, message.clone(), selected.deleted.contains(&message.id))
            })
            .collect();
        // Copied out so the `selected` borrow ends here, freeing `self` for the
        // writes and DB reads below.
        let folder = selected.folder;

        let mut requested = split_fetch_items(&items.text);
        // A CONDSTORE session — or any CHANGEDSINCE fetch — carries MODSEQ in
        // every FETCH response, even when the client did not name it.
        if (self.condstore || mods.changed_since.is_some())
            && !requested.iter().any(|i| i.eq_ignore_ascii_case("MODSEQ"))
        {
            requested.push("MODSEQ".to_string());
        }

        // VANISHED before any FETCH data (RFC 7162 ordering): a UID FETCH with
        // CHANGEDSINCE VANISHED reports the UIDs in the searched set gone since,
        // but only to a client that enabled QRESYNC.
        if by_uid && self.qresync && mods.vanished {
            if let (Some(since), Some(user_id)) =
                (mods.changed_since, self.mailbox.as_ref().map(|m| m.user_id))
            {
                match store::vanished_since(&self.db, user_id, folder, since).await {
                    Ok(uids) => {
                        let filtered: Vec<i64> =
                            uids.into_iter().filter(|u| seq_contains(&ranges, (*u).max(0) as u64)).collect();
                        if let Some(line) = vanished_earlier_line(&filtered) {
                            self.write(&line).await?;
                        }
                    }
                    Err(e) => tracing::error!(error = %e, "Lecture VANISHED au FETCH impossible"),
                }
            }
        }

        let mut newly_seen = Vec::new();

        for (seq, message, deleted) in &targets {
            let (response, touches_seen) = fetch_response(*seq, message, *deleted, &requested, by_uid);
            self.write_bytes(&response).await?;
            if touches_seen && !message.is_read {
                newly_seen.push(message.id);
            }
        }

        // A non-PEEK body fetch implicitly sets \Seen; doing it after the data
        // has gone out means a failing update never costs the client its mail.
        for id in newly_seen {
            self.mark_read(id, true).await;
        }

        self.ok(tag, if by_uid { "UID FETCH completed" } else { "FETCH completed" }).await
    }

    // ── STORE ────────────────────────────────────────────────────────────────

    async fn cmd_store(
        &mut self,
        tag: &str,
        tokens: &[Token],
        at: usize,
        by_uid: bool,
    ) -> Result<()> {
        // A leading `(UNCHANGEDSINCE <n>)` modifier group (RFC 7162) shifts the
        // action and flags one token to the right. When present, the STORE is
        // applied only to messages whose modseq is `<= n`; the rest are reported
        // in a MODIFIED response code and left untouched (optimistic locking).
        let has_modifier = tokens.get(at + 1).is_some_and(|t| is_modifier_group(&t.text));
        let unchanged_since =
            if has_modifier { tokens.get(at + 1).and_then(|t| parse_unchangedsince(&t.text)) } else { None };
        let base = if has_modifier { at + 1 } else { at };

        let (Some(spec), Some(action), Some(flags)) =
            (tokens.get(at), tokens.get(base + 1), tokens.get(base + 2))
        else {
            self.write(&format!("{tag} BAD STORE requires a set, an action and flags\r\n")).await?;
            return Ok(());
        };
        let Some(selected) = self.selected.as_ref() else {
            return self.no(tag, "No mailbox selected").await;
        };
        if selected.read_only {
            return self.no(tag, "Mailbox is read-only").await;
        }

        let action_name = action.upper();
        let silent = action_name.ends_with(".SILENT");
        let mode = match action_name.trim_end_matches(".SILENT") {
            "+FLAGS" => FlagMode::Add,
            "-FLAGS" => FlagMode::Remove,
            "FLAGS" => FlagMode::Replace,
            _ => {
                self.write(&format!("{tag} BAD Unknown STORE action\r\n")).await?;
                return Ok(());
            }
        };

        let asked: Vec<String> = flags
            .text
            .trim_matches(|c| c == '(' || c == ')')
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        let has = |flag: &str| asked.iter().any(|f| f == flag);
        let (seen, flagged, deleted) =
            (has("\\seen"), has("\\flagged"), has("\\deleted"));

        let star = star_of(selected, by_uid);
        let ranges = parse_sequence_set(&spec.text, star);
        let folder = selected.folder;
        let targets: Vec<(u64, Uuid, i64, i64)> = selected
            .messages
            .iter()
            .enumerate()
            .filter(|(index, message)| {
                let key = if by_uid { message.local_uid.max(0) as u64 } else { *index as u64 + 1 };
                seq_contains(&ranges, key)
            })
            .map(|(index, message)| (index as u64 + 1, message.id, message.modseq, message.local_uid))
            .collect();

        // The keys (sequence numbers, or UIDs for UID STORE) of messages skipped
        // because they changed since UNCHANGEDSINCE — reported back in MODIFIED.
        let mut modified: Vec<i64> = Vec::new();
        let mut applied: Vec<Uuid> = Vec::new();
        for (seq, id, modseq, uid) in &targets {
            if unchanged_since.is_some_and(|n| *modseq > n) {
                modified.push(if by_uid { *uid } else { *seq as i64 });
                continue;
            }
            if let Some(value) = mode.apply(seen) {
                self.mark_read(*id, value).await;
            }
            if let Some(value) = mode.apply(flagged) {
                self.mark_starred(*id, value).await;
            }
            if let Some(value) = mode.apply(deleted) {
                if let Some(selected) = self.selected.as_mut() {
                    if value {
                        selected.deleted.insert(*id);
                    } else {
                        selected.deleted.remove(id);
                    }
                }
            }
            applied.push(*id);
        }

        // A CONDSTORE session must report the fresh MODSEQ of each changed
        // message. The in-memory snapshot's modseq is now stale (the writes
        // bumped it in the database), so reload before building the responses.
        if self.condstore && !applied.is_empty() {
            if let (Ok(fresh), Some(sel)) = (self.load(folder).await, self.selected.as_mut()) {
                sel.deleted.retain(|id| fresh.iter().any(|m| m.id == *id));
                sel.messages = fresh;
            }
        }

        if !silent {
            let updated: Vec<(u64, String, i64, i64)> = {
                let Some(selected) = self.selected.as_ref() else {
                    return self.no(tag, "No mailbox selected").await;
                };
                applied
                    .iter()
                    .filter_map(|id| {
                        let (idx, message) =
                            selected.messages.iter().enumerate().find(|(_, m)| m.id == *id)?;
                        let flags = flags_of(message, selected.deleted.contains(id));
                        Some((idx as u64 + 1, flags, message.local_uid, message.modseq))
                    })
                    .collect()
            };
            for (seq, flags, uid, modseq) in updated {
                // FLAGS always; UID for a UID STORE; MODSEQ for a CONDSTORE session.
                let mut items = format!("FLAGS ({flags})");
                if by_uid {
                    items.push_str(&format!(" UID {uid}"));
                }
                if self.condstore {
                    items.push_str(&format!(" MODSEQ ({modseq})"));
                }
                self.write(&format!("* {seq} FETCH ({items})\r\n")).await?;
            }
        }

        // Auto-expunge (Gmail): when the policy commits `\Deleted` immediately
        // rather than waiting for the client's EXPUNGE, and this STORE just added
        // the flag, remove those messages now — routed to the purge destination
        // — emitting the untagged EXPUNGE responses the client needs to resync.
        if self.settings.imap_expunge_mode == ImapExpungeMode::Auto
            && mode.apply(deleted) == Some(true)
            && !applied.is_empty()
        {
            self.commit_deleted(folder, true).await?;
        }

        // MODIFIED (RFC 7162): the conditional store skipped these; the tagged OK
        // still reports success for the ones it did apply.
        let tail = if modified.is_empty() {
            if by_uid { "UID STORE completed".to_string() } else { "STORE completed".to_string() }
        } else {
            format!(
                "[MODIFIED {}] {}",
                uid_set_string(&modified),
                if by_uid { "UID STORE completed" } else { "STORE completed" }
            )
        };
        self.ok(tag, &tail).await
    }

    /// Commits every message currently flagged `\Deleted` in the selected folder
    /// to its purge destination, emitting shifting untagged `* n EXPUNGE`
    /// responses when `announce`, then reloads the folder snapshot. Shared by the
    /// explicit EXPUNGE command and the auto-expunge STORE path.
    async fn commit_deleted(&mut self, folder: &'static str, announce: bool) -> Result<()> {
        let Some(user_id) = self.mailbox.as_ref().map(|m| m.user_id) else { return Ok(()) };
        let doomed: Vec<(usize, Uuid)> = match self.selected.as_ref() {
            Some(selected) => selected
                .messages
                .iter()
                .enumerate()
                .filter(|(_, message)| selected.deleted.contains(&message.id))
                .map(|(index, message)| (index, message.id))
                .collect(),
            None => return Ok(()),
        };
        // Each removal renumbers what follows, which the untagged responses must
        // reflect (RFC 3501): subtract the count already expunged.
        for (removed, (index, id)) in doomed.into_iter().enumerate() {
            if let Err(e) = self.purge_message(user_id, id).await {
                tracing::error!(error = %e, "Suppression IMAP impossible");
                continue;
            }
            if announce {
                self.write(&format!("* {} EXPUNGE\r\n", index + 1 - removed)).await?;
            }
        }
        if let Ok(messages) = self.load(folder).await {
            if let Some(selected) = self.selected.as_mut() {
                selected.messages = messages;
                selected.deleted.clear();
            }
        }
        Ok(())
    }

    // ── SEARCH ───────────────────────────────────────────────────────────────

    async fn cmd_search(
        &mut self,
        tag: &str,
        tokens: &[Token],
        at: usize,
        by_uid: bool,
    ) -> Result<()> {
        let Some(selected) = self.selected.as_ref() else {
            return self.no(tag, "No mailbox selected").await;
        };

        let criteria: Vec<String> = tokens[at.min(tokens.len())..]
            .iter()
            .map(Token::upper)
            .collect();

        let matched: Vec<String> = selected
            .messages
            .iter()
            .enumerate()
            .filter(|(_, message)| {
                matches_criteria(message, selected.deleted.contains(&message.id), &criteria)
            })
            .map(|(index, message)| {
                if by_uid { message.local_uid.to_string() } else { (index + 1).to_string() }
            })
            .collect();

        let list = if matched.is_empty() {
            String::new()
        } else {
            format!(" {}", matched.join(" "))
        };
        self.write(&format!("* SEARCH{list}\r\n")).await?;
        self.ok(tag, if by_uid { "UID SEARCH completed" } else { "SEARCH completed" }).await
    }

    // ── EXPUNGE / CLOSE ──────────────────────────────────────────────────────

    async fn cmd_expunge(&mut self, tag: &str, announce: bool) -> Result<()> {
        let Some(selected) = self.selected.as_ref() else {
            return self.no(tag, "No mailbox selected").await;
        };
        if selected.read_only {
            return self.no(tag, "Mailbox is read-only").await;
        }

        let folder = selected.folder;
        self.commit_deleted(folder, announce).await?;
        self.ok(tag, "EXPUNGE completed").await
    }

    async fn cmd_close(&mut self, tag: &str) -> Result<()> {
        let read_only = match self.selected.as_ref() {
            Some(selected) => selected.read_only,
            None => return self.no(tag, "No mailbox selected").await,
        };
        // CLOSE expunges silently — untagged EXPUNGE responses are forbidden
        // here, because the client is leaving the mailbox anyway.
        if !read_only {
            self.expunge_silently().await;
        }
        self.selected = None;
        self.ok(tag, "CLOSE completed").await
    }

    async fn expunge_silently(&mut self) {
        let Some(user_id) = self.mailbox.as_ref().map(|m| m.user_id) else { return };
        let Some(selected) = self.selected.as_ref() else { return };
        let doomed: Vec<Uuid> = selected.deleted.iter().copied().collect();
        for id in doomed {
            if let Err(e) = self.purge_message(user_id, id).await {
                tracing::error!(error = %e, "Suppression IMAP impossible à la fermeture");
            }
        }
    }

    /// Commits one expunged message to its configured destination
    /// (`imap_purge_mode`): archived (kept out of the inbox), or moved to Trash.
    /// `delete` maps to Trash too — this store is a soft-delete model shared with
    /// the web UI, which never erases a row either; Trash is the terminal state.
    async fn purge_message(&self, user_id: Uuid, id: Uuid) -> anyhow::Result<()> {
        match self.settings.imap_purge_mode {
            ImapPurgeMode::Archive => store::archive(&self.db, user_id, id).await,
            ImapPurgeMode::Trash | ImapPurgeMode::Delete => {
                store::set_deleted(&self.db, user_id, id).await
            }
        }
    }

    // ── MOVE / COPY (RFC 6851, RFC 4315) ─────────────────────────────────────

    /// `COPY`/`UID COPY`: duplicate messages into another mailbox, reporting the
    /// new UIDs with a UIDPLUS COPYUID code so the client can track them.
    async fn cmd_copy(
        &mut self,
        tag: &str,
        tokens: &[Token],
        at: usize,
        by_uid: bool,
    ) -> Result<()> {
        let (Some(spec), Some(dest)) = (tokens.get(at), tokens.get(at + 1)) else {
            self.write(&format!("{tag} BAD COPY requires a set and a mailbox\r\n")).await?;
            return Ok(());
        };
        let Some(user_id) = self.mailbox.as_ref().map(|m| m.user_id) else {
            return self.no(tag, "Please log in first").await;
        };
        let targets = {
            let Some(selected) = self.selected.as_ref() else {
                return self.no(tag, "No mailbox selected").await;
            };
            collect_targets(selected, &spec.text, by_uid)
        };
        let Some(dest_folder) = store::folder_of(&dest.text) else {
            // UIDPLUS TRYCREATE: the copy would work if the mailbox existed —
            // though here the folder set is fixed and the client cannot make one.
            return self.no(tag, "[TRYCREATE] Mailbox does not exist").await;
        };

        let mut src_uids = Vec::new();
        let mut dst_uids = Vec::new();
        for (_, id, uid) in &targets {
            match store::copy_to(&self.db, user_id, *id, dest_folder).await {
                Ok(Some(new_uid)) => {
                    src_uids.push(*uid);
                    dst_uids.push(new_uid);
                }
                // The message was expunged from under us; skip it silently.
                Ok(None) => {}
                Err(e) => {
                    tracing::error!(error = %e, "Copie IMAP impossible");
                    return self.no(tag, "Internal error copying messages").await;
                }
            }
        }

        let tail = if src_uids.is_empty() {
            "COPY completed".to_string()
        } else {
            format!(
                "[COPYUID {} {} {}] COPY completed",
                store::uidvalidity(dest_folder),
                uid_set_string(&src_uids),
                uid_set_string(&dst_uids),
            )
        };
        self.ok(tag, &tail).await
    }

    /// `MOVE`/`UID MOVE`: relocate messages into another mailbox. The source
    /// mailbox loses them, so the response carries the COPYUID first, then one
    /// EXPUNGE per moved message (highest sequence number first, RFC 6851).
    async fn cmd_move(
        &mut self,
        tag: &str,
        tokens: &[Token],
        at: usize,
        by_uid: bool,
    ) -> Result<()> {
        let (Some(spec), Some(dest)) = (tokens.get(at), tokens.get(at + 1)) else {
            self.write(&format!("{tag} BAD MOVE requires a set and a mailbox\r\n")).await?;
            return Ok(());
        };
        let Some(user_id) = self.mailbox.as_ref().map(|m| m.user_id) else {
            return self.no(tag, "Please log in first").await;
        };
        let (targets, folder, read_only) = {
            let Some(selected) = self.selected.as_ref() else {
                return self.no(tag, "No mailbox selected").await;
            };
            (
                collect_targets(selected, &spec.text, by_uid),
                selected.folder,
                selected.read_only,
            )
        };
        if read_only {
            return self.no(tag, "Mailbox is read-only").await;
        }
        let Some(dest_folder) = store::folder_of(&dest.text) else {
            return self.no(tag, "[TRYCREATE] Mailbox does not exist").await;
        };
        if dest_folder == folder {
            // A message moved into its own mailbox stays put, but announcing its
            // EXPUNGE would desynchronise the client. Refuse plainly.
            return self.no(tag, "Cannot MOVE within the same mailbox").await;
        }

        let mut src_uids = Vec::new();
        let mut moved_seqs = Vec::new();
        for (seq, id, uid) in &targets {
            match store::move_to(&self.db, user_id, *id, dest_folder).await {
                // The move keeps local_uid, so the destination UID equals the
                // source UID; both sides of COPYUID are the same set.
                Ok(Some(_)) => {
                    src_uids.push(*uid);
                    moved_seqs.push(*seq);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::error!(error = %e, "Déplacement IMAP impossible");
                    return self.no(tag, "Internal error moving messages").await;
                }
            }
        }

        if !src_uids.is_empty() {
            let set = uid_set_string(&src_uids);
            self.write(&format!(
                "* OK [COPYUID {} {} {}]\r\n",
                store::uidvalidity(dest_folder),
                set,
                set
            ))
            .await?;
        }
        // Highest sequence number first: each EXPUNGE renumbers those above it.
        moved_seqs.sort_unstable_by(|a, b| b.cmp(a));
        for seq in &moved_seqs {
            self.write(&format!("* {seq} EXPUNGE\r\n")).await?;
        }

        // The current folder lost those messages; reload so its sequence numbers
        // stay truthful for the next command.
        if let Ok(messages) = self.load(folder).await {
            if let Some(selected) = self.selected.as_mut() {
                selected.deleted.retain(|id| messages.iter().any(|m| m.id == *id));
                selected.messages = messages;
            }
        }

        self.ok(tag, "MOVE completed").await
    }

    // ── ENABLE (RFC 5161) ────────────────────────────────────────────────────

    async fn cmd_enable(&mut self, tag: &str, tokens: &[Token]) -> Result<()> {
        if self.mailbox.is_none() {
            return self.no(tag, "Please log in first").await;
        }
        // CONDSTORE and QRESYNC are the extensions this server can turn on.
        // Enabling QRESYNC implies CONDSTORE (RFC 7162). Each capability actually
        // switched on is echoed in a single untagged ENABLED response; capabilities
        // it does not implement are silently ignored, per RFC 5161.
        let mut enabled: Vec<&str> = Vec::new();
        for token in &tokens[2.min(tokens.len())..] {
            match token.upper().as_str() {
                "CONDSTORE" if !self.condstore => {
                    self.condstore = true;
                    enabled.push("CONDSTORE");
                }
                "QRESYNC" => {
                    // QRESYNC pulls CONDSTORE in with it, but only QRESYNC is named
                    // in ENABLED (it is the capability the client asked for).
                    self.condstore = true;
                    if !self.qresync {
                        self.qresync = true;
                        enabled.push("QRESYNC");
                    }
                }
                _ => {}
            }
        }
        if !enabled.is_empty() {
            self.write(&format!("* ENABLED {}\r\n", enabled.join(" "))).await?;
        }
        self.ok(tag, "ENABLE completed").await
    }

    // ── IDLE (RFC 2177) ──────────────────────────────────────────────────────

    /// `IDLE`: hold the connection open and push changes to the selected folder
    /// as they happen. Returns `false` when the client disappears mid-idle, which
    /// ends the session.
    async fn cmd_idle(&mut self, tag: &str) -> Result<bool> {
        if self.mailbox.is_none() {
            self.no(tag, "Please log in first").await?;
            return Ok(true);
        }

        // Take the stream out so the `select!` arms can hold it without also
        // borrowing the session state (db, selected snapshot) the resync needs.
        let Some(mut conn) = self.conn.take() else { return Ok(false) };
        if let Err(e) = write_raw(&mut conn, b"+ idling\r\n").await {
            self.conn = Some(conn);
            return Err(e);
        }

        // PostgreSQL LISTEN is the push source: a trigger on mail.messages emits
        // NOTIFY 'mail_changes' with a JSON {user_id, folder} on every change. A
        // failed subscription degrades IDLE to "wait for DONE" — it never spins.
        let mut listener = match PgListener::connect_with(&self.db).await {
            Ok(mut l) => match l.listen("mail_changes").await {
                Ok(()) => Some(l),
                Err(e) => {
                    tracing::error!(error = %e, "Abonnement LISTEN mail_changes impossible");
                    None
                }
            },
            Err(e) => {
                tracing::error!(error = %e, "Connexion LISTEN mail_changes impossible");
                None
            }
        };

        let mut keepalive = tokio::time::interval(IDLE_KEEPALIVE);
        keepalive.tick().await; // the first tick fires immediately; discard it.
        let deadline = tokio::time::sleep(IDLE_MAX);
        tokio::pin!(deadline);

        // `true`: IDLE ended cleanly (DONE or the deadline) and the session goes
        // on. `false`: the connection is gone.
        let clean_end = loop {
            let mut line = Vec::new();
            tokio::select! {
                read = async { (&mut conn).take(MAX_LINE).read_until(b'\n', &mut line).await } => {
                    match read {
                        Ok(0) => break false,
                        Ok(_) => {
                            let done = std::str::from_utf8(&line)
                                .map(|s| s.trim().eq_ignore_ascii_case("DONE"))
                                .unwrap_or(false);
                            if done { break true; }
                            // Anything else mid-IDLE is not a command; ignore it.
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, "Lecture interrompue pendant IDLE");
                            break false;
                        }
                    }
                }
                payload = recv_notification(listener.as_mut()) => {
                    if let Some(payload) = payload {
                        if self.notification_matches(&payload) {
                            if let Err(e) = self.resync(&mut conn).await {
                                tracing::error!(error = %e, "Resynchronisation IDLE impossible");
                                break false;
                            }
                        }
                    }
                }
                _ = keepalive.tick() => {
                    if write_raw(&mut conn, b"* OK Still here\r\n").await.is_err() {
                        break false;
                    }
                }
                _ = &mut deadline => break true,
            }
        };

        self.conn = Some(conn);
        if !clean_end {
            return Ok(false);
        }
        self.ok(tag, "IDLE terminated").await?;
        Ok(true)
    }

    /// Whether a `mail_changes` payload concerns this session's selected folder.
    fn notification_matches(&self, payload: &str) -> bool {
        let Some(user_id) = self.mailbox.as_ref().map(|m| m.user_id) else { return false };
        let Some(folder) = self.selected.as_ref().map(|s| s.folder) else { return false };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else { return false };
        let same_user = value
            .get("user_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            == Some(user_id);
        let same_folder = value
            .get("folder")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|f| f.eq_ignore_ascii_case(folder));
        same_user && same_folder
    }

    /// Reloads the selected folder and pushes the difference as unsolicited
    /// responses, in the RFC 3501 order: EXPUNGE (highest sequence first), then
    /// EXISTS, then FETCH FLAGS. The snapshot is replaced afterwards so the next
    /// command numbers messages against the new view.
    async fn resync(&mut self, conn: &mut Reader) -> Result<()> {
        let Some(folder) = self.selected.as_ref().map(|s| s.folder) else { return Ok(()) };
        let Some(user_id) = self.mailbox.as_ref().map(|m| m.user_id) else { return Ok(()) };

        let fresh = store::list(&self.db, user_id, folder)
            .await
            .context("Rechargement du dossier pour la resynchronisation IDLE")?;

        let diff = {
            let Some(selected) = self.selected.as_ref() else { return Ok(()) };
            compute_resync(&selected.messages, &fresh)
        };

        for seq in &diff.expunged {
            write_raw(conn, format!("* {seq} EXPUNGE\r\n").as_bytes()).await?;
        }
        if let Some(count) = diff.exists {
            write_raw(conn, format!("* {count} EXISTS\r\n").as_bytes()).await?;
        }
        for (seq, flags) in &diff.flag_changes {
            write_raw(conn, format!("* {seq} FETCH (FLAGS ({flags}))\r\n").as_bytes()).await?;
        }

        if let Some(selected) = self.selected.as_mut() {
            selected.deleted.retain(|id| fresh.iter().any(|m| m.id == *id));
            selected.messages = fresh;
        }
        Ok(())
    }

    // ── Store access ─────────────────────────────────────────────────────────

    /// Loads a folder. Errors are logged here and surfaced as a bare `Err(())`:
    /// the protocol answer is always the same "internal error", never a database
    /// message handed to a remote client.
    async fn load(&self, folder: &str) -> std::result::Result<Vec<StoredMessage>, ()> {
        let Some(user_id) = self.mailbox.as_ref().map(|m| m.user_id) else {
            return Err(());
        };
        let messages = store::list(&self.db, user_id, folder).await.map_err(|e| {
            tracing::error!(error = %e, folder, "Lecture du dossier IMAP impossible");
        })?;
        // Honour the per-folder cap (0 = unlimited): expose only the most recent
        // N messages. Applied consistently for every SELECT/FETCH so sequence
        // numbers and UIDs stay coherent within the capped view.
        Ok(self.settings.cap_folder(messages))
    }

    async fn mark_read(&mut self, id: Uuid, read: bool) {
        let Some(user_id) = self.mailbox.as_ref().map(|m| m.user_id) else { return };
        match store::set_read(&self.db, user_id, id, read).await {
            Ok(()) => self.patch(id, |m| m.is_read = read),
            Err(e) => tracing::error!(error = %e, "Marquage lu/non-lu IMAP impossible"),
        }
    }

    async fn mark_starred(&mut self, id: Uuid, starred: bool) {
        let Some(user_id) = self.mailbox.as_ref().map(|m| m.user_id) else { return };
        match store::set_starred(&self.db, user_id, id, starred).await {
            Ok(()) => self.patch(id, |m| m.is_starred = starred),
            Err(e) => tracing::error!(error = %e, "Marquage suivi IMAP impossible"),
        }
    }

    /// Keeps the session snapshot in step with what was just written, so the
    /// FLAGS the client is told about are the FLAGS the store holds.
    fn patch(&mut self, id: Uuid, change: impl Fn(&mut StoredMessage)) {
        if let Some(selected) = self.selected.as_mut() {
            if let Some(message) = selected.messages.iter_mut().find(|m| m.id == id) {
                change(message);
            }
        }
    }

    // ── I/O ──────────────────────────────────────────────────────────────────

    /// Reads one complete command, following continuations for every literal it
    /// contains. `None` means the client closed the connection.
    async fn read_command(&mut self) -> Result<Option<Vec<Token>>> {
        let Some(line) = self.read_line().await? else { return Ok(None) };
        let (mut tokens, mut pending) = tokenize_line(&line);

        let mut literals = 0;
        while let Some(literal) = pending {
            literals += 1;
            // A command is a handful of arguments; a stream of literals is a
            // client trying to keep the session alive doing nothing.
            if literals > 16 {
                bail!("Trop de littéraux dans une seule commande IMAP");
            }
            if literal.len > MAX_LITERAL {
                bail!("Littéral IMAP de {} octets refusé", literal.len);
            }
            if !literal.non_sync {
                self.write("+ Ready for literal\r\n").await?;
            }
            let data = self.read_exact_bytes(literal.len).await?;
            tokens.push(Token { text: data, quoted: true });

            let Some(next) = self.read_line().await? else { return Ok(None) };
            let (more, next_pending) = tokenize_line(&next);
            tokens.extend(more);
            pending = next_pending;
        }

        Ok(Some(tokens))
    }

    /// The single buffered stream, for read and write. Absent only during a
    /// STARTTLS swap, which returns before the next I/O — hence the guard.
    fn conn_mut(&mut self) -> Result<&mut Reader> {
        self.conn.as_mut().context("Flux IMAP indisponible")
    }

    /// How long a silent client may hold a task before it is dropped, as the
    /// administrator set it (`imap_idle_timeout_min`, 30 minutes by default).
    ///
    /// This is the session's autologout, NOT the bound on an IDLE command:
    /// `IDLE_MAX` stays a constant because RFC 2177 fixes it, and a client
    /// parked in IDLE is not silent — it is being kept alive by `IDLE_KEEPALIVE`
    /// and served notifications. RFC 3501 §5.4 asks for at least 30 minutes of
    /// tolerated inactivity here; a shorter value is the operator's call, but it
    /// makes Thunderbird/Outlook reconnect in a loop. Never zero — a 0-minute
    /// timeout would drop every connection before its first command.
    fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.cfg.imap_idle_minutes.max(1) * 60)
    }

    async fn read_line(&mut self) -> Result<Option<String>> {
        let mut buffer = Vec::new();
        let idle_timeout = self.idle_timeout();
        let conn = self.conn_mut()?;
        let read = timeout(idle_timeout, async {
            (&mut *conn).take(MAX_LINE).read_until(b'\n', &mut buffer).await
        })
        .await
        .context("Client IMAP inactif trop longtemps")?
        .context("Lecture de la commande IMAP")?;

        if read == 0 {
            return Ok(None);
        }
        if !buffer.ends_with(b"\n") {
            bail!("Ligne IMAP dépassant {MAX_LINE} octets");
        }
        while matches!(buffer.last(), Some(b'\r' | b'\n')) {
            buffer.pop();
        }
        Ok(Some(String::from_utf8_lossy(&buffer).into_owned()))
    }

    async fn read_exact_bytes(&mut self, len: usize) -> Result<String> {
        let mut buffer = vec![0u8; len];
        let idle_timeout = self.idle_timeout();
        let conn = self.conn_mut()?;
        timeout(idle_timeout, conn.read_exact(&mut buffer))
            .await
            .context("Client IMAP inactif pendant un littéral")?
            .context("Lecture d'un littéral IMAP")?;
        Ok(String::from_utf8_lossy(&buffer).into_owned())
    }

    async fn write(&mut self, text: &str) -> Result<()> {
        self.write_bytes(text.as_bytes()).await
    }

    async fn write_bytes(&mut self, data: &[u8]) -> Result<()> {
        // The stream reads and writes through one BufReader, whose AsyncWrite
        // passes straight to the inner (TLS or plain) stream; flush so each
        // turn-based reply reaches the client — and clears the TLS record layer.
        let conn = self.conn_mut()?;
        conn.write_all(data).await.context("Écriture de la réponse IMAP")?;
        conn.flush().await.context("Vidage de la réponse IMAP")
    }

    async fn ok(&mut self, tag: &str, text: &str) -> Result<()> {
        self.write(&format!("{tag} OK {text}\r\n")).await
    }

    async fn no(&mut self, tag: &str, text: &str) -> Result<()> {
        self.write(&format!("{tag} NO {text}\r\n")).await
    }

    async fn bad(&mut self, tag: &str, text: &str) -> Result<()> {
        self.write(&format!("{tag} BAD {text}\r\n")).await
    }
}

/// What `*` denotes in a sequence set: the last sequence number for a plain
/// command, the highest UID for its `UID` form.
fn star_of(selected: &Selected, by_uid: bool) -> u64 {
    if by_uid {
        selected.messages.last().map(|m| m.local_uid.max(0) as u64).unwrap_or(0)
    } else {
        selected.messages.len() as u64
    }
}

/// Resolves a sequence/UID set against the selected snapshot into the messages
/// it names, as `(sequence number, id, uid)` in sequence order — the shape
/// MOVE and COPY both need to act on and report.
fn collect_targets(selected: &Selected, spec: &str, by_uid: bool) -> Vec<(u64, Uuid, i64)> {
    let star = star_of(selected, by_uid);
    let ranges = parse_sequence_set(spec, star);
    selected
        .messages
        .iter()
        .enumerate()
        .filter(|(index, message)| {
            let key = if by_uid { message.local_uid.max(0) as u64 } else { *index as u64 + 1 };
            seq_contains(&ranges, key)
        })
        .map(|(index, message)| (index as u64 + 1, message.id, message.local_uid))
        .collect()
}

/// Writes and flushes straight to the stream, bypassing the session's `write`.
/// IDLE holds the stream out of `self`, so its unsolicited responses and
/// keepalives go through here.
async fn write_raw(conn: &mut Reader, data: &[u8]) -> Result<()> {
    conn.write_all(data).await.context("Écriture IMAP pendant IDLE")?;
    conn.flush().await.context("Vidage IMAP pendant IDLE")
}

/// Awaits the next change notification. With no listener (a failed LISTEN) it
/// never resolves, so IDLE falls back to DONE-only rather than spinning.
async fn recv_notification(listener: Option<&mut PgListener>) -> Option<String> {
    match listener {
        Some(listener) => match listener.recv().await {
            Ok(notification) => Some(notification.payload().to_string()),
            Err(e) => {
                tracing::error!(error = %e, "Réception LISTEN mail_changes impossible");
                std::future::pending().await
            }
        },
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the real session loop over a real socket. The pool is lazy and
    /// points nowhere: everything asserted here happens before any data is
    /// touched, which is exactly the part that must never hang up on a client.
    #[tokio::test]
    async fn a_session_answers_over_a_real_socket() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("adresse locale");
        let db = sqlx::postgres::PgPoolOptions::new()
            // Without a short deadline the pool would keep retrying an address
            // that will never answer, and the test would spend that time idle.
            .acquire_timeout(Duration::from_millis(100))
            .connect_lazy("postgres://nobody@127.0.0.1:1/nothing")
            .expect("pool paresseux");
        let cfg = Arc::new(ServerConfig::default());

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let inc = crate::server::Incoming {
                db,
                cfg,
                peer: "test".to_string(),
                tls_mode: TlsMode::None,
                submission: false,
                acceptor: None,
                tarpit: Arc::new(Tarpit::new(Box::new(crate::server::limits::SystemClock::new()))),
            };
            let _ = handle(inc, MailStream::Plain(stream)).await;
        });

        let client = tokio::net::TcpStream::connect(address).await.expect("connect");
        let (read_half, mut write_half) = tokio::io::split(client);
        let mut lines = BufReader::new(read_half).lines();
        // A macro, not a closure: an async closure would have to hand out a
        // borrow of `lines` that outlives its own body.
        macro_rules! next {
            () => {
                lines.next_line().await.expect("lecture").unwrap_or_default()
            };
        }

        let caps = "IMAP4rev1 LITERAL+ ENABLE IDLE MOVE UIDPLUS CONDSTORE QRESYNC AUTH=SCRAM-SHA-256 AUTH=PLAIN";
        assert!(next!().starts_with(&format!("* OK [CAPABILITY {caps}]")));

        write_half.write_all(b"a1 CAPABILITY\r\n").await.expect("write");
        assert_eq!(next!(), format!("* CAPABILITY {caps}"));
        assert_eq!(next!(), "a1 OK CAPABILITY completed");

        write_half.write_all(b"a2 FROBNICATE now\r\n").await.expect("write");
        assert_eq!(next!(), "a2 BAD Unknown command");

        // A non-synchronising literal (LITERAL+): the bytes follow with no
        // continuation, and the login still fails cleanly.
        write_half.write_all(b"a3 LOGIN {3+}\r\nbob \"secret\"\r\n").await.expect("write");
        assert_eq!(next!(), "a3 NO Invalid credentials");

        write_half.write_all(b"a4 SELECT INBOX\r\n").await.expect("write");
        assert_eq!(next!(), "a4 NO Please log in first");

        // The modern commands are wired in: without a login they refuse cleanly
        // rather than answering BAD Unknown command.
        write_half.write_all(b"a5 IDLE\r\n").await.expect("write");
        assert_eq!(next!(), "a5 NO Please log in first");

        write_half.write_all(b"a6 MOVE 1 Trash\r\n").await.expect("write");
        assert_eq!(next!(), "a6 NO Please log in first");

        write_half.write_all(b"a7 ENABLE CONDSTORE\r\n").await.expect("write");
        assert_eq!(next!(), "a7 NO Please log in first");

        write_half.write_all(b"a8 LOGOUT\r\n").await.expect("write");
        assert_eq!(next!(), "* BYE Kubuno IMAP signing off");
        assert_eq!(next!(), "a8 OK LOGOUT completed");

        server.await.expect("fin de session");
    }

    /// The advertised capabilities depend on the connection's TLS state, without
    /// ever performing a handshake.
    #[test]
    fn capability_advertises_starttls_only_when_upgradable() {
        // Plaintext STARTTLS listener with a certificate: offer the upgrade, and
        // withhold the password-carrying mechanism entirely — LOGINDISABLED only
        // covers LOGIN, so leaving AUTH=PLAIN advertised would still invite a
        // client to hand over a password in the clear.
        assert_eq!(
            capability_line(TlsMode::StartTls, true, false),
            "IMAP4rev1 LITERAL+ ENABLE IDLE MOVE UIDPLUS CONDSTORE QRESYNC STARTTLS LOGINDISABLED AUTH=SCRAM-SHA-256"
        );
        // Same listener, no certificate: nothing to upgrade to.
        assert_eq!(
            capability_line(TlsMode::StartTls, false, false),
            "IMAP4rev1 LITERAL+ ENABLE IDLE MOVE UIDPLUS CONDSTORE QRESYNC AUTH=SCRAM-SHA-256 AUTH=PLAIN"
        );
        // Already encrypted (implicit 993, or just after STARTTLS): no STARTTLS,
        // no LOGINDISABLED.
        assert_eq!(
            capability_line(TlsMode::Implicit, true, true),
            "IMAP4rev1 LITERAL+ ENABLE IDLE MOVE UIDPLUS CONDSTORE QRESYNC AUTH=SCRAM-SHA-256 AUTH=PLAIN"
        );
        // Plain listener with no TLS at all: the modern extensions, no TLS caps.
        assert_eq!(
            capability_line(TlsMode::None, false, false),
            "IMAP4rev1 LITERAL+ ENABLE IDLE MOVE UIDPLUS CONDSTORE QRESYNC AUTH=SCRAM-SHA-256 AUTH=PLAIN"
        );
    }
}
