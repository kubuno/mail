//! Outbound SMTP delivery to a remote server: MX resolution and one delivery
//! attempt, reporting a result the queue worker turns into sent/deferred/bounced.
//!
//! This is a from-scratch Rust reimplementation of the invariants of Postfix's
//! SMTP client (`src/smtp/`): MX preference ordering with randomisation at equal
//! preference, NULL-MX (RFC 7505) and implicit-MX (RFC 5321) handling, host/IP
//! fallback, a configurable STARTTLS security level (`smtp_tls_security_level`),
//! and — the central rule of Postfix's `smtp_trouble.c` — mapping the *first
//! digit* of the server's reply to a permanent (5xx) or temporary (4xx) fate,
//! with every network/DNS fault treated as temporary and never bounced.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use hickory_resolver::TokioResolver;
use rand::seq::SliceRandom;
use rand::thread_rng;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::{rustls, TlsConnector};

use super::config::{OutboundTls, ServerConfig};
use super::relay::{RelaySecurity, RelayTarget};

/// The outcome of one delivery attempt for one recipient. The queue worker maps
/// these onto the per-recipient state: `Delivered` → sent, `Deferred` → retry
/// later, `Bounced` → give up and DSN the sender.
#[derive(Debug, Clone)]
pub enum DeliveryOutcome {
    /// Accepted by the remote server (250 on the final `.`).
    Delivered,
    /// Temporary failure (4xx, connection/DNS trouble). Retry later.
    Deferred { code: u16, reason: String },
    /// Permanent failure (5xx, NULL MX). Give up; the sender gets a DSN.
    Bounced { code: u16, reason: String },
}

/// Remote SMTP port for inter-MTA delivery (RFC 5321). Submission (587) is for
/// clients; MX-to-MX delivery is always on 25.
const SMTP_PORT: u16 = 25;
/// TCP connect budget per candidate address (Postfix `smtp_connect_timeout`).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Per-command / banner read budget.
const CMD_TIMEOUT: Duration = Duration::from_secs(120);
/// Budget for the reply to the final `.` — the server may fsync the whole
/// message before answering (Postfix `smtp_data_done_timeout` = 600s).
const DATA_DONE_TIMEOUT: Duration = Duration::from_secs(600);
/// TLS handshake budget for the opportunistic STARTTLS upgrade.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// A single SMTP reply line must never exceed 512 octets per RFC 5321; we allow
/// 8 KiB of slack and refuse anything longer rather than buffer unboundedly.
const MAX_LINE: usize = 8 * 1024;
/// Cap on the number of continuation lines in one multiline reply.
const MAX_REPLY_LINES: usize = 128;
/// Reply code reported when the configured security level demands TLS and it
/// could not be obtained. RFC 3207 gives 454 to "TLS not available due to
/// temporary reason"; Postfix reports the same condition as an enhanced 4.7.x —
/// temporary in both cases, so the message is retried, never sent in the clear.
const TLS_REQUIRED_CODE: u16 = 454;

/// What the administrator's configuration says about how to deliver. Carried
/// separately from the message so the delivery code never reaches into the
/// whole `ServerConfig`.
#[derive(Debug, Clone, Copy)]
pub struct Policy<'a> {
    /// Our own hostname, announced in EHLO/HELO.
    pub hostname: &'a str,
    /// How much transport encryption a delivery demands.
    pub tls: OutboundTls,
}

impl<'a> Policy<'a> {
    pub fn from_config(cfg: &'a ServerConfig) -> Self {
        Self {
            hostname: &cfg.hostname,
            tls:      cfg.outbound_tls,
        }
    }
}

/// Everything about the message that stays constant across connection attempts.
struct Delivery<'a> {
    /// Our own hostname, announced in EHLO/HELO.
    hostname: &'a str,
    /// The security level every connection attempt must satisfy.
    tls: OutboundTls,
    /// Envelope sender; empty means the null return path `<>`.
    envelope_from: &'a str,
    /// The single recipient this attempt is for.
    recipient: &'a str,
    /// The full RFC 5322 message (headers + body) to transmit in DATA.
    raw: &'a [u8],
}

/// Delivers `raw` to a single `recipient` whose address is `recipient@domain`,
/// with envelope sender `envelope_from` (empty = null return path `<>`).
///
/// Resolves the domain's MX (honouring NULL MX and the implicit-MX A/AAAA
/// fallback), tries each host by preference then each of its addresses, opens a
/// connection, runs the SMTP transaction under `policy.tls`, and maps the
/// reply's first digit to the outcome. Every network/DNS timeout becomes
/// `Deferred`, never `Bounced`. A recipient is only `Delivered` on a 2xx to the
/// final `.`.
pub async fn deliver(
    recipient: &str,
    domain: &str,
    envelope_from: &str,
    raw: &[u8],
    policy: &Policy<'_>,
) -> DeliveryOutcome {
    let del = Delivery {
        hostname: policy.hostname,
        tls: policy.tls,
        envelope_from,
        recipient,
        raw,
    };

    // 1. Build the DNS resolver from the system configuration.
    let resolver = match TokioResolver::builder_tokio() {
        Ok(builder) => builder.build(),
        Err(e) => {
            tracing::error!("résolveur DNS indisponible: {e}");
            return DeliveryOutcome::Deferred {
                code: 451,
                reason: "Résolveur DNS indisponible".to_string(),
            };
        }
    };

    // 2. Resolve the domain to an ordered list of concrete addresses.
    let targets = match resolve(&resolver, domain).await {
        Resolved::NullMx => {
            // RFC 7505: a single "." MX means the domain refuses all mail. This
            // is permanent — there is no fallback to A/AAAA.
            tracing::info!("livraison {domain}: null MX (RFC 7505), rejet permanent");
            return DeliveryOutcome::Bounced {
                code: 556,
                reason: "Le domaine n'accepte pas de courrier (null MX)".to_string(),
            };
        }
        Resolved::Temporary(reason) => {
            // SERVFAIL, timeout, or any transient DNS trouble → never bounce.
            tracing::warn!("livraison {domain}: DNS temporairement en échec ({reason})");
            return DeliveryOutcome::Deferred { code: 451, reason };
        }
        Resolved::NoTarget => {
            // The domain definitively resolves to nothing (NXDOMAIN / no A/AAAA
            // at all, with no transient error observed). Like Postfix's "Host or
            // domain name not found", this is a permanent condition — distinct
            // from a SERVFAIL, which is handled as `Temporary` above.
            tracing::info!("livraison {domain}: aucun serveur de messagerie (introuvable)");
            return DeliveryOutcome::Bounced {
                code: 550,
                reason: "Domaine introuvable ou sans serveur de messagerie".to_string(),
            };
        }
        Resolved::Targets(targets) => targets,
    };

    // 3. Build the TLS connector the security level calls for. `None` at a level
    //    that mandates TLS means the crypto stack itself failed to start, which
    //    Postfix reports as "TLS is required, but our TLS engine is unavailable"
    //    (4.7.5) — a deferral, never a cleartext delivery.
    let connector = tls_connector(del.tls);
    if connector.is_none() && del.tls.requires_tls() {
        tracing::error!("livraison {domain}: moteur TLS indisponible alors que le chiffrement est obligatoire");
        return DeliveryOutcome::Deferred {
            code: TLS_REQUIRED_CODE,
            reason: "Moteur TLS indisponible alors que le chiffrement est obligatoire".to_string(),
        };
    }

    // 4. Try each address in order. A TCP failure — or a host that cannot
    //    satisfy the TLS policy — moves to the next candidate, exactly as
    //    Postfix's `smtp_site_fail` does; once a usable session is open, its
    //    outcome is final for this attempt.
    let mut last_failure: Option<DeliveryOutcome> = None;
    for (ip, host) in &targets {
        let addr = SocketAddr::new(*ip, SMTP_PORT);
        match attempt(addr, host, connector.as_ref(), &del).await {
            Attempt::Done(outcome) => return outcome,
            Attempt::TryNext(reason) => {
                if reason.is_some() {
                    last_failure = reason;
                }
            }
        }
    }

    // Every candidate refused the connection, timed out, or could not offer the
    // encryption the policy demands → temporary.
    if let Some(outcome) = last_failure {
        tracing::warn!("livraison {domain}: aucun MX utilisable ({outcome:?})");
        return outcome;
    }
    tracing::warn!("livraison {domain}: aucun serveur MX joignable");
    DeliveryOutcome::Deferred {
        code: 451,
        reason: format!("Aucun serveur de messagerie joignable pour {domain}"),
    }
}

