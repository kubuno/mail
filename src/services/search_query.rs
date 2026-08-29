//! Gmail-style search query language: tokenizer, parser and SQL compiler.
//!
//! Supported operators: `in:`, `is:` (incl. `is:subscription` — Gmail's `^sub_m`
//! equivalent: the message carries a `List-Unsubscribe` header), `label:`,
//! `from:`, `to:`, `cc:`, `bcc:`,
//! `subject:`, `has:attachment|userlabels|nouserlabels|drive|document|spreadsheet|
//! presentation|youtube`, `filename:`, `after:`/`newer:`, `before:`/`older:`,
//! `older_than:`, `newer_than:`, `larger:`/`size:`, `smaller:`, `list:`,
//! `deliveredto:`, `category:`, `rfc822msgid:`, negation `-`, `OR` / `{ }`,
//! exact phrases `" "`, grouping `( )` and exact word `+word`.
//!
//! The compiler emits parameterized SQL fragments through sqlx's `QueryBuilder`
//! (never string interpolation) against the `mail.threads t` / `mail.messages m`
//! join used by `list_threads`.

use chrono::NaiveDate;
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

// ── AST ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Node {
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    Crit(Crit),
}

#[derive(Debug, Clone)]
pub enum Crit {
    /// Plain word: subject / body / sender.
    Word(String),
    /// `+word` — exact word (word-boundary match, accent-sensitive).
    ExactWord(String),
    /// `"exact phrase"`.
    Phrase(String),
    From(String),
    To(String),
    Cc(String),
    Bcc(String),
    Subject(String),
    /// `in:` folder (inbox/sent/drafts/spam/trash/archive/anywhere/all/snoozed/starred/important)
    In(String),
    /// `is:` state (read/unread/starred/unstarred/important/notimportant/snoozed/muted)
    Is(String),
    Label(String),
    HasAttachment,
    HasUserLabels,
    HasNoUserLabels,
    /// `has:drive|document|spreadsheet|presentation|youtube` → link pattern in body.
    HasLink(String),
    Filename(String),
    After(NaiveDate),
    Before(NaiveDate),
    OlderThanDays(i32),
    NewerThanDays(i32),
    LargerBytes(i64),
    SmallerBytes(i64),
    List(String),
    DeliveredTo(String),
    Category(String),
    Rfc822MsgId(String),
    /// Unknown operator / unparsable value — neutral (matches everything).
    Noop,
}

// ── Tokenizer ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    LParen,
    RParen,
    LBrace,
    RBrace,
    Or,
    Minus,
    Plus(String),
    Quoted(String),
    Word(String),
    /// `name:value` — value is raw (may itself be quoted or a `( … )` group).
    Op(String, OpVal),
}

#[derive(Debug, Clone, PartialEq)]
enum OpVal {
    Bare(String),
    Quoted(String),
    /// `subject:(urgent problème)` — every inner word is required.
    Group(Vec<String>),
}

const KNOWN_OPS: &[&str] = &[
    "in", "is", "label", "from", "to", "cc", "bcc", "subject", "has", "filename",
    "after", "before", "newer", "older", "older_than", "newer_than", "larger",
    "smaller", "size", "list", "deliveredto", "category", "rfc822msgid",
];

