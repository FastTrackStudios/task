//! The icon rail — Obsidian's ribbon for this app. A slim icon-only
//! strip of shortcuts on the far left; the vault explorer beside it
//! is the main sidebar. Labels become tooltips; active state is a
//! subtle filled pill.
//!
//! Order: first destination (Home), quick-capture second, the rest
//! of the destinations, then the foot cluster — theme, account, and
//! the active org's initials chip at the very bottom (the org chip
//! doubles as the brand spot; there is no separate logo).

use architect_ui::lucide_dioxus::Feather;
use dioxus::prelude::*;

use crate::nav::{nav_tabs_for, tabs_match, use_active_plugins};
use crate::routes::Route;

/// The rail is a shortlist, not a directory: core destinations only.
/// Everything else is reachable by search (command palette) or links —
/// most of those pages are vault notes anyway, and the vault itself
/// needs no button (EVERYTHING is the vault; the explorer beside the
/// rail is it). Connections = the whole-vault graph. Settings renders
/// in the foot cluster, not here.
const RAIL_TABS: &[&str] = &[
    "Home",
    "Inbox",
    "Email",
    "Recall",
    "Contacts",
    "Projects",
    "Tasks",
    "Schedule",
    "Timer",
    "Invoices",
    "Files",
    "Members",
    "Wiki",
    "Agents",
    "Runners",
    "Connections",
];

#[component]
pub fn IconRail(current: Route) -> Element {
    let nav = use_navigator();
    let tabs = nav_tabs_for(&use_active_plugins());
    let mut open = crate::chrome::use_fleeting_open();

    let tab_button = |tab: &crate::nav::NavTab| {
        let is_active = tabs_match(&current, tab);
        let route = tab.route.clone();
        let icon = tab.icon;
        let label = tab.label;
        let cls = if is_active {
            "flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-accent text-foreground"
        } else {
            "flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-muted-foreground hover:bg-accent/50 hover:text-foreground"
        };
        rsx! {
            button {
                key: "{label}",
                r#type: "button",
                class: "{cls}",
                title: "{label}",
                onclick: move |_| {
                    nav.push(route.clone());
                },
                {icon()}
            }
        }
    };

    rsx! {
        // h-full, not h-screen: the rail lives under the top bar in
        // app_shell's min-h-0 row — 100vh here pushes the foot cluster
        // below the fold by exactly the bar's height.
        div { class: "flex h-full min-h-0 w-12 flex-col items-center gap-0.5 border-r border-border/60 bg-card/40 py-2",
            // First destination, then quick capture in the #2 spot.
            if let Some(first) = tabs.first() {
                {tab_button(first)}
            }
            button {
                r#type: "button",
                class: "flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-primary/15 text-primary transition-colors hover:bg-primary/25",
                title: "Capture a fleeting note",
                onclick: move |_| open.set(true),
                Feather { size: 15 }
            }
            div { class: "flex min-h-0 flex-1 flex-col items-center gap-0.5 overflow-y-auto",
                for tab in tabs.iter().skip(1).filter(|t| RAIL_TABS.contains(&t.label)) {
                    {tab_button(tab)}
                }
            }
            // Foot cluster: settings, theme, account, org initials —
            // bottom-up the org chip anchors the corner.
            div { class: "mt-1 flex shrink-0 flex-col items-center gap-0.5 border-t border-border/60 pt-1.5",
                if let Some(settings) = tabs.iter().find(|t| t.label == "Settings") {
                    {tab_button(settings)}
                }
                crate::shell::org_switcher::RailThemeButton {}
                crate::auth::AccountSwitcher { rail: true }
                crate::shell::org_switcher::RailOrgButton {}
            }
        }
    }
}
