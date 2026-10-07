//! Inbox category of a message, decided from its sender.
//!
//! This is the ONLY implementation: the category is computed once, when the
//! message is stored, then written to `mail.messages.category` and mirrored on
//! the thread. Listings read the column back — they no longer recompute
//! anything, and the frontend no longer classifies either.
//!
//! Plain lowercase substring tests rather than regular expressions: the module
//! has no regex dependency, the patterns are simple alternations, and this runs
//! once per stored message.

/// The four inbox tabs. A message has exactly one, never none.
pub const CATEGORIES: [&str; 4] = ["main", "social", "notifications", "promotions"];

const SOCIAL: [&str; 10] = [
    "twitter", "facebook", "linkedin", "instagram", "tiktok",
    "youtube", "pinterest", "snapchat", "meta.com", "x.com",
];

const NOTIFICATIONS: [&str; 6] = [
    "notification", "alert", "update", "security", "account", "billing",
];

const PROMOTIONS: [&str; 9] = [
    "newsletter", "promo", "marketing", "info@", "hello@", "contact@",
    "deals@", "deal@", "offers@",
];

/// `no-reply`, `no.reply`, `no_reply`, `noreply` — the separator varies.
fn is_noreply(sender: &str) -> bool {
    ["noreply", "no-reply", "no.reply", "no_reply"]
        .iter()
        .any(|p| sender.contains(p))
}

/// Category for a sender address. Order matters: social wins over
/// notifications, which wins over promotions — a LinkedIn notification belongs
/// under "Social", not "Notifications".
pub fn for_sender(from_email: &str) -> &'static str {
    let s = from_email.to_ascii_lowercase();

    if SOCIAL.iter().any(|p| s.contains(p)) {
        return "social";
    }
    if NOTIFICATIONS.iter().any(|p| s.contains(p)) || (is_noreply(&s) && s.contains("notif")) {
        return "notifications";
    }
    if is_noreply(&s) || PROMOTIONS.iter().any(|p| s.contains(p)) || s.contains("offer@") {
        return "promotions";
    }
    "main"
}

/// True when the value is one of the four categories — used to validate input.
pub fn is_valid(value: &str) -> bool {
    CATEGORIES.contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_sender() {
        assert_eq!(for_sender("notifications@linkedin.com"), "social");
        assert_eq!(for_sender("security-noreply@accounts.example.com"), "notifications");
        assert_eq!(for_sender("newsletter@shop.example"), "promotions");
        assert_eq!(for_sender("no-reply@shop.example"), "promotions");
        assert_eq!(for_sender("marie.dupont@example.org"), "main");
    }

    #[test]
    fn is_case_insensitive() {
        assert_eq!(for_sender("NewsLetter@Shop.Example"), "promotions");
    }
}
