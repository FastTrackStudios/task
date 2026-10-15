//! Favorites — the issues, projects and pages you keep coming back to,
//! one tap from the tab bar (Linear's third tab).
//!
//! A favorite is a reference, not a copy: `(kind, id, title)`, the title
//! kept only so the list can draw before the stores load. Starred with
//! [`StarButton`] on an issue's or a project's top bar; listed on
//! `/favorites`.
//!
//! Kept per device ([`crate::local_store`]) for now — FUTURE: the
//! account's prefs, so a phone and a desk share one list.

use architect_ui::lucide_dioxus::{Box as BoxIcon, FileText, Star};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::routes::Route;

const KEY: &str = "task.favorites";

/// One starred thing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Favorite {
    /// `"issue"` | `"project"` | `"page"`.
    pub kind: String,
    pub id: String,
    pub title: String,
}

impl Favorite {
    /// Where it opens.
    #[must_use]
    pub fn route(&self) -> Option<Route> {
        match self.kind.as_str() {
            "issue" => self.id.parse().ok().map(|id| Route::TaskDetailRoute { id }),
            "project" => Some(Route::ProjectDetailRoute {
                id: self.id.clone(),
            }),
            "page" => Some(Route::VaultRoute {
                path: self.id.clone(),
                org: String::new(),
            }),
            _ => None,
        }
    }
}

/// Every favorite, most recently starred first.
#[must_use]
pub fn load() -> Vec<Favorite> {
    crate::local_store::get(KEY)
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn save(list: &[Favorite]) {
    if let Ok(j) = serde_json::to_string(list) {
        crate::local_store::set(KEY, &j);
    }
}

/// The shared list, so a star anywhere updates the Favorites tab.
#[derive(Clone, Copy)]
pub struct Favorites(pub Signal<Vec<Favorite>>);

impl Favorites {
    #[must_use]
    pub fn has(&self, kind: &str, id: &str) -> bool {
        self.0.read().iter().any(|f| f.kind == kind && f.id == id)
    }

    /// Star it, or unstar it if it was starred.
    pub fn toggle(mut self, fav: Favorite) {
        let mut list = self.0();
        if let Some(i) = list
            .iter()
            .position(|f| f.kind == fav.kind && f.id == fav.id)
        {
            list.remove(i);
        } else {
            list.insert(0, fav);
        }
        save(&list);
        self.0.set(list);
    }
}

/// Install the [`Favorites`] context. Call once in the app shell.
pub fn provide_favorites() {
    use_context_provider(|| Favorites(Signal::new(load())));
}

/// ☆ / ★ for a top bar.
#[component]
pub fn StarButton(kind: &'static str, id: String, title: String) -> Element {
    let favs = use_context::<Favorites>();
    let on = favs.has(kind, &id);
    rsx! {
        button {
            r#type: "button",
            class: if on {
                "flex size-9 items-center justify-center rounded-md text-amber-400 hover:bg-accent md:size-8"
            } else {
                "flex size-9 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground md:size-8"
            },
            title: if on { "Remove from favorites" } else { "Add to favorites" },
            aria_pressed: on.to_string(),
            onclick: move |_| favs.toggle(Favorite { kind: kind.to_owned(), id: id.clone(), title: title.clone() }),
            if on {
                span { class: "[&_svg]:fill-current", Star { size: 16 } }
            } else {
                Star { size: 16 }
            }
        }
    }
}

/// `/favorites`.
#[component]
pub fn FavoritesView() -> Element {
    let favs = use_context::<Favorites>();
    let list = favs.0();
    rsx! {
        div { class: "mx-auto flex w-full max-w-3xl flex-col gap-4 px-4 py-5 sm:px-8",
            h1 { class: "text-[28px] font-semibold tracking-tight text-foreground", "Favorites" }
            if list.is_empty() {
                p { class: "py-16 text-center text-[15px] text-muted-foreground",
                    "Your favorites will appear here. Star an issue or a project from its top bar."
                }
            } else {
                ul { class: "flex flex-col",
                    for f in list.into_iter() {
                        if let Some(route) = f.route() {
                            li { key: "{f.kind}:{f.id}",
                                Link {
                                    to: route,
                                    class: "flex min-h-12 items-center gap-3 rounded-lg px-2 text-[15px] text-foreground hover:bg-accent/50 active:bg-accent",
                                    span { class: "text-muted-foreground",
                                        match f.kind.as_str() {
                                            "project" => rsx! { BoxIcon { size: 16 } },
                                            "page" => rsx! { FileText { size: 16 } },
                                            _ => rsx! { task_ui::StatusIcon { status: "open".to_owned(), size: 16 } },
                                        }
                                    }
                                    span { class: "min-w-0 flex-1 truncate", "{f.title}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
