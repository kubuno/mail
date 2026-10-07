//! The pure, testable half of the IMAP server: turning client bytes into
//! commands, and turning stored messages into the shapes RFC 3501 responses are
//! made of.
//!
//! Nothing here touches the network or the database, which is the point: IMAP
//! is where a server is most easily broken by a client that is merely unusual —
//! a literal in the middle of a LOGIN, a bracketed section inside a
//! parenthesised fetch list, a `<0.2048>` partial whose announced length must
//! match the bytes to the octet. Keeping that away from the session loop lets it
//! be exercised by tests directly, not only through a socket.

use serde_json::Value;

use crate::server::store::{self, StoredMessage};

// ── Lexing ───────────────────────────────────────────────────────────────────

/// One argument of a command line.
///
/// Quoting is remembered because IMAP distinguishes a quoted `"NIL"` from the
/// atom `NIL`, and because an empty mailbox name (`LIST "" "*"`) only exists as
/// a quoted string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text:   String,
    pub quoted: bool,
}

impl Token {
    /// Upper-cased text, for the many places the grammar is case-insensitive.
    pub fn upper(&self) -> String {
        self.text.to_ascii_uppercase()
    }
}

/// A `{n}` marker closing a line: the client is about to send `n` raw bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Literal {
    pub len: usize,
    /// LITERAL+ (`{n+}`): the bytes follow immediately, no continuation to send.
    pub non_sync: bool,
}

/// Splits one line into tokens, stopping if it ends on a literal marker.
///
/// Atoms swallow whitespace while inside `(...)` or `[...]`, so a fetch item
/// list or `BODY[HEADER.FIELDS (FROM TO)]` survives as a single token and can be
/// taken apart later by the code that actually understands it.
pub fn tokenize_line(line: &str) -> (Vec<Token>, Option<Literal>) {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' => i += 1,
            '"' => {
                let (token, next) = read_quoted(&chars, i);
                out.push(token);
                i = next;
            }
            '{' => {
                if let Some((literal, _)) = read_literal_marker(&chars, i) {
                    return (out, Some(literal));
                }
                let (token, next) = read_atom(&chars, i);
                out.push(token);
                i = next;
            }
            _ => {
                let (token, next) = read_atom(&chars, i);
                out.push(token);
                i = next;
            }
        }
    }

    (out, None)
}

fn read_quoted(chars: &[char], start: usize) -> (Token, usize) {
    let mut text = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                text.push(chars[i + 1]);
                i += 2;
            }
            '"' => {
                i += 1;
                break;
            }
            c => {
                text.push(c);
                i += 1;
            }
        }
    }
    (Token { text, quoted: true }, i)
}

/// A `{n}` is only a literal marker when nothing but whitespace follows it;
/// anywhere else the braces are just characters of an atom.
fn read_literal_marker(chars: &[char], start: usize) -> Option<(Literal, usize)> {
    let close = chars[start..].iter().position(|c| *c == '}')? + start;
    if !chars[close + 1..].iter().all(|c| c.is_whitespace()) {
        return None;
    }
    let inner: String = chars[start + 1..close].iter().collect();
    let non_sync = inner.ends_with('+');
    let len = inner.trim_end_matches('+').trim().parse::<usize>().ok()?;
    Some((Literal { len, non_sync }, close + 1))
}

fn read_atom(chars: &[char], start: usize) -> (Token, usize) {
    let mut text = String::new();
    let mut depth = 0usize;
    let mut in_quotes = false;
    let mut i = start;

    while i < chars.len() {
        let c = chars[i];
        if in_quotes {
            if c == '\\' && i + 1 < chars.len() {
                text.push(c);
                text.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_quotes = false;
            }
            text.push(c);
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_quotes = true;
                text.push(c);
                i += 1;
            }
            '(' | '[' => {
                depth += 1;
                text.push(c);
                i += 1;
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                text.push(c);
                i += 1;
            }
            ' ' | '\t' if depth == 0 => break,
            _ => {
                text.push(c);
                i += 1;
            }
        }
    }

    (Token { text, quoted: false }, i)
}

// ── Sequence sets ────────────────────────────────────────────────────────────

/// Expands `1:*`, `2:4`, `1,3,5` into inclusive ranges.
///
/// `star` is what `*` means here: the highest sequence number for a plain
/// FETCH, the highest UID for a UID FETCH. Ranges are normalised low-to-high
/// because `4:2` is legal and means the same as `2:4`. An unparsable element is
/// dropped rather than failing the whole set: a client that sends one bad number
/// should get the messages it asked for correctly, not a protocol error.
pub fn parse_sequence_set(spec: &str, star: u64) -> Vec<(u64, u64)> {
    let mut ranges = Vec::new();
    for part in spec.trim().trim_matches(|c| c == '(' || c == ')').split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (from, to) = match part.split_once(':') {
            Some((a, b)) => (parse_seq_number(a, star), parse_seq_number(b, star)),
            None => {
                let n = parse_seq_number(part, star);
                (n, n)
            }
        };
        if let (Some(a), Some(b)) = (from, to) {
            ranges.push((a.min(b), a.max(b)));
        }
    }
    ranges
}

fn parse_seq_number(raw: &str, star: u64) -> Option<u64> {
    let raw = raw.trim();
    if raw == "*" {
        // An empty mailbox has no `*`; callers get an empty match, not a match
        // on message zero.
        return if star == 0 { None } else { Some(star) };
    }
    raw.parse::<u64>().ok().filter(|n| *n > 0)
}

pub fn seq_contains(ranges: &[(u64, u64)], n: u64) -> bool {
    ranges.iter().any(|(a, b)| n >= *a && n <= *b)
}

// ── FETCH item lists ─────────────────────────────────────────────────────────

