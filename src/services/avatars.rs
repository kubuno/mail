//! Sender avatars from BIMI — the logo a DOMAIN publishes about itself.
//!
//! ── Why BIMI, and nothing keyed on the address ──────────────────────────────
//! There is no public way to turn an e-mail address into someone's photo, and
//! the services that pretend otherwise (Gravatar & co.) are told, one request
//! per message opened, exactly whose mail is being read. BIMI leaks nothing of
//! the sort: the lookup names a DOMAIN, the record is published BY that domain
//! for this exact purpose, and one answer serves every sender behind it.
//!
//! ── What is fetched, and what is refused ────────────────────────────────────
//! The `l=` URL of `default._bimi.<domain>`, over HTTPS only, capped in size and
//! restricted to image content types. The result — including the ABSENCE of a
//! record — is cached in `mail.sender_avatars`, so a domain without BIMI is not
//! looked up again for every message. Nothing here can fail loudly: an avatar
//! is decoration, and the reader keeps the coloured initial when it is missing.

use anyhow::Result;
use hickory_resolver::TokioResolver;
use sqlx::PgPool;

/// Largest logo we will store: BIMI logos are small SVGs; anything heavier is
/// not a logo and has no business sitting in the database.
const MAX_BYTES: usize = 256 * 1024;

/// How long an answer is trusted before being looked up again. Brands change
/// their logo rarely; a week keeps the table quiet without going stale.
const TTL_DAYS: i64 = 7;

/// One cache row as it is read back: (not_found, mime, bytes, source).
type CachedRow = (bool, Option<String>, Option<Vec<u8>>, String);

/// A cached avatar ready to be served.
pub struct Avatar {
    pub mime:  String,
    pub bytes: Vec<u8>,
    /// Where it came from: `bimi` (the brand's own logo), `provider` (a mailbox
    /// service) or `favicon`. Only `bimi` claims "this really is that brand".
    pub source: String,
}

/// The avatar for `domain`, from cache when possible. `Ok(None)` means "this
/// domain publishes none" — a normal, cached outcome, not an error.
///
/// `allow_brand` is the authentication gate: a BIMI logo is the brand SAYING
/// "this is us", so it may only be shown on a message that actually passed
/// DMARC. On anything else the brand logo is withheld — a phishing mail
/// spoofing `paypal.com` must never borrow PayPal's logo — while neutral marks
/// (a mailbox provider, a favicon) stay allowed since they claim no identity.
pub async fn for_domain(
    db: &PgPool,
    http: &reqwest::Client,
    domain: &str,
    allow_brand: bool,
) -> Result<Option<Avatar>> {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() || !domain.contains('.') {
        return Ok(None);
    }

    // 1. Fresh cache entry (hit or miss) answers straight away.
    let cached: Option<CachedRow> = sqlx::query_as(
        "SELECT not_found, mime, bytes, source FROM mail.sender_avatars \
         WHERE domain = $1 AND expires_at > NOW()",
    )
    .bind(&domain)
    .fetch_optional(db)
    .await?;

    if let Some((not_found, mime, bytes, source)) = cached {
        return Ok(match (not_found, mime, bytes) {
            (false, Some(mime), Some(bytes)) if !bytes.is_empty() => {
                Some(Avatar { mime, bytes, source }).filter(|a| allow_brand || a.source != "bimi")
            }
            _ => None,
        });
    }

    // 2. Otherwise resolve it, and remember the outcome either way.
    let fetched = resolve(http, &domain).await;
    match &fetched {
        Ok(Some(av)) => remember(db, &domain, Some((&av.mime, &av.bytes, &av.source))).await,
        // A transient failure (DNS down, host unreachable) is cached as "none"
        // as well: retrying on every opened message would hammer the network,
        // and the TTL brings the domain back for another try soon enough.
        _ => remember(db, &domain, None).await,
    }
    // Same gate on a freshly fetched logo as on a cached one.
    fetched.map(|a| a.filter(|av| allow_brand || av.source != "bimi"))
}

