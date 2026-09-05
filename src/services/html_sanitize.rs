//! One HTML sanitiser for every incoming e-mail body, wherever it comes from —
//! a synced message or the plaintext of a just-decrypted PGP/MIME part. Keeping
//! it in one place means decrypted mail is cleaned by the SAME policy as ordinary
//! mail, instead of a second, drifting copy.
//!
//! The policy is permissive on presentation (legacy HTML-email tags/attributes,
//! inline `<style>`, `data:`/`cid:` images) and strict on anything active —
//! ammonia drops scripts, event handlers and unknown protocols regardless.

/// CSS an e-mail may not carry, whatever the browser would make of it today:
/// `@import` pulls a remote stylesheet (Gmail strips those — it leaks the read
/// and injects unreviewed rules), and `expression()`, `behavior:` and
/// `-moz-binding` are the legacy script-in-CSS hooks. Ammonia does not parse
/// CSS at all, so every `<style>` block and every `style=` value goes through
/// here. The whole declaration (or at-rule) holding the hazard is dropped, not
/// just the keyword: leaving its URL behind would be pointless and confusing.
fn clean_css(css: &str) -> String {
    const HAZARDS: [&str; 4] = ["@import", "expression(", "behavior:", "-moz-binding"];
    let lower = css.to_ascii_lowercase();
    let mut out = String::with_capacity(css.len());
    let mut cursor = 0usize;
    while cursor < css.len() {
        let hit = HAZARDS
            .iter()
            .filter_map(|h| lower[cursor..].find(h).map(|p| cursor + p))
            .min();
        let Some(pos) = hit else {
            out.push_str(&css[cursor..]);
            break;
        };
        // Back up to the start of the declaration / at-rule that carries it…
        let start = css[cursor..pos]
            .rfind([';', '{', '}'])
            .map(|p| cursor + p + 1)
            .unwrap_or(cursor);
        out.push_str(&css[cursor..start]);
        // …and skip to just past its end.
        cursor = css[pos..]
            .find([';', '}'])
            .map(|p| pos + p + 1)
            .unwrap_or(css.len());
    }
    out
}

/// Runs `clean_css` over the contents of every `<style>` block (ammonia keeps
/// the block but never looks inside it).
fn clean_style_blocks(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0usize;
    while let Some(rel) = lower[cursor..].find("<style") {
        let tag_start = cursor + rel;
        let Some(open_end) = lower[tag_start..].find('>').map(|p| tag_start + p + 1) else { break };
        let Some(close) = lower[open_end..].find("</style>").map(|p| open_end + p) else { break };
        out.push_str(&html[cursor..open_end]);
        out.push_str(&clean_css(&html[open_end..close]));
        cursor = close;
    }
    out.push_str(&html[cursor..]);
    out
}

/// Sanitise an untrusted HTML e-mail body for storage and rendering.
pub fn sanitize_email_html(html: &str) -> String {
    let cleaned = ammonia::Builder::default()
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
        // Allow data: (inline base64 images) and cid: (inline MIME attachments)…
        .add_url_schemes(&["data", "cid"])
        // …but ONLY as an image source. Ammonia applies allowed schemes to every
        // URL attribute, so without this a link could carry
        // `href="data:text/html;base64,<script>…"` — an inline document, i.e.
        // script delivery, which is exactly what the scheme list is not for.
        // The same filter cleans inline CSS, which ammonia passes through raw.
        .attribute_filter(|element, attribute, value| {
            let lower = value.trim().to_ascii_lowercase();
            if lower.starts_with("data:") || lower.starts_with("cid:") {
                let inline_image = element == "img"
                    && attribute == "src"
                    && (lower.starts_with("cid:") || lower.starts_with("data:image/"));
                return inline_image.then(|| value.into());
            }
            if attribute == "style" {
                return Some(clean_css(value).into());
            }
            Some(value.into())
        })
        .clean(html)
        .to_string();
    clean_style_blocks(&cleaned)
}

#[cfg(test)]
mod tests {
    use super::sanitize_email_html as s;

    /// Nothing executable may survive, whatever shape it arrives in.
    #[test]
    fn active_content_is_stripped() {
        for (name, html) in [
            ("script",        r#"<p>a</p><script>alert(1)</script>"#),
            ("script attr",   r#"<script type="text/vbscript">MsgBox 1</script>"#),
            ("onerror",       r#"<img src="x" onerror="alert(1)">"#),
            ("onclick",       r##"<a href="#" onclick="alert(1)">x</a>"##),
            ("onload body",   r#"<body onload="alert(1)">x</body>"#),
            ("javascript:",   r#"<a href="javascript:alert(1)">x</a>"#),
            ("vbscript:",     r#"<a href="vbscript:MsgBox(1)">x</a>"#),
            ("iframe",        r#"<iframe src="https://evil.example"></iframe>"#),
            ("object",        r#"<object data="evil.swf"></object>"#),
            ("embed",         r#"<embed src="evil.swf">"#),
            ("form",          r#"<form action="https://evil.example"><input name="p"></form>"#),
            ("meta refresh",  r#"<meta http-equiv="refresh" content="0;url=https://evil.example">"#),
            ("base",          r#"<base href="https://evil.example/">"#),
            ("link css",      r#"<link rel="stylesheet" href="https://evil.example/x.css">"#),
            ("svg script",    r#"<svg><script>alert(1)</script></svg>"#),
            ("svg onload",    r#"<svg onload="alert(1)"><circle r="10"/></svg>"#),
            ("data html href", r#"<a href="data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==">x</a>"#),
            ("css import",    r#"<style>@import url("https://evil.example/x.css");</style>"#),
            ("css expression", r#"<div style="width:expression(alert(1))">x</div>"#),
            ("css binding",   r##"<style>a { -moz-binding: url("https://evil.example/x.xml#e"); }</style>"##),
            ("css behavior",  r##"<style>a { behavior: url(#default#time2); }</style>"##),
        ] {
            let out = s(html).to_ascii_lowercase();
            for needle in ["<script", "<iframe", "<object", "<embed", "<form", "<input",
                           "<meta", "<base", "<link", "<svg", "javascript:", "vbscript:",
                           "onerror", "onclick", "onload", "@import", "expression(",
                           "-moz-binding", "behavior:", "data:text/html"] {
                assert!(!out.contains(needle), "{name}: « {needle} » a survécu → {out}");
            }
        }
    }

    /// …while everything a real HTML mail needs still goes through.
    #[test]
    fn legitimate_mail_survives() {
        let out = s(r##"<html><head><style>.b{color:red}</style></head><body bgcolor="#fff">
            <table cellpadding="4"><tr><td><font color="red">Bonjour</font>
            <img src="cid:part1" alt="logo"><img src="data:image/gif;base64,R0lGODlh" alt="px">
            <a href="https://exemple.fr/promo">Voir</a></td></tr></table></body></html>"##);
        for needle in ["<style", "<table", "<font", "cid:part1", "data:image/gif", "https://exemple.fr/promo"] {
            assert!(out.contains(needle), "« {needle} » a été perdu → {out}");
        }
    }
}
