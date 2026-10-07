//! Outbound fetches triggered by a THIRD PARTY, kept away from our own network.
//!
//! Several features go and fetch a URL whose host is chosen by whoever sent the
//! message — the sender's domain (favicon), a URL published in that domain's
//! BIMI record, a WKD key directory. Left unguarded that is a server-side
//! request forgery: `From: x@127.0.0.1` makes the server probe itself, and a
//! hostile DNS zone can point at `169.254.169.254` (cloud metadata) or at
//! anything on the internal network. The reply is invisible to the attacker, but
//! reachability and timing already answer "what is running in there?".
//!
//! Two rules, both needed:
//!  * the address must be PUBLIC — every address the host resolves to is
//!    checked, so a name that resolves to a private range is refused;
//!  * redirects are NOT followed — otherwise a public host would simply bounce
//!    us to `http://169.254.169.254/…` and undo the check above.

use std::net::IpAddr;
use std::time::Duration;

/// An HTTP client for third-party URLs: short timeouts, and no redirects at all
/// (a redirect would move the request to a host nobody validated).
pub fn guarded_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(6))
        .user_agent("Kubuno-Mail")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Addresses that must never be reached on behalf of a third party: our own
/// host, the private ranges, the link-local range that carries cloud metadata,
/// and everything that is not a plain routable unicast address.
fn is_forbidden(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()      // 169.254/16 — cloud metadata lives here
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || v4.octets()[0] == 0
                // 100.64/10, carrier-grade NAT — routable-looking, not public.
                || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]))
        }
        IpAddr::V6(v6) => {
            // An IPv4-mapped address is an IPv4 address wearing a costume.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_forbidden(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00  // fc00::/7 unique local
                || (v6.segments()[0] & 0xffc0) == 0xfe80  // fe80::/10 link-local
        }
    }
}

/// Resolves `host` and reports whether EVERY address it answers with is public.
/// A host that does not resolve is refused too: there is nothing to reach.
pub async fn host_is_public(host: &str, port: u16) -> bool {
    match tokio::net::lookup_host((host, port)).await {
        Ok(addrs) => {
            let mut any = false;
            for addr in addrs {
                any = true;
                if is_forbidden(addr.ip()) {
                    tracing::warn!(host, ip = %addr.ip(), "URL tierce pointant vers une adresse interne — refusée");
                    return false;
                }
            }
            any
        }
        Err(_) => false,
    }
}

/// GETs a third-party URL, or `None` when it is not safe to: only http(s), only
/// a host that resolves entirely to public addresses.
pub async fn guarded_get(http: &reqwest::Client, url: &str) -> Option<reqwest::Response> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?;
    let port = parsed.port_or_known_default().unwrap_or(443);
    // A literal IP in the URL is checked directly — no lookup needed, and no
    // chance of a resolver returning something else.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_forbidden(ip) {
            tracing::warn!(host, "URL tierce pointant vers une adresse interne — refusée");
            return None;
        }
    } else if !host_is_public(host, port).await {
        return None;
    }
    http.get(url).send().await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_addresses_are_refused() {
        for ip in [
            "127.0.0.1", "127.1.2.3", "10.0.0.5", "172.16.4.4", "192.168.1.1",
            "169.254.169.254", // cloud metadata
            "100.64.0.1",      // CGNAT
            "0.0.0.0", "255.255.255.255", "224.0.0.1",
            "::1", "fe80::1", "fc00::1", "fd00::abcd", "::ffff:127.0.0.1", "::ffff:10.0.0.1",
        ] {
            assert!(is_forbidden(ip.parse().unwrap()), "{ip} devrait être refusée");
        }
    }

    #[test]
    fn public_addresses_pass() {
        for ip in ["93.184.216.34", "8.8.8.8", "1.1.1.1", "2606:2800:220:1:248:1893:25c8:1946"] {
            assert!(!is_forbidden(ip.parse().unwrap()), "{ip} devrait être acceptée");
        }
    }

    #[tokio::test]
    async fn a_literal_internal_url_is_refused_without_resolving() {
        let http = guarded_client();
        assert!(guarded_get(&http, "http://127.0.0.1:8080/x").await.is_none());
        assert!(guarded_get(&http, "https://169.254.169.254/latest/meta-data/").await.is_none());
        // …and a scheme that is not http(s) never leaves either.
        assert!(guarded_get(&http, "file:///etc/passwd").await.is_none());
    }
}