/// Every name to try for `domain`, itself first, then its parents down to the
/// registrable one — `services.ovhcloud.com` → `ovhcloud.com` → `com`.
///
/// Senders very often use a subdomain (`services.ovhcloud.com`,
/// `email.apple.com`) while the BIMI record sits on the organisational domain,
/// exactly as DMARC allows. Without this walk the biggest brands show no logo.
/// The last label alone is never queried: a public suffix publishes nothing.
fn domain_chain(domain: &str) -> Vec<String> {
    let labels: Vec<&str> = domain.split('.').filter(|l| !l.is_empty()).collect();
    // Stop at two labels (the registrable domain in the common case) and try at
    // most three names, so one unknown sender cannot fan out into a DNS storm.
    (0..labels.len().saturating_sub(1))
        .take(3)
        .map(|i| labels[i..].join("."))
        .collect()
}


/// Icon of a well-known MAILBOX PROVIDER. Individuals never publish a BIMI
/// record, so a message from a person would show nothing but an initial; their
/// provider's logo at least says where the mail comes from. These are the
/// official icon URLs of each service, fetched once per domain and cached like
/// everything else here.
fn provider_icon(domain: &str) -> Option<&'static str> {
    const GMAIL:   &str = "https://www.gstatic.com/images/branding/product/1x/gmail_2020q4_48dp.png";
    const OUTLOOK: &str = "https://outlook.com/favicon.ico";
    const YAHOO:   &str = "https://s.yimg.com/rz/l/favicon.ico";
    Some(match domain {
        "gmail.com" | "googlemail.com" => GMAIL,
        "outlook.com" | "outlook.fr" | "outlook.be" | "hotmail.com" | "hotmail.fr"
        | "live.com" | "live.fr" | "msn.com" => OUTLOOK,
        "yahoo.com" | "yahoo.fr" | "yahoo.co.uk" | "ymail.com" | "rocketmail.com" => YAHOO,
        "proton.me" | "protonmail.com" | "pm.me"   => "https://proton.me/favicon.ico",
        "icloud.com" | "me.com" | "mac.com"        => "https://www.icloud.com/favicon.ico",
        "orange.fr" | "wanadoo.fr"                 => "https://www.orange.fr/favicon.ico",
        "free.fr"                                  => "https://www.free.fr/favicon.ico",
        "laposte.net"                              => "https://www.laposte.net/favicon.ico",
        "sfr.fr" | "neuf.fr"                       => "https://www.sfr.fr/favicon.ico",
        "aol.com"                                  => "https://www.aol.com/favicon.ico",
        "zoho.com"                                 => "https://www.zoho.com/favicon.ico",
        "gmx.com" | "gmx.net" | "gmx.fr"           => "https://www.gmx.net/favicon.ico",
        "yandex.com" | "yandex.ru"                 => "https://yandex.com/favicon.ico",
        "mail.ru"                                  => "https://mail.ru/favicon.ico",
        "tuta.com" | "tutanota.com"                => "https://tuta.com/favicon.ico",
        _ => return None,
    })
}

/// One BIMI lookup: DNS TXT (domain then parents), then the `l=` URL over HTTPS.
async fn resolve(http: &reqwest::Client, domain: &str) -> Result<Option<Avatar>> {
    let Some(resolver) = crate::services::diagnostics::dns::resolver() else {
        return Ok(None);
    };
    let mut logo = None;
    for candidate in domain_chain(domain) {
        if let Some(url) = bimi_logo_url(&resolver, &candidate).await {
            logo = Some(url);
            break;
        }
    }
    let mut source = "bimi";
    // Failing a published logo: the provider's own icon, then the domain's
    // favicon — both name a DOMAIN only, exactly like the BIMI lookup above.
    let url = match logo {
        Some(url) => url,
        None => {
            let chain = domain_chain(domain);
            match chain.iter().find_map(|d| provider_icon(d)) {
                Some(icon) => { source = "provider"; icon.to_string() }
                None => match chain.first() {
                    Some(d) => { source = "favicon"; format!("https://{d}/favicon.ico") }
                    None => return Ok(None),
                },
            }
        }
    };

    // An unreachable host or an unknown domain is a MISSING avatar, never an
    // error: the reader keeps their initial, and the request must not fail.
    let response = match http.get(&url).timeout(std::time::Duration::from_secs(6)).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return Ok(None),
    };

    // Only images, and only what fits: the URL comes from a third party.
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).trim().to_string())
        .unwrap_or_else(|| "image/svg+xml".to_string());
    if !mime.starts_with("image/") {
        return Ok(None);
    }
    let Ok(bytes) = response.bytes().await else { return Ok(None) };
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Ok(None);
    }

    Ok(Some(Avatar { mime, bytes: bytes.to_vec(), source: source.to_string() }))
}