fn tokenize(raw: &str) -> Vec<Tok> {
    let chars: Vec<char> = raw.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0usize;
    let n = chars.len();

    let read_quoted = |i: &mut usize| -> String {
        // *i points at the opening quote
        *i += 1;
        let start = *i;
        while *i < n && chars[*i] != '"' {
            *i += 1;
        }
        let s: String = chars[start..*i].iter().collect();
        if *i < n {
            *i += 1; // closing quote
        }
        s
    };

    while i < n {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            toks.push(Tok::LParen);
            i += 1;
        } else if c == ')' {
            toks.push(Tok::RParen);
            i += 1;
        } else if c == '{' {
            toks.push(Tok::LBrace);
            i += 1;
        } else if c == '}' {
            toks.push(Tok::RBrace);
            i += 1;
        } else if c == '"' {
            toks.push(Tok::Quoted(read_quoted(&mut i)));
        } else if c == '-' {
            toks.push(Tok::Minus);
            i += 1;
        } else if c == '+' && i + 1 < n && !chars[i + 1].is_whitespace() {
            i += 1;
            let start = i;
            while i < n && !chars[i].is_whitespace() && !"(){}".contains(chars[i]) {
                i += 1;
            }
            toks.push(Tok::Plus(chars[start..i].iter().collect()));
        } else {
            // word — possibly `name:value`
            let start = i;
            while i < n && !chars[i].is_whitespace() && !"(){}\"".contains(chars[i]) && chars[i] != ':' {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if i < n && chars[i] == ':' && KNOWN_OPS.contains(&word.to_lowercase().as_str()) {
                i += 1; // skip ':'
                if i < n && chars[i] == '"' {
                    toks.push(Tok::Op(word.to_lowercase(), OpVal::Quoted(read_quoted(&mut i))));
                } else if i < n && chars[i] == '(' {
                    // value group: collect bare/quoted words until ')'
                    i += 1;
                    let mut items = Vec::new();
                    while i < n && chars[i] != ')' {
                        if chars[i].is_whitespace() {
                            i += 1;
                        } else if chars[i] == '"' {
                            items.push(read_quoted(&mut i));
                        } else {
                            let s = i;
                            while i < n && !chars[i].is_whitespace() && chars[i] != ')' {
                                i += 1;
                            }
                            items.push(chars[s..i].iter().collect());
                        }
                    }
                    if i < n {
                        i += 1; // ')'
                    }
                    toks.push(Tok::Op(word.to_lowercase(), OpVal::Group(items)));
                } else {
                    let s = i;
                    while i < n && !chars[i].is_whitespace() && !"(){}\"".contains(chars[i]) {
                        i += 1;
                    }
                    let val: String = chars[s..i].iter().collect();
                    toks.push(Tok::Op(word.to_lowercase(), OpVal::Bare(val)));
                }
            } else if word.eq_ignore_ascii_case("or") {
                toks.push(Tok::Or);
            } else if !word.is_empty() {
                // Re-attach a lone ':' (e.g. inside URLs) to the word.
                if i < n && chars[i] == ':' {
                    let s = i;
                    let mut j = i;
                    while j < n && !chars[j].is_whitespace() && !"(){}\"".contains(chars[j]) {
                        j += 1;
                    }
                    i = j;
                    let rest: String = chars[s..j].iter().collect();
                    toks.push(Tok::Word(format!("{word}{rest}")));
                } else {
                    toks.push(Tok::Word(word));
                }
            } else {
                i += 1; // stray ':' or similar
            }
        }
    }
    toks
}

// ── Parser ────────────────────────────────────────────────────────────────────
// expr    := andExpr (OR andExpr)*
// andExpr := unary+
// unary   := '-' unary | primary
// primary := '(' expr ')' | '{' unary* '}' (OR-combined) | criterion

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_expr(&mut self, stop: &[Tok]) -> Node {
        let mut branches = vec![self.parse_and(stop)];
        while matches!(self.peek(), Some(Tok::Or)) {
            self.next();
            branches.push(self.parse_and(stop));
        }
        if branches.len() == 1 {
            branches.pop().unwrap_or(Node::Crit(Crit::Noop))
        } else {
            Node::Or(branches)
        }
    }

    fn parse_and(&mut self, stop: &[Tok]) -> Node {
        let mut items = Vec::new();
        while let Some(t) = self.peek() {
            if stop.contains(t) || *t == Tok::Or {
                break;
            }
            if let Some(node) = self.parse_unary(stop) {
                items.push(node);
            }
        }
        match items.len() {
            0 => Node::Crit(Crit::Noop),
            1 => items.pop().unwrap_or(Node::Crit(Crit::Noop)),
            _ => Node::And(items),
        }
    }

    fn parse_unary(&mut self, stop: &[Tok]) -> Option<Node> {
        match self.peek()? {
            Tok::Minus => {
                self.next();
                // A dangling '-' at end of input is ignored.
                self.parse_unary(stop).map(|inner| Node::Not(Box::new(inner)))
            }
            _ => self.parse_primary(stop),
        }
    }

    fn parse_primary(&mut self, _stop: &[Tok]) -> Option<Node> {
        match self.next()? {
            Tok::LParen => {
                let node = self.parse_expr(&[Tok::RParen]);
                if matches!(self.peek(), Some(Tok::RParen)) {
                    self.next();
                }
                Some(node)
            }
            Tok::LBrace => {
                // `{a b}` — OR between every item.
                let mut items = Vec::new();
                while let Some(t) = self.peek() {
                    if *t == Tok::RBrace {
                        break;
                    }
                    if *t == Tok::Or {
                        self.next();
                        continue;
                    }
                    if let Some(node) = self.parse_unary(&[Tok::RBrace]) {
                        items.push(node);
                    }
                }
                if matches!(self.peek(), Some(Tok::RBrace)) {
                    self.next();
                }
                match items.len() {
                    0 => Some(Node::Crit(Crit::Noop)),
                    1 => items.pop(),
                    _ => Some(Node::Or(items)),
                }
            }
            Tok::RParen | Tok::RBrace | Tok::Or | Tok::Minus => Some(Node::Crit(Crit::Noop)),
            Tok::Plus(w) => Some(Node::Crit(Crit::ExactWord(w))),
            Tok::Quoted(s) => {
                let s = s.trim().to_string();
                Some(Node::Crit(if s.is_empty() { Crit::Noop } else { Crit::Phrase(s) }))
            }
            Tok::Word(w) => Some(Node::Crit(Crit::Word(w))),
            Tok::Op(name, val) => Some(op_to_node(&name, val)),
        }
    }
}

