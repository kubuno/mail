//! POP3 (RFC 1939) — for clients that only speak it, and for "download my mail
//! and keep a copy" setups.
//!
//! POP3 is stateful in a way worth remembering: the mailbox is frozen at
//! authentication (the "maildrop"), message numbers are 1..N over that snapshot
//! and never change during the session, and DELE only marks — nothing is
//! actually removed until QUIT commits the session. Deviating from that is what
//! makes clients delete the wrong message.
//!
//! STLS (RFC 2595) is POP3's STARTTLS: a plaintext session on port 110 can be
//! upgraded to TLS in place before authentication. The upgrade itself, including
//! the command-injection defense, lives in `super::tls::upgrade`.

use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use uuid::Uuid;

use super::{
    auth::{self, Mailbox},
    store::{self, StoredMessage},
    tls::{MailStream, TlsMode},
};
use crate::services::pop_imap::{self, PopImapSettings, PopPostAction};

/// A silent client must not hold a task forever. RFC 1939 §8 requires an
/// autologout timer of at least 10 minutes; 300 s was below that floor.
///
/// Deliberately NOT wired to the console's IMAP idle timeout. The two look
/// alike and are not: an idle IMAP session is a client waiting for mail, and
/// cutting it costs a reconnection; an idle POP3 session holds the maildrop
/// frozen with its DELE marks uncommitted, so a long timeout keeps a mailbox
/// locked against the next POP3 login. The RFC also puts a hard floor of ten
/// minutes under this one, which the IMAP setting (one minute upwards) does not
/// respect. Should POP3 ever become configurable, it needs its own setting and
/// its own floor.
const IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// RFC 1939 caps commands at 255 octets; refuse well before anything absurd.
const MAX_LINE: usize = 1024;

/// One message in the frozen maildrop.
struct Slot {
    message: StoredMessage,
    deleted: bool,
}

