use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Response, StatusCode},
    Json,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use uuid::Uuid;
use base64::Engine as _;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    models::{EmailAccount, EmailMessage, SendMailDto},
    services::{
        crypto::MailCrypto,
        outgoing,
        smtp_service::{self, SmtpConfig},
    },
    state::AppState,
};

#[derive(serde::Deserialize)]
pub struct GetMessageQuery {
    /// Mark the message read as a side effect of this GET. Defaults to `true`
    /// to preserve the web client's behaviour; a mobile client that prefetches
    /// message bodies passes `false` so opening a row does not silently mark it
    /// read (see also `GET /threads/:id`, which never marks read).
    pub mark_read: Option<bool>,
    /// Act on another user's mailbox (account delegation); see `resolve_acting_user`.
    pub on_behalf_of: Option<Uuid>,
}

pub async fn get_message(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<GetMessageQuery>,
    Path(msg_id): Path<Uuid>,
) -> Result<Json<EmailMessage>, MailError> {
    // Delegation: read the grantor's message when authorised.
    let acting_id = crate::services::delegation::resolve_acting_user(&state.db, &user, q.on_behalf_of).await?;
    let user = AuthUser { id: acting_id, ..user };

    let msg = sqlx::query_as::<_, EmailMessage>(
        r#"SELECT id, thread_id, account_id, user_id, message_id, in_reply_to,
                  imap_uid, imap_folder, from_name, from_email,
                  to_addresses, cc_addresses, bcc_addresses, reply_to,
                  subject, body_text, body_html, attachments,
                  is_read, is_starred, is_deleted, folder, label_ids,
                  sent_at, received_at, created_at, spam_score, list_unsubscribe,
                  mailed_by, signed_by, security, auth_dmarc, structured_data, invite_response
           FROM mail.messages WHERE id = $1 AND user_id = $2"#,
    )
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Message {msg_id}")))?;
    let mut msg = msg;

    // OpenPGP: if this was stored as PGP/MIME, decrypt / verify it now with the
    // reader's own key and fill in the trust verdict. A no-op for ordinary mail.
    decode_pgp_in_place(&state, user.id, &mut msg).await;

    if q.mark_read.unwrap_or(true) && !msg.is_read {
        let _ = sqlx::query("UPDATE mail.messages SET is_read = TRUE WHERE id = $1")
            .bind(msg_id)
            .execute(&state.db)
            .await;

        let _ = sqlx::query(
            "UPDATE mail.threads SET unread_count = GREATEST(0, unread_count - 1) WHERE id = $1"
        )
        .bind(msg.thread_id)
        .execute(&state.db)
        .await;
    }

    strip_storage_paths(&mut msg);
    Ok(Json(msg))
}

pub async fn send_message(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<crate::models::OnBehalfQuery>,
    headers: HeaderMap,
    Json(dto): Json<SendMailDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Validate up front so a malformed request never claims an idempotency key.
    if dto.to_addresses.is_empty() {
        return Err(MailError::Validation("Au moins un destinataire requis".into()));
    }
    if dto.subject.trim().is_empty() {
        return Err(MailError::Validation("Sujet requis".into()));
    }

    // Account delegation: a delegate may send FROM the grantor's mailbox. The
    // request then scopes to the grantor (account lookup + Sent copy), while the
    // delegate's OWN address is stamped as `Sender:` (RFC 5322) so the recipient
    // sees who materially sent it — the `From:` stays the grantor. Read access is
    // not enough: `can_send` must be set on the accepted delegation. Resolved
    // BEFORE claiming an idempotency key so an unauthorized delegate is refused
    // without side effects.
    let acting_id = crate::services::delegation::resolve_acting_user(&state.db, &user, q.on_behalf_of).await?;
    let sender_override: Option<String> = if acting_id != user.id {
        if !crate::services::delegation::send_authority(&state.db, acting_id, user.id).await? {
            return Err(MailError::Forbidden);
        }
        tracing::info!(delegate = %user.id, grantor = %acting_id, "Envoi délégué");
        Some(user.email.clone())
    } else {
        None
    };

    // Idempotency-Key (optional): a retried send with the same key must not send
    // the mail twice. The web client omits it and keeps the historical path.
    let key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 255)
        .map(str::to_string);

    if let Some(key) = &key {
        // Lazy TTL purge (~24 h) so the table cannot grow without bound.
        let _ = sqlx::query(
            "DELETE FROM mail.send_idempotency WHERE created_at < NOW() - INTERVAL '24 hours'",
        )
        .execute(&state.db)
        .await;

        // Claim the key. ON CONFLICT DO NOTHING makes two concurrent requests
        // race for a single winner: exactly one inserts the row and sends.
        let claimed = sqlx::query(
            "INSERT INTO mail.send_idempotency (user_id, key) VALUES ($1, $2) \
             ON CONFLICT (user_id, key) DO NOTHING",
        )
        .bind(user.id)
        .bind(key)
        .execute(&state.db)
        .await?
        .rows_affected()
            == 1;

        if !claimed {
            // The key is already held: replay the memorised response, or — when
            // the first send is still in flight (no response yet) — refuse,
            // rather than risk a second delivery.
            let stored: Option<serde_json::Value> =
                sqlx::query_scalar::<_, Option<serde_json::Value>>(
                    "SELECT response_json FROM mail.send_idempotency WHERE user_id = $1 AND key = $2",
                )
                .bind(user.id)
                .bind(key)
                .fetch_optional(&state.db)
                .await?
                .flatten();

            return match stored {
                Some(resp) => Ok(Json(resp)),
                None => Err(MailError::Conflict(
                    "Un envoi avec cette clé d'idempotence est déjà en cours.".into(),
                )),
            };
        }
    }

    // We hold the claim (or no key was supplied): perform the send, scoped to the
    // acting user (the grantor on a delegated send) and stamping the delegate as
    // `Sender:` when delegated.
    let result = send_message_inner(&state, acting_id, &dto, sender_override.as_deref()).await;

    if let Some(key) = &key {
        match &result {
            // Success → memorise the response so a replay returns it verbatim.
            Ok(response) => {
                if let Err(e) = sqlx::query(
                    "UPDATE mail.send_idempotency SET response_json = $3 WHERE user_id = $1 AND key = $2",
                )
                .bind(user.id)
                .bind(key)
                .bind(response)
                .execute(&state.db)
                .await
                {
                    tracing::error!(error = %e, "Mémorisation de la réponse idempotente échouée");
                }
            }
            // Failure → drop the claim so an honest retry can proceed.
            Err(_) => {
                let _ =
                    sqlx::query("DELETE FROM mail.send_idempotency WHERE user_id = $1 AND key = $2")
                        .bind(user.id)
                        .bind(key)
                        .execute(&state.db)
                        .await;
            }
        }
    }

    result.map(Json)
}