// ─────────────────────────── MX / DNS resolution ───────────────────────────

/// The result of resolving a domain to somewhere to deliver.
enum Resolved {
    /// Ordered `(address, host)` pairs to try. `host` is the MX (or the domain
    /// itself for implicit MX) and is used as the TLS server name and in logs.
    Targets(Vec<(IpAddr, String)>),
    /// A single "." MX — the domain refuses mail (RFC 7505). Permanent.
    NullMx,
    /// The domain definitively has no address (NXDOMAIN / NODATA on both MX and
    /// A/AAAA), with no transient error along the way. Permanent.
    NoTarget,
    /// Transient DNS trouble (SERVFAIL, timeout, …). Temporary.
    Temporary(String),
}

/// Resolves `domain` to an ordered list of addresses, applying MX preference,
/// NULL MX, implicit MX and the transient-vs-permanent DNS distinction.
async fn resolve(resolver: &TokioResolver, domain: &str) -> Resolved {
    // 1. MX lookup → ordered list of exchange hostnames.
    let mx_hosts: Vec<String> = match resolver.mx_lookup(domain).await {
        Ok(mx) => {
            let mut records: Vec<(u16, String)> = Vec::new();
            let mut null_mx = false;
            for rec in mx.iter() {
                if rec.exchange().is_root() {
                    // "." exchange = NULL MX (RFC 7505).
                    null_mx = true;
                    continue;
                }
                // `to_utf8()` yields the FQDN with a trailing dot; trim it so the
                // name is usable as a TLS server name and A/AAAA query.
                let host = rec.exchange().to_utf8().trim_end_matches('.').to_string();
                if !host.is_empty() {
                    records.push((rec.preference(), host));
                }
            }
            if null_mx && records.is_empty() {
                // A lone "." MX: the only valid NULL-MX configuration.
                return Resolved::NullMx;
            }
            if records.is_empty() {
                // MX RRset present but unusable (or empty answer): fall back to
                // the implicit MX — the domain's own A/AAAA.
                vec![domain.to_string()]
            } else {
                order_by_preference(records)
            }
        }
        Err(e) if e.is_no_records_found() => {
            // No MX record at all (NODATA) or the name does not exist (NXDOMAIN).
            // RFC 5321 §5.1: fall back to the implicit MX (the domain's A/AAAA).
            // If the name truly does not exist, that lookup will also be empty
            // and we end up at `NoTarget` below.
            vec![domain.to_string()]
        }
        Err(e) => {
            // SERVFAIL, timeout, connection-refused to the resolver, … → transient.
            return Resolved::Temporary(format!("échec de la requête MX pour {domain}: {e}"));
        }
    };

    // 2. Resolve each exchange host to its addresses, preserving host order and,
    //    within a host, the resolver's address order (re-grouped IPv4-first below).
    let mut targets: Vec<(IpAddr, String)> = Vec::new();
    let mut transient_error = false;
    for host in &mx_hosts {
        match resolver.lookup_ip(host.as_str()).await {
            Ok(ips) => {
                for ip in ips.iter() {
                    targets.push((ip, host.clone()));
                }
            }
            Err(e) if e.is_no_records_found() => {
                // This exchange has no address; skip it and try the next host.
                tracing::debug!("MX {host}: aucune adresse");
            }
            Err(e) => {
                // Transient failure resolving this host's address.
                tracing::warn!("MX {host}: résolution d'adresse en échec: {e}");
                transient_error = true;
            }
        }
    }

    // Prefer IPv4 over IPv6, keeping MX-preference order within each family.
    // Delivering over IPv6 requires its own SPF `ip6:` mechanism AND a matching
    // IPv6 PTR; a host that publishes AAAA but neither (the common self-hosted
    // case) hits SPF softfail and a generic reverse name, which lands the mail in
    // spam — Gmail is especially strict on IPv6. IPv4 therefore stays the default
    // path; IPv6 is kept only as a fallback for destinations that have nothing else.
    let (mut ordered, v6): (Vec<_>, Vec<_>) = targets.into_iter().partition(|(ip, _)| ip.is_ipv4());
    ordered.extend(v6);
    let targets = ordered;

    if !targets.is_empty() {
        Resolved::Targets(targets)
    } else if transient_error {
        // We could not resolve any address, but at least one failure was
        // transient → defer rather than bounce.
        Resolved::Temporary(format!(
            "aucune adresse résolue pour {domain} (erreur DNS temporaire)"
        ))
    } else {
        // Every lookup returned "no records": the domain has no mail target.
        Resolved::NoTarget
    }
}

/// Sorts MX records by ascending preference, randomising ties. Postfix uses the
/// same rule so that equal-preference MXes share load. Returns just the hosts.
fn order_by_preference(mut records: Vec<(u16, String)>) -> Vec<String> {
    // Shuffle first, then a *stable* sort by preference: equal-preference entries
    // keep their (now random) relative order, lower preference comes first.
    records.shuffle(&mut thread_rng());
    records.sort_by_key(|(pref, _)| *pref);
    records.into_iter().map(|(_, host)| host).collect()
}

// ─────────────────────────── one connection attempt ───────────────────────────

/// What one candidate address produced.
enum Attempt {
    /// A verdict for this delivery. Stop here.
    Done(DeliveryOutcome),
    /// This host is unusable — no TCP session, or it cannot satisfy the TLS
    /// policy. Try the next candidate; the carried outcome (if any) is what to
    /// report should no candidate work.
    TryNext(Option<DeliveryOutcome>),
}

