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
use task_plugin_ui::architect_ui::lucide_dioxus::GitBranch;
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: git_ui::APP_ID,
    version: env!("CARGO_PKG_VERSION"),
    nav: &[PluginNav {
        label: "Repos",
        icon,
        path: "",
        rail: false,
    }],
    view,
    panel: None,
    claim_file: None,
    provide: None,
    widgets: None,
    fences: None,
    claim_link: None,
    claim_href: None,
};

fn icon() -> Element {
    rsx! { GitBranch { size: 16 } }
}

fn view(path: &str, _query: &str) -> Option<Element> {
    match path {
        "" => Some(rsx! { ReposView {} }),
        _ => None,
    }
}