/// The actual send, factored out so the idempotency wrapper above can memorise
/// its JSON response and replay it on a retried request. Returns the response
/// body as a bare `Value`.
pub(crate) async fn send_message_inner(
    state: &AppState,
    user_id: Uuid,
    dto: &SendMailDto,
    sender_override: Option<&str>,
) -> Result<serde_json::Value, MailError> {
    // Envoi PROGRAMMÉ : on stocke comme brouillon planifié ; le worker scheduler
    // l'enverra quand l'heure sera venue. (Voir workers::scheduler_worker.)
    if let Some(when) = dto.scheduled_at {
        sqlx::query(
            r#"INSERT INTO mail.drafts
               (id, account_id, user_id, to_addresses, cc_addresses, bcc_addresses, subject, body_html, reply_to_id, scheduled_at)
               VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        )
        .bind(dto.account_id)
        .bind(user_id)
        .bind(serde_json::to_value(&dto.to_addresses).unwrap_or_else(|_| serde_json::json!([])))
        .bind(serde_json::to_value(dto.cc_addresses.clone().unwrap_or_default()).unwrap_or_else(|_| serde_json::json!([])))
        .bind(serde_json::to_value(dto.bcc_addresses.clone().unwrap_or_default()).unwrap_or_else(|_| serde_json::json!([])))
        .bind(&dto.subject)
        .bind(ammonia::clean(&dto.body_html))
        .bind(dto.reply_to_id)
        .bind(when)
        .execute(&state.db)
        .await?;
        if let Some(draft_id) = dto.draft_id {
            let _ = sqlx::query("DELETE FROM mail.drafts WHERE id = $1 AND user_id = $2")
                .bind(draft_id).bind(user_id).execute(&state.db).await;
        }
        return Ok(serde_json::json!({ "message": "Envoi programmé", "scheduled_at": when }));
    }

    let account = sqlx::query_as::<_, EmailAccount>(
        r#"SELECT id, user_id, name, email_address, kind, mailbox_id,
                  incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  smtp_host, smtp_port, smtp_security, smtp_username, auth_kind,
                  is_default, is_active, last_sync_at, last_error,
                  created_at, updated_at
           FROM mail.accounts WHERE id = $1 AND user_id = $2"#,
    )
    .bind(dto.account_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Compte {}", dto.account_id)))?;

    // The module's own hardened policy, the one the reader applies too — not
    // bare `ammonia::clean`, which strips every `style` attribute and so sent
    // each message, invitations included, stripped of its formatting.
    let body_html = crate::services::html_sanitize::sanitize_email_html(&dto.body_html);

    // OpenPGP: resolve the sender's secret key and recipients' public keys up front
    // (None unless the message asks to be signed/encrypted). Both send paths hand
    // this to build_email, which wraps the body in PGP/MIME.
    let pgp = resolve_pgp(state, user_id, &account.email_address, dto).await?;

    // Autocrypt: advertise our public key on EVERY outgoing message (not only
    // encrypted ones) so correspondents learn it passively. `None` when OpenPGP
    // is off instance-wide or we hold no key for this address.
    let autocrypt = sender_autocrypt_key(state, user_id, &account.email_address).await;

    // A local account (a hosted mailbox of this instance) has no external SMTP
    // relay: the instance itself delivers. Local recipients are filed straight
    // into the store, remote recipients go through the instance's outbound queue.
    // An external account keeps the historical SMTP path. See migration 000025.
    let message_id = if account.kind == "local" {
        outgoing::send_from_local_account(state, &account, dto, &dto.subject, &body_html, pgp.as_ref(), autocrypt.as_deref(), sender_override).await?
    } else {
        let crypto = MailCrypto::new(&state.settings.mail.encryption_key)
            .map_err(|_| MailError::Crypto)?;

        // OAuth accounts send with XOAUTH2 (access token); password accounts keep
        // the historical credentials path.
        let (smtp_secret, xoauth2) = if account.auth_kind.starts_with("oauth") {
            let token = crate::services::oauth::valid_access_token(
                &state.db, &crypto, &state.settings.mail, account.id,
            )
            .await
            .map_err(|e| MailError::Smtp(e.to_string()))?;
            (token, true)
        } else {
            let (smtp_pass_enc, smtp_nonce): (Vec<u8>, Vec<u8>) = sqlx::query_as(
                "SELECT smtp_password, smtp_password_nonce FROM mail.accounts WHERE id = $1"
            )
            .bind(account.id)
            .fetch_one(&state.db)
            .await?;
            let smtp_pass = crate::services::app_password_normalize(
                &account.smtp_host,
                &crypto.decrypt(&smtp_pass_enc, &smtp_nonce).map_err(|_| MailError::Crypto)?,
            );
            (smtp_pass, false)
        };

        let smtp_cfg = SmtpConfig {
            host:       account.smtp_host.clone(),
            port:       account.smtp_port as u16,
            security:   account.smtp_security.clone(),
            username:   account.smtp_username.clone(),
            password:   smtp_secret,
            xoauth2,
            from_name:  account.name.clone(),
            from_email: account.email_address.clone(),
        };

        smtp_service::send_message(&smtp_cfg, dto, &dto.subject, &body_html, pgp.as_ref(), autocrypt.as_deref(), sender_override)
            .await
            .map_err(|e| MailError::Smtp(e.to_string()))?
    };

    // Local Sent copy — the message is already sent, so a storage failure must
    // not fail the request; log it instead. On success we get the thread the copy
    // was filed under, so the composer's labels can be attached to it.
    match crate::services::sent_copy::store_sent_copy(
        &state.db, &account, dto, &body_html, &message_id,
        &state.settings.mail.attachments_dir,
    ).await {
        Ok(thread_id) => {
            if let Some(requested) = dto.label_ids.as_deref() {
                apply_sent_labels(&state.db, user_id, thread_id, requested).await;
            }
        }
        Err(e) => {
            tracing::error!(error = %e, account_id = %account.id, "Copie locale « envoyés » échouée");
        }
    }

    if let Some(draft_id) = dto.draft_id {
        let _ = sqlx::query("DELETE FROM mail.drafts WHERE id = $1 AND user_id = $2")
            .bind(draft_id)
            .bind(user_id)
            .execute(&state.db)
            .await;
    }

    // Feed the recipient-autocomplete index — people the user WRITES TO rank
    // highest (weight 3 vs 1 for synced mail).
    let mut sent_to: Vec<(String, Option<String>)> = Vec::new();
    for a in dto.to_addresses.iter()
        .chain(dto.cc_addresses.as_deref().unwrap_or(&[]))
        .chain(dto.bcc_addresses.as_deref().unwrap_or(&[]))
    {
        sent_to.push((a.email.clone(), a.name.clone()));
    }
    crate::services::address_index::upsert(&state.db, user_id, &sent_to, 3).await;

    Ok(serde_json::json!({ "message": "Message envoyé" }))
}

#[derive(serde::Deserialize)]
pub struct SuggestQuery {
    pub q: String,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct AddressSuggestion {
    pub email: String,
    pub name:  Option<String>,
}

/// Recipient autocompletion: search the per-user address index (kept up to date
/// by the sync worker and outgoing sends — no scan of mail.messages).
pub async fn suggest_addresses(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<SuggestQuery>,
) -> Result<Json<Vec<AddressSuggestion>>, MailError> {
    let term = q.q.trim().to_lowercase();
    if term.is_empty() {
        return Ok(Json(vec![]));
    }
    let rows = sqlx::query_as::<_, AddressSuggestion>(
        r#"SELECT email, name FROM mail.address_index
           WHERE user_id = $1
             AND (email LIKE $2 || '%' OR email LIKE '%' || $2 || '%'
                  OR LOWER(COALESCE(name, '')) LIKE '%' || $2 || '%')
           ORDER BY (email LIKE $2 || '%') DESC, use_count DESC, last_used_at DESC
           LIMIT 8"#,
    )
    .bind(user.id)
    .bind(&term)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

pub async fn star_message(
    State(state): State<AppState>,
    user: AuthUser,
    Path(msg_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let row = sqlx::query_scalar::<_, bool>(
        "UPDATE mail.messages SET is_starred = NOT is_starred WHERE id = $1 AND user_id = $2 RETURNING is_starred"
    )
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Message {msg_id}")))?;

    Ok(Json(serde_json::json!({ "is_starred": row })))
}

pub async fn mark_read(
    State(state): State<AppState>,
    user: AuthUser,
    Path(msg_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, MailError> {
    let is_read = body["is_read"].as_bool().unwrap_or(true);

    let thread_id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE mail.messages SET is_read = $1 WHERE id = $2 AND user_id = $3 RETURNING thread_id"
    )
    .bind(is_read)
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?;

    if thread_id.is_none() {
        return Err(MailError::NotFound(format!("Message {msg_id}")));
    }

    let unread: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mail.messages WHERE thread_id = $1 AND is_read = FALSE AND is_deleted = FALSE"
    )
    .bind(thread_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    sqlx::query("UPDATE mail.threads SET unread_count = $1 WHERE id = $2")
        .bind(unread as i32)
        .bind(thread_id)
        .execute(&state.db)
        .await?;

    Ok(Json(serde_json::json!({ "is_read": is_read })))
}

/// Sandboxed, script-free, frame-free: what an attachment response is allowed to
/// be even if a browser decides to render it.
const ATTACHMENT_CSP: &str =
    "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox; frame-ancestors 'none'";

/// The MIME types an attachment may keep — everything a viewer needs to preview
/// a file, and nothing that can execute or carry markup.
///
/// The sender chooses the `Content-Type` of a MIME part, so honouring it turns
/// an attachment into a document served from Kubuno's own origin: `text/html`
/// renders, and a companion part declared `application/javascript` then loads
/// same-origin — which is precisely how `script-src 'self'` gets defeated. Only
/// this list is echoed back; the rest becomes `application/octet-stream`.
fn safe_inline_mime(claimed: &str) -> Option<&'static str> {
    // Compare on the essence only: parameters (charset, name…) are the sender's too.
    let essence = claimed.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    Some(match essence.as_str() {
        "image/jpeg" | "image/jpg" => "image/jpeg",
        "image/png"                => "image/png",
        "image/gif"                => "image/gif",
        "image/webp"               => "image/webp",
        "image/bmp"                => "image/bmp",
        "image/x-icon" | "image/vnd.microsoft.icon" => "image/x-icon",
        "application/pdf"          => "application/pdf",
        "audio/mpeg"               => "audio/mpeg",
        "audio/ogg"                => "audio/ogg",
        "audio/wav" | "audio/x-wav" => "audio/wav",
        "video/mp4"                => "video/mp4",
        "video/webm"               => "video/webm",
        // Deliberately absent: image/svg+xml (carries script), text/html,
        // text/xml, application/xhtml+xml, application/javascript, and every
        // text/* — a text preview is fetched and rendered by the app itself.
        _ => return None,
    })
}

pub async fn download_attachment(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path((msg_id, index)): Path<(Uuid, usize)>,
) -> Result<Response<Body>, MailError> {
    let row = sqlx::query_as::<_, (serde_json::Value,)>(
        "SELECT attachments FROM mail.messages WHERE id = $1 AND user_id = $2",
    )
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Message {msg_id}")))?;

    let attachments: Vec<serde_json::Value> = serde_json::from_value(row.0).unwrap_or_default();
    let att = attachments
        .get(index)
        .ok_or_else(|| MailError::NotFound(format!("Pièce jointe {index}")))?;

    let storage_path = att
        .get("storage_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| MailError::NotFound("storage_path manquant".into()))?;

    // The stored MIME is the one the SENDER wrote in the message: never trust it
    // to decide how the browser treats the bytes. Only the few types that can be
    // shown safely keep their own Content-Type — everything else is handed over
    // as an opaque download. See `safe_inline_mime`.
    let claimed_mime = att.get("mime").and_then(|v| v.as_str()).unwrap_or("");
    let (mime_type, inline_ok) = match safe_inline_mime(claimed_mime) {
        Some(m) => (m.to_string(), true),
        None => ("application/octet-stream".to_string(), false),
    };

    let name = att
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("attachment")
        .to_string();

    // Stream from disk rather than slurping the whole file into memory — a large
    // attachment must not cost its full size in RAM per download.
    let mut file = tokio::fs::File::open(storage_path)
        .await
        .map_err(|e| MailError::Internal(anyhow::anyhow!("Ouverture fichier: {e}")))?;
    let total = file
        .metadata()
        .await
        .map_err(|e| MailError::Internal(anyhow::anyhow!("Taille fichier: {e}")))?
        .len();

    // `inline` only for the handful of types the viewer previews; anything else
    // is `attachment`, so an HTML/SVG/XML part can never be rendered AS A
    // DOCUMENT in Kubuno's own origin (which would run with the reader's
    // session). Control characters — bidi overrides above all — are stripped
    // from the filename so `facture\u{202e}exe.pdf` cannot masquerade.
    let safe_name: String = name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '"' | '\\') { '_' } else { c })
        .collect();
    let disposition = format!(
        "{}; filename=\"{safe_name}\"",
        if inline_ok { "inline" } else { "attachment" }
    );

    // A byte-range request lets the mobile client resume an interrupted download
    // and stream large files. An absent or unparseable Range is served whole.
    match parse_byte_range(headers.get(header::RANGE), total) {
        ByteRange::None => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime_type)
            .header(header::CONTENT_DISPOSITION, disposition)
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            // Belt and braces: even served as a document, this response gets an
            // opaque origin and no scripting — it cannot reach the session.
            .header(header::CONTENT_SECURITY_POLICY, ATTACHMENT_CSP)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, total)
            .body(Body::from_stream(ReaderStream::new(file)))
            .map_err(|e| MailError::Internal(anyhow::anyhow!("Build response: {e}"))),

        ByteRange::Satisfiable { start, end } => {
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|e| MailError::Internal(anyhow::anyhow!("Seek fichier: {e}")))?;
            let len = end - start + 1;
            // `.take(len)` bounds the stream to the requested slice.
            let stream = ReaderStream::new(file.take(len));
            Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, mime_type)
                .header(header::CONTENT_DISPOSITION, disposition)
                .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                .header(header::CONTENT_SECURITY_POLICY, ATTACHMENT_CSP)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}"))
                .header(header::CONTENT_LENGTH, len)
                .body(Body::from_stream(stream))
                .map_err(|e| MailError::Internal(anyhow::anyhow!("Build response: {e}")))
        }

        ByteRange::Unsatisfiable => Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{total}"))
            .body(Body::empty())
            .map_err(|e| MailError::Internal(anyhow::anyhow!("Build response: {e}"))),
    }
}

