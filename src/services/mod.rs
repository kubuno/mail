pub mod provisioning;
pub mod autocrypt;
pub mod avatars;
pub mod address_index;
pub mod wkd;
pub mod categorize;
pub mod delegation;
pub mod diagnostics;
pub mod forwarding;
pub mod search_query;
pub mod sent_copy;
pub mod crypto;
pub mod html_sanitize;
pub mod imap_service;
pub mod import_service;
pub mod pgp;
pub mod pgp_mime;
pub mod oauth;
pub mod outgoing;
pub mod pop_imap;
pub mod recipient_groups;
pub mod send_as;
pub mod templates;
pub mod smtp_service;
pub mod spam_classifier;
pub mod sync_service;
pub mod vacation;

/// Hosts whose accounts authenticate with an app password (16 chars, shown by
/// the provider in spaced groups). Whitespace in a stored password for these
/// hosts can only come from a paste of that spaced display — real passwords
/// play no role there since basic auth is refused — so it is safe to strip.
/// Repairs accounts saved before the client-side normalization existed.
pub fn app_password_normalize(host: &str, password: &str) -> String {
    let h = host.to_ascii_lowercase();
    let is_app_pw_host = h.ends_with("gmail.com")
        || h.ends_with("googlemail.com")
        || h.ends_with("yahoo.com")
        || h.ends_with("mail.me.com")
        || h.ends_with("icloud.com");
    if is_app_pw_host && password.chars().any(|c| c.is_whitespace()) {
        password.chars().filter(|c| !c.is_whitespace()).collect()
    } else {
        password.to_string()
    }
}
