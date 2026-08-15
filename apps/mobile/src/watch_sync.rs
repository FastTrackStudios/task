//! iOS → Apple Watch config push over WatchConnectivity.
//!
//! The platform half of `ui::watch_sync` (see
//! `apps/task/mobile/ios/watch-config-bridge.md`): the shared UI crate
//! observes the active {server, org, session} and publishes a
//! [`ui::watch_sync::WatchConfig`] into a registered sink; this module
//! IS that sink on iOS — a `WCSession` host that forwards the config as
//! `updateApplicationContext(["baseURL", "orgSlug", "token"])`, which
//! the watch's `PhoneSync.swift` applies to its `TaskStore`.
//!
//! Pure Rust over the objc2 ecosystem (`objc2-watch-connectivity`) — no
//! Swift shim, no AppDelegate injection into the dx-generated Xcode
//! project. Everything is `cfg(target_os = "ios")`; on other targets
//! [`init`] is a no-op so the crate builds unchanged for host checks.

#[cfg(target_os = "ios")]
mod ios {
    use std::sync::Mutex;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{AnyThread as _, define_class, msg_send};
    use objc2_foundation::{NSDictionary, NSError, NSObject, NSObjectProtocol, NSString};
    use objc2_watch_connectivity::{WCSession, WCSessionActivationState, WCSessionDelegate};
    use ui::watch_sync::WatchConfig;

    /// The latest config the UI published. `updateApplicationContext`
    /// only works on an *activated* session with the watch app
    /// installed, and activation completes asynchronously after boot —
    /// so the sink parks the value here and the activation callback
    /// replays it.
    static PENDING: Mutex<Option<WatchConfig>> = Mutex::new(None);

    define_class!(
        /// Minimal `WCSessionDelegate` for a one-way sender. iOS
        /// requires the three lifecycle methods; only activation
        /// completion does real work (replaying the parked config).
        #[unsafe(super(NSObject))]
        #[name = "TaskWatchConfigDelegate"]
        struct ConfigDelegate;

        unsafe impl NSObjectProtocol for ConfigDelegate {}

        unsafe impl WCSessionDelegate for ConfigDelegate {
            #[unsafe(method(session:activationDidCompleteWithState:error:))]
            fn activation_did_complete(
                &self,
                _session: &WCSession,
                state: WCSessionActivationState,
                error: Option<&NSError>,
            ) {
                if state == WCSessionActivationState::Activated {
                    push_pending();
                } else if let Some(e) = error {
                    tracing::warn!("watch session activation failed: {e:?}");
                }
            }

            #[unsafe(method(sessionDidBecomeInactive:))]
            fn did_become_inactive(&self, _session: &WCSession) {}

            #[unsafe(method(sessionDidDeactivate:))]
            fn did_deactivate(&self, session: &WCSession) {
                // The user switched watches — re-activate for the new
                // one (Apple's documented pattern), then re-push.
                unsafe { session.activateSession() };
            }
        }
    );

    impl ConfigDelegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    fn push_pending() {
        let cfg = PENDING.lock().ok().and_then(|p| p.clone());
        if let Some(cfg) = cfg {
            push(&cfg);
        }
    }

    /// Forward a config to the watch. Silent no-op (log only) when the
    /// session isn't activated yet or no watch app is installed —
    /// `PENDING` keeps the value for the activation callback, and
    /// `applicationContext` is latest-value-wins anyway.
    fn push(cfg: &WatchConfig) {
        let session = unsafe { WCSession::defaultSession() };
        unsafe {
            if session.activationState() != WCSessionActivationState::Activated {
                tracing::debug!("watch config parked: session not activated yet");
                return;
            }
            if !session.isWatchAppInstalled() {
                tracing::debug!("watch config dropped: no watch app installed");
                return;
            }
        }
        let keys = [
            NSString::from_str("baseURL"),
            NSString::from_str("orgSlug"),
            NSString::from_str("token"),
        ];
        let values = [
            NSString::from_str(&cfg.base_url),
            NSString::from_str(&cfg.org_slug),
            NSString::from_str(&cfg.token),
        ];
        // `&*` reborrows through `Retained`: the generic `CopiedKey`
        // key parameter gets no deref coercion from `&Retained<_>`, and
        // the values coerce `&NSString` → `&AnyObject` only from a
        // plain reference.
        let context = NSDictionary::<NSString, AnyObject>::from_slices(
            &[&*keys[0], &*keys[1], &*keys[2]],
            &[&*values[0], &*values[1], &*values[2]],
        );
        match unsafe { session.updateApplicationContext_error(&context) } {
            Ok(()) => tracing::info!(
                base_url = %cfg.base_url,
                org_slug = %cfg.org_slug,
                "pushed watch config"
            ),
            Err(e) => tracing::warn!("updateApplicationContext failed: {e:?}"),
        }
    }

    /// Activate the `WCSession` and register the config sink. Call once
    /// from `main`, before the Dioxus launch.
    pub fn init() {
        if !unsafe { WCSession::isSupported() } {
            return;
        }
        let delegate = ConfigDelegate::new();
        let session = unsafe { WCSession::defaultSession() };
        unsafe { session.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        unsafe { session.activateSession() };
        // WCSession holds its delegate weakly; keep ours alive for the
        // life of the process (it is a singleton by construction).
        std::mem::forget(delegate);

        ui::watch_sync::set_watch_config_sink(|cfg| {
            if let Ok(mut p) = PENDING.lock() {
                *p = Some(cfg.clone());
            }
            push(cfg);
        });
    }
}

#[cfg(target_os = "ios")]
pub use ios::init;

/// No watch to talk to on this platform.
#[cfg(not(target_os = "ios"))]
pub fn init() {}