pub async fn handle(inc: crate::server::Incoming, stream: MailStream) -> Result<()> {
    // A single BufReader carries both directions: it reads commands and, since
    // it delegates AsyncWrite to the inner stream, also writes replies. Splitting
    // the stream is impossible now that it may be upgraded in place by STLS.
    let mut tls = stream.is_tls();
    let mut reader = BufReader::new(stream);

    reader
        .write_all(format!("+OK {} Kubuno POP3 ready\r\n", inc.cfg.hostname).as_bytes())
        .await?;
    reader.flush().await?;

    let mut mailbox: Option<Mailbox> = None;
    let mut pending_user: Option<String> = None;
    let mut maildrop: Vec<Slot> = Vec::new();
    let mut commands = 0i32;
    let mut failure: Option<String> = None;
    // This mailbox's POP/IMAP policy, read once at login. Governs which messages
    // enter the maildrop and what QUIT does to the ones the client fetched.
    let mut settings = PopImapSettings::default();
    // Messages RETRieved during this session, for the post-fetch action applied
    // at QUIT (Gmail: keep / mark read / archive / delete).
    let mut retrieved: Vec<Uuid> = Vec::new();

    loop {
        let mut line = String::new();
        let read = tokio::time::timeout(IDLE_TIMEOUT, reader.read_line(&mut line)).await;
        let bytes = match read {
            Ok(Ok(0)) => break,                       // client hung up
            Ok(Ok(n)) => n,
            Ok(Err(e)) => { failure = Some(e.to_string()); break }
            Err(_) => {
                let _ = reader.write_all(b"-ERR Timeout, closing connection\r\n").await;
                let _ = reader.flush().await;
                failure = Some("Inactivité".into());
                break;
            }
        };
        if bytes > MAX_LINE {
            reader.write_all(b"-ERR Line too long\r\n").await?;
            reader.flush().await?;
            continue;
        }

        let line = line.trim_end_matches(['\r', '\n']).to_string();
        let (verb, rest) = split_command(&line);
        commands += 1;

        // The password never reaches a log, not even truncated.
        if verb != "PASS" {
            tracing::debug!(peer = %inc.peer, command = %verb, "POP3");
        }

        match verb.as_str() {
            "CAPA" => {
                let mut caps = String::from("+OK Capability list follows\r\nUSER\r\nUIDL\r\nTOP\r\n");
                if advertise_stls(inc.tls_mode, inc.acceptor.is_some(), tls) {
                    caps.push_str("STLS\r\n");
                }
                caps.push_str(".\r\n");
                reader.write_all(caps.as_bytes()).await?;
            }
            "STLS" => {
                // RFC 2595: STLS is only valid in the AUTHORIZATION state, i.e.
                // before the session has authenticated.
                if mailbox.is_some() {
                    reader.write_all(b"-ERR Command not permitted now\r\n").await?;
                    reader.flush().await?;
                    continue;
                }
                if tls {
                    reader.write_all(b"-ERR TLS already active\r\n").await?;
                    reader.flush().await?;
                    continue;
                }
                let Some(acceptor) = inc.acceptor.as_deref() else {
                    reader.write_all(b"-ERR STLS not available\r\n").await?;
                    reader.flush().await?;
                    continue;
                };
                // The client must see this reply before the handshake begins, so
                // flush before handing the stream to the upgrade helper.
                reader.write_all(b"+OK Begin TLS negotiation\r\n").await?;
                reader.flush().await?;
                match crate::server::tls::upgrade(reader, acceptor).await {
                    Ok(upgraded) => {
                        reader = upgraded;
                        tls = true;
                        // The client starts over inside the tunnel: everything
                        // gathered in the clear is discarded (RFC 2595).
                        pending_user = None;
                        maildrop.clear();
                    }
                    Err(crate::server::tls::UpgradeError::Pipelined) => {
                        tracing::warn!(peer = %inc.peer, "STLS : données pipelinées en clair — connexion fermée");
                        return Ok(());
                    }
                    Err(crate::server::tls::UpgradeError::Handshake(e)) => {
                        tracing::debug!(peer = %inc.peer, error = %e, "STLS : échec du handshake");
                        return Ok(());
                    }
                }
            }
            "USER" => {
                pending_user = Some(rest.to_string());
                reader.write_all(b"+OK Send PASS\r\n").await?;
            }
            "PASS" => {
                let Some(username) = pending_user.take() else {
                    reader.write_all(b"-ERR Send USER first\r\n").await?;
                    reader.flush().await?;
                    continue;
                };
                match auth::authenticate(&inc.db, &username, rest).await {
                    Some(authed) => {
                        inc.tarpit.record_success(inc.peer_ip());
                        // The policy is read now and frozen for the session, like
                        // the maildrop. A read failure falls back to the
                        // compatibility default (POP open) so a bad settings row
                        // never locks a client out. See `services::pop_imap`.
                        settings = pop_imap::load(&inc.db, authed.user_id)
                            .await
                            .unwrap_or_else(|e| {
                                tracing::error!(error = %e, "POP3 : lecture de la politique POP/IMAP");
                                PopImapSettings::default()
                            });
                        // POP disabled for this mailbox: refuse after a VALID
                        // login (the auth succeeded, so this is not a brute-force
                        // signal — no tarpit). Same wording whatever the reason.
                        if !settings.pop_enabled {
                            tracing::info!(user = %authed.username, "POP3 refusé : accès POP désactivé");
                            reader
                                .write_all(b"-ERR [SYS/PERM] POP access is disabled for this mailbox\r\n")
                                .await?;
                            reader.flush().await?;
                            continue;
                        }
                        // The maildrop is a snapshot: everything the session
                        // reports afterwards must agree with this list. Under
                        // `from_now`, only messages past the cursor are exposed.
                        match store::list(&inc.db, authed.user_id, "inbox").await {
                            Ok(messages) => {
                                maildrop = messages
                                    .into_iter()
                                    .filter(|message| settings.pop_shows(message.local_uid))
                                    .map(|message| Slot { message, deleted: false })
                                    .collect();
                                let bytes: usize = maildrop.iter().map(rendered_size).sum();
                                reader
                                    .write_all(
                                        format!("+OK {} messages ({bytes} octets)\r\n", maildrop.len())
                                            .as_bytes(),
                                    )
                                    .await?;
                                mailbox = Some(authed);
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "POP3 : lecture de la boîte");
                                reader.write_all(b"-ERR Mailbox unavailable\r\n").await?;
                            }
                        }
                    }
                    None => {
                        // Same answer for "no such mailbox" and "wrong
                        // password" — telling them apart is an address oracle.
                        // The reply is held back by the growing tarpit delay to
                        // throttle brute force (Dovecot auth-penalty).
                        inc.tarpit.record_failure(inc.peer_ip());
                        tokio::time::sleep(inc.tarpit.delay_for(inc.peer_ip())).await;
                        reader.write_all(b"-ERR Authentication failed\r\n").await?;
                    }
                }
            }
            _ if mailbox.is_none() => {
                reader.write_all(b"-ERR Authenticate first\r\n").await?;
            }
            "STAT" => {
                let live: Vec<&Slot> = maildrop.iter().filter(|s| !s.deleted).collect();
                let bytes: usize = live.iter().map(|s| rendered_size(s)).sum();
                reader
                    .write_all(format!("+OK {} {bytes}\r\n", live.len()).as_bytes())
                    .await?;
            }
            "LIST" => {
                if rest.is_empty() {
                    let mut out = String::from("+OK Maildrop listing follows\r\n");
                    for (idx, slot) in maildrop.iter().enumerate() {
                        if slot.deleted { continue }
                        out.push_str(&format!("{} {}\r\n", idx + 1, rendered_size(slot)));
                    }
                    out.push_str(".\r\n");
                    reader.write_all(out.as_bytes()).await?;
                } else {
                    match live_slot(&maildrop, rest) {
                        Some((idx, slot)) => {
                            reader
                                .write_all(
                                    format!("+OK {} {}\r\n", idx + 1, rendered_size(slot)).as_bytes(),
                                )
                                .await?
                        }
                        None => reader.write_all(b"-ERR No such message\r\n").await?,
                    }
                }
            }
            "UIDL" => {
                if rest.is_empty() {
                    let mut out = String::from("+OK Unique-id listing follows\r\n");
                    for (idx, slot) in maildrop.iter().enumerate() {
                        if slot.deleted { continue }
                        out.push_str(&format!("{} {}\r\n", idx + 1, slot.message.local_uid));
                    }
                    out.push_str(".\r\n");
                    reader.write_all(out.as_bytes()).await?;
                } else {
                    match live_slot(&maildrop, rest) {
                        Some((idx, slot)) => {
                            reader
                                .write_all(
                                    format!("+OK {} {}\r\n", idx + 1, slot.message.local_uid).as_bytes(),
                                )
                                .await?
                        }
                        None => reader.write_all(b"-ERR No such message\r\n").await?,
                    }
                }
            }
            "RETR" => match live_slot(&maildrop, rest) {
                Some((_, slot)) => {
                    let body = store::render(&slot.message);
                    reader
                        .write_all(format!("+OK {} octets\r\n", body.len()).as_bytes())
                        .await?;
                    reader.write_all(body.as_bytes()).await?;
                    reader.write_all(b"\r\n.\r\n").await?;
                    // Record the fetch; the configured post-action (default:
                    // mark read, the historical behaviour) is applied at QUIT.
                    // Idempotent, so a client that RETRs twice is harmless.
                    if !retrieved.contains(&slot.message.id) {
                        retrieved.push(slot.message.id);
                    }
                }
                None => reader.write_all(b"-ERR No such message\r\n").await?,
            },
            "TOP" => {
                let mut parts = rest.split_whitespace();
                let which = parts.next().unwrap_or("");
                let lines: usize = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                match live_slot(&maildrop, which) {
                    Some((_, slot)) => {
                        let body = store::render(&slot.message);
                        reader.write_all(b"+OK Top of message follows\r\n").await?;
                        reader.write_all(top_of(&body, lines).as_bytes()).await?;
                        reader.write_all(b"\r\n.\r\n").await?;
                    }
                    None => reader.write_all(b"-ERR No such message\r\n").await?,
                }
            }
            "DELE" => match live_index(&maildrop, rest) {
                Some(idx) => {
                    // Marked only: RFC 1939 commits deletions at QUIT, and a
                    // client that drops the connection expects nothing lost.
                    maildrop[idx].deleted = true;
                    reader
                        .write_all(format!("+OK Message {} deleted\r\n", idx + 1).as_bytes())
                        .await?;
                }
                None => reader.write_all(b"-ERR No such message\r\n").await?,
            },
            "RSET" => {
                for slot in &mut maildrop {
                    slot.deleted = false;
                }
                reader.write_all(b"+OK Maildrop reset\r\n").await?;
            }
            "NOOP" => reader.write_all(b"+OK\r\n").await?,
            "QUIT" => {
                // UPDATE state: this is where deletions actually happen, and
                // where the post-fetch action is applied to what was RETRieved.
                let mut removed = 0;
                if let Some(m) = &mailbox {
                    // Deletions first: a DELE'd message is on its way to Trash,
                    // so the post-action must not also touch it.
                    let deleted: std::collections::HashSet<Uuid> = maildrop
                        .iter()
                        .filter(|s| s.deleted)
                        .map(|s| s.message.id)
                        .collect();
                    for id in &deleted {
                        match store::set_deleted(&inc.db, m.user_id, *id).await {
                            Ok(()) => removed += 1,
                            Err(e) => tracing::error!(error = %e, "POP3 : suppression"),
                        }
                    }
                    // Post-fetch action on every RETRieved message that was not
                    // also deleted. Best-effort and idempotent; a failure on one
                    // message is logged and does not abort the sign-off.
                    apply_post_fetch(&inc.db, m.user_id, settings.pop_post_action, &retrieved, &deleted).await;
                }
                reader
                    .write_all(format!("+OK Kubuno POP3 signing off ({removed} supprimés)\r\n").as_bytes())
                    .await?;
                reader.flush().await?;
                break;
            }
            "" => {}
            other => {
                reader
                    .write_all(format!("-ERR Unknown command: {other}\r\n").as_bytes())
                    .await?;
            }
        }

        // TLS buffers writes, so a reply is only really sent once flushed; do it
        // before blocking on the next command.
        reader.flush().await?;
    }

    super::log_session(&inc.db, "pop3", &inc.peer, mailbox.as_ref(), commands, failure).await;
    Ok(())
}

