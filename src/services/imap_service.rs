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

pub struct ImapConfig {
    pub host:     String,
    pub port:     u16,
    pub security: String,
    pub username: String,
    pub password: String,
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
            let session = client
                .login(&cfg.username, &cfg.password)
                .await
                .map_err(|(e, _)| anyhow::anyhow!("Authentification IMAP: {e}"))?;
            Ok(ImapSession::Tls(session))
        }
        _ => {
            let tcp = TcpStream::connect(format!("{}:{}", cfg.host, cfg.port))
                .await
                .with_context(|| format!("Connexion TCP à {}:{}", cfg.host, cfg.port))?;
            let client = async_imap::Client::new(tcp);
            let session = client
                .login(&cfg.username, &cfg.password)
                .await
                .map_err(|(e, _)| anyhow::anyhow!("Authentification IMAP: {e}"))?;
            Ok(ImapSession::Plain(session))
        }
    }
}

/// Données brutes d'un message fetchsé
pub struct RawMessage {
    pub uid:  u32,
    pub body: Vec<u8>,
}

/// Récupère les messages d'un dossier depuis un UID donné
pub async fn fetch_recent(
    session: &mut ImapSession,
    folder: &str,
    max_count: u32,
    since_uid: Option<u32>,
) -> Result<Vec<RawMessage>> {
    match session {
        ImapSession::Tls(s)   => fetch_inner(s, folder, max_count, since_uid).await,
        ImapSession::Plain(s) => fetch_inner(s, folder, max_count, since_uid).await,
    }
}

async fn fetch_inner<T>(
    session: &mut Session<T>,
    folder: &str,
    max_count: u32,
    since_uid: Option<u32>,
) -> Result<Vec<RawMessage>>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let mailbox = session.select(folder).await.context("SELECT dossier IMAP")?;

    let mut result: Vec<RawMessage> = match since_uid {
        // Incrémental : uniquement les UID strictement supérieurs au dernier connu.
        Some(uid) => {
            let set = format!("{}:*", uid + 1);
            let messages = session
                .uid_fetch(&set, "(UID FLAGS BODY.PEEK[])")
                .await
                .context("UID FETCH")?;
            messages
                .try_collect::<Vec<_>>()
                .await
                .context("Collecte messages IMAP")?
                .into_iter()
                .filter_map(|msg| Some(RawMessage { uid: msg.uid?, body: msg.body()?.to_vec() }))
                .collect()
        }
        // Premier sync : se limiter aux `max_count` messages LES PLUS RÉCENTS, par
        // numéro de séquence. Sinon `uid_fetch("1:*", BODY.PEEK[])` rapatrie TOUT le
        // dossier (corps complets) → lent / mémoire / blocage du worker sur une
        // grosse boîte (cause du sync qui ne « descend » jamais les mails).
        None => {
            let exists = mailbox.exists;
            if exists == 0 {
                return Ok(Vec::new());
            }
            let start = exists.saturating_sub(max_count).saturating_add(1);
            let set = format!("{}:{}", start, exists);
            let messages = session
                .fetch(&set, "(UID FLAGS BODY.PEEK[])")
                .await
                .context("FETCH")?;
            messages
                .try_collect::<Vec<_>>()
                .await
                .context("Collecte messages IMAP")?
                .into_iter()
                .filter_map(|msg| Some(RawMessage { uid: msg.uid?, body: msg.body()?.to_vec() }))
                .collect()
        }
    };

    result.sort_by_key(|m| std::cmp::Reverse(m.uid));
    result.truncate(max_count as usize);
    Ok(result)
}

/// Logout propre
pub async fn logout(session: ImapSession) {
    match session {
        ImapSession::Tls(mut s)   => { let _ = s.logout().await; }
        ImapSession::Plain(mut s) => { let _ = s.logout().await; }
    }
}

/// Liste les dossiers IMAP disponibles
pub async fn list_folders(session: &mut ImapSession) -> Result<Vec<String>> {
    match session {
        ImapSession::Tls(s)   => list_folders_inner(s).await,
        ImapSession::Plain(s) => list_folders_inner(s).await,
    }
}

async fn list_folders_inner<T>(session: &mut Session<T>) -> Result<Vec<String>>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let names = session.list(None, Some("*")).await.context("LIST dossiers")?;
    let result: Vec<String> = names
        .try_collect::<Vec<_>>()
        .await
        .context("Collecte dossiers")?
        .into_iter()
        .map(|n| n.name().to_string())
        .collect();
    Ok(result)
}
