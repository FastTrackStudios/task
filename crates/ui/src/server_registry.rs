//! Phase 8 — client-side server registry.
//!
//! Persists across reloads via `localStorage` on web; in-memory
//! only on native (the desktop launcher carries its own state).
//! Stored as JSON under the key
//! [`STORAGE_KEY`](Self::STORAGE_KEY). The registry exposes a
//! `Signal<Vec<ServerEntry>>` so Dioxus components re-render when
//! entries change.
//!
//! Phase 8 keeps the shape small: each entry is `(label,
//! server_url, session_token, my_user_id)`. The token is what
//! Phase 2's `AuthService::sign_in_email_password` returned; the
//! federated view forwards it via `AuthClientMiddleware` so the
//! server knows who's asking.

use std::collections::BTreeMap;

use architect_ui::prelude::*;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const STORAGE_KEY: &str = "task-architect.servers.v1";
/// Storage key / filename stem for the active-server pointer.
pub const ACTIVE_KEY: &str = "task-architect.active.v1";

/// One row of the registry. `label` is user-chosen ("work",
/// "studio"); `server_url` is the WS endpoint root
/// (`ws://host:port/vox`); `session_token` is the bearer from
/// Phase 2; `my_user_id` is the user-id Phase 2 returned at
/// sign-in (used by the federated view to disambiguate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEntry {
    pub id: Uuid,
    pub label: String,
    pub server_url: String,
    pub session_token: Option<String>,
    pub my_user_id: Option<Uuid>,
    pub my_email: Option<String>,
}

impl ServerEntry {
    pub fn new(label: impl Into<String>, server_url: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            label: label.into(),
            server_url: server_url.into(),
            session_token: None,
            my_user_id: None,
            my_email: None,
        }
    }
}

/// Clone-cheap handle over the registry. Wraps Dioxus `Signal`s so
/// consumers re-render on changes. `active` names the entry the app
/// currently connects to (`None` = fall back to env/same-origin).
#[derive(Debug, Clone, Copy)]
pub struct ServerRegistry {
    entries: Signal<BTreeMap<Uuid, ServerEntry>>,
    active: Signal<Option<Uuid>>,
}

impl ServerRegistry {
    /// Build a registry, loading any persisted entries + active
    /// selection from storage (localStorage on wasm, a JSON file on
    /// native).
    #[must_use]
    pub fn new() -> Self {
        let initial = load_from_storage().unwrap_or_default();
        // Drop a stale active pointer if its entry is gone.
        let active = load_active().filter(|id| initial.contains_key(id));
        Self {
            entries: Signal::new(initial),
            active: Signal::new(active),
        }
    }

    /// Snapshot of current entries, sorted by label.
    #[must_use]
    pub fn list(&self) -> Vec<ServerEntry> {
        let mut out: Vec<ServerEntry> = self.entries.read().values().cloned().collect();
        out.sort_by(|a, b| a.label.cmp(&b.label));
        out
    }

    #[must_use]
    pub fn get(&self, id: Uuid) -> Option<ServerEntry> {
        self.entries.read().get(&id).cloned()
    }

    pub fn upsert(&mut self, entry: ServerEntry) {
        self.entries.with_mut(|map| {
            map.insert(entry.id, entry);
        });
        self.persist();
    }

    pub fn remove(&mut self, id: Uuid) {
        self.entries.with_mut(|map| {
            map.remove(&id);
        });
        if *self.active.peek() == Some(id) {
            self.set_active(None);
        }
        self.persist();
    }

    /// The active entry's id (the reactive signal — reads subscribe).
    #[must_use]
    pub fn active_id(&self) -> Option<Uuid> {
        *self.active.read()
    }

    /// The active entry, if any.
    #[must_use]
    pub fn active_entry(&self) -> Option<ServerEntry> {
        self.active_id().and_then(|id| self.get(id))
    }