/// Runs one delivery attempt against a single address.
async fn attempt(
    addr: SocketAddr,
    host: &str,
    connector: Option<&TlsConnector>,
    del: &Delivery<'_>,
) -> Attempt {
    // `allow_tls` lets us reconnect in cleartext after a failed opportunistic
    // handshake, exactly as Postfix does at TLS security level `may`. It is only
    // ever cleared when the level permits cleartext (see `tls_unavailable`).
    let mut allow_tls = connector.is_some();

    loop {
        let tcp = match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(sock)) => sock,
            Ok(Err(e)) => {
                tracing::warn!("connexion à {addr} ({host}) refusée: {e}");
                return Attempt::TryNext(None);
            }
            Err(_) => {
                tracing::warn!("connexion à {addr} ({host}) expirée");
                return Attempt::TryNext(None);
            }
        };

        let mut plain = BufReader::new(tcp);

        // Read the greeting banner. A 4xx *or* 5xx greeting is only Deferred: a
        // server may greylist or be temporarily down at connect time, and
        // bouncing on it would be wrong.
        let banner = match read_reply(&mut plain, CMD_TIMEOUT).await {
            Ok(reply) => reply,
            Err(e) => return Attempt::Done(io_deferred("bannière", &e)),
        };
        if banner.class() != 2 {
            tracing::warn!("{host}: accueil non-2xx ({}) — différé", banner.code);
            return Attempt::Done(DeliveryOutcome::Deferred {
                code: 451,
                reason: format!("Accueil du serveur refusé: {}", banner.summary()),
            });
        }

        // EHLO, falling back to HELO like Postfix.
        let ehlo = match ehlo_or_helo(&mut plain, del.hostname).await {
            Ok(reply) => reply,
            Err(e) => return Attempt::Done(e),
        };

        // A connector is present unless the level is `none` or this attempt has
        // already downgraded — both of which mean cleartext is permitted.
        let Some(conn) = connector.filter(|_| allow_tls) else {
            log_cleartext(host, del.tls);
            return Attempt::Done(finish(&mut plain, del).await);
        };

        if !offers_starttls(&ehlo.text) {
            // Postfix: "TLS is required, but was not offered by host".
            if let Some(outcome) = tls_unavailable(del.tls, host, "STARTTLS non proposé par le serveur") {
                return Attempt::TryNext(Some(outcome));
            }
            log_cleartext(host, del.tls);
            return Attempt::Done(finish(&mut plain, del).await);
        }

        // The name we authenticate against and send as SNI is the MX host we are
        // actually talking to — not the recipient's domain, and not a placeholder.
        let Some(server_name) = server_name_of(host) else {
            let detail = "nom d'hôte MX inutilisable comme nom de serveur TLS";
            if let Some(outcome) = tls_unavailable(del.tls, host, detail) {
                return Attempt::TryNext(Some(outcome));
            }
            tracing::warn!("{host}: {detail} — livraison en clair");
            log_cleartext(host, del.tls);
            return Attempt::Done(finish(&mut plain, del).await);
        };

        match send_cmd(&mut plain, "STARTTLS", CMD_TIMEOUT).await {
            Ok(reply) if reply.class() == 2 => {
                // The server agreed. Anything the peer pipelined before the
                // handshake would be smuggled cleartext — refuse it.
                if !plain.buffer().is_empty() {
                    tracing::warn!("{host}: données pipelinées avant TLS — différé");
                    return Attempt::Done(DeliveryOutcome::Deferred {
                        code: 451,
                        reason: "Données SMTP inattendues avant TLS".to_string(),
                    });
                }
                let tcp = plain.into_inner();
                match handshake(conn, tcp, server_name).await {
                    Ok(tls) => {
                        log_encrypted(host, &tls, del.tls);
                        let mut secure = BufReader::new(tls);
                        // RFC 3207: the client MUST re-issue EHLO on the freshly
                        // encrypted channel.
                        match ehlo_or_helo(&mut secure, del.hostname).await {
                            Ok(_) => return Attempt::Done(finish(&mut secure, del).await),
                            Err(e) => return Attempt::Done(e),
                        }
                    }
                    Err(e) => {
                        // At `verify` this is also how a bad chain or a mismatched
                        // hostname surfaces: rustls rejects the peer mid-handshake.
                        let detail = format!("échec du handshake TLS ({e})");
                        if let Some(outcome) = tls_unavailable(del.tls, host, &detail) {
                            return Attempt::TryNext(Some(outcome));
                        }
                        // Opportunistic TLS encrypts without authenticating, so a
                        // handshake failure is not fatal: reconnect and deliver in
                        // cleartext (Postfix level `may`).
                        tracing::warn!("{host}: {detail} — repli en clair");
                        allow_tls = false;
                        continue;
                    }
                }
            }
            Ok(reply) => {
                let detail = format!("STARTTLS refusé ({})", reply.summary());
                if let Some(outcome) = tls_unavailable(del.tls, host, &detail) {
                    return Attempt::TryNext(Some(outcome));
                }
                // STARTTLS refused (4xx/5xx): keep the cleartext session.
                tracing::info!("{host}: {detail} — livraison en clair");
                log_cleartext(host, del.tls);
                return Attempt::Done(finish(&mut plain, del).await);
            }
            Err(e) => return Attempt::Done(io_deferred("STARTTLS", &e)),
        }
    }
}

/// Performs the client-side TLS handshake toward `server_name`, which is both
/// the SNI sent and — from security level `verify` up — the name the peer's
/// certificate must match.
async fn handshake(
    connector: &TlsConnector,
    tcp: TcpStream,
    server_name: rustls::pki_types::ServerName<'static>,
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    match timeout(TLS_HANDSHAKE_TIMEOUT, connector.connect(server_name, tcp)).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "handshake TLS expiré",
        )),
    }
}

/// The MX hostname as a rustls server name, or `None` when it is not a usable
/// DNS name. There is deliberately no placeholder fallback: a name we cannot
/// state is a name we cannot verify.
fn server_name_of(host: &str) -> Option<rustls::pki_types::ServerName<'static>> {
    rustls::pki_types::ServerName::try_from(host.to_string()).ok()
}

/// Sends EHLO, falling back to HELO if EHLO is not accepted. Returns the (2xx)
/// reply, or a `Deferred` outcome if the server would not greet us.
async fn ehlo_or_helo<S>(stream: &mut S, hostname: &str) -> Result<SmtpReply, DeliveryOutcome>
where
    S: AsyncBufRead + AsyncWrite + Unpin,
{
    match send_cmd(stream, &format!("EHLO {hostname}"), CMD_TIMEOUT).await {
        Ok(reply) if reply.class() == 2 => Ok(reply),
        Ok(_) => match send_cmd(stream, &format!("HELO {hostname}"), CMD_TIMEOUT).await {
            Ok(reply) if reply.class() == 2 => Ok(reply),
            Ok(reply) => Err(DeliveryOutcome::Deferred {
                code: 451,
                reason: format!("HELO refusé: {}", reply.summary()),
            }),
            Err(e) => Err(io_deferred("HELO", &e)),
        },
        Err(e) => Err(io_deferred("EHLO", &e)),
    }
}