/// The outcome of interpreting a `Range` request header against a known total
/// size.
enum ByteRange {
    /// No range asked (or a header we choose to ignore): serve the whole file.
    None,
    /// A valid, in-bounds single range `[start, end]` (inclusive).
    Satisfiable { start: u64, end: u64 },
    /// A syntactically valid range that cannot be served against this size.
    Unsatisfiable,
}

/// Parses a single-range `Range: bytes=…` header (RFC 7233).
///
/// Supports `bytes=start-end`, `bytes=start-` and the suffix form `bytes=-n`.
/// A missing, malformed or multi-range header is IGNORED (whole file, `200`),
/// which the spec permits; only a well-formed but out-of-bounds range is
/// reported unsatisfiable (`416`).
fn parse_byte_range(header: Option<&HeaderValue>, total: u64) -> ByteRange {
    let Some(raw) = header.and_then(|v| v.to_str().ok()) else {
        return ByteRange::None;
    };
    let Some(spec) = raw.trim().strip_prefix("bytes=") else {
        return ByteRange::None;
    };
    // Only single ranges are supported; a multi-range request is served whole.
    if spec.contains(',') {
        return ByteRange::None;
    }
    let Some((s, e)) = spec.split_once('-') else {
        return ByteRange::None;
    };
    let (s, e) = (s.trim(), e.trim());

    // No content can satisfy any concrete range.
    if total == 0 {
        return ByteRange::Unsatisfiable;
    }

    let (start, end) = if s.is_empty() {
        // Suffix range: the last `n` bytes.
        let Ok(n) = e.parse::<u64>() else {
            return ByteRange::None;
        };
        if n == 0 {
            return ByteRange::Unsatisfiable;
        }
        let n = n.min(total);
        (total - n, total - 1)
    } else {
        let Ok(start) = s.parse::<u64>() else {
            return ByteRange::None;
        };
        let end = if e.is_empty() {
            total - 1
        } else {
            match e.parse::<u64>() {
                Ok(v) => v.min(total - 1),
                Err(_) => return ByteRange::None,
            }
        };
        (start, end)
    };

    if start > end || start >= total {
        return ByteRange::Unsatisfiable;
    }
    ByteRange::Satisfiable { start, end }
}

