//! Obsidian-style tabs over routes — the top-bar tab strip.
//!
//! The **router stays the source of truth**: tabs are a view over
//! navigation history, not a second navigation system. A `use_effect`
//! watching [`use_route`] keeps [`TabsState`] in sync — when the route
//! changes it either refreshes the active tab's title (same route),
//! activates an existing tab that already shows the route, or (the
//! Obsidian move) **replaces the active tab's route in place**. Only
//! an explicit action — the "+" button or a Ctrl-click on a pill —
//! opens a NEW tab; middle-click (or the ×) closes one. The last tab
//! never closes.
//!
//! Open tabs persist to `localStorage` (`task.tabs`) on web, restored
//! on boot — routes round-trip through `Routable`'s `Display`/`FromStr`
//! string paths, so stale/unknown paths are silently dropped.
//!
//! // FUTURE(split): a true split view needs a second routed pane, but
//! // dioxus-router renders exactly ONE active route per router — there
//! // is no second `Outlet` showing a *different* route, and nesting a
//! // second `Router` would fork history/state. The clean design is a
//! // pane-tree in shell state (`Vec<Pane { tabs, active }>` + focused
//! // pane): the focused pane stays routed (URL = its active tab) and
//! // secondary panes render page components DIRECTLY by matching on
//! // their `Route` value (the `Route → page` match already lives in
//! // `routes.rs`; it would be extracted into a `render_route(route)`
//! // helper both the router variants and panes call). Deferred rather
//! // than hacked in.

use dioxus::prelude::*;

use crate::routes::Route;

// ── state ───────────────────────────────────────────────────────────

/// One open tab: a stable id (render key), the route it shows, and its
/// display title.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub id: u64,
    pub route: Route,
    pub title: String,
}

/// App-wide tab state, provided once by the shell. `Copy` — grab it
/// with `use_context::<TabsState>()` anywhere under the shell.
#[derive(Clone, Copy)]
pub struct TabsState {
    pub tabs: Signal<Vec<Tab>>,
    /// Index of the active tab in `tabs`.
    pub active: Signal<usize>,
    next_id: Signal<u64>,
}

/// Install the tabs context. Call once in the app shell; restores the
/// persisted strip (web) seeded/reconciled against the boot route.
pub fn provide_tabs(current: Route) {
    use_context_provider(|| TabsState::restore(current));
}

impl TabsState {
    fn restore(current: Route) -> Self {
        let (mut tabs, mut active) = load_persisted()
            .map(|p| {
                let tabs: Vec<Tab> = p
                    .tabs
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| {
                        let route: Route = s.parse().ok()?;
                        let title = tab_title(&route);
                        Some(Tab {
                            id: i as u64,
                            route,
                            title,
                        })
                    })
                    .collect();
                let active = p.active.min(tabs.len().saturating_sub(1));
                (tabs, active)
            })
            .unwrap_or_default();
        // Boot route wins: if a restored tab already shows it, start
        // there; otherwise the sync effect will fold it into the
        // active tab on first run. Empty restore seeds one tab.
        if tabs.is_empty() {
            let title = tab_title(&current);
            tabs.push(Tab {
                id: 0,
                route: current,
                title,
            });
            active = 0;
        } else if let Some(i) = tabs.iter().position(|t| t.route == current) {
            active = i;
        }
        let next_id = tabs.len() as u64;
        Self {
            tabs: Signal::new(tabs),
            active: Signal::new(active),
            next_id: Signal::new(next_id),
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = *self.next_id.peek();
        self.next_id.set(id + 1);
        id
    }

