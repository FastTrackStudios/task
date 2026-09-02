//! Agents, as a Task app.
//!
//! Kept a plugin rather than folded into the platform, and that is the
//! interesting decision here. Agents are the one part of Task that acts
//! on its own, so "off" should be a state an org can simply be in —
//! not a thing anybody has to argue for, or achieve by not opening a
//! tab. Being an app makes it a setting: turn `agent` off and the
//! screens stop resolving, the services stop mounting, and nothing else
//! in Task notices, because nothing else depends on agents existing.
//!
//! Two screens. **Agents** is the conversation UI — sessions, messages,
//! dispatch, the inspector. **Runners** is the fleet surface: everything
//! blocking a human across every project, which is deliberately its own
//! screen rather than a panel inside a conversation, because the
//! question "what needs me?" is not asked from inside one session.

use agent_ui::{AgentPanel, AgentSurfaceView, AgentsView, provide_stores};
use task_plugin_ui::architect_ui::lucide_dioxus::{Bot, Users};
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: agent_ui::APP_ID,
    version: env!("CARGO_PKG_VERSION"),
    nav: &[
        PluginNav {
            label: "Agents",
            icon: icon_agents,
            path: "",
            rail: true,
        },
        PluginNav {
            label: "Runners",
            icon: icon_runners,
            path: "runners",
            // "What needs me?" is a thing people check the way they
            // check an inbox.
            rail: true,
        },
    ],
    view,
    // The docked conversation strip — refer to an agent while doing
    // something else, without leaving the page you are on.
    panel: Some(panel),
    claim_file: None,
    // No optimistic store — the agent screens drive their own
    // resources. What this installs is the docked panel's selection,
    // which has to outlive both the panel closing and the route
    // changing under it.
    provide: Some(provide),
    widgets: None,
    fences: None,
    claim_link: None,
    claim_href: None,
};

fn panel() -> Element {
    // The conversation strip is its own wasm chunk on the web,
    // downloaded the first time the dock is opened.
    task_plugin_ui::lazy_element!("agent_panel", agent_panel)
}

fn agent_panel() -> Element {
    rsx! { AgentPanel {} }
}

fn provide() {
    task_plugin_ui::lazy_provide!("agent_stores", provide_stores)
}

fn icon_agents() -> Element {
    rsx! { Bot { size: 16 } }
}

fn icon_runners() -> Element {
    rsx! { Users { size: 16 } }
}

fn view(path: &str, query: &str) -> Option<Element> {
    // The screens are their own wasm chunk on the web, downloaded the
    // first time somebody opens this app; everything else the app
    // registers stays in the shell. A plain call everywhere else.
    task_plugin_ui::lazy_view!("agent", agent_screen, path, query)
}

fn agent_screen(path: &str, query: &str) -> Option<Element> {
    match path {
        // `session` deep-links straight to one conversation; empty
        // opens the list.
        "" => {
            let session = task_plugin_ui::query_param(query, "session").unwrap_or_default();
            Some(rsx! { AgentsView { session } })
        }
        "runners" => Some(rsx! { RunnersScreen {} }),
        _ => None,
    }
}

/// The fleet surface, scoped to one org.
///
/// Still per-org on the wire — one runner registry per org — so it
/// reads the first selected org, the same way the conversation screen
/// picks its active one.
#[component]
fn RunnersScreen() -> Element {
    let selection = use_context::<Signal<task_ui_core::orgs::OrgSelection>>();
    let org_list = use_context::<Signal<Vec<task_ui_core::orgs::OrgMeta>>>();
    let slug = use_memo(move || {
        task_ui_core::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
            .unwrap_or_default()
    });
    rsx! {
        div { class: "p-4",
            AgentSurfaceView { slug: slug(), project: None, heading: false }
        }
    }
}