fn op_to_node(name: &str, val: OpVal) -> Node {
    // Multi-valued group: every inner term is required (Gmail `subject:(a b)`).
    if let OpVal::Group(items) = val {
        let nodes: Vec<Node> = items
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| op_to_node(name, OpVal::Bare(s)))
            .collect();
        return match nodes.len() {
            0 => Node::Crit(Crit::Noop),
            1 => nodes.into_iter().next().unwrap_or(Node::Crit(Crit::Noop)),
            _ => Node::And(nodes),
        };
    }
    let v = match val {
        OpVal::Bare(s) | OpVal::Quoted(s) => s,
        OpVal::Group(_) => unreachable!(),
    };
    let v = v.trim().to_string();
    if v.is_empty() && name != "has" {
        return Node::Crit(Crit::Noop);
    }
    let crit = match name {
        "from" => Crit::From(v),
        "to" => Crit::To(v),
        "cc" => Crit::Cc(v),
        "bcc" => Crit::Bcc(v),
        "subject" => Crit::Subject(v),
        "in" => Crit::In(v.to_lowercase()),
        "is" => Crit::Is(v.to_lowercase()),
        "label" => Crit::Label(v),
        "filename" => Crit::Filename(v),
        "list" => Crit::List(v),
        "deliveredto" => Crit::DeliveredTo(v),
        "category" => Crit::Category(v.to_lowercase()),
        "rfc822msgid" => Crit::Rfc822MsgId(v),
        "has" => match v.to_lowercase().as_str() {
            "attachment" | "attachments" => Crit::HasAttachment,
            "userlabels" => Crit::HasUserLabels,
            "nouserlabels" => Crit::HasNoUserLabels,
            "drive" | "document" | "spreadsheet" | "presentation" | "youtube" => {
                Crit::HasLink(v.to_lowercase())
            }
            _ => Crit::Noop,
        },
        "after" | "newer" => parse_date(&v).map(Crit::After).unwrap_or(Crit::Noop),
        "before" | "older" => parse_date(&v).map(Crit::Before).unwrap_or(Crit::Noop),
        "older_than" => parse_period_days(&v).map(Crit::OlderThanDays).unwrap_or(Crit::Noop),
        "newer_than" => parse_period_days(&v).map(Crit::NewerThanDays).unwrap_or(Crit::Noop),
        "larger" | "size" => parse_size_bytes(&v).map(Crit::LargerBytes).unwrap_or(Crit::Noop),
        "smaller" => parse_size_bytes(&v).map(Crit::SmallerBytes).unwrap_or(Crit::Noop),
        _ => Crit::Noop,
    };
    Node::Crit(crit)
}

// ── Value parsing helpers ─────────────────────────────────────────────────────

/// `2024/01/31`, `2024-01-31` or `31/01/2024` (FR).
fn parse_date(v: &str) -> Option<NaiveDate> {
    for fmt in ["%Y/%m/%d", "%Y-%m-%d", "%d/%m/%Y", "%m/%d/%Y"] {
        if let Ok(d) = NaiveDate::parse_from_str(v, fmt) {
            return Some(d);
        }
    }
    None
}

