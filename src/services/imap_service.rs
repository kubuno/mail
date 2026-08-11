use anyhow::{Context, Result};
use async_imap::Session;
use futures::TryStreamExt;
use native_tls::TlsConnector as NativeTlsConnector;
use tokio::net::TcpStream;
use tokio_native_tls::{TlsConnector, TlsStream};

// With async-imap runtime-tokio feature, Session<T> requires:
// T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Debug
type TlsSession   = Session<TlsStream<TcpStream>>;
type PlainSession = Session<TcpStream>;

pub enum ImapSession {
    Tls(TlsSession),
    Plain(PlainSession),
}

/// How the IMAP session authenticates: classic password LOGIN, or the
/// SASL XOAUTH2 mechanism with an OAuth2 access token (Gmail, Microsoft).
#[derive(Clone)]
pub enum ImapAuth {
    Password(String),
    Xoauth2(String),
}

pub struct ImapConfig {
    pub host:     String,
    pub port:     u16,
    pub security: String,
    pub username: String,
    pub auth:     ImapAuth,
}

/// SASL XOAUTH2 initial response: "user={email}\x01auth=Bearer {token}\x01\x01"
/// (base64-encoded by async-imap before being sent).
struct XOAuth2Authenticator {
    user:  String,
    token: String,
}

impl async_imap::Authenticator for XOAuth2Authenticator {
    type Response = String;
    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        format!("user={}\x01auth=Bearer {}\x01\x01", self.user, self.token)
    }
}

async fn authenticate_client<T>(
    client: async_imap::Client<T>,
    username: &str,
    auth: &ImapAuth,
) -> Result<Session<T>>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    match auth {
        ImapAuth::Password(password) => client
            .login(username, password)
            .await
            .map_err(|(e, _)| anyhow::anyhow!("Authentification IMAP: {e}")),
        ImapAuth::Xoauth2(token) => client
            .authenticate(
                "XOAUTH2",
                XOAuth2Authenticator { user: username.to_string(), token: token.clone() },
            )
            .await
            .map_err(|(e, _)| anyhow::anyhow!("Authentification IMAP (XOAUTH2): {e}")),
    }
}

/// Établit une connexion IMAP et retourne une session authentifiée
pub async fn connect(cfg: &ImapConfig) -> Result<ImapSession> {
    match cfg.security.as_str() {
        "ssl" => {
            let tcp = TcpStream::connect(format!("{}:{}", cfg.host, cfg.port))
                .await
                .with_context(|| format!("Connexion TCP à {}:{}", cfg.host, cfg.port))?;

            let native = NativeTlsConnector::new().context("TLS connector")?;
            let connector = TlsConnector::from(native);
            let tls = connector
                .connect(&cfg.host, tcp)
                .await
                .context("Négociation TLS")?;

            let client = async_imap::Client::new(tls);
            let session = authenticate_client(client, &cfg.username, &cfg.auth).await?;
            Ok(ImapSession::Tls(session))
        }
        _ => {
            let tcp = TcpStream::connect(format!("{}:{}", cfg.host, cfg.port))
                .await
                .with_context(|| format!("Connexion TCP à {}:{}", cfg.host, cfg.port))?;
            let client = async_imap::Client::new(tcp);
            let session = authenticate_client(client, &cfg.username, &cfg.auth).await?;
            Ok(ImapSession::Plain(session))
        }
    }
}

/// Données brutes d'un message fetchsé
pub struct RawMessage {
    pub uid:  u32,
    pub body: Vec<u8>,
    /// `\Seen` on the server. Carried over so a backfilled mailbox does not
    /// come back as thousands of unread messages.
    pub seen: bool,
    /// `\Flagged` on the server — the provider's equivalent of our star.
    pub flagged: bool,
}

/// Reads `\Seen` / `\Flagged` off a fetched message.
fn flags_of(msg: &async_imap::types::Fetch) -> (bool, bool) {
    use async_imap::types::Flag;
    let mut seen = false;
    let mut flagged = false;
    for f in msg.flags() {
        match f {
            Flag::Seen    => seen = true,
            Flag::Flagged => flagged = true,
            _ => {}
        }
    }
    (seen, flagged)
}