/// Runs MAIL FROM / RCPT TO / DATA and maps the outcome. Generic over the stream
/// so it serves both the cleartext and the TLS session.
async fn finish<S>(stream: &mut S, del: &Delivery<'_>) -> DeliveryOutcome
where
    S: AsyncBufRead + AsyncWrite + Unpin,
{
    // MAIL FROM:<sender> — empty sender is the null return path `<>`.
    let mail_from = format!("MAIL FROM:<{}>", del.envelope_from);
    match send_cmd(stream, &mail_from, CMD_TIMEOUT).await {
        Ok(reply) if reply.class() == 2 => {}
        Ok(reply) => {
            best_effort_quit(stream).await;
            return classify(reply.code, format!("MAIL FROM refusé: {}", reply.summary()));
        }
        Err(e) => return io_deferred("MAIL FROM", &e),
    }

    // RCPT TO:<recipient>.
    let rcpt_to = format!("RCPT TO:<{}>", del.recipient);
    match send_cmd(stream, &rcpt_to, CMD_TIMEOUT).await {
        Ok(reply) if reply.class() == 2 => {}
        Ok(reply) => {
            best_effort_quit(stream).await;
            return classify(reply.code, format!("RCPT TO refusé: {}", reply.summary()));
        }
        Err(e) => return io_deferred("RCPT TO", &e),
    }

    // DATA — the server answers 354 to invite the message.
    match send_cmd(stream, "DATA", CMD_TIMEOUT).await {
        Ok(reply) if reply.class() == 3 => {}
        Ok(reply) => {
            best_effort_quit(stream).await;
            return classify(reply.code, format!("DATA refusé: {}", reply.summary()));
        }
        Err(e) => return io_deferred("DATA", &e),
    }

    // Transmit the message with dot-stuffing and the terminating `.`.
    let payload = build_data_payload(del.raw);
    if let Err(e) = write_all(stream, &payload, DATA_DONE_TIMEOUT).await {
        return io_deferred("transfert DATA", &e);
    }

    // The reply to the final `.` decides the recipient's fate.
    let outcome = match read_reply(stream, DATA_DONE_TIMEOUT).await {
        Ok(reply) => classify(reply.code, reply.summary()),
        Err(e) => return io_deferred("réponse finale DATA", &e),
    };
    best_effort_quit(stream).await;
    outcome
}

/// Sends QUIT and drains its reply, ignoring all errors — the transaction is
/// already decided by the time we get here.
async fn best_effort_quit<S>(stream: &mut S)
where
    S: AsyncBufRead + AsyncWrite + Unpin,
{
    let _ = write_all(stream, b"QUIT\r\n", CMD_TIMEOUT).await;
    let _ = read_reply(stream, CMD_TIMEOUT).await;
}

// ─────────────────────────── pure protocol helpers ───────────────────────────

/// A parsed SMTP reply: the (final-line) status code and the joined message text.
#[derive(Debug, Clone)]
struct SmtpReply {
    code: u16,
    text: String,
}

impl SmtpReply {
    /// The reply class = first digit of the code (2 = ok, 3 = intermediate,
    /// 4 = transient, 5 = permanent). This single digit is the whole basis of
    /// the permanent-vs-temporary decision.
    fn class(&self) -> u8 {
        (self.code / 100) as u8
    }

    /// A compact `code text` string for logs and DSN reasons.
    fn summary(&self) -> String {
        if self.text.is_empty() {
            self.code.to_string()
        } else {
            format!("{} {}", self.code, self.text)
        }
    }
}

/// Maps a reply code's first digit onto the delivery outcome — the core of
/// Postfix `smtp_trouble.c`: 2xx delivered, 5xx permanent (bounce), everything
/// else (4xx and any unexpected class) temporary (defer).
fn classify(code: u16, reason: String) -> DeliveryOutcome {
    match code / 100 {
        2 => DeliveryOutcome::Delivered,
        5 => DeliveryOutcome::Bounced { code, reason },
        _ => DeliveryOutcome::Deferred { code, reason },
    }
}

/// Turns a session I/O error (timeout, EOF, reset) into a `Deferred` outcome —
/// network trouble is always temporary, never a bounce.
fn io_deferred(stage: &str, err: &std::io::Error) -> DeliveryOutcome {
    tracing::warn!("erreur SMTP pendant « {stage} »: {err}");
    DeliveryOutcome::Deferred {
        code: 451,
        reason: format!("Erreur réseau ({stage})"),
    }
}

/// Parses one line of an SMTP reply into `(code, is_final, text)`.
///
/// A reply line is `dddSPtext` (final), `ddd-text` (continuation) or `ddd`
/// alone. Returns `None` if the line is not a well-formed reply line.
fn parse_reply_line(line: &str) -> Option<(u16, bool, &str)> {
    let bytes = line.as_bytes();
    if bytes.len() < 3 || !bytes[..3].iter().all(u8::is_ascii_digit) {
        return None;
    }
    // Safe: the first three bytes are ASCII digits, so index 3 is a char boundary.
    let code = (u16::from(bytes[0] - b'0')) * 100
        + (u16::from(bytes[1] - b'0')) * 10
        + u16::from(bytes[2] - b'0');
    let rest = &line[3..];
    match rest.as_bytes().first() {
        None => Some((code, true, "")),
        Some(b' ') => Some((code, true, &rest[1..])),
        Some(b'-') => Some((code, false, &rest[1..])),
        Some(_) => None,
    }
}

/// Assembles `raw` into the DATA payload: CRLF line endings, dot-stuffing (a line
/// beginning with `.` gets a second `.`), and the terminating `\r\n.\r\n`.
fn build_data_payload(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 16);
    let mut start = 0;

    let emit = |out: &mut Vec<u8>, line: &[u8]| {
        // Dot-stuffing (RFC 5321 §4.5.2): a leading `.` is doubled so it is not
        // mistaken for the end-of-data marker.
        if line.first() == Some(&b'.') {
            out.push(b'.');
        }
        out.extend_from_slice(line);
        out.extend_from_slice(b"\r\n");
    };

    for i in 0..raw.len() {
        if raw[i] == b'\n' {
            let mut line = &raw[start..i];
            // Normalise CRLF: drop a bare trailing `\r` so we re-add exactly one.
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            emit(&mut out, line);
            start = i + 1;
        }
    }
    // A trailing partial line not ended by `\n`.
    if start < raw.len() {
        emit(&mut out, &raw[start..]);
    }

    // End-of-data marker.
    out.extend_from_slice(b".\r\n");
    out
}

// ─────────────────────────── stream I/O helpers ───────────────────────────

/// Writes `bytes` and flushes, under a timeout.
async fn write_all<S>(stream: &mut S, bytes: &[u8], budget: Duration) -> std::io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let fut = async {
        stream.write_all(bytes).await?;
        stream.flush().await
    };
    match timeout(budget, fut).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "écriture SMTP expirée",
        )),
    }
}

/// Sends one command line (`cmd\r\n`) and reads the reply.
async fn send_cmd<S>(stream: &mut S, cmd: &str, budget: Duration) -> std::io::Result<SmtpReply>
where
    S: AsyncBufRead + AsyncWrite + Unpin,
{
    let mut line = String::with_capacity(cmd.len() + 2);
    line.push_str(cmd);
    line.push_str("\r\n");
    write_all(stream, line.as_bytes(), budget).await?;
    read_reply(stream, budget).await
}

