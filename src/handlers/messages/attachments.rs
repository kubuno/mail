use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, Response, StatusCode},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{errors::MailError, middleware::AuthUser, state::AppState};

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