/// `2d`, `3m`, `1y` — also accepts the French `j` (jours) and `a` (années),
/// plus `w`/`s` for weeks.
fn parse_period_days(v: &str) -> Option<i32> {
    let v = v.trim().to_lowercase();
    let split = v.find(|c: char| !c.is_ascii_digit())?;
    let (num, unit) = v.split_at(split);
    let num: i32 = num.parse().ok()?;
    let days = match unit {
        "d" | "j" => num,
        "w" | "s" => num.checked_mul(7)?,
        "m" => num.checked_mul(30)?,
        "y" | "a" => num.checked_mul(365)?,
        _ => return None,
    };
    Some(days)
}

/// `500000`, `5K`, `10M`, `1G` (also `Ko/Mo/Go`, `KB/MB/GB`).
fn parse_size_bytes(v: &str) -> Option<i64> {
    let v = v.trim().to_lowercase();
    let split = v.find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len());
    let (num, unit) = v.split_at(split);
    let num: i64 = num.parse().ok()?;
    let mult: i64 = match unit {
        "" | "b" | "o" => 1,
        "k" | "kb" | "ko" => 1_024,
        "m" | "mb" | "mo" => 1_048_576,
        "g" | "gb" | "go" => 1_073_741_824,
        _ => return None,
    };
    num.checked_mul(mult)
}

// ── Public API ────────────────────────────────────────────────────────────────

pub struct ParsedSearch {
    pub root: Node,
    /// True when the query names a location explicitly (`in:`) — in that case
    /// the default "hide spam & trash" filter is skipped, like Gmail.
    pub has_in: bool,
}

pub fn parse(raw: &str) -> ParsedSearch {
    let mut p = Parser { toks: tokenize(raw), pos: 0 };
    let root = p.parse_expr(&[]);
    let has_in = node_has_in(&root);
    ParsedSearch { root, has_in }
}

fn node_has_in(n: &Node) -> bool {
    match n {
        Node::And(v) | Node::Or(v) => v.iter().any(node_has_in),
        Node::Not(b) => node_has_in(b),
        Node::Crit(Crit::In(_)) => true,
        Node::Crit(_) => false,
    }
}

// ── SQL compiler ──────────────────────────────────────────────────────────────

/// Escapes SQL LIKE wildcards and wraps in `%…%` for a "contains" match.
fn like(term: &str) -> String {
    format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

/// Escapes a literal for safe inclusion inside a bound POSIX regex.
fn regex_escape(term: &str) -> String {
    let mut out = String::with_capacity(term.len() * 2);
    for c in term.chars() {
        if "\\^$.|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Pushes the WHERE fragment for `node` onto `qb`. The caller is responsible
/// for the surrounding `AND`. `user_id` feeds the `from:me` / `to:me` subquery.
pub fn push_sql(qb: &mut QueryBuilder<'_, Postgres>, node: &Node, user_id: Uuid) {
    match node {
        Node::And(items) => {
            qb.push("(");
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    qb.push(" AND ");
                }
                push_sql(qb, it, user_id);
            }
            qb.push(")");
        }
        Node::Or(items) => {
            qb.push("(");
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    qb.push(" OR ");
                }
                push_sql(qb, it, user_id);
            }
            qb.push(")");
        }
        Node::Not(inner) => {
            qb.push("NOT ");
            push_sql(qb, inner, user_id);
        }
        Node::Crit(c) => push_crit(qb, c, user_id),
    }
}

/// `m.<col> ILIKE unaccent($term)` — accent- and case-insensitive contains.
fn push_ilike(qb: &mut QueryBuilder<'_, Postgres>, col: &str, term: &str) {
    qb.push("unaccent(")
        .push(col)
        .push(") ILIKE unaccent(")
        .push_bind(like(term))
        .push(")");
}

/// Subquery matching any of the user's own account addresses (`from:me`).
fn push_me_subquery(qb: &mut QueryBuilder<'_, Postgres>, col_expr: &str, user_id: Uuid) {
    qb.push("EXISTS (SELECT 1 FROM mail.accounts acc WHERE acc.user_id = ")
        .push_bind(user_id)
        .push(" AND ")
        .push(col_expr)
        .push(" ILIKE '%' || acc.email_address || '%')");
}