/// Splits `(FLAGS UID BODY.PEEK[HEADER.FIELDS (FROM TO)])` into its items,
/// keeping bracketed sections whole.
pub fn split_fetch_items(spec: &str) -> Vec<String> {
    let trimmed = spec.trim();
    let inner = trimmed
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(trimmed);

    let mut items = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for c in inner.chars() {
        match c {
            '(' | '[' | '<' => {
                depth += 1;
                current.push(c);
            }
            ')' | ']' | '>' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ' ' | '\t' if depth == 0 => {
                if !current.is_empty() {
                    items.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        items.push(current);
    }
    items
}

/// Which part of a message a `BODY[...]` item asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    Full,
    Header,
    Text,
    HeaderFields(Vec<String>),
    HeaderFieldsNot(Vec<String>),
}

/// Reads the text between the brackets of a `BODY[...]` item.
///
/// Part numbers (`BODY[1.2]`) are answered with the whole message: this store
/// rebuilds MIME rather than replaying it, so it has no stable part numbering to
/// honour, and serving the message is more useful than an error.
pub fn parse_section(inside: &str) -> Section {
    let upper = inside.trim().to_ascii_uppercase();
    if upper.is_empty() {
        return Section::Full;
    }
    if let Some(rest) = upper.strip_prefix("HEADER.FIELDS.NOT") {
        return Section::HeaderFieldsNot(field_names(rest));
    }
    if let Some(rest) = upper.strip_prefix("HEADER.FIELDS") {
        return Section::HeaderFields(field_names(rest));
    }
    match upper.as_str() {
        "HEADER" => Section::Header,
        "TEXT" => Section::Text,
        _ => Section::Full,
    }
}

fn field_names(raw: &str) -> Vec<String> {
    raw.trim()
        .trim_matches(|c| c == '(' || c == ')')
        .split_whitespace()
        .map(|f| f.trim_matches('"').to_ascii_uppercase())
        .filter(|f| !f.is_empty())
        .collect()
}

/// Separates a trailing `<start.len>` partial specifier from a fetch item.
pub fn split_partial(item: &str) -> (&str, Option<(usize, usize)>) {
    let Some(open) = item.rfind('<') else { return (item, None) };
    if !item.ends_with('>') {
        return (item, None);
    }
    let inner = &item[open + 1..item.len() - 1];
    let Some((start, len)) = inner.split_once('.') else { return (item, None) };
    match (start.parse::<usize>(), len.parse::<usize>()) {
        (Ok(s), Ok(l)) => (&item[..open], Some((s, l))),
        _ => (item, None),
    }
}

/// Keeps or drops headers by name, preserving folded continuation lines.
pub fn filter_headers(headers: &str, names: &[String], keep: bool) -> String {
    let mut out = String::with_capacity(headers.len());
    let mut including = false;
    for line in headers.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        let is_continuation = line.starts_with(' ') || line.starts_with('\t');
        if !is_continuation {
            let name = line.split(':').next().unwrap_or("").trim().to_ascii_uppercase();
            let listed = names.contains(&name);
            including = listed == keep;
        }
        if including {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    // A header section always ends on its own blank line, even when empty.
    out.push_str("\r\n");
    out
}

// ── AUTHENTICATE PLAIN ───────────────────────────────────────────────────────

/// Decodes the SASL PLAIN payload: `authzid NUL authcid NUL password`.
///
/// The authorisation identity is ignored: this server has no notion of one
/// mailbox acting for another, and silently accepting such a request would be a
/// way to log in as somebody else.
pub fn decode_plain(payload: &str) -> Option<(String, String)> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(payload.trim().as_bytes())
        .ok()?;
    let text = String::from_utf8(raw).ok()?;
    let mut parts = text.split('\u{0}');
    let _authzid = parts.next()?;
    let authcid = parts.next()?.to_string();
    let password = parts.next()?.to_string();
    if authcid.is_empty() || password.is_empty() {
        return None;
    }
    Some((authcid, password))
}

// ── ENVELOPE ─────────────────────────────────────────────────────────────────

/// Builds the parenthesised ENVELOPE of a message.
///
/// Order is fixed by RFC 3501: date, subject, from, sender, reply-to, to, cc,
/// bcc, in-reply-to, message-id. Absent fields are the atom `NIL`, never an
/// empty string — clients test for NIL.
pub fn envelope(message: &StoredMessage) -> String {
    let date = message.sent_at.unwrap_or(message.received_at);
    let from = address_list_of(message.from_name.as_deref(), &message.from_email);

    format!(
        "({} {} {} {} {} {} {} NIL NIL {})",
        quoted(&date.format("%a, %d %b %Y %H:%M:%S %z").to_string()),
        nstring(&encode_header_value(&message.subject)),
        from,
        from,
        from,
        addresses(&message.to_addresses),
        addresses(&message.cc_addresses),
        nstring(&message.message_id.clone().map(bracketed).unwrap_or_default()),
    )
}

fn bracketed(id: String) -> String {
    if id.starts_with('<') { id } else { format!("<{id}>") }
}

/// One address as `(name adl mailbox host)`; the address list wrapper included.
fn address_list_of(name: Option<&str>, email: &str) -> String {
    if email.is_empty() {
        return "NIL".to_string();
    }
    format!("({})", address(name, email))
}

fn address(name: Option<&str>, email: &str) -> String {
    let (local, host) = match email.split_once('@') {
        Some((l, h)) => (l, h),
        None => (email, ""),
    };
    format!(
        "({} NIL {} {})",
        nstring(&name.map(encode_header_value).unwrap_or_default()),
        quoted(local),
        nstring(host),
    )
}

fn addresses(value: &Value) -> String {
    let rendered: Vec<String> = value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|a| {
                    let email = a.get("email")?.as_str()?;
                    if email.is_empty() {
                        return None;
                    }
                    Some(address(a.get("name").and_then(Value::as_str), email))
                })
                .collect()
        })
        .unwrap_or_default();

    if rendered.is_empty() {
        "NIL".to_string()
    } else {
        format!("({})", rendered.join(""))
    }
}

/// An IMAP quoted string, with the two characters that may not appear raw.
pub fn quoted(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\r', '\n'], " ");
    format!("\"{escaped}\"")
}

/// A quoted string, or `NIL` when there is nothing to say.
fn nstring(value: &str) -> String {
    if value.is_empty() {
        "NIL".to_string()
    } else {
        quoted(value)
    }
}

