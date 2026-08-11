//! Server-side TLS for the offered SMTP/IMAP/POP3 services.
//!
//! Two things live here: a stream type that can be plaintext OR TLS and can be
//! upgraded in place (for STARTTLS/STLS), and the construction of the rustls
//! acceptor from the administrator's certificate.
//!
//! rustls with the ring provider, aligned with the core: TLS 1.2 + 1.3 only,
//! forward secrecy and AEAD enforced by the library — there are no weak
//! ciphersuites to accidentally re-enable, unlike an OpenSSL setup.

use std::{io, path::Path, pin::Pin, sync::Arc, task::{Context, Poll}};

use tokio::{
    io::{AsyncRead, AsyncWrite, BufReader, ReadBuf},
    net::TcpStream,
};
use tokio_rustls::{rustls, server::TlsStream, TlsAcceptor};

use super::config::TlsFloor;

/// How a listener handles TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// Plaintext only — no encryption offered (LAN / behind a TLS terminator).
    None,
    /// Plaintext, upgradable on client request (STARTTLS on 25/587/143, STLS on 110).
    StartTls,
    /// TLS from the first byte (SMTPS 465, IMAPS 993, POP3S 995).
    Implicit,
}

/// A connection that may be plaintext or TLS, and can move from the first to
/// the second without being torn down — that is what STARTTLS needs.
///
/// It implements `AsyncRead`/`AsyncWrite` by delegating to whichever it is, so
/// the protocol handlers read and write through it without caring which.
pub enum MailStream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

/// How long a TLS handshake may take. Generous for a slow link, far short of
/// leaving a stalled peer parked on a task.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl MailStream {
    /// True once the connection is encrypted — used to refuse cleartext auth and
    /// to reject a second STARTTLS.
    pub fn is_tls(&self) -> bool {
        matches!(self, MailStream::Tls(_))
    }

    /// Performs the server-side TLS handshake, consuming the plaintext stream and
    /// returning the encrypted one. On a stream that is already TLS it is an
    /// error to call this (a double STARTTLS), so the caller must guard with
    /// `is_tls()` first.
    pub async fn into_tls(self, acceptor: &TlsAcceptor) -> io::Result<MailStream> {
        match self {
            MailStream::Plain(tcp) => {
                // Bounded: a peer that opens a connection and then never sends
                // its ClientHello would otherwise hold a task and a socket
                // forever — the session's own idle timeout does not apply yet,
                // because the session does not exist until this returns. A few
                // hundred such connections are a free denial of service.
                let tls = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp))
                    .await
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::TimedOut, "poignée de main TLS expirée")
                    })??;
                Ok(MailStream::Tls(Box::new(tls)))
            }
            MailStream::Tls(_) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "flux déjà en TLS",
            )),
        }
    }
}

impl AsyncRead for MailStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MailStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MailStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MailStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MailStream::Plain(s) => Pin::new(s).poll_flush(cx),
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MailStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Why a STARTTLS/STLS upgrade was refused.
#[derive(Debug)]
pub enum UpgradeError {
    /// The client sent bytes after the STARTTLS command, before the handshake —
    /// they travelled in the clear. Honouring them inside the TLS session is the
    /// STARTTLS command-injection attack (RFC 3207, CVE-2011-0411), so the
    /// connection must be dropped, not upgraded.
    Pipelined,
    /// The TLS handshake itself failed. The stream state is now indeterminate;
    /// the caller must disconnect rather than fall back to plaintext.
    Handshake(io::Error),
}

/// Upgrades a plaintext reader to TLS, enforcing the anti-injection rule.
///
/// The caller MUST have already written and flushed its "ready to start TLS"
/// reply. This function then verifies that nothing the client pipelined is still
/// buffered (the injection defense), performs the handshake, and hands back a
/// fresh reader over the encrypted stream. On any error the caller disconnects.
/// After it returns, the caller MUST reset all session state gathered in the
/// clear (HELO/MAIL/RCPT/auth) and require the client to start again.
pub async fn upgrade(
    reader: BufReader<MailStream>,
    acceptor: &TlsAcceptor,
) -> Result<BufReader<MailStream>, UpgradeError> {
    // Any plaintext still buffered was sent before the handshake — it must never
    // be interpreted as a command inside the tunnel. Postfix's `vstream_fpurge`
    // discards it; we refuse the whole connection, which is stricter.
    if !reader.buffer().is_empty() {
        return Err(UpgradeError::Pipelined);
    }
    let inner = reader.into_inner();
    let tls = inner.into_tls(acceptor).await.map_err(UpgradeError::Handshake)?;
    Ok(BufReader::new(tls))
}

