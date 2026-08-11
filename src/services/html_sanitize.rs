//! One HTML sanitiser for every incoming e-mail body, wherever it comes from —
//! a synced message or the plaintext of a just-decrypted PGP/MIME part. Keeping
//! it in one place means decrypted mail is cleaned by the SAME policy as ordinary
//! mail, instead of a second, drifting copy.
//!
//! The policy is permissive on presentation (legacy HTML-email tags/attributes,
//! inline `<style>`, `data:`/`cid:` images) and strict on anything active —
//! ammonia drops scripts, event handlers and unknown protocols regardless.

/// Sanitise an untrusted HTML e-mail body for storage and rendering.
pub fn sanitize_email_html(html: &str) -> String {
    ammonia::Builder::default()
        // Keep <style> blocks and structural tags common in HTML e-mail.
        .rm_clean_content_tags(&["style"])
        .add_tags(&["style", "head", "html", "body", "font", "center"])
        // Generic attributes present on nearly every HTML-email element.
        .add_generic_attributes(&[
            "style", "class", "id", "dir", "lang",
            "align", "valign",
            "bgcolor", "background", "color",
            "width", "height",
            "role", "aria-label", "aria-hidden",
        ])
        // <a>: allow target and name (anchors). NOT `rel`: ammonia 4.x panics if
        // `rel` is listed here while `link_rel` (default: noopener noreferrer)
        // already adds it to links automatically.
        .add_tag_attributes("a", &["target", "name"])
        // <img>: legacy HTML-email attributes + lazy loading.
        .add_tag_attributes("img", &["border", "hspace", "vspace", "loading"])
        // <font>: colour, face, size (old mail / Outlook).
        .add_tag_attributes("font", &["color", "face", "size"])
        // <table> and friends: common HTML-email attributes.
        .add_tag_attributes("table", &["cellpadding", "cellspacing", "border", "bgcolor", "background", "summary"])
        .add_tag_attributes("tr",    &["bgcolor", "valign", "height"])
        .add_tag_attributes("td",    &["cellpadding", "cellspacing", "bgcolor", "background", "nowrap", "valign", "width", "height"])
        .add_tag_attributes("th",    &["cellpadding", "cellspacing", "bgcolor", "background", "nowrap", "valign", "width", "height"])
        // <body>: legacy background colours.
        .add_tag_attributes("body",  &["bgcolor", "background", "text", "link", "alink", "vlink"])
        // Allow data: (inline base64 images) and cid: (inline MIME attachments).
        .add_url_schemes(&["data", "cid"])
        .clean(html)
        .to_string()
}