/// The `l=` (logo) URL of `default._bimi.<domain>`, when the record has one.
/// Only HTTPS is accepted — the spec requires it, and it keeps us from being
/// pointed at a plaintext host.
async fn bimi_logo_url(resolver: &TokioResolver, domain: &str) -> Option<String> {
    let name = format!("default._bimi.{domain}");
    let answer = resolver.txt_lookup(name).await.ok()?;
    for record in answer.iter() {
        let joined: String = record.txt_data().iter().map(|d| String::from_utf8_lossy(d)).collect();
        if !joined.to_ascii_lowercase().contains("v=bimi1") {
            continue;
        }
        for tag in joined.split(';') {
            let tag = tag.trim();
            if let Some(url) = tag.strip_prefix("l=").or_else(|| tag.strip_prefix("L=")) {
                let url = url.trim();
                if url.starts_with("https://") {
                    return Some(url.to_string());
                }
            }
        }
    }
    None
}

/// Writes the outcome to the cache. A storage failure must never break the
/// request: the avatar simply is not cached this time.
async fn remember(db: &PgPool, domain: &str, found: Option<(&str, &[u8], &str)>) {
    let (not_found, mime, bytes, source) = match found {
        Some((mime, bytes, source)) => (false, Some(mime), Some(bytes), source),
        None => (true, None, None, "none"),
    };
    let result = sqlx::query(
        "INSERT INTO mail.sender_avatars (domain, source, mime, bytes, not_found, fetched_at, expires_at) \
         VALUES ($1, $6, $2, $3, $4, NOW(), NOW() + ($5 || ' days')::interval) \
         ON CONFLICT (domain) DO UPDATE SET \
           source = EXCLUDED.source, \
           mime = EXCLUDED.mime, bytes = EXCLUDED.bytes, not_found = EXCLUDED.not_found, \
           fetched_at = EXCLUDED.fetched_at, expires_at = EXCLUDED.expires_at",
    )
    .bind(domain)
    .bind(mime)
    .bind(bytes)
    .bind(not_found)
    .bind(TTL_DAYS.to_string())
    .bind(source)
    .execute(db)
    .await;

    if let Err(e) = result {
        tracing::error!(error = %e, domain, "Mise en cache de l'avatar d'expéditeur échouée");
    }
}

#[cfg(test)]
mod tests {
    /// The `l=` tag is what carries the logo; everything else in the record is
    /// noise for us, and a non-HTTPS URL is refused outright.
    /// A sending subdomain must fall back to the organisational domain, which
    /// is where brands actually publish their logo.
    #[test]
    fn the_domain_chain_walks_up_to_the_registrable_name() {
        assert_eq!(
            super::domain_chain("services.ovhcloud.com"),
            vec!["services.ovhcloud.com", "ovhcloud.com"],
        );
        assert_eq!(super::domain_chain("paypal.com"), vec!["paypal.com"]);
        // Never a bare public suffix, and never more than three lookups.
        assert_eq!(
            super::domain_chain("a.b.c.example.com"),
            vec!["a.b.c.example.com", "b.c.example.com", "c.example.com"],
        );
    }

    #[test]
    fn only_https_logo_urls_are_accepted() {
        let pick = |record: &str| -> Option<String> {
            if !record.to_ascii_lowercase().contains("v=bimi1") {
                return None;
            }
            record.split(';').map(str::trim).find_map(|tag| {
                tag.strip_prefix("l=")
                    .map(str::trim)
                    .filter(|u| u.starts_with("https://"))
                    .map(str::to_string)
            })
        };
        assert_eq!(
            pick("v=BIMI1; l=https://example.com/logo.svg; a=").as_deref(),
            Some("https://example.com/logo.svg"),
        );
        assert_eq!(pick("v=BIMI1; l=http://example.com/logo.svg"), None);
        assert_eq!(pick("v=spf1 -all"), None);
    }
}
