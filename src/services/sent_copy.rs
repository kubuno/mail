//! Local "Sent" copy of an outgoing message.
//!
//! SMTP submission does not place a copy in the account's Sent folder (that is
//! a client's job, and most servers do not do it automatically), so without
//! this the Sent view stays empty. Called after a successful SMTP send, both
//! for direct sends and for the scheduled-send worker.

use anyhow::{Context, Result};
use base64::Engine;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{EmailAccount, SendMailDto};

/// Stores the sent message (thread + message, folder='sent') atomically.
/// Attachments are written to `attachments_dir` so download works like for
/// synced messages.
pub async fn store_sent_copy(
    db: &PgPool,
    account: &EmailAccount,
    dto: &SendMailDto,
    body_html: &str,
    message_id: &str,
    attachments_dir: &str,
) -> Result<()> {
    let body_text = html2text::from_read(body_html.as_bytes(), 80);
    let snippet: String = body_text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect();

    // Write attachments to disk and build the metadata array.
    let inputs = dto.attachments.as_deref().unwrap_or(&[]);
    let mut attachments_meta: Vec<serde_json::Value> = Vec::with_capacity(inputs.len());
    for a in inputs {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(a.content.as_bytes())
            .context("Pièce jointe base64 invalide")?;
        let safe_name: String = a
            .filename
            .chars()
            .map(|c| if c == '/' || c == '\\' || c == '\0' { '_' } else { c })
            .collect();
        let path = format!("{}/{}_{}", attachments_dir.trim_end_matches('/'), Uuid::new_v4(), safe_name);
        tokio::fs::create_dir_all(attachments_dir).await.ok();
        tokio::fs::write(&path, &bytes)
            .await
            .with_context(|| format!("Écriture pièce jointe {path}"))?;
        attachments_meta.push(serde_json::json!({
            "name": a.filename,
            "mime": a.mime,
            "size": bytes.len(),
            "storage_path": path,
        }));
    }
    let has_attachments = !attachments_meta.is_empty();

    // Reply: attach the copy to the existing conversation; also fetch the
    // replied message's Message-ID for the In-Reply-To column.
    let mut reply_thread: Option<Uuid> = None;
    let mut in_reply_to: Option<String> = None;
    if let Some(reply_id) = dto.reply_to_id {
        if let Some((tid, mid)) = sqlx::query_as::<_, (Uuid, Option<String>)>(
            "SELECT thread_id, message_id FROM mail.messages WHERE id = $1 AND user_id = $2",
        )
        .bind(reply_id)
        .bind(account.user_id)
        .fetch_optional(db)
        .await?
        {
            reply_thread = Some(tid);
            in_reply_to = mid;
        }
    }

    let mut tx = db.begin().await?;

    let thread_id: Uuid = if let Some(tid) = reply_thread {
        sqlx::query(
            r#"UPDATE mail.threads
               SET message_count   = message_count + 1,
                   last_message_at = NOW(),
                   snippet         = $2,
                   has_attachments = has_attachments OR $3,
                   last_sender_name  = $4,
                   last_sender_email = $5
               WHERE id = $1"#,
        )
        .bind(tid)
        .bind(&snippet)
        .bind(has_attachments)
        .bind(&account.name)
        .bind(&account.email_address)
        .execute(&mut *tx)
        .await?;
        tid
    } else {
        sqlx::query_scalar::<_, Uuid>(
            r#"INSERT INTO mail.threads
               (account_id, user_id, subject, message_count, unread_count, has_attachments,
                snippet, last_sender_name, last_sender_email, last_message_at)
               VALUES ($1, $2, $3, 1, 0, $4, $5, $6, $7, NOW())
               RETURNING id"#,
        )
        .bind(account.id)
        .bind(account.user_id)
        .bind(&dto.subject)
        .bind(has_attachments)
        .bind(&snippet)
        .bind(&account.name)
        .bind(&account.email_address)
        .fetch_one(&mut *tx)
        .await?
    };

    sqlx::query(
        r#"INSERT INTO mail.messages
           (id, thread_id, account_id, user_id, message_id, in_reply_to, imap_uid, imap_folder,
            from_name, from_email, to_addresses, cc_addresses, bcc_addresses,
            subject, body_text, body_html, attachments, is_read, folder, sent_at, received_at)
           VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, NULL, 'Sent',
                   $6, $7, $8, $9, $10, $11, $12, $13, $14, TRUE, 'sent', NOW(), NOW())"#,
    )
    .bind(thread_id)
    .bind(account.id)
    .bind(account.user_id)
    .bind(message_id)
    .bind(in_reply_to)
    .bind(&account.name)
    .bind(&account.email_address)
    .bind(serde_json::to_value(&dto.to_addresses).unwrap_or_else(|_| serde_json::json!([])))
    .bind(serde_json::to_value(dto.cc_addresses.clone().unwrap_or_default()).unwrap_or_else(|_| serde_json::json!([])))
    .bind(serde_json::to_value(dto.bcc_addresses.clone().unwrap_or_default()).unwrap_or_else(|_| serde_json::json!([])))
    .bind(&dto.subject)
    .bind(&body_text)
    .bind(body_html)
    .bind(serde_json::Value::Array(attachments_meta))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}