/// RFC 2047 encoding, applied only to non-ASCII values: an accent sent raw in an
/// ENVELOPE is what a client displays as mojibake.
fn encode_header_value(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    use base64::Engine;
    format!(
        "=?UTF-8?B?{}?=",
        base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
    )
}

/// A single-part BODYSTRUCTURE. The store rebuilds messages from parsed pieces,
/// so a faithful multipart structure would describe a rendering the client is
/// free to re-derive anyway; a truthful "one text part of this size" keeps
/// clients that insist on BODY working.
pub fn body_structure(rendered: &str) -> String {
    let text = rendered.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    format!(
        "(\"TEXT\" \"PLAIN\" (\"CHARSET\" \"UTF-8\") NIL NIL \"8BIT\" {} {})",
        text.len(),
        text.lines().count().max(1),
    )
}

// ── Flag arithmetic ──────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub enum FlagMode {
    Add,
    Remove,
    Replace,
}

impl FlagMode {
    /// What a STORE does to one flag: `None` when the command leaves it alone.
    pub fn apply(self, listed: bool) -> Option<bool> {
        match (self, listed) {
            (FlagMode::Add, true) => Some(true),
            (FlagMode::Remove, true) => Some(false),
            (FlagMode::Replace, listed) => Some(listed),
            _ => None,
        }
    }
}

pub fn flags_of(message: &StoredMessage, deleted: bool) -> String {
    let mut flags = Vec::new();
    if message.is_read {
        flags.push("\\Seen");
    }
    if message.is_starred {
        flags.push("\\Flagged");
    }
    if deleted {
        flags.push("\\Deleted");
    }
    flags.join(" ")
}

// ── SEARCH criteria ──────────────────────────────────────────────────────────

/// Evaluates the criteria this server understands.
///
/// An unknown criterion excludes every message rather than failing the command:
/// a client that asked something unsupported gets "no results", which it knows
/// how to display, instead of an error it may treat as a broken account.
pub fn matches_criteria(message: &StoredMessage, deleted: bool, criteria: &[String]) -> bool {
    if criteria.is_empty() {
        return true;
    }
    let mut index = 0;
    while index < criteria.len() {
        let term = criteria[index].as_str();
        index += 1;
        let ok = match term {
            "ALL" => true,
            // The charset argument belongs to the next token, not to us.
            "CHARSET" => {
                index += 1;
                true
            }
            "SEEN" => message.is_read,
            "UNSEEN" | "NEW" => !message.is_read,
            "DELETED" => deleted,
            "UNDELETED" => !deleted,
            "FLAGGED" => message.is_starred,
            "UNFLAGGED" => !message.is_starred,
            // Nothing is ever \Recent here, so OLD is everything.
            "OLD" => true,
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

// ── FETCH rendering ──────────────────────────────────────────────────────────

/// Builds one `* n FETCH (...)` response. The second value says whether the
/// client asked for content in a way that implicitly sets `\Seen`.
pub fn fetch_response(
    seq: u64,
    message: &StoredMessage,
    deleted: bool,
    requested: &[String],
    force_uid: bool,
) -> (Vec<u8>, bool) {
    let rendered = store::render(message);
    let (head, body) = rendered
        .split_once("\r\n\r\n")
        .map(|(h, b)| (h.to_string(), b.to_string()))
        .unwrap_or((rendered.clone(), String::new()));
    let headers = format!("{head}\r\n\r\n");

    let mut items: Vec<String> = Vec::new();
    for item in requested {
        match item.to_ascii_uppercase().as_str() {
            "ALL" => items.extend(["FLAGS", "INTERNALDATE", "RFC822.SIZE", "ENVELOPE"].map(String::from)),
            "FAST" => items.extend(["FLAGS", "INTERNALDATE", "RFC822.SIZE"].map(String::from)),
            "FULL" => items.extend(
                ["FLAGS", "INTERNALDATE", "RFC822.SIZE", "ENVELOPE", "BODY"].map(String::from),
            ),
            _ => items.push(item.clone()),
        }
    }
    // RFC 3501 requires UID in every response to a UID FETCH, asked for or not.
    if force_uid && !items.iter().any(|i| i.eq_ignore_ascii_case("UID")) {
        items.push("UID".to_string());
    }

    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut touches_seen = false;

    for item in &items {
        let (base, partial) = split_partial(item);
        let upper = base.to_ascii_uppercase();

        if let Some(open) = upper.find('[') {
            let peek = upper[..open].ends_with(".PEEK");
            let close = base.rfind(']').filter(|c| *c > open).unwrap_or(base.len());
            let inside = base.get(open + 1..close).unwrap_or("");
            let data = match parse_section(inside) {
                Section::Full => rendered.clone(),
                Section::Header => headers.clone(),
                Section::Text => body.clone(),
                Section::HeaderFields(names) => filter_headers(&head, &names, true),
                Section::HeaderFieldsNot(names) => filter_headers(&head, &names, false),
            };
            touches_seen |= !peek;

            let label = if upper.starts_with("RFC822") {
                base.to_string()
            } else {
                format!("BODY[{inside}]")
            };
            parts.push(literal_part(&label, data.as_bytes(), partial));
            continue;
        }

        let piece = match upper.as_str() {
            "UID" => format!("UID {}", message.local_uid),
            // CONDSTORE (RFC 7162): the modification sequence, in its own parens.
            "MODSEQ" => format!("MODSEQ ({})", message.modseq),
            "FLAGS" => format!("FLAGS ({})", flags_of(message, deleted)),
            "INTERNALDATE" => {
                let date = message.sent_at.unwrap_or(message.received_at);
                format!("INTERNALDATE \"{}\"", date.format("%d-%b-%Y %H:%M:%S %z"))
            }
            "RFC822.SIZE" => format!("RFC822.SIZE {}", rendered.len()),
            "ENVELOPE" => format!("ENVELOPE {}", envelope(message)),
            "BODY" | "BODYSTRUCTURE" => format!("{upper} {}", body_structure(&rendered)),
            "RFC822" => {
                touches_seen = true;
                parts.push(literal_part("RFC822", rendered.as_bytes(), partial));
                continue;
            }
            "RFC822.HEADER" => {
                parts.push(literal_part("RFC822.HEADER", headers.as_bytes(), partial));
                continue;
            }
            "RFC822.TEXT" => {
                touches_seen = true;
                parts.push(literal_part("RFC822.TEXT", body.as_bytes(), partial));
                continue;
            }
            // An item nobody implements is answered NIL: the client keeps the
            // rest of the response, which is what it actually needed.
            other => format!("{other} NIL"),
        };
        parts.push(piece.into_bytes());
    }

    let mut out = format!("* {seq} FETCH (").into_bytes();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            out.push(b' ');
        }
        out.extend_from_slice(part);
    }
    out.extend_from_slice(b")\r\n");
    (out, touches_seen)
}

/// A `LABEL {n}\r\n<bytes>` item, honouring a `<origin.count>` partial fetch.
/// Slicing is done on bytes so the announced length is the length actually sent
/// — a mismatch there desynchronises the client for the rest of the session.
pub fn literal_part(label: &str, data: &[u8], partial: Option<(usize, usize)>) -> Vec<u8> {
    let (slice, suffix) = match partial {
        Some((start, count)) => {
            let start = start.min(data.len());
            let end = start.saturating_add(count).min(data.len());
            (&data[start..end], format!("<{start}>"))
        }
        None => (data, String::new()),
    };
    let mut part = format!("{label}{suffix} {{{}}}\r\n", slice.len()).into_bytes();
    part.extend_from_slice(slice);
    part
}

// ── Mailbox naming ───────────────────────────────────────────────────────────

pub fn display_name(folder: &str) -> &'static str {
    match folder {
        "sent" => "Sent",
        "drafts" => "Drafts",
        "spam" => "Spam",
        "trash" => "Trash",
        _ => "INBOX",
    }
}