/// Logout propre
pub async fn logout(session: ImapSession) {
    match session {
        ImapSession::Tls(mut s)   => { let _ = s.logout().await; }
        ImapSession::Plain(mut s) => { let _ = s.logout().await; }
    }
}

/// One selectable IMAP mailbox and the local bucket it maps to.
#[derive(Debug, Clone)]
pub struct MailboxInfo {
    /// Name as the server spells it, e.g. `INBOX`, `[Gmail]/Sent Mail`, `Projets/2026`.
    pub name: String,
    /// Local bucket: inbox, sent, drafts, spam, trash, archive or custom.
    pub kind: &'static str,
}

/// Every mailbox the account holds, each tagged with the local folder it feeds.
///
/// Special-use attributes (RFC 6154) are authoritative when the server sends
/// them; otherwise the name is matched against the usual spellings, including
/// the French ones many providers use.
pub async fn list_mailboxes(session: &mut ImapSession) -> Result<Vec<MailboxInfo>> {
    match session {
        ImapSession::Tls(s)   => list_mailboxes_inner(s).await,
        ImapSession::Plain(s) => list_mailboxes_inner(s).await,
    }
}

async fn list_mailboxes_inner<T>(session: &mut Session<T>) -> Result<Vec<MailboxInfo>>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    use async_imap::types::NameAttribute;

    let names = session.list(None, Some("*")).await.context("LIST dossiers")?;
    let collected = names.try_collect::<Vec<_>>().await.context("Collecte dossiers")?;

    let mut out = Vec::new();
    for n in collected {
        let attrs = n.attributes();
        // \Noselect mailboxes are pure hierarchy nodes: SELECT on them fails.
        if attrs.iter().any(|a| matches!(a, NameAttribute::NoSelect)) {
            continue;
        }
        let kind = attrs
            .iter()
            .find_map(|a| match a {
                NameAttribute::Sent    => Some("sent"),
                NameAttribute::Drafts  => Some("drafts"),
                NameAttribute::Junk    => Some("spam"),
                NameAttribute::Trash   => Some("trash"),
                NameAttribute::Archive => Some("archive"),
                // \All is Gmail's virtual "All Mail": every message shows up
                // there too, so syncing it would duplicate the whole mailbox.
                NameAttribute::All     => Some("__skip__"),
                _ => None,
            })
            .unwrap_or_else(|| kind_from_name(n.name()));
        if kind == "__skip__" {
            continue;
        }
        out.push(MailboxInfo { name: n.name().to_string(), kind });
    }
    Ok(out)
}

/// Decodes modified UTF-7, the encoding IMAP mailbox names use (RFC 3501
/// §5.1.3): `&` opens a base64 run (with `,` for `/`) closed by `-`, and `&-`
/// is a literal `&`. Without this, "Éléments envoyés" reaches us as
/// `&AMk-l&AOk-ments envoy&AOk-s` — unreadable in the sidebar, and unmatchable
/// against the well-known folder names.
pub fn decode_imap_utf7(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = String::with_capacity(name.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'&' {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        // "&-" is an escaped ampersand.
        if bytes.get(i + 1) == Some(&b'-') {
            out.push('&');
            i += 2;
            continue;
        }
        let Some(end) = bytes[i + 1..].iter().position(|b| *b == b'-').map(|p| i + 1 + p) else {
            // Unterminated run: keep the rest verbatim rather than lose it.
            out.push_str(&name[i..]);
            break;
        };
        match decode_utf7_run(&name[i + 1..end]) {
            Some(text) => out.push_str(&text),
            None       => out.push_str(&name[i..=end]),
        }
        i = end + 1;
    }
    out
}

/// One base64 run of modified UTF-7: UTF-16BE code units, `,` standing in for
/// `/`, no padding.
fn decode_utf7_run(run: &str) -> Option<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

    let mut bits: u32 = 0;
    let mut nbits: u8 = 0;
    let mut units: Vec<u16> = Vec::new();

    for c in run.bytes() {
        let v = ALPHABET.iter().position(|a| *a == c)? as u32;
        bits = (bits << 6) | v;
        nbits += 6;
        if nbits >= 16 {
            nbits -= 16;
            units.push((bits >> nbits) as u16);
            bits &= (1 << nbits) - 1;
        }
    }
    String::from_utf16(&units).ok()
}