fn push_crit(qb: &mut QueryBuilder<'_, Postgres>, c: &Crit, user_id: Uuid) {
    match c {
        Crit::Noop => {
            qb.push("TRUE");
        }
        Crit::Word(w) => {
            qb.push("(");
            push_ilike(qb, "m.subject", w);
            qb.push(" OR ");
            push_ilike(qb, "COALESCE(m.body_text,'')", w);
            qb.push(" OR ");
            push_ilike(qb, "m.from_email", w);
            qb.push(" OR ");
            push_ilike(qb, "COALESCE(m.from_name,'')", w);
            qb.push(")");
        }
        Crit::ExactWord(w) => {
            // Word-boundary, case-insensitive, accent-sensitive (Gmail's `+`).
            let pat = format!("\\m{}\\M", regex_escape(w));
            qb.push("(m.subject ~* ").push_bind(pat.clone());
            qb.push(" OR COALESCE(m.body_text,'') ~* ").push_bind(pat);
            qb.push(")");
        }
        Crit::Phrase(p) => {
            qb.push("(");
            push_ilike(qb, "m.subject", p);
            qb.push(" OR ");
            push_ilike(qb, "COALESCE(m.body_text,'')", p);
            qb.push(")");
        }
        Crit::From(v) => {
            if v.eq_ignore_ascii_case("me") {
                push_me_subquery(qb, "m.from_email", user_id);
            } else {
                qb.push("(");
                push_ilike(qb, "m.from_email", v);
                qb.push(" OR ");
                push_ilike(qb, "COALESCE(m.from_name,'')", v);
                qb.push(")");
            }
        }
        Crit::To(v) => {
            if v.eq_ignore_ascii_case("me") {
                push_me_subquery(qb, "m.to_addresses::text", user_id);
            } else {
                push_ilike(qb, "m.to_addresses::text", v);
            }
        }
        Crit::Cc(v) => {
            if v.eq_ignore_ascii_case("me") {
                push_me_subquery(qb, "m.cc_addresses::text", user_id);
            } else {
                push_ilike(qb, "m.cc_addresses::text", v);
            }
        }
        Crit::Bcc(v) => {
            if v.eq_ignore_ascii_case("me") {
                push_me_subquery(qb, "m.bcc_addresses::text", user_id);
            } else {
                push_ilike(qb, "m.bcc_addresses::text", v);
            }
        }
        Crit::Subject(v) => push_ilike(qb, "m.subject", v),
        Crit::In(place) => match place.as_str() {
            "anywhere" | "all" => {
                qb.push("TRUE");
            }
            "inbox" | "sent" | "drafts" | "spam" | "trash" | "archive" => {
                qb.push("m.folder = ").push_bind(place.clone());
            }
            "snoozed" => {
                qb.push("t.snoozed_until > NOW()");
            }
            "starred" => {
                qb.push("t.is_starred = TRUE");
            }
            "important" => {
                qb.push("t.is_important = TRUE");
            }
            "unread" => {
                qb.push("m.is_read = FALSE");
            }
            _ => {
                qb.push("m.folder = ").push_bind(place.clone());
            }
        },
        Crit::Is(state) => match state.as_str() {
            "unread" => {
                qb.push("m.is_read = FALSE");
            }
            "read" => {
                qb.push("m.is_read = TRUE");
            }
            "starred" => {
                qb.push("t.is_starred = TRUE");
            }
            "unstarred" => {
                qb.push("t.is_starred = FALSE");
            }
            "important" => {
                qb.push("t.is_important = TRUE");
            }
            "notimportant" | "unimportant" => {
                qb.push("t.is_important = FALSE");
            }
            "snoozed" => {
                qb.push("t.snoozed_until > NOW()");
            }
            "muted" => {
                qb.push("t.is_muted = TRUE");
            }
            // Subscription-type messages — the Kubuno equivalent of Gmail's
            // internal `^sub_m` system label: a message that carries a
            // `List-Unsubscribe` header. Used by the "Manage subscriptions"
            // view when clicking a sender (`from:X … is:subscription`).
            "subscription" | "subscriptions" => {
                qb.push("(m.list_unsubscribe IS NOT NULL AND m.list_unsubscribe <> '')");
            }
            _ => {
                qb.push("TRUE");
            }
        },
        Crit::Label(name) => {
            // Exact label-name match; Gmail turns spaces into dashes in queries,
            // so `label:mon-travail` also matches the label "mon travail".
            qb.push(
                "EXISTS (SELECT 1 FROM mail.thread_labels tl \
                 JOIN mail.labels l ON l.id = tl.label_id \
                 WHERE tl.thread_id = t.id AND (unaccent(l.name) ILIKE unaccent(",
            )
            .push_bind(name.clone())
            .push(") OR unaccent(l.name) ILIKE unaccent(")
            .push_bind(name.replace(['-', '_'], " "))
            .push(")))");
        }
        Crit::HasAttachment => {
            qb.push("jsonb_array_length(m.attachments) > 0");
        }
        Crit::HasUserLabels => {
            qb.push("EXISTS (SELECT 1 FROM mail.thread_labels tl WHERE tl.thread_id = t.id)");
        }
        Crit::HasNoUserLabels => {
            qb.push("NOT EXISTS (SELECT 1 FROM mail.thread_labels tl WHERE tl.thread_id = t.id)");
        }
        Crit::HasLink(kind) => {
            let pat = match kind.as_str() {
                "drive" => r"(drive\.google\.com|/files/|/api/v1/files/)",
                "document" => r"(docs\.google\.com/document|/office/documents/)",
                "spreadsheet" => r"(docs\.google\.com/spreadsheets|/office/spreadsheets/)",
                "presentation" => r"(docs\.google\.com/presentation|/office/presentations/)",
                _ => r"(youtube\.com/(watch|shorts|embed)|youtu\.be/)",
            };
            qb.push("COALESCE(m.body_html, m.body_text, '') ~* ").push_bind(pat);
        }
        Crit::Filename(v) => {
            qb.push(
                "EXISTS (SELECT 1 FROM jsonb_array_elements(m.attachments) a \
                 WHERE unaccent(COALESCE(a->>'name','')) ILIKE unaccent(",
            )
            .push_bind(like(v))
            .push("))");
        }
        Crit::After(d) => {
            qb.push("m.received_at >= ").push_bind(*d);
        }
        Crit::Before(d) => {
            qb.push("m.received_at < ").push_bind(*d);
        }
        Crit::OlderThanDays(days) => {
            qb.push("m.received_at < NOW() - make_interval(days => ").push_bind(*days).push(")");
        }
        Crit::NewerThanDays(days) => {
            qb.push("m.received_at > NOW() - make_interval(days => ").push_bind(*days).push(")");
        }
        Crit::LargerBytes(b) => {
            push_size_expr(qb);
            qb.push(" > ").push_bind(*b);
        }
        Crit::SmallerBytes(b) => {
            push_size_expr(qb);
            qb.push(" < ").push_bind(*b);
        }
        Crit::List(v) => {
            qb.push("(");
            push_ilike(qb, "COALESCE(m.list_unsubscribe,'')", v);
            qb.push(" OR ");
            push_ilike(qb, "m.from_email", v);
            qb.push(" OR ");
            push_ilike(qb, "m.to_addresses::text", v);
            qb.push(")");
        }
        Crit::DeliveredTo(v) => {
            qb.push("(");
            push_ilike(qb, "m.to_addresses::text", v);
            qb.push(" OR ");
            push_ilike(qb, "m.cc_addresses::text", v);
            qb.push(" OR ");
            push_ilike(qb, "m.bcc_addresses::text", v);
            qb.push(")");
        }
        Crit::Category(cat) => push_category(qb, cat),
        Crit::Rfc822MsgId(v) => {
            let clean = v.trim_matches(|c| c == '<' || c == '>').to_string();
            qb.push("TRIM(BOTH '<>' FROM COALESCE(m.message_id,'')) = ").push_bind(clean);
        }
    }
}

