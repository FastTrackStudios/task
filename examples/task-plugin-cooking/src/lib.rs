//! Cooking, as a Task app — the worked example of the plugin seam.
//!
//! It exists to prove one property, and it is checked by its own
//! `Cargo.toml`: **this crate depends on the plugin SDK and on nothing
//! else in the workspace.** Not `task-ui`, not `task-plugin`, not a
//! feature crate. If that list ever grows, the extension point has
//! stopped being one.
//!
//! What it demonstrates:
//!
//! - contributing screens without naming a route — the shell owns
//!   `Route`, this speaks in its own paths (`""`, `"recipes"`), and the
//!   translation happens once, in the shell;
//! - using Dioxus and the component library *through* the SDK, so there
//!   is one version by construction and a skew cannot silently make
//!   `Element` two different types;
//! - keeping its data in Task's. A recipe here is a markdown note in
//!   the vault, which is what buys sync, sharing, version history and
//!   being readable by anything — including a person with a text
//!   editor and no Task at all.
//!
//! The app binary registers it, and the app binary is the only crate
//! that knows both this and Task:
//!
//! ```ignore
//! task_plugin_ui::register(task_plugin_cooking::APP);
//! ```

use task_plugin_ui::architect_ui::lucide_dioxus::Utensils;
use task_plugin_ui::architect_ui::prelude::*;
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    // The id an org's manifest turns on and off. `mealplan` already
    // exists in Task's catalog and is what cooking belongs under; a new
    // app would add its id there.
    id: "mealplan",
    nav: &[
        PluginNav {
            label: "Cooking",
            icon: icon,
            path: "",
        },
        PluginNav {
            label: "Recipes",
            icon: icon,
            path: "recipes",
        },
    ],
    view: view,
    // A recipe note could render as a method with its own timers here —
    // the same seam the player uses to turn a song note into a player.
    widgets: None,
    fences: None,
};

fn icon() -> Element {
    rsx! { Utensils { size: 16 } }
}

/// Every screen this app has.
///
/// `None` for a path it does not recognise — the shell then says so
/// itself, rather than this pretending to have a page. That is the
/// difference between a bad link and a broken app, and only this crate
/// knows which one a path is.
fn view(path: &str, _query: &str) -> Option<Element> {
    match path {
        "" => Some(kitchen()),
        "recipes" => Some(recipes()),
        _ => None,
    }
}

fn kitchen() -> Element {
    rsx! {
        section { class: "flex flex-col gap-3 p-6",
            Heading { level: HeadingLevel::H2, "Kitchen" }
            Text {
                variant: TextVariant::Muted,
                "This screen is contributed by a crate that depends on the plugin \
                 SDK and nothing else in Task — no route, no shell, no feature crate."
            }
        }
    }
}

fn recipes() -> Element {
    rsx! {
        section { class: "flex flex-col gap-3 p-6",
            Heading { level: HeadingLevel::H2, "Recipes" }
            Text {
                variant: TextVariant::Muted,
                "A recipe is a markdown note in the vault. That is the trade the \
                 whole design turns on: an app gets file management, sync, sharing \
                 and version history for free, and stores nothing only it can read."
            }
        }
    }
}
