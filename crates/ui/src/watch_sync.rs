//! Watch config publisher — the phone side of the iOS → Apple Watch
//! config inheritance (`apps/task/mobile/ios/watch-config-bridge.md`).
//!
//! The Apple Watch has no independent way to know which Task server +
//! account to talk to; it inherits the paired iPhone's active config
//! (`{baseURL, orgSlug, token}`) over WatchConnectivity. The reactive
//! state that config is derived from — the multi-server registry, the
//! org selection, and the signed-in account — all lives in THIS crate,
//! so the observer lives here too: [`use_watch_config_publisher`] is a
//! root-level effect that recomputes the config on every change
//! (including boot restore) and hands it to a platform *sink*.
//!
//! The sink is the platform seam: this module is pure Rust and compiles
//! on every target; only the iOS app shell registers a real sink (a
//! `WCSession.updateApplicationContext` pusher, see
//! `apps/task/mobile/src/watch_sync.rs`). With no sink registered the
//! publisher is a cheap no-op, so desktop/web/wasm builds carry no
//! platform code and no extra work.

use std::sync::{Mutex, RwLock};

use dioxus::prelude::*;

use crate::orgs::{OrgMeta, OrgSelection, active_slug};

/// The wire contract of the watch config bridge. Field values map 1:1
/// onto the `updateApplicationContext` keys `baseURL` / `orgSlug` /
/// `token` (see `PhoneSync.swift` on the watch side — keys MUST match).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchConfig {
    /// HTTP(S) base of the active server, e.g.
    /// `https://task.starcommand.live` — the watch appends
    /// `org/{slug}/watch/v1/…`.
    pub base_url: String,
    /// The org slug the app is currently scoped to.
    pub org_slug: String,
    /// The phone's active session token (what the watch presents as its
    /// bearer; the server's watch bridge validates it via
    /// `current_session`).
    pub token: String,
}

type Sink = Box<dyn Fn(&WatchConfig) + Send + Sync>;

/// The registered platform sink, if any.
static SINK: RwLock<Option<Sink>> = RwLock::new(None);
/// Last complete config published — dedups repeats and replays to a
/// sink that registers after the first publish (init order freedom).
static LAST: Mutex<Option<WatchConfig>> = Mutex::new(None);

/// Register the platform pusher. Called once at app boot by the shell
/// that can actually reach a watch (the iOS app). If a config was
/// already published before registration, it is replayed immediately.
pub fn set_watch_config_sink(sink: impl Fn(&WatchConfig) + Send + Sync + 'static) {
    let replay = LAST.lock().ok().and_then(|l| l.clone());
    if let Some(cfg) = replay.as_ref() {
        sink(cfg);
    }
    if let Ok(mut w) = SINK.write() {
        *w = Some(Box::new(sink));
    }
}

/// Publish a config to the sink. Incomplete configs (any empty field)
/// are dropped — the watch only ever receives whole, usable configs, so
/// a transient state during boot/sign-out never wipes a working setup
/// (matching `PhoneSync.swift`'s non-empty-only application). Identical
/// repeats are deduped (applicationContext is latest-value-wins, but
/// there is no reason to spam it).
fn publish(cfg: WatchConfig) {
    if cfg.base_url.is_empty() || cfg.org_slug.is_empty() || cfg.token.is_empty() {
        return;
    }
    if let Ok(mut last) = LAST.lock() {
        if last.as_ref() == Some(&cfg) {
            return;
        }
        *last = Some(cfg.clone());
    }
    if let Ok(sink) = SINK.read() {
        if let Some(sink) = sink.as_ref() {
            sink(&cfg);
        }
    }
}

/// Derive the HTTP(S) base the watch dials from a vox base URL as
/// stored in the server registry (`ws(s)://host[:port][/vox]`).
#[must_use]
pub fn http_base_from_vox_url(url: &str) -> String {
    let t = url.trim().trim_end_matches('/');
    let t = t.strip_suffix("/vox").unwrap_or(t);
    if let Some(rest) = t.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = t.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        // Already http(s) (or something exotic) — pass through.
        t.to_owned()
    }
}

/// Root-level effect: recompute `{baseURL, orgSlug, token}` whenever the
/// active server, org selection/discovery, or signed-in account changes
/// — including the boot restore path (registry loads from disk, auth
/// validates the persisted session) — and publish it to the platform
/// sink. Mount once in [`crate::app::App`], after `provide_auth`.
pub fn use_watch_config_publisher() {
    let registry = use_context::<crate::server_registry::ServerRegistry>();
    let org_selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();

    use_effect(move || {
        // Reads subscribe: registry entries + active id, selection,
        // discovery list, and the active account signal.
        let entry = registry.active_entry();
        let org_slug = active_slug(&org_selection.read(), &org_list.read());
        // Token: the live signed-in session wins; fall back to the
        // token persisted on the registry entry (boot, before auth
        // resolves). Guest mirrors `sync_active_server_entry` — an
        // anonymous session is not an identity the watch should inherit.
        let live = account
            .read()
            .as_ref()
            .filter(|a| a.email != crate::auth::GUEST_EMAIL)
            .map(|a| a.token.clone());
        let token = live
            .filter(|t| !t.is_empty())
            .or_else(|| entry.as_ref().and_then(|e| e.session_token.clone()))
            .unwrap_or_default();
        let base_url = entry
            .map(|e| http_base_from_vox_url(&e.server_url))
            .unwrap_or_default();
        publish(WatchConfig {
            base_url,
            org_slug,
            token,
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_base_maps_ws_schemes_and_strips_vox() {
        assert_eq!(
            http_base_from_vox_url("wss://task.starcommand.live/vox"),
            "https://task.starcommand.live"
        );
        assert_eq!(
            http_base_from_vox_url("ws://127.0.0.1:18080/vox"),
            "http://127.0.0.1:18080"
        );
        assert_eq!(
            http_base_from_vox_url("wss://task.starcommand.live"),
            "https://task.starcommand.live"
        );
        // Trailing slash + already-http pass-through.
        assert_eq!(
            http_base_from_vox_url("https://task.starcommand.live/"),
            "https://task.starcommand.live"
        );
    }

    #[test]
    fn publish_drops_incomplete_and_dedups() {
        // Note: SINK/LAST are process-global; this test is the only
        // writer in the suite (the publisher hook needs a Dioxus
        // runtime, so nothing else touches them under `cargo test`).
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        set_watch_config_sink(|_| {
            CALLS.fetch_add(1, Ordering::SeqCst);
        });

        // Incomplete → dropped.
        publish(WatchConfig {
            base_url: String::new(),
            org_slug: "org".into(),
            token: "t".into(),
        });
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);

        let full = WatchConfig {
            base_url: "https://x".into(),
            org_slug: "org".into(),
            token: "t".into(),
        };
        publish(full.clone());
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        // Identical repeat → deduped.
        publish(full.clone());
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        // Changed field → published.
        publish(WatchConfig {
            token: "t2".into(),
            ..full
        });
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }
}