/// Reads a possibly multiline SMTP reply. Continuation lines (`ddd-`) are joined
/// until the final line (`ddd ` or `ddd`); the final line supplies the status
/// code. Each line is bounded to `MAX_LINE`.
async fn read_reply<S>(stream: &mut S, budget: Duration) -> std::io::Result<SmtpReply>
where
    S: AsyncBufRead + Unpin,
{
    let mut text = String::new();
    let mut count = 0usize;

    loop {
        let line = match read_line_bounded(stream, MAX_LINE, budget).await? {
            Some(line) => line,
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connexion fermée pendant la réponse",
                ))
            }
        };
        count += 1;
        if count > MAX_REPLY_LINES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "réponse SMTP trop longue",
            ));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let (code, is_final, message) = parse_reply_line(trimmed).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "réponse SMTP malformée")
        })?;
        if !message.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(message);
        }
        // The final line supplies the status code for the whole reply.
        if is_final {
            return Ok(SmtpReply { code, text });
        }
    }
}

/// Reads a single line (up to and including `\n`) with an upper bound on length
/// and a timeout. Returns `Ok(None)` on a clean EOF before any byte.
async fn read_line_bounded<S>(
    stream: &mut S,
    max: usize,
    budget: Duration,
) -> std::io::Result<Option<String>>
where
    S: AsyncBufRead + Unpin,
{
    let fut = async {
        let mut line: Vec<u8> = Vec::new();
        loop {
            let available = stream.fill_buf().await?;
            if available.is_empty() {
                // EOF: signal end only if we have not started a line.
                if line.is_empty() {
                    return Ok(None);
                }
                break;
            }
            if let Some(pos) = available.iter().position(|&b| b == b'\n') {
                line.extend_from_slice(&available[..=pos]);
                stream.consume(pos + 1);
                break;
            }
            let n = available.len();
            line.extend_from_slice(available);
            stream.consume(n);
            if line.len() > max {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "ligne SMTP trop longue",
                ));
            }
        }
        if line.len() > max {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ligne SMTP trop longue",
            ));
        }
        Ok(Some(String::from_utf8_lossy(&line).into_owned()))
    };
    match timeout(budget, fut).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "lecture SMTP expirée",
        )),
    }
}

/// True if an EHLO reply advertises the STARTTLS extension.
fn offers_starttls(ehlo_text: &str) -> bool {
    ehlo_text
        .split_whitespace()
        .any(|token| token.eq_ignore_ascii_case("STARTTLS"))
}

// ─────────────────────────── TLS policy ───────────────────────────

/// The administrator's level, in Postfix's own vocabulary, for logs and reasons.
fn level_name(level: OutboundTls) -> &'static str {
    match level {
        OutboundTls::None => "none",
        OutboundTls::May => "may",
        OutboundTls::Encrypt => "encrypt",
        OutboundTls::Verify => "verify",
    }
}

/// The encrypted channel could not be established, for whatever reason.
///
/// Postfix's rule (`smtp_proto.c`,
/// `PLAINTEXT_FALLBACK_OK_AFTER_STARTTLS_FAILURE`): at level `may` we continue
/// to the very same host in the clear, because deferring would stall mail to
/// every destination with a single broken MX; from `encrypt` upward, "we must
/// never, ever end up in plain-text mode", so the delivery is deferred with a
/// 4.7.x-class code and retried later.
///
/// Returns `None` when cleartext is permitted, `Some(outcome)` when the message
/// must be deferred instead. Pure — this is the decision the whole feature
/// hinges on, so it is tested on its own.
fn tls_unavailable(level: OutboundTls, host: &str, detail: &str) -> Option<DeliveryOutcome> {
    if !level.requires_tls() {
        return None;
    }
    tracing::warn!(
        host,
        tls_level = level_name(level),
        "Sortant : chiffrement obligatoire impossible ({detail}) — différé plutôt qu'en clair"
    );
    Some(DeliveryOutcome::Deferred {
        code:   TLS_REQUIRED_CODE,
        reason: format!(
            "Chiffrement obligatoire (niveau {}) impossible avec {host} : {detail}",
            level_name(level)
        ),
    })
}

/// Records the encryption actually obtained. Without this line no operator can
/// tell an unreachable destination from one rejected on certificate grounds.
fn log_encrypted(
    host: &str,
    stream: &tokio_rustls::client::TlsStream<TcpStream>,
    level: OutboundTls,
) {
    let (_, conn) = stream.get_ref();
    let version = conn
        .protocol_version()
        .map(|v| format!("{v:?}"))
        .unwrap_or_else(|| "inconnue".to_string());
    let cipher = conn
        .negotiated_cipher_suite()
        .map(|s| format!("{:?}", s.suite()))
        .unwrap_or_else(|| "inconnue".to_string());
    tracing::info!(
        host,
        tls_level = level_name(level),
        tls_version = %version,
        cipher = %cipher,
        // At `may`/`encrypt` the chain is deliberately not checked, so the peer
        // is encrypted but unauthenticated. Only `verify` proves who it is.
        cert_verified = level.verifies_certificate(),
        "Sortant : session chiffrée"
    );
}

/// Same trace for the other half of the story: this delivery went out in the
/// clear, and at which level that was allowed.
fn log_cleartext(host: &str, level: OutboundTls) {
    tracing::info!(
        host,
        tls_level = level_name(level),
        tls_version = "aucune",
        cert_verified = false,
        "Sortant : session EN CLAIR"
    );
}

/// Builds the TLS connector the security level calls for, or `None` when no
/// handshake should be attempted — either the level forbids it (`none`) or the
/// crypto stack could not be set up. The caller turns the latter into a deferral
/// whenever the level mandates TLS.
///
/// `may` and `encrypt` accept any certificate: RFC 3207 inter-MTA servers
/// routinely present self-signed or mismatched certificates, and Postfix's own
/// `encrypt` level likewise checks nothing — it only guarantees the bytes are
/// encrypted. `verify` checks the chain against the bundled Mozilla trust store
/// AND the hostname against the MX name, which is what `with_root_certificates`
/// installs (rustls's `WebPkiServerVerifier`).
fn tls_connector(level: OutboundTls) -> Option<TlsConnector> {
    if level == OutboundTls::None {
        return None;
    }

    // Naming the provider explicitly rather than relying on a process-wide
    // default: `ClientConfig::builder()` panics when none is installed, and a
    // panic in the delivery path would take the worker down.
    let builder = match rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    {
        Ok(builder) => builder,
        Err(e) => {
            tracing::error!("moteur TLS sortant indisponible: {e}");
            return None;
        }
    };

    let config = match level {
        OutboundTls::None => return None,
        OutboundTls::May | OutboundTls::Encrypt => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCert))
            .with_no_client_auth(),
        OutboundTls::Verify => {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            if roots.is_empty() {
                tracing::error!("magasin de confiance vide — vérification des certificats impossible");
                return None;
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
    };

    Some(TlsConnector::from(Arc::new(config)))
}