/// Local bucket guessed from a mailbox name, for servers without special-use.
fn kind_from_name(name: &str) -> &'static str {
    let lower = decode_imap_utf7(name).to_lowercase();
    // Compare on the leaf: "INBOX.Sent" and "[Gmail]/Sent Mail" are both Sent.
    let leaf = lower
        .rsplit(['/', '.'])
        .find(|s| !s.is_empty())
        .unwrap_or(&lower)
        .trim()
        .to_string();

    if lower == "inbox" {
        return "inbox";
    }
    let has = |needles: &[&str]| needles.iter().any(|n| leaf.contains(n));
    if has(&["sent", "envoyé", "envoye"])                    { return "sent" }
    if has(&["draft", "brouillon"])                          { return "drafts" }
    if has(&["spam", "junk", "indésirable", "indesirable"])  { return "spam" }
    if has(&["trash", "corbeille", "deleted", "supprimé", "supprime"]) { return "trash" }
    if has(&["archive"])                                     { return "archive" }
    if has(&["all mail", "tous les messages"])               { return "__skip__" }
    "custom"
}

/// SELECT a mailbox and report how many messages it holds.
pub async fn select_folder(session: &mut ImapSession, folder: &str) -> Result<u32> {
    match session {
        ImapSession::Tls(s)   => Ok(s.select(folder).await.context("SELECT dossier IMAP")?.exists),
        ImapSession::Plain(s) => Ok(s.select(folder).await.context("SELECT dossier IMAP")?.exists),
    }
}

/// UIDs present in a range of the currently selected mailbox, ascending.
/// `range` is an IMAP UID set such as `1:*` or `1:4200`.
pub async fn uid_list(session: &mut ImapSession, range: &str) -> Result<Vec<u32>> {
    let set = match session {
        ImapSession::Tls(s)   => s.uid_search(format!("UID {range}")).await,
        ImapSession::Plain(s) => s.uid_search(format!("UID {range}")).await,
    }
    .context("UID SEARCH")?;

    let mut uids: Vec<u32> = set.into_iter().collect();
    uids.sort_unstable();
    Ok(uids)
}

/// Bodies for an explicit list of UIDs of the currently selected mailbox.
/// Callers pass one batch at a time — the whole batch is held in memory.
pub async fn fetch_uids(session: &mut ImapSession, uids: &[u32]) -> Result<Vec<RawMessage>> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    let set = uids.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
    match session {
        ImapSession::Tls(s)   => fetch_set_inner(s, &set).await,
        ImapSession::Plain(s) => fetch_set_inner(s, &set).await,
    }
}

async fn fetch_set_inner<T>(session: &mut Session<T>, set: &str) -> Result<Vec<RawMessage>>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let messages = session
        .uid_fetch(set, "(UID FLAGS BODY.PEEK[])")
        .await
        .context("UID FETCH")?;
    let out = messages
        .try_collect::<Vec<_>>()
        .await
        .context("Collecte messages IMAP")?
        .into_iter()
        .filter_map(|msg| {
                    let (seen, flagged) = flags_of(&msg);
                    Some(RawMessage { uid: msg.uid?, body: msg.body()?.to_vec(), seen, flagged })
                })
        .collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_modified_utf7() {
        assert_eq!(decode_imap_utf7("INBOX.&AMk-l&AOk-ments envoy&AOk-s"), "INBOX.Éléments envoyés");
        assert_eq!(decode_imap_utf7("Projets/2026"), "Projets/2026");
        assert_eq!(decode_imap_utf7("R&AOk-ponses &- suivi"), "Réponses & suivi");
    }

    #[test]
    fn maps_localized_folders_to_buckets() {
        assert_eq!(kind_from_name("INBOX.&AMk-l&AOk-ments envoy&AOk-s"), "sent");
        assert_eq!(kind_from_name("[Gmail]/Sent Mail"), "sent");
        assert_eq!(kind_from_name("INBOX.Corbeille"), "trash");
        assert_eq!(kind_from_name("Projets/2026"), "custom");
        assert_eq!(kind_from_name("INBOX"), "inbox");
    }
}
