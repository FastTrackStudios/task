//! Helpers for cross-cutting integration tests against a live
//! `mailpit` container. Library shape (not bins) so individual
//! tests under `tests/` can compose the helpers freely.
//!
//! mailpit is a single Go binary that speaks SMTP (port 1025
//! by default), IMAP (port 1143), and a JSON HTTP API
//! (port 8025) — one container exercises every wire shape we
//! care about.
//!
//! Everything in this crate is gated by `#[cfg(feature =
//! "integration")]` so the dependency graph stays light when
//! the suite is disabled. Run with:
//! ```text
//! cargo test -p email-integration-tests --features integration -- --ignored
//! ```

#![cfg(feature = "integration")]

use std::time::Duration;

use email_config::{AccountConfig, BackendKind, FolderAliases, SmtpConfig, TlsMode};
use email_proto::AccountId;
use email_secret::Secret;
use testcontainers::core::{ContainerPort, IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// One running mailpit. Bound ports for SMTP / IMAP / HTTP API
/// are exposed so tests can build configs against the host
/// 127.0.0.1 + the mapped port testcontainers picked.
pub struct Mailpit {
    pub smtp_port: u16,
    pub imap_port: u16,
    pub api_port: u16,
    pub container: ContainerAsync<GenericImage>,
}

impl Mailpit {
    /// Spawn a `mailpit` container with default auth
    /// (username `test`, password `test`). Waits for the API
    /// port to be ready before returning.
    pub async fn spawn() -> Self {
        // Mailpit's default tag tracks the latest stable
        // release. `axllent/mailpit` is the canonical image
        // (Docker Hub). All three ports listen on 0.0.0.0 by
        // default in the official image.
        let image = GenericImage::new("axllent/mailpit", "latest")
            .with_exposed_port(ContainerPort::Tcp(1025))
            .with_exposed_port(ContainerPort::Tcp(1143))
            .with_exposed_port(ContainerPort::Tcp(8025))
            .with_wait_for(WaitFor::message_on_stdout("[http] starting on"))
            .with_env_var("MP_SMTP_AUTH_ACCEPT_ANY", "true")
            .with_env_var("MP_SMTP_AUTH_ALLOW_INSECURE", "true");

        let container = image.start().await.expect("start mailpit");

        let smtp_port = container
            .get_host_port_ipv4(1025.tcp())
            .await
            .expect("smtp port");
        let imap_port = container
            .get_host_port_ipv4(1143.tcp())
            .await
            .expect("imap port");
        let api_port = container
            .get_host_port_ipv4(8025.tcp())
            .await
            .expect("api port");

        // Give the IMAP server an extra beat to bind — mailpit
        // logs HTTP ready before IMAP in some builds.
        tokio::time::sleep(Duration::from_millis(500)).await;

        Self {
            smtp_port,
            imap_port,
            api_port,
            container,
        }
    }

    /// Build an `AccountConfig` whose IMAP backend points at
    /// the running container. SMTP submission piggy-backs via
    /// the `submit` field on the IMAP variant.
    pub fn account_config(&self) -> AccountConfig {
        AccountConfig {
            id: AccountId("mailpit".into()),
            name: "mailpit".into(),
            address: "test@example.com".into(),
            display_name: Some("Test".into()),
            backend: BackendKind::Imap {
                host: "127.0.0.1".into(),
                port: self.imap_port,
                tls: TlsMode::None,
                username: "test".into(),
                password: Secret::raw("test"),
                submit: Some(SmtpConfig {
                    host: "127.0.0.1".into(),
                    port: self.smtp_port,
                    tls: TlsMode::None,
                    username: "test".into(),
                    password: Secret::raw("test"),
                }),
            },
            signature: None,
            folder_aliases: FolderAliases::new(),
        }
    }

    /// Convenience: HTTP API URL prefix
    /// (`http://127.0.0.1:<port>`).
    pub fn api_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.api_port)
    }

    /// GET `<api>/api/v1/messages` and return the parsed count
    /// — easiest read-side assertion after we send.
    pub async fn message_count(&self) -> usize {
        #[derive(serde::Deserialize)]
        struct Listing {
            #[serde(default)]
            messages: Vec<serde_json::Value>,
        }
        let resp = reqwest::get(format!("{}/api/v1/messages", self.api_url()))
            .await
            .expect("api request");
        let body: Listing = resp.json().await.expect("api json");
        body.messages.len()
    }
}
