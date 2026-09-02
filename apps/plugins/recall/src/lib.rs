//! Recall, as a Task app — the spaced-repetition deck.
//!
//! The first app with a **live** store. Its cards subscribe to the
//! org's recall stream and fold events in as they arrive, so a card
//! authored on another device appears without a refetch. That runs
//! through `provide`, at the app root, which is the only place it can:
//! a subscription started when the deck screen mounts would end the
//! moment somebody navigated away, and the whole point is that it is
//! current when they come back.

use recall_ui::{RecallView, provide_stores};
use task_plugin_ui::architect_ui::lucide_dioxus::Brain;
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: recall_ui::APP_ID,
    version: env!("CARGO_PKG_VERSION"),
    nav: &[PluginNav {
        label: "Recall",
        icon,
        path: "",
        // A review queue is a daily errand — it is worth a rail slot
        // for the same reason Inbox is: the value is in being seen
        // without being looked for.
        rail: true,
    }],
    view,
    panel: None,
    claim_file: None,
    provide: Some(provide_stores),
    widgets: None,
    fences: None,
    // No claims. A card is authored *from* a note rather than linked
    // to by one, so there is no text a wikilink would carry that means
    // "the card for this" — and nothing in the vault to claim.
    claim_link: None,
    claim_href: None,
};

fn icon() -> Element {
    rsx! { Brain { size: 16 } }
}

fn view(path: &str, query: &str) -> Option<Element> {
    // The screens are their own wasm chunk on the web, downloaded the
    // first time somebody opens this app; everything else the app
    // registers stays in the shell. A plain call everywhere else.
    task_plugin_ui::lazy_view!("recall", recall_screen, path, query)
}

fn recall_screen(path: &str, _query: &str) -> Option<Element> {
    match path {
        "" => Some(rsx! { RecallView {} }),
        _ => None,
    }
}