/// RFC 6154 special-use attributes, which is how a client knows which folder is
/// its Sent without guessing from the name.
pub fn special_use(folder: &str) -> &'static str {
    match folder {
        "Sent" => "\\Sent",
        "Drafts" => "\\Drafts",
        "Spam" => "\\Junk",
        "Trash" => "\\Trash",
        _ => "",
    }
}

// The stable-per-folder UIDVALIDITY lives in `store::uidvalidity`, so IMAP and
// every other consumer derive the same value from the same folder name.

// ── UID sets (UIDPLUS / COPYUID) ─────────────────────────────────────────────

/// Renders a list of UIDs as a UID set, ascending, collapsing consecutive runs
/// into `a:b` ranges. This is the shape COPYUID (RFC 4315) reports source and
/// destination UIDs in, and the compact form real clients emit and expect.
pub fn uid_set_string(uids: &[i64]) -> String {
    let mut values: Vec<i64> = uids.to_vec();
    values.sort_unstable();
    values.dedup();

    let mut parts = Vec::new();
    let mut i = 0;
    while i < values.len() {
        let start = values[i];
        let mut end = start;
        while i + 1 < values.len() && values[i + 1] == end + 1 {
            end += 1;
            i += 1;
        }
        if start == end {
            parts.push(start.to_string());
        } else {
            parts.push(format!("{start}:{end}"));
        }
        i += 1;
    }
    parts.join(",")
}

// ── CONDSTORE / QRESYNC modifiers (RFC 7162) ─────────────────────────────────

/// The modifier group that may trail a FETCH: `(CHANGEDSINCE <n> [VANISHED])`.
/// CHANGEDSINCE restricts the fetch to messages whose modseq is greater than
/// `<n>`; VANISHED asks for a `VANISHED (EARLIER)` report of UIDs gone since it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FetchModifiers {
    pub changed_since: Option<i64>,
    pub vanished:      bool,
}

/// Reads the parenthesised modifier group of a FETCH. Unknown words are ignored,
/// so a client mixing in a modifier this server does not implement still gets a
/// sensible fetch rather than a protocol error.
pub fn parse_fetch_modifiers(text: &str) -> FetchModifiers {
    let inner = text.trim().trim_start_matches('(').trim_end_matches(')');
    let mut tokens = inner.split_whitespace();
    let mut mods = FetchModifiers::default();
    while let Some(token) = tokens.next() {
        match token.to_ascii_uppercase().as_str() {
            "CHANGEDSINCE" => mods.changed_since = tokens.next().and_then(|v| v.parse::<i64>().ok()),
            "VANISHED" => mods.vanished = true,
            _ => {}
        }
    }
    mods
}

/// Whether a token is a FETCH/STORE modifier group rather than a fetch-item list
/// or a STORE action — i.e. it names a CONDSTORE modifier. Lets the command
/// dispatch tell `(UNCHANGEDSINCE 12)` apart from `(\Seen)`.
pub fn is_modifier_group(text: &str) -> bool {
    let upper = text.to_ascii_uppercase();
    upper.contains("CHANGEDSINCE") || upper.contains("UNCHANGEDSINCE")
}

/// Reads the `<n>` of a STORE `(UNCHANGEDSINCE <n>)` modifier group. The store is
/// applied only to messages whose modseq is `<= n`; the rest are reported back in
/// a `MODIFIED` response code and left untouched.
pub fn parse_unchangedsince(text: &str) -> Option<i64> {
    let inner = text.trim().trim_start_matches('(').trim_end_matches(')');
    let mut tokens = inner.split_whitespace();
    while let Some(token) = tokens.next() {
        if token.eq_ignore_ascii_case("UNCHANGEDSINCE") {
            return tokens.next().and_then(|v| v.parse::<i64>().ok());
        }
    }
    None
}

/// The parameters of a `SELECT/EXAMINE ... (QRESYNC (<uidvalidity> <modseq>
/// [<known-uids>]))` reconnection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QResyncParams {
    pub uidvalidity: u32,
    pub modseq:      i64,
    /// UIDs the client believes it still holds; empty means "all of them". A
    /// VANISHED report is narrowed to this set when it is non-empty.
    pub known_uids:  Vec<(u64, u64)>,
}

