//! Fitness, as a Task app.
//!
//! The first migration that had to bring its own state. Scripture and
//! Email are read-mostly screens over a service; fitness has create
//! forms, and those run through optimistic stores that used to live in
//! the shell's `stores.rs` alongside every other domain's.
//!
//! So this is the app that the `provide` contribution exists for. The
//! store has to be installed at the app *root* — provided on the
//! screen instead, it would be discarded every time somebody navigated
//! away, taking the cache and any in-flight write with it, and it
//! would not be there at all for a note widget or a search result that
//! wanted a row.

use fitness_ui::{FitnessView, provide_stores};
use task_plugin_ui::architect_ui::lucide_dioxus::Dumbbell;
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: "fitness",
    version: env!("CARGO_PKG_VERSION"),
    nav: &[PluginNav {
        label: "Fitness",
        icon: icon,
        path: "",
        // Training is a daily thing for the people who use it at all,
        // but the rail is shared with everybody's; the sidebar is the
        // right home until somebody says otherwise.
        rail: false,
    }],
    view: view,
    // Body metrics and exercises, installed at the app root.
    claim_file: None,
    provide: Some(provide_stores),
    widgets: None,
    fences: None,
    // No claims. Fitness has no vocabulary that a wikilink would carry
    // — `[[Bench Press]]` is as likely to be somebody's note about
    // their programme as it is the catalog entry, and the exercise
    // catalog is not a thing notes link into today. When it is, this
    // is where the claim goes.
    claim_link: None,
    claim_href: None,
};

fn icon() -> Element {
    rsx! { Dumbbell { size: 16 } }
}

fn view(path: &str, _query: &str) -> Option<Element> {
    match path {
        "" => Some(rsx! { FitnessView {} }),
        _ => None,
    }
}
