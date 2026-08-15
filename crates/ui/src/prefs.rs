//! Per-user preferences — server-backed with a localStorage boot cache.
//!
//! The authoritative record is the home org's `UserPrefs` row
//! (`prefs-proto`, keyed by the auth user id), so defaults follow the
//! account to every device and the CLI. localStorage keeps the last
//! known values per account for instant first paint; the server
//! response reconciles it.
//!
//! One root [`use_coroutine`] service owns all prefs I/O (the same
//! idiom as the auth service — unmount-safe, sequential, coalesced):
//! auth switches send `LoadFor`, UI toggles send `Save` after an
//! optimistic signal write. Prefs are convenience state: a failed
//! save logs and stays optimistic rather than interrupting the user.

use dioxus::prelude::*;
use prefs_proto::{PrefsServiceClient, UserPrefs};
use uuid::Uuid;

use crate::orgs::{OrgMeta, home_slug};
use crate::vox_clients::establish_for;

/// Messages for the root prefs service.
pub enum PrefsAction {
    /// Fetch the signed-in user's prefs (auth switch / boot).
    LoadFor(Uuid),
    /// Persist the given record (already applied optimistically).
    Save(UserPrefs),
}

/// Copyable prefs handle — provided at the app root.
#[derive(Clone, Copy)]
pub struct PrefsCtx {
    /// The active user's preferences. Defaults until the server
    /// responds; consumers just read it reactively.
    pub prefs: Signal<UserPrefs>,
    actions: Coroutine<PrefsAction>,
}

impl PrefsCtx {
    /// Patch + persist: applies the change to the signal immediately
    /// (optimistic — every consumer re-renders now) and queues the
    /// server write. Safe from self-closing UI.
    pub fn update(mut self, patch: impl FnOnce(&mut UserPrefs)) {
        let mut next = self.prefs.peek().clone();
        patch(&mut next);
        self.prefs.set(next.clone());
        save_cache(&next);
        self.actions.send(PrefsAction::Save(next));
    }
}

/// Provide the prefs context + service. Call at the app root after
/// [`crate::auth::provide_auth`] (it watches the active account).
pub fn provide_prefs() -> PrefsCtx {
    use futures_util::StreamExt as _;

    let auth = use_context::<crate::auth::AuthCtx>();
    let orgs = use_context::<Signal<Vec<OrgMeta>>>();
    let mut prefs = use_signal(|| UserPrefs::defaults_for(Uuid::nil()));

    let actions = use_coroutine(move |mut rx: UnboundedReceiver<PrefsAction>| async move {
        while let Some(mut msg) = rx.next().await {
            while let Ok(newer) = rx.try_recv() {
                msg = newer;
            }
            let slug = home_slug(&orgs.peek());
            if slug.is_empty() {
                continue; // discovery pending; the next action retries
            }
            match msg {
                PrefsAction::LoadFor(user_id) => {
                    // Cache-first paint, then the server reconciles.
                    prefs.set(load_cache(user_id));
                    if let Ok(client) = establish_for::<PrefsServiceClient>(&slug).await {
                        if let Ok(server) = client.get(user_id).await {
                            save_cache(&server);
                            prefs.set(server);
                        }
                    }
                }
                PrefsAction::Save(record) => {
                    match establish_for::<PrefsServiceClient>(&slug).await {
                        Ok(client) => {
                            if let Err(e) = client.set(record).await {
                                // Convenience state: stay optimistic,
                                // don't interrupt.
                                tracing::warn!(?e, "prefs save failed");
                            }
                        }
                        Err(e) => tracing::warn!(%e, "prefs save: no connection"),
                    }
                }
            }
        }
    });

    let ctx = PrefsCtx { prefs, actions };
    use_context_provider(|| ctx);

    // Re-load whenever the signed-in user changes (boot + switches).
    let active = auth.active;
    let mut last_user = use_signal(|| None::<Uuid>);
    use_effect(move || {
        let user = active.read().as_ref().map(|a| a.user_id);
        if user != *last_user.peek() {
            last_user.set(user);
            if let Some(id) = user {
                ctx.actions.send(PrefsAction::LoadFor(id));
            }
        }
    });
    ctx
}

// ── localStorage boot cache (web only) ──────────────────────────────

#[cfg(any(target_arch = "wasm32", test))]
fn cache_key(user_id: Uuid) -> String {
    format!("task.prefs.{user_id}")
}

#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

fn load_cache(user_id: Uuid) -> UserPrefs {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(json) = storage().and_then(|s| s.get_item(&cache_key(user_id)).ok().flatten()) {
            if let Ok(p) = serde_json::from_str::<UserPrefs>(&json) {
                if p.user_id == user_id {
                    return p;
                }
            }
        }
    }
    UserPrefs::defaults_for(user_id)
}

fn save_cache(prefs: &UserPrefs) {
    #[cfg(target_arch = "wasm32")]
    if let (Some(s), Ok(json)) = (storage(), serde_json::to_string(prefs)) {
        let _ = s.set_item(&cache_key(prefs.user_id), &json);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = prefs;
}

#[cfg(test)]
mod tests {
    use super::cache_key;
    use uuid::Uuid;

    #[test]
    fn cache_keys_are_user_scoped() {
        let id = Uuid::nil();
        assert_eq!(cache_key(id), format!("task.prefs.{id}"));
    }
}