/// Approximate message size: body HTML + body text + declared attachment sizes.
fn push_size_expr(qb: &mut QueryBuilder<'_, Postgres>) {
    qb.push(
        "(octet_length(COALESCE(m.body_html,'')) + octet_length(COALESCE(m.body_text,'')) \
         + COALESCE((SELECT SUM(COALESCE((a->>'size')::BIGINT, 0)) \
                     FROM jsonb_array_elements(m.attachments) a), 0))",
    );
}

// Same sender-based heuristics as the inbox tabs in the frontend (threadTab).
const CAT_SOCIAL: &str =
    r"twitter|facebook|linkedin|instagram|tiktok|youtube|pinterest|snapchat|meta\.com|x\.com";
const CAT_NOTIF: &str = r"notification|alert|update|security|account|billing";
const CAT_PROMO: &str =
    r"no.?reply|newsletter|noreply|promo|marketing|info@|hello@|contact@|deals?@|offers?@";

fn push_category(qb: &mut QueryBuilder<'_, Postgres>, cat: &str) {
    // Classification order mirrors the frontend: social > notifications > promotions.
    match cat {
        "social" | "reseaux" | "réseaux" => {
            qb.push("m.from_email ~* ").push_bind(CAT_SOCIAL);
        }
        "notifications" | "updates" | "notification" => {
            qb.push("(m.from_email ~* ").push_bind(CAT_NOTIF);
            qb.push(" AND m.from_email !~* ").push_bind(CAT_SOCIAL);
            qb.push(")");
        }
        "promotions" | "promos" | "promotion" => {
            qb.push("(m.from_email ~* ").push_bind(CAT_PROMO);
            qb.push(" AND m.from_email !~* ").push_bind(CAT_SOCIAL);
            qb.push(" AND m.from_email !~* ").push_bind(CAT_NOTIF);
            qb.push(")");
        }
        "primary" | "main" | "principale" => {
            qb.push("(m.from_email !~* ").push_bind(CAT_SOCIAL);
            qb.push(" AND m.from_email !~* ").push_bind(CAT_NOTIF);
            qb.push(" AND m.from_email !~* ").push_bind(CAT_PROMO);
            qb.push(")");
        }
        _ => {
            qb.push("TRUE");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crit_count(n: &Node) -> usize {
        match n {
            Node::And(v) | Node::Or(v) => v.iter().map(crit_count).sum(),
            Node::Not(b) => crit_count(b),
            Node::Crit(Crit::Noop) => 0,
            Node::Crit(_) => 1,
        }
    }

    #[test]
    fn parses_operators() {
        let p = parse("from:jean OR from:marie has:attachment -is:unread subject:(urgent problème) \"compte rendu\" +licorne");
        assert!(crit_count(&p.root) >= 7);
        assert!(!p.has_in);
    }

    #[test]
    fn detects_in() {
        assert!(parse("in:trash test").has_in);
        assert!(parse("-in:trash test").has_in);
    }

    /// The exact query the "Manage subscriptions" view issues when a sender row
    /// is clicked — Gmail's `from:X (-label:spam OR label:trash) label:^sub_m`
    /// transposed to our operators. It must parse as a combined tree and lift
    /// the implicit spam/trash exclusion (the query names locations itself).
    #[test]
    fn subscription_sender_query() {
        let p = parse("from:x@y.z (-in:spam OR in:trash) is:subscription");
        assert!(p.has_in);
        assert!(crit_count(&p.root) >= 3);
    }

    #[test]
    fn period_and_size() {
        assert_eq!(parse_period_days("2m"), Some(60));
        assert_eq!(parse_period_days("5j"), Some(5));
        assert_eq!(parse_size_bytes("10M"), Some(10 * 1_048_576));
        assert_eq!(parse_size_bytes("500000"), Some(500_000));
        assert_eq!(parse_size_bytes("5Ko"), Some(5 * 1_024));
    }

    #[test]
    fn dates() {
        assert!(parse_date("2024/01/31").is_some());
        assert!(parse_date("31/01/2024").is_some());
        assert!(parse_date("garbage").is_none());
    }

    #[test]
    fn braces_or() {
        let p = parse("{from:a from:b}");
        match &p.root {
            Node::Or(v) => assert_eq!(v.len(), 2),
            other => panic!("expected Or, got {other:?}"),
        }
    }

    #[test]
    fn url_word_not_operator() {
        let p = parse("https://example.com");
        match &p.root {
            Node::Crit(Crit::Word(w)) => assert_eq!(w, "https://example.com"),
            other => panic!("expected Word, got {other:?}"),
        }
    }
}
