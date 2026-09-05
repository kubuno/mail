use anyhow::{Context, Result};
use async_imap::Session;
use futures::TryStreamExt;
use native_tls::TlsConnector as NativeTlsConnector;
use std::collections::HashMap;
use tokio::net::TcpStream;
use tokio_native_tls::{TlsConnector, TlsStream};

/// Largest single message this module will pull down from a remote IMAP server.
///
/// The peer is not ours: a hostile — or merely broken — server can answer a
/// FETCH with a literal of any length, and the whole body has to be buffered
/// before `mail_parser` can even look at it. The value mirrors the inbound SMTP
/// ceiling (`server::config::ServerConfig::max_message_bytes`, 25 MiB by
/// default) so both ingestion paths agree on what is too big to accept.
/// Deliberately a constant rather than an instance setting: those live in the
/// core and cost an internal HTTP round trip, which the sync loop must not pay
/// once per batch.
pub const MAX_MESSAGE_BYTES: usize = 25 * 1024 * 1024;

/// Ceiling on what one FETCH round trip may bring back at once.
///
/// `mail.max_fetch_per_sync` bounds the NUMBER of messages in a batch (200 by
/// default) but says nothing about their weight: 200 messages at the
/// per-message ceiling would be 5 GiB resident. A batch is therefore split
/// again into groups whose announced sizes stay under this budget. Must stay
/// above `MAX_MESSAGE_BYTES` so an accepted message always fits in a group.
pub const MAX_FETCH_GROUP_BYTES: usize = 64 * 1024 * 1024;

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

/// Announced sizes (RFC822.SIZE) for a list of UIDs, for the ones the server
/// reports. One cheap round trip whose whole point is that an oversized message
/// can then be skipped WITHOUT ever being downloaded.
pub async fn uid_sizes(session: &mut ImapSession, uids: &[u32]) -> Result<HashMap<u32, u32>> {
    if uids.is_empty() {
        return Ok(HashMap::new());
    }
    let set = uid_set(uids);
    match session {
        ImapSession::Tls(s)   => uid_sizes_inner(s, &set).await,
        ImapSession::Plain(s) => uid_sizes_inner(s, &set).await,
    }
}

async fn uid_sizes_inner<T>(session: &mut Session<T>, set: &str) -> Result<HashMap<u32, u32>>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let messages = session
        .uid_fetch(set, "(UID RFC822.SIZE)")
        .await
        .context("UID FETCH RFC822.SIZE")?;
    Ok(messages
        .try_collect::<Vec<_>>()
        .await
        .context("Collecte tailles IMAP")?
        .into_iter()
        .filter_map(|msg| Some((msg.uid?, msg.size?)))
        .collect())
}

/// One batch of UIDs turned into FETCH round trips that stay within the memory
/// ceilings, plus the UIDs that are too big to be downloaded at all.
#[derive(Debug, Default)]
pub struct FetchPlan {
    pub groups: Vec<Vec<u32>>,
    /// `(uid, announced size)` of the messages left on the server untouched.
    pub oversized: Vec<(u32, u32)>,
}

/// Plans the FETCHes for one batch of UIDs from their announced sizes.
pub fn plan_fetches(uids: &[u32], sizes: &HashMap<u32, u32>) -> FetchPlan {
    plan_fetches_with(uids, sizes, MAX_MESSAGE_BYTES, MAX_FETCH_GROUP_BYTES)
}

/// A UID the server reported no size for is budgeted at the full per-message
/// ceiling: silence is not a promise of smallness, and paying one extra round
/// trip is cheaper than discovering the truth with the bytes already in RAM.
fn plan_fetches_with(
    uids: &[u32],
    sizes: &HashMap<u32, u32>,
    max_message_bytes: usize,
    max_group_bytes: usize,
) -> FetchPlan {
    let mut plan = FetchPlan::default();
    let mut current: Vec<u32> = Vec::new();
    let mut current_bytes = 0usize;

    for &uid in uids {
        let announced = sizes.get(&uid).copied();
        if let Some(size) = announced {
            if size as usize > max_message_bytes {
                plan.oversized.push((uid, size));
                continue;
            }
        }
        let weight = announced.map_or(max_message_bytes, |s| s as usize);
        if !current.is_empty() && current_bytes.saturating_add(weight) > max_group_bytes {
            plan.groups.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current.push(uid);
        current_bytes = current_bytes.saturating_add(weight);
    }
    if !current.is_empty() {
        plan.groups.push(current);
    }
    plan
}

/// Bodies for an explicit list of UIDs of the currently selected mailbox.
/// Callers pass one group at a time — the whole group is held in memory, so it
/// must have gone through [`plan_fetches`] first.
pub async fn fetch_uids(session: &mut ImapSession, uids: &[u32]) -> Result<Vec<RawMessage>> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    let set = uid_set(uids);
    match session {
        ImapSession::Tls(s)   => fetch_set_inner(s, &set).await,
        ImapSession::Plain(s) => fetch_set_inner(s, &set).await,
    }
}

fn uid_set(uids: &[u32]) -> String {
    uids.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",")
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
            let uid  = msg.uid?;
            let body = msg.body()?;
            // Second line of defence: RFC822.SIZE is only what the server
            // CLAIMS. One that under-reports, or reports nothing at all, must
            // not be able to push an unbounded body down to the parser.
            if body.len() > MAX_MESSAGE_BYTES {
                tracing::warn!(uid, size = body.len(), "Message IMAP hors limite de taille — ignoré");
                return None;
            }
            Some(RawMessage { uid, body: body.to_vec(), seen, flagged })
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

    fn sizes(pairs: &[(u32, u32)]) -> HashMap<u32, u32> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn ecarte_un_message_trop_volumineux_avant_de_le_telecharger() {
        let too_big = (MAX_MESSAGE_BYTES + 1) as u32;
        let plan = plan_fetches(&[1, 2, 3], &sizes(&[(1, 1_000), (2, too_big), (3, 2_000)]));
        assert_eq!(
            plan.oversized,
            vec![(2, too_big)],
            "le message hors limite doit être signalé et jamais fetché"
        );
        assert_eq!(
            plan.groups,
            vec![vec![1, 3]],
            "les autres UID doivent rester dans un seul groupe"
        );
    }

    #[test]
    fn decoupe_le_lot_selon_le_volume_cumule_annonce() {
        let plan = plan_fetches_with(
            &[1, 2, 3, 4],
            &sizes(&[(1, 40), (2, 40), (3, 40), (4, 40)]),
            100,
            100,
        );
        assert_eq!(
            plan.groups,
            vec![vec![1, 2], vec![3, 4]],
            "aucun groupe ne doit dépasser le budget mémoire du lot"
        );
        assert!(plan.oversized.is_empty(), "aucun message n'est hors limite ici");
    }

    #[test]
    fn budgete_au_maximum_un_uid_sans_taille_annoncee() {
        let plan = plan_fetches_with(&[1, 2], &HashMap::new(), 100, 100);
        assert_eq!(
            plan.groups,
            vec![vec![1], vec![2]],
            "un serveur muet sur RFC822.SIZE doit être traité au pire cas, un message à la fois"
        );
    }

    #[test]
    fn un_lot_entierement_hors_limite_ne_declenche_aucun_fetch() {
        let too_big = (MAX_MESSAGE_BYTES + 1) as u32;
        let plan = plan_fetches(&[7, 8], &sizes(&[(7, too_big), (8, too_big)]));
        assert!(plan.groups.is_empty(), "aucun FETCH ne doit être planifié");
        assert_eq!(plan.oversized.len(), 2, "les deux UID doivent être signalés");
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
