//! SMTP transport. Wraps `mail-send` so the rest of the
//! codebase stays library-agnostic — if we ever swap submission
//! providers (Postmark API, etc), only this module changes.

use email_config::{SmtpConfig, TlsMode};
use email_proto::{Addr, Draft};
use email_secret::SecretError;
use mail_send::{SmtpClientBuilder, mail_builder::MessageBuilder};
use thiserror::Error;

use crate::build::{BuildError, build_message};

#[derive(Debug, Error)]
pub enum SendError {
    #[error("build: {0}")]
    Build(#[from] BuildError),
    #[error("secret: {0}")]
    Secret(#[from] SecretError),
    #[error("connect: {0}")]
    Connect(String),
    #[error("send: {0}")]
    Send(String),
    /// No longer produced on the send path (STARTTLS is
    /// implemented via mail-send's `connect()` upgrade); kept for
    /// API stability and any future plaintext-only transport.
    #[allow(dead_code)]
    #[error("starttls is not supported by this transport")]
    StarttlsUnsupported,
    #[error("plaintext SMTP is refused (tests/loopback only)")]
    PlaintextRefused,
}

/// One configured SMTP submission endpoint.
#[derive(Clone)]
pub struct SmtpSender {
    config: SmtpConfig,
}

impl SmtpSender {
    #[must_use]
    pub fn new(config: SmtpConfig) -> Self {
        // rustls needs a process-level CryptoProvider, and a workspace
        // build has both `ring` and `aws-lc-rs` enabled, so it cannot
        // choose one itself. Install ring once; `Err` means somebody
        // already did, which is fine.
        static CRYPTO: std::sync::Once = std::sync::Once::new();
        CRYPTO.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
        Self { config }
    }

    /// Build the message + submit it. Returns the
    /// freshly-assigned Message-ID so callers can record it
    /// against the sent record.
    pub async fn send(&self, draft: &Draft) -> Result<String, SendError> {
        let (bytes, message_id) = build_message(draft)?;
        let recipients = self.collect_recipients(draft);
        self.send_raw(&draft.from.email, &recipients, &bytes, message_id)
            .await
    }

    /// Submit pre-built raw bytes. Backends that have already
    /// produced the RFC5322 representation (eg
    /// `email-imap::append_draft` writing a sent copy) use this
    /// directly; [`Self::send`] is just `build_message` + this.
    pub async fn send_raw(
        &self,
        from: &str,
        recipients: &[String],
        raw: &[u8],
        message_id: String,
    ) -> Result<String, SendError> {
        let password = self.config.password.resolve().await?;
        let creds: (String, String) = (self.config.username.clone(), password.as_str().to_string());

        // Pick a builder configured for the requested TLS mode.
        // The plaintext path is gated behind the `test-plaintext`
        // feature; production builds refuse it.
        let builder = match self.config.tls {
            TlsMode::Implicit => SmtpClientBuilder::new(self.config.host.clone(), self.config.port)
                .implicit_tls(true)
                .credentials(creds),
            // STARTTLS: connect in clear text, then upgrade. With
            // `implicit_tls(false)` mail-send's `connect()` reads
            // the greeting, sends EHLO, and issues `STARTTLS` when
            // the server advertises it — yielding the same
            // `SmtpClient<TlsStream<..>>` as the implicit path
            // (and erroring with `MissingStartTls` if the server
            // doesn't offer it).
            TlsMode::Starttls => SmtpClientBuilder::new(self.config.host.clone(), self.config.port)
                .implicit_tls(false)
                .credentials(creds),
            TlsMode::None => {
                #[cfg(feature = "test-plaintext")]
                {
                    SmtpClientBuilder::new(self.config.host.clone(), self.config.port)
                        .implicit_tls(false)
                        .credentials(creds)
                }
                #[cfg(not(feature = "test-plaintext"))]
                {
                    let _ = creds;
                    return Err(SendError::PlaintextRefused);
                }
            }
        };

        // `mail-send` builds the envelope from the
        // MessageBuilder fields, then uses our raw bytes as the
        // body. body_raw is the path that lets us emit the
        // message we already encoded with mail-builder upstream
        // — keeps the wire format we wrote in `build_message`
        // intact.
        let message = MessageBuilder::new()
            .from(from.to_string())
            .to(recipients.to_vec());
        let raw_payload = String::from_utf8_lossy(raw).into_owned();
        let message = message.text_body(raw_payload);

        // The connect path differs between TLS and plaintext: TLS
        // returns `SmtpClient<TlsStream<...>>`, plaintext returns
        // `SmtpClient<TcpStream>`. Branch + drive each
        // separately so the type isn't shared across the
        // boundary.
        match self.config.tls {
            TlsMode::None if cfg!(feature = "test-plaintext") => {
                let mut client = builder
                    .connect_plain()
                    .await
                    .map_err(|e| SendError::Connect(e.to_string()))?;
                client
                    .send(message)
                    .await
                    .map_err(|e| SendError::Send(e.to_string()))?;
            }
            // Both implicit TLS and STARTTLS resolve to a
            // TLS-wrapped client via `connect()`; the builder
            // above already encoded which handshake to run.
            TlsMode::Implicit | TlsMode::Starttls => {
                let mut client = builder
                    .connect()
                    .await
                    .map_err(|e| SendError::Connect(e.to_string()))?;
                client
                    .send(message)
                    .await
                    .map_err(|e| SendError::Send(e.to_string()))?;
            }
            // Plaintext without the test feature: refused above
            // when building the builder, so this only fires in
            // production builds where `None` is rejected.
            TlsMode::None => return Err(SendError::PlaintextRefused),
        }

        Ok(message_id)
    }

    #[allow(clippy::unused_self)]
    fn collect_recipients(&self, draft: &Draft) -> Vec<String> {
        let mut out = Vec::with_capacity(draft.to.len() + draft.cc.len() + draft.bcc.len());
        for a in draft.to.iter().chain(&draft.cc).chain(&draft.bcc) {
            out.push(a.email.clone());
        }
        out
    }
}

#[allow(dead_code)]
fn _addr_email(a: &Addr) -> &str {
    &a.email
}
