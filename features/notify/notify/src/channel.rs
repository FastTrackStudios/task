//! Delivery channels — how a materialized notification reaches the
//! user.
//!
//! The notifier mints one [`Notification`] per rule hit
//! ([`Store::mint`]) and hands it to every configured
//! [`DeliveryChannel`]. Two ship today:
//!
//! - [`InApp`] — persists into the org's [`Store`] and publishes on
//!   the `#[subscribe]` stream. The store + stream **are** the in-app
//!   channel; the bell UI folds the stream.
//! - [`Webhook`] — ntfy-style JSON POST to `TASK_NOTIFY_WEBHOOK`.
//!
//! Both are fire-and-forget: `deliver` spawns and returns; failures
//! are logged, never propagated — a dead webhook must not block (or
//! fail) the event pump. APNs / email later = one more impl.

use notify_proto::Notification;

use crate::store::Store;

/// One way to get a notification in front of the user. Send + Sync so
/// the notifier can hold a `Vec<Arc<dyn DeliveryChannel>>` across its
/// spawned pumps.
pub trait DeliveryChannel: Send + Sync {
    /// Deliver `n` for org `org`. MUST NOT block: spawn any I/O and
    /// return; log failures instead of surfacing them.
    fn deliver(&self, org: &str, n: &Notification);
}

/// The in-app channel: persist + stream. Wraps the org's [`Store`];
/// `deliver` inserts the already-minted row as-is ([`Store::insert`]),
/// so every channel reports the same `id` / `created_at`.
pub struct InApp {
    store: Store,
}

impl InApp {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl DeliveryChannel for InApp {
    fn deliver(&self, org: &str, n: &Notification) {
        let store = self.store.clone();
        let org = org.to_owned();
        let row = n.clone();
        tokio::spawn(async move {
            if let Err(e) = store.insert(row).await {
                tracing::warn!(org = %org, error = ?e, "in-app notification store failed");
            }
        });
    }
}

/// Env var naming the webhook endpoint. Unset/empty = channel off.
pub const WEBHOOK_ENV: &str = "TASK_NOTIFY_WEBHOOK";

/// How long a webhook POST may take before it is abandoned.
const WEBHOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// The push channel: one JSON POST per notification, ntfy-style
/// fields (`title`, `message`, `click`) plus Task context (`kind`,
/// `org`, `actor`, `created_at`). Point [`WEBHOOK_ENV`] at an ntfy
/// topic proxy, a Shortcuts webhook, or anything that accepts JSON.
pub struct Webhook {
    url: String,
    client: reqwest::Client,
}

impl Webhook {
    #[must_use]
    pub fn new(url: String) -> Self {
        Self {
            url,
            client: reqwest::Client::new(),
        }
    }

    /// Build from [`WEBHOOK_ENV`]; `None` when unset or empty.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        std::env::var(WEBHOOK_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(Self::new)
    }

    /// The POSTed body — public so tests (and future channels) can
    /// assert the shape without an HTTP server.
    #[must_use]
    pub fn payload(org: &str, n: &Notification) -> serde_json::Value {
        serde_json::json!({
            "title": n.title,
            "message": n.body,
            "click": n.source.href,
            "kind": n.kind.as_str(),
            "org": org,
            "actor": n.actor,
            "created_at": n.created_at.to_rfc3339(),
        })
    }
}

impl DeliveryChannel for Webhook {
    fn deliver(&self, org: &str, n: &Notification) {
        let url = self.url.clone();
        let client = self.client.clone();
        let body = Self::payload(org, n);
        let org = org.to_owned();
        tokio::spawn(async move {
            let sent = tokio::time::timeout(
                WEBHOOK_TIMEOUT,
                client.post(&url).json(&body).send(),
            )
            .await;
            match sent {
                Ok(Ok(resp)) if resp.status().is_success() => {}
                Ok(Ok(resp)) => {
                    tracing::warn!(org = %org, status = %resp.status(), "notify webhook rejected");
                }
                Ok(Err(e)) => {
                    tracing::warn!(org = %org, error = %e, "notify webhook failed");
                }
                Err(_) => {
                    tracing::warn!(org = %org, timeout = ?WEBHOOK_TIMEOUT, "notify webhook timed out");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify_proto::{NotifyKind, NotifySource};

    #[test]
    fn webhook_payload_shape() {
        let n = Notification {
            id: uuid::Uuid::nil(),
            kind: NotifyKind::BookingCreated,
            title: "New booking: Alice".into(),
            body: "Aug 1, 09:00".into(),
            source: NotifySource {
                service: "scheduling".into(),
                entity: "b1".into(),
                href: "/bookings".into(),
            },
            actor: "Alice".into(),
            created_at: chrono::Utc::now(),
            read_at: None,
        };
        let p = Webhook::payload("alpha", &n);
        assert_eq!(p["title"], "New booking: Alice");
        assert_eq!(p["message"], "Aug 1, 09:00");
        assert_eq!(p["click"], "/bookings");
        assert_eq!(p["kind"], "booking-created");
        assert_eq!(p["org"], "alpha");
    }
}