/// Applies the configured POP post-fetch action to every message the session
/// RETRieved, skipping any that were also DELE'd (already headed to Trash).
/// Best-effort: each message is handled independently and a failure is logged,
/// never surfaced — the client has already received its mail. All four actions
/// are idempotent, so re-running over the same ids changes nothing.
async fn apply_post_fetch(
    db: &sqlx::PgPool,
    user_id: Uuid,
    action: PopPostAction,
    retrieved: &[Uuid],
    deleted: &std::collections::HashSet<Uuid>,
) {
    if action == PopPostAction::Keep {
        return;
    }
    for id in retrieved.iter().filter(|id| !deleted.contains(id)) {
        let result = match action {
            PopPostAction::Keep => Ok(()),
            PopPostAction::MarkRead => store::set_read(db, user_id, *id, true).await,
            PopPostAction::Archive => store::archive(db, user_id, *id).await,
            PopPostAction::Delete => store::set_deleted(db, user_id, *id).await,
        };
        if let Err(e) = result {
            tracing::error!(error = %e, action = action.as_str(), "POP3 : action post-récupération");
        }
    }
}

/// STLS is advertised only when the client could actually use it: a STARTTLS
/// listener with a certificate configured, and not already inside TLS.
fn advertise_stls(mode: TlsMode, acceptor_present: bool, tls: bool) -> bool {
    mode == TlsMode::StartTls && acceptor_present && !tls
}