    /// Point the app at `id` (or clear back to the env/same-origin default).
    pub fn set_active(&mut self, id: Option<Uuid>) {
        self.active.set(id);
        save_active(id);
        // Push straight to the vox holder here rather than relying on the app
        // root re-rendering to sync it: on mobile that reactive propagation
        // didn't fire when a server was added/selected at runtime, so the
        // holder stayed empty and every call errored "no server URL
        // configured" — even though the UI showed the server as active.
        // Verified on desktop the load-from-disk path already works; this makes
        // the runtime add/select path work the same everywhere.
        crate::vox_session::set_active_server(id.and_then(|i| self.get(i)).map(|e| {
            crate::vox_session::ActiveServer {
                url: e.server_url,
                token: e.session_token,
            }
        }));
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }

    fn persist(&self) {
        let snapshot: BTreeMap<Uuid, ServerEntry> = self.entries.read().clone();
        save_to_storage(&snapshot);
    }
}

impl Default for ServerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalize a user-typed server address into a vox base URL. Bare
/// hosts default to `wss://` (production over TLS); an explicit scheme
/// is respected. `http(s)` is mapped to `ws(s)`.
fn normalize_url(raw: &str) -> String {
    let t = raw.trim().trim_end_matches('/');
    if let Some(rest) = t.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = t.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if t.contains("://") {
        t.to_owned()
    } else {
        format!("wss://{t}")
    }
}

/// Host portion of a normalized URL, for a default label.
fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_owned()
}

/// Add / select / remove the servers the app can connect to. Reads the
/// [`ServerRegistry`] from context; selecting an entry re-points the app
/// (org discovery + connection) at that server via the app root's holder
/// sync. Sign-in to the selected server happens through the login form
/// (which dials the active server).
#[component]
pub fn ServersPanel() -> Element {
    let mut registry = use_context::<ServerRegistry>();
    let mut label = use_signal(String::new);
    let mut url = use_signal(String::new);
    // Why discovery didn't resolve for the active server, if it failed.
    let discovery_err = use_context::<crate::orgs::DiscoveryError>().0;

    let servers = registry.list();
    let active = registry.active_id();

    let add = move |_| {
        let raw = url.peek().trim().to_owned();
        if raw.is_empty() {
            return;
        }
        let normalized = normalize_url(&raw);
        let chosen = label.peek().trim().to_owned();
        let entry_label = if chosen.is_empty() {
            host_of(&normalized)
        } else {
            chosen
        };
        let entry = ServerEntry::new(entry_label, normalized);
        let id = entry.id;
        registry.upsert(entry);
        registry.set_active(Some(id));
        label.set(String::new());
        url.set(String::new());
    };

    rsx! {
        div { class: "flex flex-col gap-3",
            ThisDevice {}
            div { class: "px-1 pt-1 text-xs font-medium text-muted-foreground",
                "Connect to a host / another device"
            }
            if let Some(err) = discovery_err.read().as_ref() {
                div { class: "rounded-lg border border-destructive/40 bg-destructive/10 px-2 py-2 text-xs text-destructive",
                    div { class: "font-medium", "Couldn't reach this server" }
                    div { class: "mt-0.5 break-words opacity-90", "{err}" }
                }
            }
            if servers.iter().all(|s| task_ui_core::node::is_local(&s.server_url)) {
                div { class: "px-1 text-xs text-muted-foreground",
                    "No hosts yet. Add one below (e.g. task.starcommand.live) — a host is just another node of your org that is always on."
                }
            } else {
                div { class: "flex flex-col gap-1",
                    for s in servers.into_iter().filter(|s| !task_ui_core::node::is_local(&s.server_url)) {
                        div {
                            key: "{s.id}",
                            class: "flex items-center gap-2 rounded-lg border border-border px-2 py-2",
                            button {
                                r#type: "button",
                                class: "flex min-w-0 flex-1 flex-col text-left",
                                onclick: {
                                    let id = s.id;
                                    move |_| registry.set_active(Some(id))
                                },
                                span { class: "truncate text-sm font-medium text-foreground", "{s.label}" }
                                span { class: "truncate text-xs text-muted-foreground", "{s.server_url}" }
                            }
                            if Some(s.id) == active {
                                span { class: "text-xs text-primary", "● active" }
                            }
                            button {
                                r#type: "button",
                                class: "text-xs text-muted-foreground hover:text-destructive",
                                onclick: {
                                    let id = s.id;
                                    move |_| registry.remove(id)
                                },
                                "Remove"
                            }
                        }
                    }
                }
            }

            div { class: "flex flex-col gap-2",
                Input { value: label, placeholder: "Label (optional)" }
                Input {
                    value: url,
                    input_type: "url".to_string(),
                    placeholder: "Host URL (task.starcommand.live)",
                }
                Button {
                    variant: ButtonVariant::Secondary,
                    size: ButtonSize::Medium,
                    on_click: add,
                    class: "w-full",
                    "Add host"
                }
            }
        }
    }
}