/// The TLS versions a floor allows, highest first — the order rustls negotiates
/// in. TLS 1.0/1.1 are not in the library at all, so the only real choice is
/// whether 1.2 is still accepted.
///
/// Raising the floor to TLS 1.3 is NOT free on the reception port (25). An MTA
/// that only speaks TLS 1.2 does not "fail over" to another version: its
/// STARTTLS handshake aborts, and every sane sender then retries the delivery in
/// the clear rather than dropping the mail. So on port 25 a 1.3-only floor
/// converts opportunistic encryption into plaintext — a net loss. It is a sound
/// setting only on the ports whose clients we control or whose failure mode is
/// "cannot connect": submission (587), SMTPS (465), IMAPS (993), POP3S (995).
fn protocol_versions(floor: TlsFloor) -> &'static [&'static rustls::SupportedProtocolVersion] {
    static FROM_TLS12: &[&rustls::SupportedProtocolVersion] =
        &[&rustls::version::TLS13, &rustls::version::TLS12];
    static FROM_TLS13: &[&rustls::SupportedProtocolVersion] = &[&rustls::version::TLS13];

    match floor {
        TlsFloor::Tls12 => FROM_TLS12,
        TlsFloor::Tls13 => FROM_TLS13,
    }
}

/// Human-readable name of the floor, for the startup log.
fn floor_label(floor: TlsFloor) -> &'static str {
    match floor {
        TlsFloor::Tls12 => "TLS 1.2",
        TlsFloor::Tls13 => "TLS 1.3",
    }
}

/// Builds the acceptor from the operator's certificate chain and private key
/// (PEM), refusing to negotiate anything below `floor`. Returns `Ok(None)` when
/// no certificate is configured, which simply means the TLS listeners will not
/// start — never a hard failure, so a LAN instance runs fine without a
/// certificate.
pub fn build_acceptor(
    cert_path: &str,
    key_path: &str,
    floor: TlsFloor,
) -> io::Result<Option<TlsAcceptor>> {
    if cert_path.trim().is_empty() || key_path.trim().is_empty() {
        return Ok(None);
    }
    // rustls 0.23 needs a process-wide crypto provider installed before any
    // handshake. Installing twice is harmless (ignored), so we do it lazily here.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let certs = load_certs(cert_path.as_ref())?;
    if certs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("aucun certificat dans {cert_path}"),
        ));
    }
    let key = load_key(key_path.as_ref())?;

    let config = rustls::ServerConfig::builder_with_protocol_versions(protocol_versions(floor))
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("certificat/clé TLS: {e}")))?;

    // Logged at every (re)build so the version actually in force is visible in
    // the journal, not only in the console where it was typed.
    tracing::info!(
        plancher = floor_label(floor),
        "TLS serveur : version minimale négociable appliquée"
    );

    Ok(Some(TlsAcceptor::from(Arc::new(config))))
}

fn load_certs(path: &Path) -> io::Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let data = std::fs::read(path)
        .map_err(|e| io::Error::new(e.kind(), format!("lecture du certificat {}: {e}", path.display())))?;
    rustls_pemfile::certs(&mut data.as_slice()).collect()
}

fn load_key(path: &Path) -> io::Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let data = std::fs::read(path)
        .map_err(|e| io::Error::new(e.kind(), format!("lecture de la clé {}: {e}", path.display())))?;
    rustls_pemfile::private_key(&mut data.as_slice())?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("aucune clé privée dans {}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_rustls::rustls::ProtocolVersion;

    fn versions(floor: TlsFloor) -> Vec<ProtocolVersion> {
        protocol_versions(floor).iter().map(|v| v.version).collect()
    }

    #[test]
    fn the_floor_decides_which_versions_are_offered() {
        // Highest first: that is the order rustls walks when negotiating.
        assert_eq!(
            versions(TlsFloor::Tls12),
            vec![ProtocolVersion::TLSv1_3, ProtocolVersion::TLSv1_2]
        );
        assert_eq!(versions(TlsFloor::Tls13), vec![ProtocolVersion::TLSv1_3]);
    }

    /// The floor never removes TLS 1.3 — an operator can only ever restrict the
    /// set downwards from 1.2, never end up with no modern version at all.
    #[test]
    fn tls13_is_always_available() {
        for floor in [TlsFloor::Tls12, TlsFloor::Tls13] {
            assert!(versions(floor).contains(&ProtocolVersion::TLSv1_3));
        }
    }

    /// An absent certificate is a configuration state, not an error: the TLS
    /// listeners simply do not start.
    #[test]
    fn no_certificate_means_no_acceptor() {
        let acceptor = build_acceptor("", "  ", TlsFloor::Tls12).expect("pas une erreur");
        assert!(acceptor.is_none());
    }
}