/// A certificate verifier that accepts any server certificate. Sound ONLY at
/// security levels `may` and `encrypt`, where confidentiality — not peer
/// authentication — is the goal (see `tls_connector`).
#[derive(Debug)]
struct AcceptAnyServerCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        use rustls::SignatureScheme::*;
        vec![
            RSA_PKCS1_SHA256,
            RSA_PKCS1_SHA384,
            RSA_PKCS1_SHA512,
            ECDSA_NISTP256_SHA256,
            ECDSA_NISTP384_SHA384,
            ECDSA_NISTP521_SHA512,
            RSA_PSS_SHA256,
            RSA_PSS_SHA384,
            RSA_PSS_SHA512,
            ED25519,
        ]
    }
}

// ─────────────────────────── relay (smarthost) delivery ───────────────────────────

/// Delivers `raw` to a single `recipient` through the configured relay
/// (smarthost) instead of resolving the recipient's MX.
///
/// One connection carries one recipient (one message per queue row), matching
/// how the worker already claims and delivers recipients individually — no
/// batching, no shared connection to reason about. The relay speaks for the
/// destination from here on: its 5xx to MAIL/RCPT/DATA is a real bounce, exactly
/// as Postfix relays a smarthost's rejection back to the sender.
///
/// The exceptions, both deferrals never bounces:
///   * any connection/TLS/network failure — the relay (a tunnel to a VPS) may be
///     momentarily down, and the recipient is likely fine;
///   * an AUTH failure — a credential/config problem on OUR side, not a property
///     of the recipient; retried after the operator fixes it.
///
/// DKIM is already applied by the worker to `raw` before this is called; the
/// relay does not re-sign.
pub async fn deliver_via_relay(
    recipient: &str,
    envelope_from: &str,
    raw: &[u8],
    relay: &RelayTarget,
    hostname: &str,
) -> DeliveryOutcome {
    let del = Delivery {
        hostname,
        // Unused on the relay path — TLS is driven by `relay.security` below,
        // not by the MX policy. Carried only so `finish`/`ehlo_or_helo` can be
        // reused unchanged.
        tls: OutboundTls::None,
        envelope_from,
        recipient,
        raw,
    };

    // Connect. A refused/failed/timed-out connection is ALWAYS temporary here:
    // the tunnel to the relay may simply be down right now, and bouncing would
    // wrongly tell the sender the recipient is bad.
    let tcp = match timeout(
        CONNECT_TIMEOUT,
        TcpStream::connect((relay.host.as_str(), relay.port)),
    )
    .await
    {
        Ok(Ok(sock)) => sock,
        Ok(Err(e)) => {
            tracing::warn!(host = %relay.host, port = relay.port, "Relais : connexion refusée: {e}");
            return DeliveryOutcome::Deferred {
                code:   451,
                reason: format!("Connexion au relais {}:{} impossible", relay.host, relay.port),
            };
        }
        Err(_) => {
            tracing::warn!(host = %relay.host, port = relay.port, "Relais : connexion expirée");
            return DeliveryOutcome::Deferred {
                code:   451,
                reason: format!("Connexion au relais {}:{} expirée", relay.host, relay.port),
            };
        }
    };

    match relay.security {
        RelaySecurity::None => {
            let mut plain = BufReader::new(tcp);
            let ehlo = match relay_greeting_and_ehlo(&mut plain, &del).await {
                Ok(reply) => reply,
                Err(outcome) => return outcome,
            };
            log_cleartext(&relay.host, OutboundTls::None);
            relay_authenticate_and_finish(&mut plain, &del, relay, &ehlo).await
        }
        RelaySecurity::Tls => {
            // Implicit TLS: the handshake happens before any SMTP is spoken.
            let Some(connector) = relay_tls_connector() else {
                return relay_tls_engine_unavailable();
            };
            let Some(server_name) = server_name_of(&relay.host) else {
                return DeliveryOutcome::Deferred {
                    code:   TLS_REQUIRED_CODE,
                    reason: format!("Nom de relais «{}» inutilisable comme nom TLS", relay.host),
                };
            };
            let tls = match handshake(&connector, tcp, server_name).await {
                Ok(tls) => tls,
                Err(e) => {
                    tracing::warn!(host = %relay.host, "Relais : handshake TLS implicite échoué: {e}");
                    return DeliveryOutcome::Deferred {
                        code:   TLS_REQUIRED_CODE,
                        reason: format!("Handshake TLS avec le relais {} échoué", relay.host),
                    };
                }
            };
            log_encrypted(&relay.host, &tls, OutboundTls::Encrypt);
            let mut secure = BufReader::new(tls);
            let ehlo = match relay_greeting_and_ehlo(&mut secure, &del).await {
                Ok(reply) => reply,
                Err(outcome) => return outcome,
            };
            relay_authenticate_and_finish(&mut secure, &del, relay, &ehlo).await
        }
        RelaySecurity::StartTls => {
            let mut plain = BufReader::new(tcp);
            let ehlo = match relay_greeting_and_ehlo(&mut plain, &del).await {
                Ok(reply) => reply,
                Err(outcome) => return outcome,
            };
            if !offers_starttls(&ehlo.text) {
                return DeliveryOutcome::Deferred {
                    code:   TLS_REQUIRED_CODE,
                    reason: format!("Le relais {} ne propose pas STARTTLS", relay.host),
                };
            }
            let Some(connector) = relay_tls_connector() else {
                return relay_tls_engine_unavailable();
            };
            let Some(server_name) = server_name_of(&relay.host) else {
                return DeliveryOutcome::Deferred {
                    code:   TLS_REQUIRED_CODE,
                    reason: format!("Nom de relais «{}» inutilisable comme nom TLS", relay.host),
                };
            };
            match send_cmd(&mut plain, "STARTTLS", CMD_TIMEOUT).await {
                Ok(reply) if reply.class() == 2 => {
                    // Anything pipelined before the handshake would be smuggled
                    // cleartext — refuse it, as the MX path does.
                    if !plain.buffer().is_empty() {
                        tracing::warn!(host = %relay.host, "Relais : données pipelinées avant TLS — différé");
                        return DeliveryOutcome::Deferred {
                            code:   451,
                            reason: "Données SMTP inattendues avant TLS (relais)".to_string(),
                        };
                    }
                    let tcp = plain.into_inner();
                    let tls = match handshake(&connector, tcp, server_name).await {
                        Ok(tls) => tls,
                        Err(e) => {
                            tracing::warn!(host = %relay.host, "Relais : handshake STARTTLS échoué: {e}");
                            return DeliveryOutcome::Deferred {
                                code:   TLS_REQUIRED_CODE,
                                reason: format!("Handshake STARTTLS avec le relais {} échoué", relay.host),
                            };
                        }
                    };
                    log_encrypted(&relay.host, &tls, OutboundTls::Encrypt);
                    let mut secure = BufReader::new(tls);
                    // RFC 3207: re-issue EHLO on the encrypted channel (no banner).
                    match ehlo_or_helo(&mut secure, del.hostname).await {
                        Ok(ehlo) => relay_authenticate_and_finish(&mut secure, &del, relay, &ehlo).await,
                        Err(outcome) => outcome,
                    }
                }
                Ok(reply) => {
                    tracing::warn!(host = %relay.host, "Relais : STARTTLS refusé ({})", reply.summary());
                    DeliveryOutcome::Deferred {
                        code:   TLS_REQUIRED_CODE,
                        reason: format!("STARTTLS refusé par le relais {}", relay.host),
                    }
                }
                Err(e) => io_deferred("STARTTLS (relais)", &e),
            }
        }
    }
}