/// Parses a QRESYNC select parameter. Returns `None` for anything that is not a
/// well-formed QRESYNC group, so a plain `(CONDSTORE)` or a malformed request
/// simply falls back to an ordinary SELECT.
pub fn parse_qresync(text: &str) -> Option<QResyncParams> {
    if !text.to_ascii_uppercase().contains("QRESYNC") {
        return None;
    }
    // The parameters are the inner parenthesised group; find it and its matching
    // close by balance so a `known-uids` set that contains no parens is safe.
    let bytes = text.as_bytes();
    let outer = text.find('(')?;
    let inner_open = text[outer + 1..].find('(')? + outer + 1;
    let inner_close = matching_close(bytes, inner_open)?;
    let inner = text.get(inner_open + 1..inner_close)?;

    let mut parts = inner.split_whitespace();
    let uidvalidity = parts.next()?.parse::<u32>().ok()?;
    let modseq = parts.next()?.parse::<i64>().ok()?;
    let known_uids = match parts.next() {
        Some(set) => parse_sequence_set(set, u64::MAX),
        None => Vec::new(),
    };
    Some(QResyncParams { uidvalidity, modseq, known_uids })
}

/// Index of the `)` that closes the `(` at `open`, matched by depth.
fn matching_close(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// A `* VANISHED (EARLIER) <uidset>` line for QRESYNC, or `None` when nothing
/// vanished. The UIDs are collapsed into ascending ranges — the compact form
/// clients expect for a set that may span a whole mailbox.
pub fn vanished_earlier_line(uids: &[i64]) -> Option<String> {
    if uids.is_empty() {
        return None;
    }
    Some(format!("* VANISHED (EARLIER) {}\r\n", uid_set_string(uids)))
}

// ── IDLE resynchronisation diff ──────────────────────────────────────────────

/// The unsolicited responses that bring a client's view of a folder back in
/// step with the store, computed by comparing the folder as the client last saw
/// it (`old`) with the folder as it is now (`new`).
///
/// The fields are kept in the order RFC 3501 requires them to be sent:
/// EXPUNGE first (already sorted high-to-low, because each EXPUNGE renumbers the
/// messages after it), then a single EXISTS, then per-message FETCH FLAGS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResyncDiff {
    /// Sequence numbers to EXPUNGE, highest first.
    pub expunged:     Vec<u32>,
    /// The new message count to announce, when it must be announced.
    pub exists:       Option<u32>,
    /// `(sequence number in the new list, flags string)` for changed flags.
    pub flag_changes: Vec<(u32, String)>,
}

