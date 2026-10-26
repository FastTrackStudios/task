//! Git, as a Task app — the org's repositories, issues and PRs.
//!
//! Backed by whichever forge the org's server is configured against
//! (Forgejo or GitHub). Everything goes through Task's own
//! `RepoCatalog` / `IssueTracker` services, so the forge token lives on
//! the server and a browser tab never holds one.
//!
//! ## One screen, not two
//!
//! `Connections` used to be tagged with this plugin, and is not part of
//! it. It graphs the vault's typed links — `verse ↔ song ↔ sermon ↔
//! topic` — which has nothing to do with a code forge; the tag was only
//! ever plausible while the plugin was called "forge" and the word was
//! vague. Turning off a GitHub integration should not take somebody's
//! knowledge graph with it, so Connections moved to `wiki`, beside the
//! wikilink graph it belongs next to.

use git_ui::ReposView;
use task_plugin_ui::PluginApp;
use task_plugin_ui::dioxus::prelude::*;

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: git_ui::APP_ID,
    version: task_plugin_ui::SHIPS_WITH_TASK,
    // No sidebar entry: reached from where it belongs (see the crate
    // docs), not as a section of its own.
    nav: &[],
    view,
    panel: None,
    claim_file: None,
    provide: None,
    widgets: None,
    fences: None,
    claim_link: None,
    claim_href: None,
};

fn view(path: &str, query: &str) -> Option<Element> {
    // The screens are their own wasm chunk on the web, downloaded the
    // first time somebody opens this app; everything else the app
    // registers stays in the shell. A plain call everywhere else.
    task_plugin_ui::lazy_view!("git", git_screen, path, query)
}

fn git_screen(path: &str, _query: &str) -> Option<Element> {
    match path {
        "" => Some(rsx! { ReposView {} }),
        _ => None,
    }
}
