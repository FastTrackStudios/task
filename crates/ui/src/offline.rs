//! Offline, at the app root (`docs/spec/offline.md`): the device copy
//! behind every list, the durable queue for service calls made with no
//! connection, and the remembered account that lets the app open
//! without the server.
//!
//! The machinery is architect's (`LocalRows`, `use_persisted_store_list`,
//! `CallQueue`, `Mutation::run_queued`); this module is Task's glue:
//! which store opens ([`crate::device_store`]), which calls can queue and
//! how they are sent again ([`TimerCalls`]), and when to replay (on
//! mount, after a push, and whenever the org connection comes back).

use std::rc::Rc;

use architect::{CallQueue, CallTransport, Failure, QueuedCall, classify};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Call names the queue carries — the timer's service calls.
pub mod calls {
    pub const TIMER_START: &str = "timer.start";
    pub const TIMER_STOP: &str = "timer.stop";
    pub const TIMER_SWITCH: &str = "timer.switch";
    pub const TIMER_LOG: &str = "timer.log";
    pub const TIMER_UPDATE: &str = "timer.update";
    pub const TIMER_DELETE: &str = "timer.delete";
}

/// `timer.stop`'s payload.
#[derive(Serialize, Deserialize)]
pub struct StopPayload {
    pub user_id: uuid::Uuid,
}

/// `timer.delete`'s payload.
#[derive(Serialize, Deserialize)]
pub struct DeletePayload {
    pub id: uuid::Uuid,
}

fn decode<T: serde::de::DeserializeOwned>(call: &QueuedCall) -> Result<T, Failure> {
    serde_json::from_value(call.payload.clone())
        .map_err(|e| Failure::Refused(format!("{}: unreadable queued call: {e}", call.call)))
}

fn sent<T>(r: Result<T, String>) -> Result<T, Failure> {
    r.map_err(|e| classify(&e))
}

/// Replays queued timer calls through the same `feeds` calls a live
/// write makes (`offline.outbox`).
pub struct TimerCalls;

// t[impl offline.outbox]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl CallTransport for TimerCalls {
    async fn send(&self, call: &QueuedCall) -> Result<Option<String>, Failure> {
        let slug = call.route.as_str();
        match call.call.as_str() {
            calls::TIMER_START => {
                let req = decode(call)?;
                let s = sent(crate::feeds::start_timer(slug, req).await)?;
                Ok(Some(s.id.to_string()))
            }
            calls::TIMER_SWITCH => {
                let req = decode(call)?;
                let s = sent(crate::feeds::switch_timer(slug, req).await)?;
                Ok(Some(s.id.to_string()))
            }
            calls::TIMER_STOP => {
                let p: StopPayload = decode(call)?;
                sent(crate::feeds::stop_timer(slug, p.user_id).await)?;
                Ok(None)
            }
            calls::TIMER_LOG => {
                let req = decode(call)?;
                let s = sent(crate::feeds::log_session(slug, req).await)?;
                Ok(Some(s.id.to_string()))
            }
            calls::TIMER_UPDATE => {
                let req = decode(call)?;
                sent(crate::feeds::update_session(slug, req).await)?;
                Ok(None)
            }
            calls::TIMER_DELETE => {
                let p: DeletePayload = decode(call)?;
                sent(crate::feeds::delete_session(slug, p.id).await)?;
                Ok(None)
            }
            other => Err(Failure::Refused(format!("unknown queued call `{other}`"))),
        }
    }
}

/// Provide the device copy and the call queue at the app root, and
/// replay the queue whenever the org connection becomes ready. Call after
/// `use_app_supervised` (the connection, the notifications) and before
/// the stores' list hooks mount.
pub fn provide_offline(conn: architect::Connection<vox_core::Caller>) {
    let rows = use_hook(crate::device_store::device_rows);
    architect::provide_local_rows(rows.clone());
    let queue = architect::provide_call_queue(CallQueue::new(rows), Rc::new(TimerCalls));
    // t[impl offline.replay] — reconnect is the moment to send what waited.
    use_effect(move || {
        if conn.ready().is_some() {
            queue.kick();
        }
    });
}

/// A JSON payload for a queued call (serialisation of our own request
/// types cannot fail; `Null` keeps the call refusable rather than lost).
#[must_use]
pub fn payload<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

// ── the remembered account (offline.boot) ───────────────────────────

/// Who was signed in on this device, per server — enough to open the
/// workspace with no connection. The session token itself stays where it
/// always was (the token store); this holds only the principal.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RememberedAccount {
    pub user_id: uuid::Uuid,
    pub email: String,
    pub name: String,
}

fn remembered_key(base: &str, email: &str) -> String {
    let safe = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    };
    format!("task.offline.account.{}.{}", safe(base), safe(email))
}

/// Remember the signed-in principal for `base` (the server).
pub fn remember_account(base: &str, account: &RememberedAccount) {
    if let Ok(json) = serde_json::to_string(account) {
        crate::local_store::set(&remembered_key(base, &account.email), &json);
    }
}

/// The principal last signed in as `email` against `base`, if any.
#[must_use]
pub fn remembered_account(base: &str, email: &str) -> Option<RememberedAccount> {
    crate::local_store::get(&remembered_key(base, email))
        .and_then(|j| serde_json::from_str(&j).ok())
}

/// Forget it (sign-out).
pub fn forget_account(base: &str, email: &str) {
    crate::local_store::remove(&remembered_key(base, email));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembered_accounts_are_per_server_and_per_person() {
        assert_ne!(
            remembered_key("https://tasks.example", "a@x.io"),
            remembered_key("http://127.0.0.1:18080", "a@x.io")
        );
        assert_ne!(
            remembered_key("https://tasks.example", "a@x.io"),
            remembered_key("https://tasks.example", "b@x.io")
        );
    }
}