/// Reads the banner and sends EHLO on a freshly opened (or freshly encrypted)
/// relay connection, returning the EHLO reply. A non-2xx banner defers: the
/// relay may be momentarily unavailable or greylisting us.
async fn relay_greeting_and_ehlo<S>(
    stream: &mut S,
    del: &Delivery<'_>,
) -> Result<SmtpReply, DeliveryOutcome>
where
    S: AsyncBufRead + AsyncWrite + Unpin,
{
    let banner = match read_reply(stream, CMD_TIMEOUT).await {
        Ok(reply) => reply,
        Err(e) => return Err(io_deferred("bannière du relais", &e)),
    };
    if banner.class() != 2 {
        return Err(DeliveryOutcome::Deferred {
            code:   451,
            reason: format!("Accueil du relais refusé: {}", banner.summary()),
        });
    }
    ehlo_or_helo(stream, del.hostname).await
}

/// Authenticates (when the relay wants credentials) and runs MAIL/RCPT/DATA.
async fn relay_authenticate_and_finish<S>(
    stream: &mut S,
    del: &Delivery<'_>,
    relay: &RelayTarget,
    ehlo: &SmtpReply,
) -> DeliveryOutcome
where
    S: AsyncBufRead + AsyncWrite + Unpin,
{
    if !relay.username.is_empty() {
        if let Some(outcome) = relay_authenticate(stream, ehlo, relay).await {
            return outcome;
        }
    }
    finish(stream, del).await
}

/// Runs SMTP AUTH against the relay, choosing PLAIN (one round trip) over LOGIN
/// from what EHLO advertised. Returns `Some(Deferred)` on ANY failure — an
/// authentication problem is ours to fix, so the message waits rather than
/// bouncing — or `None` on success.
async fn relay_authenticate<S>(
    stream: &mut S,
    ehlo: &SmtpReply,
    relay: &RelayTarget,
) -> Option<DeliveryOutcome>
where
    S: AsyncBufRead + AsyncWrite + Unpin,
{
    let (plain, login) = relay_auth_offer(&ehlo.text);
    if plain {
        relay_auth_plain(stream, relay).await
    } else if login {
        relay_auth_login(stream, relay).await
    } else {
        tracing::warn!(host = %relay.host, "Relais : aucun mécanisme AUTH utilisable (PLAIN/LOGIN) proposé");
        Some(DeliveryOutcome::Deferred {
            code:   454,
            reason: format!("Le relais {} ne propose ni AUTH PLAIN ni AUTH LOGIN", relay.host),
        })
    }
}

/// Whether the relay's EHLO advertised AUTH PLAIN and/or AUTH LOGIN. Continuation
/// lines are space-joined by `read_reply`, so line boundaries are gone; this
/// looks for an `AUTH` keyword and the mechanism tokens in the capability text,
/// tolerating both `AUTH PLAIN LOGIN` and the legacy `AUTH=PLAIN` form.
fn relay_auth_offer(ehlo_text: &str) -> (bool, bool) {
    let upper = ehlo_text.to_ascii_uppercase();
    let tokens: Vec<&str> = upper
        .split(|c: char| c.is_whitespace() || c == '=')
        .filter(|t| !t.is_empty())
        .collect();
    if !tokens.contains(&"AUTH") {
        return (false, false);
    }
    let plain = tokens.contains(&"PLAIN");
    let login = tokens.contains(&"LOGIN");
    (plain, login)
}

/// SASL PLAIN: `authzid \0 authcid \0 password`, base64-encoded, in one command.
async fn relay_auth_plain<S>(stream: &mut S, relay: &RelayTarget) -> Option<DeliveryOutcome>
where
    S: AsyncBufRead + AsyncWrite + Unpin,
{
    let secret = format!("\0{}\0{}", relay.username, relay.password);
    let token = base64::engine::general_purpose::STANDARD.encode(secret.as_bytes());
    match send_cmd(stream, &format!("AUTH PLAIN {token}"), CMD_TIMEOUT).await {
        Ok(reply) if reply.class() == 2 => None,
        Ok(reply) => Some(relay_auth_failed(relay, reply.code, &reply.summary())),
        Err(e) => Some(io_deferred("AUTH PLAIN (relais)", &e)),
    }
}

/// SASL LOGIN: the server prompts (334) for the base64 username, then password.
async fn relay_auth_login<S>(stream: &mut S, relay: &RelayTarget) -> Option<DeliveryOutcome>
where
    S: AsyncBufRead + AsyncWrite + Unpin,
{
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
    match send_cmd(stream, "AUTH LOGIN", CMD_TIMEOUT).await {
        Ok(reply) if reply.code == 334 => {}
        Ok(reply) => return Some(relay_auth_failed(relay, reply.code, &reply.summary())),
        Err(e) => return Some(io_deferred("AUTH LOGIN (relais)", &e)),
    }
    match send_cmd(stream, &b64(&relay.username), CMD_TIMEOUT).await {
        Ok(reply) if reply.code == 334 => {}
        Ok(reply) => return Some(relay_auth_failed(relay, reply.code, &reply.summary())),
        Err(e) => return Some(io_deferred("AUTH LOGIN (utilisateur)", &e)),
    }
    match send_cmd(stream, &b64(&relay.password), CMD_TIMEOUT).await {
        Ok(reply) if reply.class() == 2 => None,
        Ok(reply) => Some(relay_auth_failed(relay, reply.code, &reply.summary())),
        Err(e) => Some(io_deferred("AUTH LOGIN (mot de passe)", &e)),
    }
}

/// A rejected authentication. Always temporary: a wrong or expired credential is
/// an operator problem, not a property of the recipient, so the queued mail
/// waits for a fix rather than bouncing. Never logs the credential.
fn relay_auth_failed(relay: &RelayTarget, code: u16, summary: &str) -> DeliveryOutcome {
    tracing::error!(host = %relay.host, code, "Relais : authentification refusée ({summary}) — différé");
    DeliveryOutcome::Deferred {
        code:   454,
        reason: format!("Authentification au relais {} refusée", relay.host),
    }
}

/// The TLS connector for the relay: encrypts but does not authenticate the
/// peer's certificate. A smarthost on a private tunnel routinely presents a
/// self-signed or internal-CA certificate, so verifying the chain would defer
/// every message; the relay is trusted by network placement, and TLS here buys
/// confidentiality of the credentials in transit. Reuses `tls_connector`'s
/// `encrypt`-level connector (accept-any-cert).
fn relay_tls_connector() -> Option<TlsConnector> {
    tls_connector(OutboundTls::Encrypt)
}

/// The crypto stack failed to start while the relay demands TLS: defer, never
/// deliver the credentials in the clear.
fn relay_tls_engine_unavailable() -> DeliveryOutcome {
    tracing::error!("Relais : moteur TLS indisponible alors que le chiffrement est demandé");
    DeliveryOutcome::Deferred {
        code:   TLS_REQUIRED_CODE,
        reason: "Moteur TLS indisponible pour le relais".to_string(),
    }
}