/// Removes the server-side `storage_path` from a message's attachments before
/// it leaves the module. The client downloads by `(message id, index)` through
/// the API and never needs — nor should learn — where the file physically
/// lives on the server.
pub(crate) fn strip_storage_paths(msg: &mut EmailMessage) {
    if let Some(arr) = msg.attachments.as_array_mut() {
        for att in arr {
            if let Some(obj) = att.as_object_mut() {
                obj.remove("storage_path");
            }
        }
    }
}

pub async fn delete_message(
    State(state): State<AppState>,
    user: AuthUser,
    Path(msg_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let result = sqlx::query(
        "UPDATE mail.messages SET is_deleted = TRUE, folder = 'trash' WHERE id = $1 AND user_id = $2"
    )
    .bind(msg_id)
    .bind(user.id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Message {msg_id}")));
    }
    Ok(Json(serde_json::json!({ "message": "Message supprimé" })))
}

/// Decrypt / verify a message IN PLACE when it was stored as PGP/MIME (its raw
/// MIME preserved by the sync path) and the reader has OpenPGP enabled with a
/// usable key. On any failure the stored (opaque) body is left untouched — a mail
/// you cannot open must still open, showing the ciphertext, not an error.
///
/// Shared by the single-message and thread endpoints, since a message card is
/// populated from either. Skips the whole path — including the config lookup —
/// for ordinary mail, whose `pgp_raw` is NULL.
pub async fn decode_pgp_in_place(state: &AppState, user_id: Uuid, msg: &mut EmailMessage) {
    // Preserved raw MIME? Only PGP messages have it; everything else exits here.
    let raw: Option<Vec<u8>> = sqlx::query_scalar::<_, Option<Vec<u8>>>(
        "SELECT pgp_raw FROM mail.messages WHERE id = $1",
    )
    .bind(msg.id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .flatten();
    let Some(raw) = raw else { return };

    // Honour the instance switch: OpenPGP off ⇒ leave the message as delivered.
    match crate::handlers::addresses::server_config(state).await {
        Ok(cfg) if cfg.gpg_enabled => {}
        _ => return,
    }

    let Ok(crypto) = MailCrypto::new(&state.settings.mail.encryption_key) else { return };

    // Every one of the reader's secret keys — the message may be encrypted to any
    // of them; decryption tries each until one opens it.
    let key_rows = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
        "SELECT private_key, private_key_nonce FROM mail.pgp_keys WHERE user_id = $1 ORDER BY is_default DESC, created_at",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let secrets: Vec<String> = key_rows
        .iter()
        .filter_map(|(enc, nonce)| crypto.decrypt(enc, nonce).ok())
        .collect();

    // The sender's public key (for signature verification), by their From address.
    let sender_public: Option<String> = sqlx::query_scalar(
        "SELECT public_key FROM mail.pgp_contacts WHERE user_id = $1 AND lower(email) = lower($2) LIMIT 1",
    )
    .bind(user_id)
    .bind(&msg.from_email)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let parsed = match crate::services::pgp_mime::parse_incoming(&raw, &secrets, sender_public.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(msg = %msg.id, error = %e, "Déchiffrement PGP entrant échoué");
            return;
        }
    };

    // Surface the decrypted body when we recovered one; keep the stored body
    // otherwise (e.g. encrypted with a key we do not hold).
    if parsed.body_html.is_some() {
        msg.body_html = parsed.body_html;
    }
    if parsed.body_text.is_some() {
        msg.body_text = parsed.body_text;
    }
    msg.pgp_encrypted = parsed.encrypted;
    if let Some(verdict) = parsed.signed {
        msg.pgp_signature_valid = Some(verdict.valid);
        msg.pgp_signer_fingerprint = verdict.fingerprint;
    }
}

/// Resolve the OpenPGP material for a send: the sender's secret key (for signing)
/// and every recipient's public key (for encryption), or `None` when the message
/// asked for neither. Fails early with a clear message when a required key is
/// missing, rather than silently sending in clear.
async fn resolve_pgp(
    state: &AppState,
    user_id: Uuid,
    from_email: &str,
    dto: &SendMailDto,
) -> Result<Option<crate::services::pgp_mime::PgpParams>, MailError> {
    let sign = dto.sign.unwrap_or(false);
    let encrypt = dto.encrypt.unwrap_or(false);
    if !sign && !encrypt {
        return Ok(None);
    }
    if !crate::handlers::addresses::server_config(state).await?.gpg_enabled {
        return Err(MailError::Forbidden);
    }

    let crypto = MailCrypto::new(&state.settings.mail.encryption_key).map_err(|_| MailError::Crypto)?;

    // The sender's own key: prefer one whose address matches the From, then default.
    let own = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, String)>(
        r#"SELECT private_key, private_key_nonce, public_key
           FROM mail.pgp_keys WHERE user_id = $1
           ORDER BY (lower(email) = lower($2)) DESC, is_default DESC, created_at
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(from_email)
    .fetch_optional(&state.db)
    .await?;

    if sign && own.is_none() {
        return Err(MailError::Validation(
            "Aucune clé OpenPGP : générez-en une dans Réglages ▸ Chiffrement.".into(),
        ));
    }

    let (sender_secret_armored, sender_public) = match &own {
        Some((enc, nonce, public)) => (
            crypto.decrypt(enc, nonce).map_err(|_| MailError::Crypto)?,
            Some(public.clone()),
        ),
        None => (String::new(), None),
    };

    let mut recipient_public_armored = Vec::new();
    if encrypt {
        let mut recipients: Vec<String> = dto.to_addresses.iter().map(|a| a.email.clone()).collect();
        recipients.extend(dto.cc_addresses.as_deref().unwrap_or(&[]).iter().map(|a| a.email.clone()));
        recipients.extend(dto.bcc_addresses.as_deref().unwrap_or(&[]).iter().map(|a| a.email.clone()));

        let wkd_http = reqwest::Client::new();
        for email in &recipients {
            let key: Option<String> = sqlx::query_scalar(
                "SELECT public_key FROM mail.pgp_contacts WHERE user_id = $1 AND lower(email) = lower($2) LIMIT 1",
            )
            .bind(user_id)
            .bind(email)
            .fetch_optional(&state.db)
            .await?;
            match key {
                Some(k) => recipient_public_armored.push(k),
                None => {
                    // No stored key: try Web Key Directory before giving up. On a
                    // hit we cache it as a contact (source='wkd') so the next send
                    // is offline, then encrypt to it.
                    match crate::services::wkd::discover(&wkd_http, email).await {
                        Some((armored, fingerprint)) => {
                            store_discovered_contact(state, user_id, email, &armored, &fingerprint, "wkd").await;
                            recipient_public_armored.push(armored);
                        }
                        None => {
                            return Err(MailError::Validation(format!(
                                "Pas de clé publique pour {email} — introuvable via WKD ; importez-la dans Réglages ▸ Chiffrement."
                            )))
                        }
                    }
                }
            }
        }
        // Encrypt to ourselves too, so the Sent copy stays readable.
        if let Some(pub_own) = sender_public {
            recipient_public_armored.push(pub_own);
        }
    }

    Ok(Some(crate::services::pgp_mime::PgpParams {
        sign,
        encrypt,
        sender_secret_armored,
        recipient_public_armored,
    }))
}

/// Cache a public key learned out-of-band (WKD or Autocrypt) as a correspondent
/// contact. A rotated key replaces the stored one (upsert on `lower(email)`).
/// Best-effort: a storage failure must not fail the send that discovered it.
pub(crate) async fn store_discovered_contact(
    state: &AppState,
    user_id: Uuid,
    email: &str,
    public_armored: &str,
    fingerprint: &str,
    source: &str,
) {
    let res = sqlx::query(
        r#"INSERT INTO mail.pgp_contacts (user_id, email, fingerprint, public_key, source)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (user_id, lower(email))
           DO UPDATE SET fingerprint = EXCLUDED.fingerprint,
                         public_key  = EXCLUDED.public_key,
                         source      = EXCLUDED.source"#,
    )
    .bind(user_id)
    .bind(email.to_lowercase())
    .bind(fingerprint)
    .bind(public_armored)
    .bind(source)
    .execute(&state.db)
    .await;
    if let Err(e) = res {
        tracing::warn!(email, source, error = %e, "Stockage clé publique découverte échoué");
    }
}

/// The armored public key to advertise in an outgoing `Autocrypt:` header for
/// `from_email`, or `None` when OpenPGP is off instance-wide or the sender holds
/// no key. Prefers a key whose User ID matches the From address, else the default.
pub(crate) async fn sender_autocrypt_key(
    state: &AppState,
    user_id: Uuid,
    from_email: &str,
) -> Option<String> {
    // Query the local key FIRST: the common sender holds none, and this avoids an
    // internal HTTP round-trip (server_config) on every send. Only a sender who
    // actually has a key pays the switch check.
    let key: String = sqlx::query_scalar(
        r#"SELECT public_key FROM mail.pgp_keys WHERE user_id = $1
           ORDER BY (lower(email) = lower($2)) DESC, is_default DESC, created_at
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(from_email)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()?;

    if !crate::handlers::addresses::server_config(state).await.ok()?.gpg_enabled {
        return None;
    }
    Some(key)
}

/// Attach the composer-picked labels to the Sent copy's thread. Only labels the
/// user actually OWNS are applied (filtered in SQL by `user_id`), so a foreign
/// or stale id is silently dropped rather than turning a successful send into an
/// error. Best-effort: a labelling failure only gets logged — the mail already
/// left. Uses `mail.thread_labels`, the same link table every other labelling
/// path writes to (handlers::labels, filters, sync).
async fn apply_sent_labels(db: &sqlx::PgPool, user_id: Uuid, thread_id: Uuid, requested: &[Uuid]) {
    if requested.is_empty() {
        return;
    }
    // Which of the requested ids does this user own? Ownership is enforced here.
    let owned: Vec<Uuid> = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM mail.labels WHERE id = ANY($1) AND user_id = $2",
    )
    .bind(requested)
    .bind(user_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let to_apply = retain_owned_labels(requested, &owned);
    if to_apply.is_empty() {
        return;
    }

    if let Err(e) = sqlx::query(
        "INSERT INTO mail.thread_labels (thread_id, label_id) \
         SELECT $1, unnest($2::uuid[]) ON CONFLICT DO NOTHING",
    )
    .bind(thread_id)
    .bind(&to_apply)
    .execute(db)
    .await
    {
        tracing::error!(error = %e, %thread_id, "Application des libellés à la copie « envoyés » échouée");
    }
}

/// Keep only the requested label ids the user owns, preserving request order and
/// dropping duplicates. Pure companion to `apply_sent_labels`, isolated so the
/// ownership-filtering decision is unit-testable without a database.
fn retain_owned_labels(requested: &[Uuid], owned: &[Uuid]) -> Vec<Uuid> {
    let owned: std::collections::HashSet<Uuid> = owned.iter().copied().collect();
    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    requested
        .iter()
        .copied()
        .filter(|id| owned.contains(id) && seen.insert(*id))
        .collect()
}


// ── Calendar invitation RSVP (iMIP) ─────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct InviteReplyDto {
    /// accepted | tentative | declined
    pub response: String,
    /// Optional note for the organizer — typically the reason for a decline.
    /// Travels as the iCalendar COMMENT of the reply.
    #[serde(default)]
    pub comment: Option<String>,
    /// An alternative time the invitee suggests. Turns the message into an iTIP
    /// COUNTER: same event, proposed at this time instead.
    #[serde(default)]
    pub proposed_start: Option<String>,
    #[serde(default)]
    pub proposed_end: Option<String>,
}

/// Reply to a calendar invitation the way Gmail does: email the organizer a
/// `text/calendar; method=REPLY` with our PARTSTAT, and remember the answer on
/// the message so the card shows it on reload. Adding the event to the calendar
/// is done client-side (via the calendar module), so this endpoint only sends
/// the iMIP reply and records the choice.
pub async fn invite_reply(
    State(state): State<AppState>,
    user: AuthUser,
    Path(msg_id): Path<Uuid>,
    Json(dto): Json<InviteReplyDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    let rsvp = crate::services::imip::Rsvp::parse(dto.response.trim())
        .ok_or_else(|| MailError::Validation("Réponse d'invitation invalide".into()))?;

    // Load just what we need: the message's account, subject and structured data.
    let row = sqlx::query_as::<_, (Uuid, String, Option<serde_json::Value>)>(
        "SELECT account_id, subject, structured_data FROM mail.messages \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(msg_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Message {msg_id}")))?;
    let (account_id, subject, structured_data) = row;

    // Find the invitation node (an ICS-sourced Event) and its reply fields.
    let node = structured_data
        .as_ref()
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find(|n| {
                n.get("_source").and_then(|s| s.as_str()) == Some("ics")
                    && n.get("_invite").and_then(|b| b.as_bool()) == Some(true)
            })
        })
        .ok_or_else(|| MailError::Validation("Ce message n'est pas une invitation".into()))?;

    let organizer_email = node.get("organizerEmail").and_then(|v| v.as_str());
    let request_ics = node.get("_ics").and_then(|v| v.as_str()).unwrap_or("");
    let summary = node
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(subject.as_str())
        .to_string();

    // Always remember the choice, even when no organizer address is available to
    // reply to (some invites omit a usable ORGANIZER mailto).
    sqlx::query("UPDATE mail.messages SET invite_response = $1 WHERE id = $2 AND user_id = $3")
        .bind(rsvp.as_db())
        .bind(msg_id)
        .bind(user.id)
        .execute(&state.db)
        .await?;

    let Some(organizer_email) = organizer_email.filter(|e| !e.is_empty()) else {
        return Ok(Json(serde_json::json!({ "invite_response": rsvp.as_db(), "sent": false })));
    };

    // Our own address is the ATTENDEE and the From of the reply.
    let account = sqlx::query_as::<_, EmailAccount>(
        r#"SELECT id, user_id, name, email_address, kind, mailbox_id,
                  incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  smtp_host, smtp_port, smtp_security, smtp_username, auth_kind,
                  is_default, is_active, last_sync_at, last_error,
                  created_at, updated_at
           FROM mail.accounts WHERE id = $1 AND user_id = $2"#,
    )
    .bind(account_id)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    let proposal = dto
        .proposed_start
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|start| crate::services::imip::Proposal {
            start: start.to_string(),
            end: dto.proposed_end.clone().filter(|e| !e.trim().is_empty()),
        });
    let reply_ics = crate::services::imip::build_reply(
        request_ics,
        &account.email_address,
        Some(&account.name),
        rsvp,
        dto.comment.as_deref(),
        proposal.as_ref(),
    );

    let verb = match rsvp {
        crate::services::imip::Rsvp::Accepted => "Accepté",
        crate::services::imip::Rsvp::Tentative => "Provisoire",
        crate::services::imip::Rsvp::Declined => "Refusé",
    };
    // A counter-proposal is announced as such: the organizer is being asked to
    // move the event, not merely told we decline.
    let heading = if proposal.is_some() {
        format!("Nouvel horaire proposé : {}", html_escape(&summary))
    } else {
        format!("{verb} : {}", html_escape(&summary))
    };
    let note = dto
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| format!("<p>{}</p>", html_escape(c)))
        .unwrap_or_default();
    let body_html = format!("<p>{heading}</p>{note}");
    let ics_b64 = base64::engine::general_purpose::STANDARD.encode(reply_ics.as_bytes());
    let send_dto = SendMailDto {
        account_id,
        to_addresses: vec![crate::models::EmailAddress {
            name: None,
            email: organizer_email.to_string(),
        }],
        cc_addresses: None,
        bcc_addresses: None,
        subject: if proposal.is_some() {
            format!("Nouvel horaire proposé : {summary}")
        } else {
            format!("{verb} : {summary}")
        },
        body_html,
        reply_to_id: None,
        draft_id: None,
        scheduled_at: None,
        attachments: Some(vec![crate::models::AttachmentInput {
            filename: "invite.ics".into(),
            mime: "text/calendar; method=REPLY; charset=utf-8".into(),
            content: ics_b64,
        }]),
        sign: None,
        encrypt: None,
        label_ids: None,
    };

    if let Err(e) = send_message_inner(&state, user.id, &send_dto, None).await {
        tracing::error!(error = %e, msg_id = %msg_id, "Envoi de la réponse d'invitation échoué");
        // The RSVP is already recorded; report that the reply mail didn't go out.
        return Ok(Json(serde_json::json!({ "invite_response": rsvp.as_db(), "sent": false })));
    }

    Ok(Json(serde_json::json!({ "invite_response": rsvp.as_db(), "sent": true })))
}