/// Splits "RETR 3" into ("RETR", "3"). The verb is upper-cased; the argument is
/// left as sent, since passwords and identifiers are case-sensitive.
fn split_command(line: &str) -> (String, &str) {
    let trimmed = line.trim_start();
    match trimmed.split_once(' ') {
        Some((verb, rest)) => (verb.to_ascii_uppercase(), rest.trim()),
        None => (trimmed.to_ascii_uppercase(), ""),
    }
}

/// 1-based message number → live slot, refusing anything already deleted.
fn live_index(maildrop: &[Slot], argument: &str) -> Option<usize> {
    let number: usize = argument.trim().parse().ok()?;
    let idx = number.checked_sub(1)?;
    let slot = maildrop.get(idx)?;
    if slot.deleted { None } else { Some(idx) }
}

fn live_slot<'a>(maildrop: &'a [Slot], argument: &str) -> Option<(usize, &'a Slot)> {
    let idx = live_index(maildrop, argument)?;
    Some((idx, &maildrop[idx]))
}

/// Size as the client will receive it. Rendering twice is wasteful but honest:
/// announcing a size that does not match the bytes sent breaks clients that
/// trust LIST.
fn rendered_size(slot: &Slot) -> usize {
    store::render(&slot.message).len()
}

/// Headers plus the first `lines` lines of the body, for TOP.
fn top_of(message: &str, lines: usize) -> String {
    let (headers, body) = match message.split_once("\r\n\r\n") {
        Some((h, b)) => (h, b),
        None => (message, ""),
    };
    let kept: Vec<&str> = body.split("\r\n").take(lines).collect();
    if kept.is_empty() {
        format!("{headers}\r\n")
    } else {
        format!("{headers}\r\n\r\n{}", kept.join("\r\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn slot(deleted: bool) -> Slot {
        Slot {
            message: StoredMessage {
                id: Uuid::nil(),
                local_uid: 42,
                message_id: None,
                from_name: None,
                from_email: "a@b.c".into(),
                to_addresses: json!([]),
                cc_addresses: json!([]),
                subject: "Sujet".into(),
                body_text: Some("ligne1\nligne2\nligne3".into()),
                body_html: None,
                attachments: json!([]),
                is_read: false,
                is_starred: false,
                folder: "inbox".into(),
                sent_at: None,
                received_at: Utc::now(),
                modseq: 1,
            },
            deleted,
        }
    }

    #[test]
    fn splits_verb_and_argument() {
        assert_eq!(split_command("retr 3"), ("RETR".into(), "3"));
        assert_eq!(split_command("QUIT"), ("QUIT".into(), ""));
        assert_eq!(split_command("  USER  bob@example.com "), ("USER".into(), "bob@example.com"));
    }

    #[test]
    fn numbering_is_one_based_and_skips_deleted() {
        let drop = vec![slot(false), slot(true), slot(false)];
        assert_eq!(live_index(&drop, "1"), Some(0));
        assert_eq!(live_index(&drop, "2"), None, "un message supprimé n'est plus adressable");
        assert_eq!(live_index(&drop, "3"), Some(2));
        assert_eq!(live_index(&drop, "0"), None);
        assert_eq!(live_index(&drop, "9"), None);
        assert_eq!(live_index(&drop, "x"), None);
    }

    #[test]
    fn top_keeps_headers_and_counts_body_lines() {
        let rendered = store::render(&slot(false).message);
        let top = top_of(&rendered, 1);
        assert!(top.contains("Subject: Sujet"));
        assert!(top.contains("ligne1"));
        assert!(!top.contains("ligne2"), "TOP 1 ne rend qu'une ligne de corps");
    }

    #[test]
    fn announced_size_matches_what_is_sent() {
        let s = slot(false);
        assert_eq!(rendered_size(&s), store::render(&s.message).len());
    }

    #[test]
    fn stls_advertised_only_on_starttls_listener_with_cert_before_upgrade() {
        // Announced on a STARTTLS listener that has a certificate, before TLS.
        assert!(advertise_stls(TlsMode::StartTls, true, false));
        // Not once the connection is already encrypted.
        assert!(!advertise_stls(TlsMode::StartTls, true, true));
        // Not without a certificate to actually perform the upgrade.
        assert!(!advertise_stls(TlsMode::StartTls, false, false));
        // Not on plaintext-only or implicit-TLS listeners.
        assert!(!advertise_stls(TlsMode::None, true, false));
        assert!(!advertise_stls(TlsMode::Implicit, true, false));
    }
}