/// Diffs two snapshots of a folder into the responses IDLE must push.
///
/// Messages are matched by `local_uid`, which is stable across the move that a
/// flag change or a delivery represents. A message present before and gone after
/// is an EXPUNGE at its *old* position; a `local_uid` that only appears after is
/// a new arrival, which — like any change to the count — forces an EXISTS so the
/// client learns the true total after the expunges have shrunk its view.
pub fn compute_resync(old: &[StoredMessage], new: &[StoredMessage]) -> ResyncDiff {
    let mut expunged: Vec<u32> = old
        .iter()
        .enumerate()
        .filter(|(_, m)| !new.iter().any(|n| n.local_uid == m.local_uid))
        .map(|(index, _)| (index + 1) as u32)
        .collect();
    // Highest sequence number first: each EXPUNGE renumbers everything above it,
    // so emitting top-down keeps every number valid as it is sent.
    expunged.sort_unstable_by(|a, b| b.cmp(a));

    let added = new
        .iter()
        .any(|n| !old.iter().any(|m| m.local_uid == n.local_uid));
    // EXISTS is needed whenever the client's count no longer matches the store:
    // after any expunge (which shrank its view) or any new arrival.
    let exists =
        if !expunged.is_empty() || added { Some(new.len() as u32) } else { None };

    let mut flag_changes = Vec::new();
    for (index, message) in new.iter().enumerate() {
        if let Some(previous) = old.iter().find(|m| m.local_uid == message.local_uid) {
            if previous.is_read != message.is_read
                || previous.is_starred != message.is_starred
            {
                flag_changes.push(((index + 1) as u32, flags_of(message, false)));
            }
        }
    }

    ResyncDiff { expunged, exists, flag_changes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn tokenises_quoted_arguments() {
        let (tokens, literal) = tokenize_line("a1 LOGIN \"bob@example.com\" \"pa ss\\\"word\"");
        assert!(literal.is_none());
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0], Token { text: "a1".into(), quoted: false });
        assert_eq!(tokens[2].text, "bob@example.com");
        assert!(tokens[2].quoted);
        assert_eq!(tokens[3].text, "pa ss\"word");
    }

    #[test]
    fn an_empty_quoted_string_survives() {
        let (tokens, _) = tokenize_line("a2 LIST \"\" \"*\"");
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[2].text, "");
        assert!(tokens[2].quoted);
        assert_eq!(tokens[3].text, "*");
    }

    #[test]
    fn a_trailing_brace_marker_is_a_literal() {
        let (tokens, literal) = tokenize_line("a3 LOGIN {16}");
        assert_eq!(tokens.len(), 2);
        assert_eq!(literal, Some(Literal { len: 16, non_sync: false }));

        let (_, plus) = tokenize_line("a3 LOGIN {16+}");
        assert_eq!(plus, Some(Literal { len: 16, non_sync: true }));
    }

    #[test]
    fn braces_inside_a_line_are_not_a_literal() {
        let (tokens, literal) = tokenize_line("a4 SELECT {weird} INBOX");
        assert!(literal.is_none());
        assert_eq!(tokens.len(), 4);
    }

    #[test]
    fn bracketed_fetch_items_stay_whole() {
        let (tokens, _) = tokenize_line("a5 UID FETCH 1:* (FLAGS BODY.PEEK[HEADER.FIELDS (FROM TO)])");
        assert_eq!(tokens.len(), 5);
        assert_eq!(tokens[3].text, "1:*");
        assert_eq!(tokens[4].text, "(FLAGS BODY.PEEK[HEADER.FIELDS (FROM TO)])");
    }

    #[test]
    fn sequence_sets_cover_every_form() {
        assert_eq!(parse_sequence_set("1:*", 5), vec![(1, 5)]);
        assert_eq!(parse_sequence_set("1,3,5", 10), vec![(1, 1), (3, 3), (5, 5)]);
        assert_eq!(parse_sequence_set("2:4", 10), vec![(2, 4)]);
        assert_eq!(parse_sequence_set("4:2", 10), vec![(2, 4)], "bornes inversées");
        assert_eq!(parse_sequence_set("*", 7), vec![(7, 7)]);
        assert_eq!(parse_sequence_set("*:*", 0), vec![], "boîte vide");
        assert_eq!(parse_sequence_set("1,zz,3", 9), vec![(1, 1), (3, 3)]);
    }

    #[test]
    fn sequence_membership() {
        let set = parse_sequence_set("2:4,9", 20);
        assert!(!seq_contains(&set, 1));
        assert!(seq_contains(&set, 3));
        assert!(seq_contains(&set, 9));
        assert!(!seq_contains(&set, 10));
    }

    #[test]
    fn fetch_items_split_on_top_level_spaces_only() {
        let items = split_fetch_items("(UID FLAGS BODY.PEEK[HEADER.FIELDS (FROM TO)]<0.512>)");
        assert_eq!(items, vec![
            "UID".to_string(),
            "FLAGS".to_string(),
            "BODY.PEEK[HEADER.FIELDS (FROM TO)]<0.512>".to_string(),
        ]);
        assert_eq!(split_fetch_items("FLAGS"), vec!["FLAGS".to_string()]);
    }

    #[test]
    fn sections_and_partials_are_read() {
        assert_eq!(parse_section(""), Section::Full);
        assert_eq!(parse_section("header"), Section::Header);
        assert_eq!(parse_section("TEXT"), Section::Text);
        assert_eq!(
            parse_section("HEADER.FIELDS (From Subject)"),
            Section::HeaderFields(vec!["FROM".into(), "SUBJECT".into()])
        );
        assert_eq!(split_partial("BODY[]<0.100>"), ("BODY[]", Some((0, 100))));
        assert_eq!(split_partial("BODY[]"), ("BODY[]", None));
    }

    #[test]
    fn header_filtering_keeps_folded_lines() {
        let headers = "From: a@b\r\nSubject: hello\r\n there\r\nTo: c@d\r\n";
        let kept = filter_headers(headers, &["SUBJECT".to_string()], true);
        assert!(kept.contains("Subject: hello\r\n there\r\n"));
        assert!(!kept.contains("From:"));
        assert!(kept.ends_with("\r\n\r\n"));

        let dropped = filter_headers(headers, &["SUBJECT".to_string()], false);
        assert!(dropped.contains("From: a@b"));
        assert!(!dropped.contains("hello"));
    }

    #[test]
    fn decodes_authenticate_plain() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::STANDARD
            .encode("\0bob@example.com\0s3cret".as_bytes());
        assert_eq!(
            decode_plain(&payload),
            Some(("bob@example.com".to_string(), "s3cret".to_string()))
        );

        // An authorisation identity is present but must not change who logs in.
        let with_authz = base64::engine::general_purpose::STANDARD
            .encode("admin\0bob@example.com\0s3cret".as_bytes());
        assert_eq!(
            decode_plain(&with_authz),
            Some(("bob@example.com".to_string(), "s3cret".to_string()))
        );

        assert_eq!(decode_plain("not base64 at all!!"), None);
        let no_password = base64::engine::general_purpose::STANDARD.encode("\0bob\0".as_bytes());
        assert_eq!(decode_plain(&no_password), None);
    }

    fn message() -> StoredMessage {
        StoredMessage {
            id: Uuid::nil(),
            local_uid: 12,
            message_id: Some("abc@example.com".into()),
            from_name: Some("Renée Dupont".into()),
            from_email: "renee@example.com".into(),
            to_addresses: json!([{ "name": "Bob", "email": "bob@example.org" }]),
            cc_addresses: json!([]),
            subject: "Réunion \"lundi\"".into(),
            body_text: Some("Bonjour".into()),
            body_html: None,
            attachments: json!([]),
            is_read: false,
            is_starred: false,
            folder: "inbox".into(),
            sent_at: Utc.with_ymd_and_hms(2026, 2, 7, 9, 30, 0).single(),
            received_at: Utc::now(),
            modseq: 1,
        }
    }

    #[test]
    fn envelope_has_ten_fields_in_order() {
        let env = envelope(&message());
        assert!(env.starts_with("(\"Sat, 07 Feb 2026 09:30:00 +0000\" "), "{env}");
        assert!(env.contains("=?UTF-8?B?"), "sujet non-ASCII encodé: {env}");
        assert!(env.contains("((\"Bob\" NIL \"bob\" \"example.org\"))"), "{env}");
        assert!(env.ends_with(" NIL NIL \"<abc@example.com>\")"), "{env}");
    }

    #[test]
    fn envelope_uses_nil_for_absent_lists() {
        let mut m = message();
        m.cc_addresses = json!([]);
        m.message_id = None;
        let env = envelope(&m);
        assert!(env.contains(" NIL NIL NIL NIL)"), "cc, bcc, in-reply-to, message-id: {env}");
    }

    #[test]
    fn envelope_escapes_quotes_in_the_subject() {
        let mut m = message();
        m.subject = "say \"hi\"".into();
        let env = envelope(&m);
        assert!(env.contains("\"say \\\"hi\\\"\""), "{env}");
    }

    fn simple(uid: i64) -> StoredMessage {
        StoredMessage {
            id: Uuid::nil(),
            local_uid: uid,
            message_id: Some("abc@example.com".into()),
            from_name: Some("Alice".into()),
            from_email: "alice@example.com".into(),
            to_addresses: json!([{ "name": "Bob", "email": "bob@example.org" }]),
            cc_addresses: json!([]),
            subject: "Hello".into(),
            body_text: Some("Body line".into()),
            body_html: None,
            attachments: json!([]),
            is_read: false,
            is_starred: true,
            folder: "inbox".into(),
            sent_at: Utc.with_ymd_and_hms(2026, 2, 7, 9, 30, 0).single(),
            received_at: Utc::now(),
            modseq: 1,
        }
    }

    fn text_of(response: &[u8]) -> String {
        String::from_utf8_lossy(response).into_owned()
    }

    #[test]
    fn flags_reflect_the_store_and_the_session() {
        let mut m = simple(3);
        assert_eq!(flags_of(&m, false), "\\Flagged");
        m.is_read = true;
        assert_eq!(flags_of(&m, true), "\\Seen \\Flagged \\Deleted");
    }

    #[test]
    fn store_modes_only_touch_listed_flags() {
        assert_eq!(FlagMode::Add.apply(true), Some(true));
        assert_eq!(FlagMode::Add.apply(false), None, "un flag absent n'est pas modifié");
        assert_eq!(FlagMode::Remove.apply(true), Some(false));
        assert_eq!(FlagMode::Replace.apply(false), Some(false), "FLAGS remplace tout");
    }

    #[test]
    fn a_peek_fetch_does_not_set_seen() {
        let items = vec!["BODY.PEEK[]".to_string()];
        let (_, seen) = fetch_response(1, &simple(1), false, &items, false);
        assert!(!seen);

        let items = vec!["BODY[]".to_string()];
        let (_, seen) = fetch_response(1, &simple(1), false, &items, false);
        assert!(seen, "BODY[] sans PEEK marque comme lu");
    }

    #[test]
    fn uid_fetch_always_reports_the_uid() {
        let items = vec!["FLAGS".to_string()];
        let (response, _) = fetch_response(2, &simple(41), false, &items, true);
        let text = text_of(&response);
        assert!(text.starts_with("* 2 FETCH ("), "{text}");
        assert!(text.contains("UID 41"), "{text}");
    }

    #[test]
    fn body_sections_carry_the_right_bytes() {
        let items = vec!["BODY.PEEK[HEADER]".to_string()];
        let (response, _) = fetch_response(1, &simple(1), false, &items, false);
        let text = text_of(&response);
        assert!(text.contains("BODY[HEADER] {"), "l'étiquette perd le .PEEK: {text}");
        assert!(text.contains("Subject: Hello"));
        assert!(!text.contains("Body line"), "l'en-tête ne contient pas le corps");

        let items = vec!["BODY.PEEK[TEXT]".to_string()];
        let (response, _) = fetch_response(1, &simple(1), false, &items, false);
        let text = text_of(&response);
        assert!(text.contains("Body line"));
        assert!(!text.contains("Subject:"));
    }

    #[test]
    fn literal_lengths_match_the_bytes_sent() {
        let items = vec!["BODY.PEEK[]".to_string()];
        let (response, _) = fetch_response(1, &simple(1), false, &items, false);
        let text = text_of(&response);
        let open = text.find('{').unwrap_or_default();
        let close = text[open..].find("}\r\n").unwrap_or_default() + open;
        let announced: usize = text[open + 1..close].parse().expect("longueur annoncée");
        // The response closes on `)\r\n`, so the payload is everything between
        // the literal marker and those three trailing bytes.
        let payload = &response[close + 3..response.len() - 3];
        assert_eq!(announced, payload.len(), "longueur annoncée ≠ octets envoyés");
        assert!(payload.starts_with(b"Date: "), "le littéral commence au message");
    }

    #[test]
    fn partial_fetch_is_clamped_and_labelled() {
        let part = literal_part("BODY[]", b"0123456789", Some((3, 4)));
        assert_eq!(text_of(&part), "BODY[]<3> {4}\r\n3456");

        let beyond = literal_part("BODY[]", b"012", Some((10, 5)));
        assert_eq!(text_of(&beyond), "BODY[]<3> {0}\r\n", "au-delà de la fin: vide");
    }

    #[test]
    fn macros_expand_to_their_items() {
        let items = vec!["FAST".to_string()];
        let (response, _) = fetch_response(1, &simple(9), false, &items, false);
        let text = text_of(&response);
        assert!(text.contains("FLAGS ("));
        assert!(text.contains("INTERNALDATE \"07-Feb-2026"), "{text}");
        assert!(text.contains("RFC822.SIZE "));
    }

    #[test]
    fn unknown_fetch_items_answer_nil() {
        let items = vec!["X-QUOTA".to_string()];
        let (response, _) = fetch_response(1, &simple(1), false, &items, false);
        assert!(text_of(&response).contains("X-QUOTA NIL"));
    }

    #[test]
    fn search_criteria_are_combined() {
        let mut m = simple(1);
        m.is_read = false;
        m.is_starred = true;
        assert!(matches_criteria(&m, false, &["ALL".to_string()]));
        assert!(matches_criteria(&m, false, &["UNSEEN".to_string(), "FLAGGED".to_string()]));
        assert!(!matches_criteria(&m, false, &["SEEN".to_string()]));
        assert!(matches_criteria(&m, true, &["DELETED".to_string()]));
        assert!(!matches_criteria(&m, false, &["DELETED".to_string()]));
        assert!(
            !matches_criteria(&m, false, &["SINCE".to_string()]),
            "critère non géré ⇒ aucun résultat, pas d'erreur"
        );
    }

    #[test]
    fn uid_sets_collapse_into_ranges() {
        assert_eq!(uid_set_string(&[3, 1, 2]), "1:3", "runs consécutifs → plage");
        assert_eq!(uid_set_string(&[1, 2, 4, 5, 7]), "1:2,4:5,7");
        assert_eq!(uid_set_string(&[42]), "42");
        assert_eq!(uid_set_string(&[]), "");
        assert_eq!(uid_set_string(&[5, 5, 6]), "5:6", "doublons ignorés");
    }

    #[test]
    fn resync_diff_orders_expunge_then_exists_then_flags() {
        // Old: UIDs 10, 11, 12, 13 (seq 1..4). New: 11 gone, 10 read, 14 arrived.
        let mut a = simple(10);
        let b = simple(11);
        let mut c = simple(12);
        let d = simple(13);
        c.is_starred = true;
        let old = vec![a.clone(), b, c.clone(), d.clone()];

        a.is_read = true; // flag change on UID 10
        let e = simple(14); // new arrival
        let new = vec![a, c, d, e];

        let diff = compute_resync(&old, &new);
        // UID 11 was at old sequence 2 — expunged, highest-first (just the one).
        assert_eq!(diff.expunged, vec![2]);
        // The count changed (one gone, one new) → EXISTS with the new total.
        assert_eq!(diff.exists, Some(4));
        // UID 10 is now the first message and turned \Seen.
        assert_eq!(diff.flag_changes.len(), 1);
        assert_eq!(diff.flag_changes[0].0, 1);
        assert!(diff.flag_changes[0].1.contains("\\Seen"));
    }

    #[test]
    fn resync_diff_is_empty_when_nothing_moved() {
        let old = vec![simple(1), simple(2)];
        let new = vec![simple(1), simple(2)];
        assert_eq!(compute_resync(&old, &new), ResyncDiff::default());
    }

    #[test]
    fn resync_flag_only_change_sends_no_exists() {
        let old = vec![simple(1), simple(2)];
        let mut changed = simple(2);
        changed.is_read = true;
        let new = vec![simple(1), changed];
        let diff = compute_resync(&old, &new);
        assert!(diff.expunged.is_empty());
        assert_eq!(diff.exists, None, "un simple changement de flag n'émet pas EXISTS");
        assert_eq!(diff.flag_changes.len(), 1);
        assert_eq!(diff.flag_changes[0].0, 2);
    }

    #[test]
    fn folder_names_round_trip() {
        for folder in store::FOLDERS {
            let bucket = store::folder_of(folder).expect("dossier connu");
            assert_eq!(display_name(bucket), folder);
        }
    }

    // ── CONDSTORE / QRESYNC (RFC 7162) ────────────────────────────────────────

    #[test]
    fn parses_changedsince_with_and_without_vanished() {
        let plain = parse_fetch_modifiers("(CHANGEDSINCE 12)");
        assert_eq!(plain.changed_since, Some(12));
        assert!(!plain.vanished);

        let with_vanished = parse_fetch_modifiers("(CHANGEDSINCE 40 VANISHED)");
        assert_eq!(with_vanished.changed_since, Some(40));
        assert!(with_vanished.vanished);

        // A fetch with no modifier group parses to "nothing asked".
        assert_eq!(parse_fetch_modifiers("(FLAGS UID)"), FetchModifiers::default());
    }

    #[test]
    fn parses_unchangedsince() {
        assert_eq!(parse_unchangedsince("(UNCHANGEDSINCE 12)"), Some(12));
        assert_eq!(parse_unchangedsince("(unchangedsince 7)"), Some(7));
        // A plain flag list is not a modifier group.
        assert_eq!(parse_unchangedsince("(\\Seen)"), None);
        assert!(is_modifier_group("(UNCHANGEDSINCE 12)"));
        assert!(is_modifier_group("(CHANGEDSINCE 3 VANISHED)"));
        assert!(!is_modifier_group("(\\Seen \\Flagged)"));
    }

    #[test]
    fn parses_qresync_select_parameter() {
        let full = parse_qresync("(QRESYNC (4123 12 1:100))").expect("QRESYNC valide");
        assert_eq!(full.uidvalidity, 4123);
        assert_eq!(full.modseq, 12);
        assert_eq!(full.known_uids, vec![(1, 100)]);

        // No known-uids: the client trusts the server to report every change.
        let no_uids = parse_qresync("(QRESYNC (4123 12))").expect("QRESYNC sans uids");
        assert!(no_uids.known_uids.is_empty());

        // A bare CONDSTORE parameter is not QRESYNC.
        assert_eq!(parse_qresync("(CONDSTORE)"), None);
        assert_eq!(parse_qresync("(QRESYNC (garbage))"), None, "uidvalidity non numérique");
    }

    #[test]
    fn vanished_line_collapses_into_ranges() {
        assert_eq!(
            vanished_earlier_line(&[1, 2, 3, 7]),
            Some("* VANISHED (EARLIER) 1:3,7\r\n".to_string())
        );
        // Order does not matter; the set is sorted and collapsed.
        assert_eq!(
            vanished_earlier_line(&[9, 8, 4]),
            Some("* VANISHED (EARLIER) 4,8:9\r\n".to_string())
        );
        assert_eq!(vanished_earlier_line(&[]), None, "rien de disparu ⇒ pas de ligne");
    }

    #[test]
    fn fetch_serves_modseq() {
        let mut m = simple(7);
        m.modseq = 42;
        let items = vec!["FLAGS".to_string(), "MODSEQ".to_string()];
        let (response, _) = fetch_response(3, &m, false, &items, false);
        let text = text_of(&response);
        assert!(text.contains("MODSEQ (42)"), "{text}");
    }

    #[test]
    fn append_command_tokenises_flags_and_literal() {
        // The parse path cmd_append relies on: the flag list survives as one
        // token and the trailing `{n}` is reported as a pending literal.
        let (tokens, literal) = tokenize_line("a1 APPEND Drafts (\\Seen \\Draft) {310}");
        assert_eq!(literal, Some(Literal { len: 310, non_sync: false }));
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[1].text, "APPEND");
        assert_eq!(tokens[2].text, "Drafts");
        assert_eq!(tokens[3].text, "(\\Seen \\Draft)");
        // cmd_append's own flag test: case-insensitive substring match.
        let lower = tokens[3].text.to_ascii_lowercase();
        assert!(lower.contains("\\seen"));
        assert!(!lower.contains("\\flagged"));
    }
}
