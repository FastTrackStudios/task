//! Connection bootstrap. TLS-on-connect, STARTTLS, and plain
//! TCP variants are kept behind one entry point so the rest of
//! the backend doesn't branch on `TlsMode`.

use async_imap::{Client, Session};
use email_config::TlsMode;
use email_secret::SecretValue;
use thiserror::Error;
use tokio::net::TcpStream;

/// The stream type the session wraps. In production we always
/// use a TLS-on-TCP socket. The `test-plaintext` feature swaps
/// in raw `TcpStream` so an in-process mock IMAP server on
/// `127.0.0.1` can drive the backend end-to-end without needing
/// to ship a fake cert.
#[cfg(not(feature = "test-plaintext"))]
pub type ImapStream = async_native_tls::TlsStream<TcpStream>;
#[cfg(feature = "test-plaintext")]
pub type ImapStream = TcpStream;

/// Live, authenticated IMAP session. Stream type flips between
/// TLS + plaintext via the `test-plaintext` feature.
pub type ImapSession = Session<ImapStream>;

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("tcp connect: {0}")]
    Tcp(String),
    #[error("tls handshake: {0}")]
    Tls(String),
    /// Failure reading / parsing the server greeting during the
    /// STARTTLS handshake. Only constructed on the production
    /// (non-`test-plaintext`) path, so the allow keeps the
    /// test-feature build quiet.
    #[allow(dead_code)]
    #[error("imap greeting: {0}")]
    Greeting(String),
    #[error("login: {0}")]
    Login(String),
    /// No longer hit on the production STARTTLS path (it's
    /// implemented), but retained so `map_connect_err` and any
    /// future plaintext-only backend can still surface it.
    #[allow(dead_code)]
    #[error("starttls is not supported by this backend")]
    StarttlsUnsupported,
    /// Same — reserved for the upcoming plaintext-refusal
    /// path when STARTTLS isn't supported.
    #[allow(dead_code)]
    #[error("plaintext IMAP is refused (tests/loopback only)")]
    PlaintextRefused,
}

pub async fn connect_and_login(
    host: &str,
    port: u16,
    tls: TlsMode,
    username: &str,
    password: &SecretValue,
) -> Result<ImapSession, ConnectError> {
    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|e| ConnectError::Tcp(e.to_string()))?;

    let client = build_client(host, tcp, tls).await?;
    let session = client
        .login(username, password.as_str())
        .await
        .map_err(|(e, _)| ConnectError::Login(e.to_string()))?;
    Ok(session)
}

/// Speak the IMAP `STARTTLS` handshake over a plaintext socket
/// and hand back the now-encrypted stream.
///
/// The dance: wrap the raw socket in a throwaway `Client`, drain
/// the unauthenticated server greeting, issue `STARTTLS`, reclaim
/// the socket with [`Client::into_inner`], then run the TLS
/// handshake over it with the same `async-native-tls` connector
/// the implicit-TLS path uses. The caller wraps the result in a
/// fresh `Client` and proceeds to `LOGIN`.
#[cfg(not(feature = "test-plaintext"))]
async fn starttls_upgrade(host: &str, tcp: TcpStream) -> Result<ImapStream, ConnectError> {
    let mut plain = Client::new(tcp);
    // The greeting is the first untagged line on the connection;
    // it must be consumed before any command is sent.
    plain
        .read_response()
        .await
        .map_err(|e| ConnectError::Greeting(e.to_string()))?
        .ok_or_else(|| ConnectError::Greeting("connection closed before greeting".into()))?;
    // `None` for the unsolicited-response sink: there are no
    // mailbox notifications to route on a pre-auth STARTTLS
    // exchange. This is the same shape async-imap's own `login`
    // uses internally.
    plain
        .run_command_and_check_ok("STARTTLS", None)
        .await
        .map_err(|e| ConnectError::Tls(format!("STARTTLS command: {e}")))?;
    let tcp = plain.into_inner();
    let connector = async_native_tls::TlsConnector::new();
    connector
        .connect(host, tcp)
        .await
        .map_err(|e| ConnectError::Tls(e.to_string()))
}

#[cfg(not(feature = "test-plaintext"))]
async fn build_client(
    host: &str,
    tcp: TcpStream,
    tls: TlsMode,
) -> Result<Client<ImapStream>, ConnectError> {
    let stream = match tls {
        TlsMode::Implicit => {
            let connector = async_native_tls::TlsConnector::new();
            connector
                .connect(host, tcp)
                .await
                .map_err(|e| ConnectError::Tls(e.to_string()))?
        }
        TlsMode::Starttls => starttls_upgrade(host, tcp).await?,
        TlsMode::None => return Err(ConnectError::PlaintextRefused),
    };
    Ok(Client::new(stream))
}

#[cfg(feature = "test-plaintext")]
async fn build_client(
    _host: &str,
    tcp: TcpStream,
    tls: TlsMode,
) -> Result<Client<ImapStream>, ConnectError> {
    // Test build: accept TlsMode::None on plain TCP. Other
    // modes still fail — tests shouldn't accidentally hit a
    // real TLS endpoint with this feature on.
    match tls {
        TlsMode::None => Ok(Client::new(tcp)),
        TlsMode::Implicit => Err(ConnectError::Tls(
            "test-plaintext build: implicit TLS not available".into(),
        )),
        TlsMode::Starttls => Err(ConnectError::Tls(
            "test-plaintext build: STARTTLS not available".into(),
        )),
    }
}