/// The registry entry that means "this device's own node", created on
/// first use. One per registry, found by its address.
fn this_device_entry(registry: &mut ServerRegistry) -> Uuid {
    if let Some(found) = registry
        .list()
        .into_iter()
        .find(|e| task_ui_core::node::is_local(&e.server_url))
    {
        return found.id;
    }
    let entry = ServerEntry::new("This device", task_ui_core::node::LOCAL_VOX_URL);
    let id = entry.id;
    registry.upsert(entry);
    id
}

/// "This device" — the node this install runs (`offline.local`,
/// `offline.peers`). Run on it, see its address, and keep a node of a
/// host's org here. Renders nothing on the browser, which holds no node.
#[component]
fn ThisDevice() -> Element {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rsx! { this_device::Panel {} }
    }
    #[cfg(target_arch = "wasm32")]
    {
        rsx! {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod this_device {
    use architect_ui::prelude::*;
    use dioxus::prelude::*;
    use files_proto::service::sync::SyncServiceClient;
    use task_ui_core::orgs::OrgMeta;

    use super::{ServerRegistry, this_device_entry};

    #[component]
    pub fn Panel() -> Element {
        let Some(node) = task_ui_core::node::host() else {
            return rsx! {};
        };
        let mut registry = use_context::<ServerRegistry>();
        let auth = use_context::<crate::auth::AuthCtx>();
        let orgs = use_context::<Signal<Vec<OrgMeta>>>();
        let mut note = use_signal(|| None::<String>);
        let local = task_ui_core::node::local_active();
        let root = node.data_root().display().to_string();

        let run_here = move |_| {
            let id = this_device_entry(&mut registry);
            registry.set_active(Some(id));
            spawn(async move {
                // Straight in as the device's person: the node knows them.
                if let Some(owner) = task_ui_core::node::local_owner().await {
                    auth.switch_account(owner.email);
                }
            });
        };

        // Orgs a host serves that this node does not hold yet — what
        // "keep a node here" applies to.
        let joinable: Vec<OrgMeta> = if local {
            Vec::new()
        } else {
            orgs.read()
                .iter()
                .filter(|o| o.member != Some(false) && o.id.is_some() && !node.holds(&o.slug))
                .cloned()
                .collect()
        };
        let held: Vec<(String, Option<String>)> = orgs
            .read()
            .iter()
            .filter(|o| node.holds(&o.slug))
            .map(|o| (o.name.clone(), node.endpoint_id(&o.slug)))
            .collect();

        rsx! {
            div { class: "flex flex-col gap-2 rounded-lg border border-border px-2 py-2",
                div { class: "flex items-center gap-2",
                    div { class: "flex min-w-0 flex-1 flex-col",
                        span { class: "text-sm font-medium text-foreground", "This device" }
                        span { class: "truncate text-xs text-muted-foreground",
                            "Runs Task itself, with no server — {root}"
                        }
                    }
                    if local {
                        span { class: "text-xs text-primary", "● running here" }
                    } else {
                        Button {
                            variant: ButtonVariant::Secondary,
                            size: ButtonSize::Small,
                            on_click: run_here,
                            "Run on this device"
                        }
                    }
                }
                for (name, id) in held {
                    div { class: "truncate px-1 text-xs text-muted-foreground",
                        "{name} — node "
                        span { class: "font-mono", {id.unwrap_or_else(|| "not bound yet".to_owned())} }
                    }
                }
                for org in joinable {
                    div { class: "flex items-center gap-2 px-1",
                        span { class: "min-w-0 flex-1 truncate text-xs text-foreground", "{org.name}" }
                        Button {
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Small,
                            on_click: {
                                let org = org.clone();
                                move |_| {
                                    let org = org.clone();
                                    spawn(async move {
                                        note.set(Some(format!("Joining {}…", org.name)));
                                        let said = join(&org).await;
                                        note.set(Some(said));
                                    });
                                }
                            },
                            "Keep on this device"
                        }
                    }
                }
                if let Some(n) = note.read().as_ref() {
                    div { class: "px-1 text-xs text-muted-foreground break-words", "{n}" }
                }
            }
        }
    }

    /// The join handshake, both halves, with the session already held
    /// on the host: the host's endpoint id for the org → this node takes
    /// the org and admits it → the host enrolls this node's endpoint id.
    /// From then each admits the other (`offline.peers`).
    pub(super) async fn join(org: &OrgMeta) -> String {
        let Some(node) = task_ui_core::node::host() else {
            return "This build holds no node.".into();
        };
        let Some(id) = org.id else {
            return format!(
                "{} did not say its org id — cannot hold a node of it.",
                org.name
            );
        };
        let host: SyncServiceClient =
            match task_ui_core::vox_clients::establish_for(&org.slug).await {
                Ok(c) => c,
                Err(e) => return format!("Could not reach {}'s host: {e}", org.name),
            };
        let peer = host.coordinator().await.ok();
        let mine = match node
            .join(task_ui_core::node::JoinRequest {
                slug: org.slug.clone(),
                id,
                display_name: org.name.clone(),
                peer,
                role: None,
            })
            .await
        {
            Ok(Some(mine)) => mine,
            Ok(None) => {
                return format!(
                    "{} is now held here, but this node has no endpoint to enroll (peers off?).",
                    org.name
                );
            }
            Err(e) => return format!("Could not take a node of {}: {e}", org.name),
        };
        match host
            .enroll_device(mine.clone(), "Task app node".to_owned())
            .await
        {
            Ok(_) => format!(
                "{} is held on this device too (node {}). Its host admits this node.",
                org.name,
                &mine[..mine.len().min(8)]
            ),
            Err(e) => format!("Held here, but the host refused to enroll this node: {e}"),
        }
    }
}

// ── localStorage I/O (wasm-only) ────────────────────────────────

#[cfg(target_arch = "wasm32")]
fn load_from_storage() -> Option<BTreeMap<Uuid, ServerEntry>> {
    let storage = web_sys::window()?.local_storage().ok()??;
    let json = storage.get_item(STORAGE_KEY).ok()??;
    serde_json::from_str(&json).ok()
}

#[cfg(target_arch = "wasm32")]
fn save_to_storage(snapshot: &BTreeMap<Uuid, ServerEntry>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(storage)) = window.local_storage() else {
        return;
    };
    let Ok(json) = serde_json::to_string(snapshot) else {
        return;
    };
    let _ = storage.set_item(STORAGE_KEY, &json);
}

#[cfg(target_arch = "wasm32")]
fn load_active() -> Option<Uuid> {
    let storage = web_sys::window()?.local_storage().ok()??;
    let raw = storage.get_item(ACTIVE_KEY).ok()??;
    Uuid::parse_str(raw.trim()).ok()
}

#[cfg(target_arch = "wasm32")]
fn save_active(id: Option<Uuid>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(storage)) = window.local_storage() else {
        return;
    };
    match id {
        Some(id) => {
            let _ = storage.set_item(ACTIVE_KEY, &id.to_string());
        }
        None => {
            let _ = storage.remove_item(ACTIVE_KEY);
        }
    }
}

