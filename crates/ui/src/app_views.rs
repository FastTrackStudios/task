//! App-view registry — the custom pages as vault view types.
//!
//! `plans/vault-views.md` slice 1: a `.base` entry in the vault can
//! declare `type: tasks-board` (etc.) and opening it renders the
//! full custom page — stores, mutations, Now bar and all — in place,
//! like any note. That makes the app's rich surfaces *vault
//! citizens*: linkable with `[[Views/Tasks]]`, visible to backlinks
//! and the graph, and targetable by sidebar shortcuts.
//!
//! Unknown kinds return `None` and keep the generic table fallback
//! (hand-authored/Obsidian files stay forward-compatible).

use dioxus::prelude::*;

/// Render the registered app view for `kind`, if any.
///
/// Deliberately a plain match, not a dynamic registry — the set of
/// custom pages is compiled in, and exhaustive dispatch here is the
/// single place to look.
#[must_use]
pub fn render(kind: &str) -> Option<Element> {
    match kind {
        "tasks-board" => Some(rsx! {
            crate::pages::tasks::TasksView {}
        }),
        "dashboard" => Some(rsx! {
            crate::pages::home::HomeView {}
        }),
        "inbox" => Some(rsx! {
            crate::pages::inbox::InboxView {}
        }),
        "timer" => Some(rsx! {
            crate::pages::timer::TimerView {}
        }),
        _ => None,
    }
}