    /// Reconcile tabs with a route change (the router is the source of
    /// truth). Same route as the active tab → refresh its title; a
    /// different tab already shows it → activate that tab; otherwise
    /// the active tab navigates IN PLACE (Obsidian semantics).
    pub fn sync_route(&mut self, route: &Route) {
        let title = tab_title(route);
        let idx = *self.active.peek();
        let same_active = self.tabs.peek().get(idx).is_some_and(|t| &t.route == route);
        if same_active {
            let mut tabs = self.tabs.write();
            if tabs[idx].title != title {
                tabs[idx].title = title;
            }
        } else if let Some(i) = self.tabs.peek().iter().position(|t| &t.route == route) {
            self.active.set(i);
        } else if idx < self.tabs.peek().len() {
            let mut tabs = self.tabs.write();
            tabs[idx].route = route.clone();
            tabs[idx].title = title;
        } else {
            // Defensive: no valid active tab — seed one.
            let id = self.alloc_id();
            self.tabs.write().push(Tab {
                id,
                route: route.clone(),
                title,
            });
            self.active.set(self.tabs.peek().len() - 1);
        }
        self.persist();
    }

    /// Open a NEW tab showing `route`, inserted after the active tab
    /// and activated. The caller navigates if `route` isn't current.
    pub fn open_new(&mut self, route: Route) {
        let title = tab_title(&route);
        let id = self.alloc_id();
        let at = (*self.active.peek() + 1).min(self.tabs.peek().len());
        self.tabs.write().insert(at, Tab { id, route, title });
        self.active.set(at);
        self.persist();
    }

    /// Activate tab `i` (the caller navigates to its route).
    pub fn activate(&mut self, i: usize) {
        if i < self.tabs.peek().len() {
            self.active.set(i);
            self.persist();
        }
    }

    /// Close tab `i`. The last tab never closes. Returns the route to
    /// navigate to when closing changed the active tab (the neighbor
    /// that took its place).
    pub fn close(&mut self, i: usize) -> Option<Route> {
        let len = self.tabs.peek().len();
        if len <= 1 || i >= len {
            return None;
        }
        let was_active = *self.active.peek();
        self.tabs.write().remove(i);
        let new_active = if i < was_active {
            was_active - 1
        } else {
            // Closing the active tab activates the neighbor that
            // slid into its slot (or the new last tab).
            was_active.min(len - 2)
        };
        self.active.set(new_active);
        self.persist();
        (i == was_active).then(|| self.tabs.peek()[new_active].route.clone())
    }

    fn persist(&self) {
        let snapshot = Persisted {
            tabs: self
                .tabs
                .peek()
                .iter()
                .map(|t| t.route.to_string())
                .collect(),
            active: *self.active.peek(),
        };
        save_persisted(&snapshot);
    }
}

/// Tab title for a route: [`crate::nav::route_title`], except vault
/// notes, which show the note's basename (parsed from the path query —
/// no vault internals involved).
pub fn tab_title(route: &Route) -> String {
    if let Route::VaultRoute { path, .. } = route {
        if !path.is_empty() {
            let name = path.rsplit('/').next().unwrap_or(path.as_str());
            let base = name
                .rsplit_once('.')
                .map(|(stem, _)| stem)
                .filter(|s| !s.is_empty())
                .unwrap_or(name);
            if !base.is_empty() {
                return base.to_string();
            }
        }
    }
    crate::nav::route_title(route).to_string()
}

// ── strip ───────────────────────────────────────────────────────────