// Native (desktop/mobile): persist to `$XDG_DATA_HOME/task/servers.json`
// (stays inside the iOS/macOS app container). Mirrors the token store's
// data-dir choice in `auth.rs`.
/// Where the registry lives on a native target.
///
/// Apple platforms get `Library/Application Support`, not
/// `~/.local/share`. That is not cosmetic: it is the sanctioned location
/// for app data on iOS/macOS, it is preserved across app updates, and it
/// is included in backups. A dotted XDG-style directory at the container
/// root is a Linux convention that iOS makes no promises about — which
/// is the most likely reason the app kept forgetting its server URL
/// between launches.
///
/// `XDG_DATA_HOME` still wins when explicitly set, so a Linux desktop or
/// a test harness can point this wherever it likes.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn data_dir() -> Option<std::path::PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return Some(xdg.join("task"));
    }
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_vendor = "apple") {
        Some(
            home.join("Library")
                .join("Application Support")
                .join("task"),
        )
    } else {
        Some(home.join(".local").join("share").join("task"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn servers_path() -> Option<std::path::PathBuf> {
    Some(data_dir()?.join("servers.json"))
}

#[cfg(not(target_arch = "wasm32"))]
fn load_from_storage() -> Option<BTreeMap<Uuid, ServerEntry>> {
    let path = servers_path()?;
    match std::fs::read_to_string(&path) {
        Ok(json) => match serde_json::from_str(&json) {
            Ok(map) => Some(map),
            // A corrupt file is worth saying out loud: it silently
            // resets the app to "no servers configured".
            Err(e) => {
                tracing::warn!(path = %path.display(), "server registry: unreadable, ignoring: {e}");
                None
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!(path = %path.display(), "server registry: read failed: {e}");
            None
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn save_to_storage(snapshot: &BTreeMap<Uuid, ServerEntry>) {
    // Failures are LOGGED, not swallowed. Every write here used to be
    // `let _ = …`, so a registry that never persisted looked exactly like
    // one that did — the app simply forgot its server on next launch with
    // nothing anywhere to say why.
    let Some(path) = servers_path() else {
        tracing::warn!("server registry: no data dir (no HOME/XDG_DATA_HOME) — not persisting");
        return;
    };
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(dir = %parent.display(), "server registry: mkdir failed: {e}");
        return;
    }
    match serde_json::to_string_pretty(snapshot) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!(path = %path.display(), "server registry: write failed: {e}");
            } else {
                tracing::debug!(path = %path.display(), servers = snapshot.len(), "server registry saved");
            }
        }
        Err(e) => tracing::warn!("server registry: encode failed: {e}"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn active_path() -> Option<std::path::PathBuf> {
    servers_path().map(|p| p.with_file_name("active-server"))
}

#[cfg(not(target_arch = "wasm32"))]
fn load_active() -> Option<Uuid> {
    let raw = std::fs::read_to_string(active_path()?).ok()?;
    Uuid::parse_str(raw.trim()).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn save_active(id: Option<Uuid>) {
    let Some(path) = active_path() else {
        return;
    };
    match id {
        Some(id) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, id.to_string());
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_serialization() {
        let mut map: BTreeMap<Uuid, ServerEntry> = BTreeMap::new();
        let entry = ServerEntry {
            id: Uuid::new_v4(),
            label: "work".into(),
            server_url: "ws://example.test:9090/vox".into(),
            session_token: Some("tok-123".into()),
            my_user_id: Some(Uuid::new_v4()),
            my_email: Some("alice@example.test".into()),
        };
        map.insert(entry.id, entry.clone());
        let json = serde_json::to_string(&map).expect("serialize");
        let back: BTreeMap<Uuid, ServerEntry> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, map);
    }
}