/// Minimal HTML escape for the one-line reply body.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod attachment_mime_tests {
    use super::safe_inline_mime;

    /// Anything that can execute or carry markup loses its Content-Type, so the
    /// browser downloads opaque bytes instead of rendering a document in our
    /// origin. The two-part trick — an HTML part plus a JS part passing nosniff
    /// — is what defeats `script-src 'self'`, so `application/javascript` must
    /// never be echoed back either.
    #[test]
    fn active_types_are_never_echoed() {
        for claimed in [
            "text/html",
            "TEXT/HTML; charset=utf-8",
            "image/svg+xml",
            "application/xhtml+xml",
            "application/javascript",
            "text/javascript",
            "application/x-javascript",
            "text/xml",
            "application/xml",
            "text/plain",
            "application/octet-stream",
            "",
        ] {
            assert!(safe_inline_mime(claimed).is_none(), "« {claimed} » ne doit pas être renvoyé tel quel");
        }
    }

    /// …while what a viewer legitimately previews keeps its type.
    #[test]
    fn previewable_types_survive() {
        assert_eq!(safe_inline_mime("image/png"), Some("image/png"));
        assert_eq!(safe_inline_mime("IMAGE/JPEG; name=\"x.jpg\""), Some("image/jpeg"));
        assert_eq!(safe_inline_mime("application/pdf"), Some("application/pdf"));
    }
}

#[cfg(test)]
mod tests {
    use super::retain_owned_labels;
    use uuid::Uuid;

    #[test]
    fn foreign_labels_are_refused() {
        let mine_a = Uuid::new_v4();
        let mine_b = Uuid::new_v4();
        let someone_else = Uuid::new_v4();

        // The user asked for two of their own labels plus one belonging to
        // another user; only the owned pair survives, foreign id is dropped.
        let requested = [mine_a, someone_else, mine_b];
        let owned = [mine_a, mine_b];
        let applied = retain_owned_labels(&requested, &owned);

        assert_eq!(applied, vec![mine_a, mine_b]);
        assert!(!applied.contains(&someone_else));
    }

    #[test]
    fn duplicates_and_unknown_ids_are_dropped() {
        let mine = Uuid::new_v4();
        let requested = [mine, mine, Uuid::new_v4()];
        let owned = [mine];
        // Deduped to a single entry; the unknown/unowned id never appears.
        assert_eq!(retain_owned_labels(&requested, &owned), vec![mine]);
    }

    #[test]
    fn empty_request_applies_nothing() {
        let owned = [Uuid::new_v4()];
        assert!(retain_owned_labels(&[], &owned).is_empty());
    }
}