/// The Obsidian-style tab strip for the top bar: pills for every open
/// tab (active accented, inactive muted), each with an × on hover, a
/// trailing "+" that opens a new tab at the current route. Click
/// activates + navigates; Ctrl-click duplicates into a new tab;
/// middle-click closes.
#[component]
pub fn TabStrip() -> Element {
    let state = use_context::<TabsState>();
    let nav = use_navigator();
    let route = use_route::<Route>();

    // Router → tabs sync (the router remains the source of truth).
    let current = route.clone();
    use_effect(use_reactive!(|(route,)| {
        let mut state = state;
        state.sync_route(&route);
    }));

    let tabs = state.tabs.read().clone();
    let active = *state.active.read();
    let closable = tabs.len() > 1;

    rsx! {
        div { class: "flex min-w-0 flex-1 items-center gap-1 overflow-x-auto px-1 [scrollbar-width:none]",
            for (i , tab) in tabs.into_iter().enumerate() {
                {
                    let tab_route = tab.route.clone();
                    let mid_route = tab.route.clone();
                    rsx! {
                        div {
                            key: "{tab.id}",
                            class: if i == active {
                                "group flex max-w-56 shrink-0 cursor-pointer items-center gap-1 rounded-md bg-accent/60 py-1 pl-3 pr-1 text-xs text-foreground"
                            } else {
                                "group flex max-w-56 shrink-0 cursor-pointer items-center gap-1 rounded-md py-1 pl-3 pr-1 text-xs text-muted-foreground transition-colors hover:bg-accent/30 hover:text-foreground"
                            },
                            onclick: {
                                let mut state = state;
                                move |e: MouseEvent| {
                                    if e.modifiers().ctrl() || e.modifiers().meta() {
                                        // Ctrl-click: duplicate into a NEW tab.
                                        state.open_new(tab_route.clone());
                                    } else {
                                        state.activate(i);
                                    }
                                    nav.push(tab_route.clone());
                                }
                            },
                            // Middle-click closes (auxclick fires for the
                            // middle button after the full press).
                            onauxclick: {
                                let mut state = state;
                                move |e: PointerEvent| {
                                    use dioxus::html::point_interaction::PointerInteraction as _;
                                    if e.trigger_button() == Some(dioxus::html::input_data::MouseButton::Auxiliary) {
                                        e.prevent_default();
                                        if let Some(next) = state.close(i) {
                                            if next != mid_route {
                                                nav.push(next);
                                            }
                                        }
                                    }
                                }
                            },
                            span { class: "truncate", "{tab.title}" }
                            if closable {
                                button {
                                    r#type: "button",
                                    class: if i == active {
                                        "flex h-4 w-4 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground"
                                    } else {
                                        "flex h-4 w-4 shrink-0 items-center justify-center rounded text-transparent group-hover:text-muted-foreground hover:bg-accent hover:text-foreground"
                                    },
                                    title: "Close tab",
                                    onclick: {
                                        let mut state = state;
                                        move |e: MouseEvent| {
                                            e.stop_propagation();
                                            if let Some(next) = state.close(i) {
                                                nav.push(next);
                                            }
                                        }
                                    },
                                    architect_ui::lucide_dioxus::X { size: 12 }
                                }
                            } else {
                                // Keep the pill width stable when the ×
                                // disappears on the last tab.
                                span { class: "h-4 w-1 shrink-0" }
                            }
                        }
                    }
                }
            }
            button {
                r#type: "button",
                class: "flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent/50 hover:text-foreground",
                title: "New tab",
                onclick: {
                    let mut state = state;
                    move |_| state.open_new(current.clone())
                },
                architect_ui::lucide_dioxus::Plus { size: 14 }
            }
        }
    }
}

// ── persistence (localStorage on web) ───────────────────────────────

// localStorage persistence exists only in the browser build.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const STORAGE_KEY: &str = "task.tabs";

#[derive(serde::Serialize, serde::Deserialize)]
struct Persisted {
    /// Routes as their URL strings (`Routable` Display / FromStr).
    tabs: Vec<String>,
    active: usize,
}

#[cfg(target_arch = "wasm32")]
fn load_persisted() -> Option<Persisted> {
    let storage = web_sys::window()?.local_storage().ok()??;
    let json = storage.get_item(STORAGE_KEY).ok()??;
    serde_json::from_str(&json).ok()
}

#[cfg(target_arch = "wasm32")]
fn save_persisted(snapshot: &Persisted) {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return;
    };
    let Ok(json) = serde_json::to_string(snapshot) else {
        return;
    };
    let _ = storage.set_item(STORAGE_KEY, &json);
}

#[cfg(not(target_arch = "wasm32"))]
fn load_persisted() -> Option<Persisted> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn save_persisted(_snapshot: &Persisted) {}
