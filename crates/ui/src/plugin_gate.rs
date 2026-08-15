//! Route-level plugin gating.
//!
//! Dioxus's router is a static enum — a disabled plugin's routes stay
//! routable (deep links, stale bookmarks, the mobile back stack). So
//! instead of unrouting them, the route shim wraps its page in
//! [`PluginGate`]: when the owning plugin is off for the ACTIVE org the
//! standard [`PluginDisabledPanel`] renders in place of the page.
//! Sibling gates: nav tabs hide via [`crate::nav::nav_tabs_for`] and
//! widgets drop via `task_widgets::WidgetRegistry::set_plugin_set` — this
//! is the last line, for navigation that bypasses both.

use dioxus::prelude::*;

use crate::nav::use_active_plugins;

/// Render `children` only while `plugin` is enabled for the active org;
/// otherwise the standard disabled panel. Mount as the OUTERMOST element
/// of a route shim — the gate holds the hooks, so the page component
/// (with all its hooks) mounts and unmounts wholesale.
#[component]
pub fn PluginGate(plugin: &'static str, children: Element) -> Element {
    let plugins = use_active_plugins();
    if plugins.contains(plugin) {
        children
    } else {
        rsx! {
            PluginDisabledPanel { plugin }
        }
    }
}

/// The standard "this plugin is off for this org" panel.
#[component]
pub fn PluginDisabledPanel(plugin: &'static str) -> Element {
    let name = task_plugin::find(plugin).map_or(plugin, |p| p.name);
    rsx! {
        div { class: "flex h-full min-h-[50vh] flex-col items-center justify-center gap-3 p-8 text-center",
            div { class: "text-lg font-semibold text-foreground", "The {name} plugin is off for this org" }
            div { class: "max-w-md text-sm text-muted-foreground",
                "An org admin can turn it back on with "
                code { class: "rounded bg-muted px-1.5 py-0.5 font-mono text-xs", "task org plugins enable {plugin}" }
                " — everything the plugin stored is kept and picks up where it left off."
            }
        }
    }
}