// ─────────────────────────── tests ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_final_and_continuation_lines() {
        assert_eq!(parse_reply_line("250 OK"), Some((250, true, "OK")));
        assert_eq!(
            parse_reply_line("250-mx.example.com"),
            Some((250, false, "mx.example.com"))
        );
        assert_eq!(parse_reply_line("220"), Some((220, true, "")));
        assert_eq!(parse_reply_line("550 5.1.1 no such user"), Some((550, true, "5.1.1 no such user")));
        // Malformed: no code, wrong separator.
        assert_eq!(parse_reply_line("hello"), None);
        assert_eq!(parse_reply_line("25 short"), None);
        assert_eq!(parse_reply_line("250x bad separator"), None);
    }

    #[tokio::test]
    async fn reads_a_multiline_reply() {
        let data = b"250-mx.example.com at your service\r\n250-PIPELINING\r\n250-SIZE 52428800\r\n250 STARTTLS\r\n";
        let mut stream = BufReader::new(&data[..]);
        let reply = read_reply(&mut stream, Duration::from_secs(1))
            .await
            .expect("reply should parse");
        assert_eq!(reply.code, 250);
        assert!(offers_starttls(&reply.text), "STARTTLS should be detected");
        assert!(reply.text.contains("PIPELINING"));
    }

    #[tokio::test]
    async fn eof_before_reply_is_error() {
        let data: &[u8] = b"";
        let mut stream = BufReader::new(data);
        let err = read_reply(&mut stream, Duration::from_secs(1))
            .await
            .expect_err("EOF must be an error");
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn dot_stuffing_and_terminator() {
        // A line consisting of a single dot must be stuffed to "..".
        let payload = build_data_payload(b"Hello\r\n.\r\nWorld\r\n");
        let text = String::from_utf8(payload).unwrap();
        assert_eq!(text, "Hello\r\n..\r\nWorld\r\n.\r\n");
    }

    #[test]
    fn dot_stuffing_leading_dot_only() {
        // Leading dot doubled; interior dots untouched.
        let payload = build_data_payload(b".start\r\nmid.dle\r\n");
        let text = String::from_utf8(payload).unwrap();
        assert_eq!(text, "..start\r\nmid.dle\r\n.\r\n");
    }

    #[test]
    fn payload_normalises_bare_lf_and_no_trailing_newline() {
        // Bare LF becomes CRLF; a final line without a newline still terminates.
        let payload = build_data_payload(b"a\nb");
        let text = String::from_utf8(payload).unwrap();
        assert_eq!(text, "a\r\nb\r\n.\r\n");
    }

    #[test]
    fn empty_message_is_just_the_terminator() {
        let payload = build_data_payload(b"");
        assert_eq!(String::from_utf8(payload).unwrap(), ".\r\n");
    }

    #[test]
    fn code_class_maps_to_outcome() {
        // 2xx → delivered.
        assert!(matches!(classify(250, "ok".into()), DeliveryOutcome::Delivered));
        // 4xx → deferred (temporary).
        match classify(451, "busy".into()) {
            DeliveryOutcome::Deferred { code, .. } => assert_eq!(code, 451),
            other => panic!("attendu Deferred, obtenu {other:?}"),
        }
        // 5xx → bounced (permanent).
        match classify(550, "no mailbox".into()) {
            DeliveryOutcome::Bounced { code, .. } => assert_eq!(code, 550),
            other => panic!("attendu Bounced, obtenu {other:?}"),
        }
        // Unexpected class (e.g. 3xx out of place) is treated as temporary.
        assert!(matches!(classify(354, "x".into()), DeliveryOutcome::Deferred { .. }));
    }

    /// The whole point of the `encrypt`/`verify` levels: a destination that
    /// cannot give us TLS is retried later, never handed the message in the
    /// clear. At `may` (and `none`) the historical fallback is preserved.
    #[test]
    fn mandatory_tls_defers_rather_than_delivering_in_the_clear() {
        assert!(
            tls_unavailable(OutboundTls::None, "mx.example.com", "x").is_none(),
            "niveau none : jamais de TLS, donc jamais de blocage"
        );
        assert!(
            tls_unavailable(OutboundTls::May, "mx.example.com", "x").is_none(),
            "niveau may : repli en clair autorisé"
        );
        for level in [OutboundTls::Encrypt, OutboundTls::Verify] {
            match tls_unavailable(level, "mx.example.com", "STARTTLS non proposé") {
                Some(DeliveryOutcome::Deferred { code, reason }) => {
                    assert_eq!(code, TLS_REQUIRED_CODE);
                    assert!(reason.contains("mx.example.com"), "l'hôte doit être nommé");
                    assert!(reason.contains(level_name(level)), "le niveau doit être nommé");
                }
                other => panic!("niveau {level:?} : attendu Deferred, obtenu {other:?}"),
            }
        }
    }

    /// A mandatory-TLS failure must never be reported as permanent — the
    /// destination may well be fixed, or a sibling MX may work.
    #[test]
    fn mandatory_tls_failure_is_temporary() {
        let outcome = tls_unavailable(OutboundTls::Verify, "mx.example.com", "certificat invalide");
        assert!(matches!(outcome, Some(DeliveryOutcome::Deferred { .. })));
    }

    #[test]
    fn no_connector_is_built_when_tls_is_switched_off() {
        assert!(tls_connector(OutboundTls::None).is_none());
        // Every other level must produce a usable connector, otherwise `encrypt`
        // and `verify` would defer every message on a healthy instance.
        for level in [OutboundTls::May, OutboundTls::Encrypt, OutboundTls::Verify] {
            assert!(tls_connector(level).is_some(), "connecteur manquant pour {level:?}");
        }
    }

    /// The verified name is the MX we connect to. A name rustls cannot express
    /// yields `None` rather than a placeholder, so `verify` cannot be satisfied
    /// by a certificate for some other name.
    #[test]
    fn server_name_comes_from_the_mx_host() {
        assert!(server_name_of("mx1.example.com").is_some());
        assert!(server_name_of("").is_none());
        assert!(server_name_of("pas un nom").is_none());
    }

    /// The relay AUTH decision reads the EHLO capabilities: PLAIN and/or LOGIN
    /// only when an `AUTH` keyword is present, in either the modern or legacy
    /// spelling.
    #[test]
    fn relay_auth_offer_reads_the_ehlo_capabilities() {
        let (plain, login) = relay_auth_offer("mx at your service PIPELINE AUTH PLAIN LOGIN SIZE 10");
        assert!(plain && login);
        // Legacy `AUTH=PLAIN` form.
        let (plain, login) = relay_auth_offer("AUTH=PLAIN");
        assert!(plain && !login);
        // No AUTH keyword: nothing is offered even if the words appear elsewhere.
        let (plain, login) = relay_auth_offer("PIPELINING SIZE 20971520");
        assert!(!plain && !login);
    }

    #[test]
    fn preference_ordering_is_ascending() {
        let records = vec![
            (30, "c.example.com".to_string()),
            (10, "a.example.com".to_string()),
            (20, "b.example.com".to_string()),
        ];
        let ordered = order_by_preference(records);
        assert_eq!(
            ordered,
            vec![
                "a.example.com".to_string(),
                "b.example.com".to_string(),
                "c.example.com".to_string()
            ]
        );
    }
}
