//! Sidebar + mobile-tab definitions.
//!
//! `NavTab` is the shared shape; `nav_tabs()` is the desktop set
//! and `primary_mobile_tabs()` is the smaller bottom-bar set.

use architect_ui::lucide_dioxus::{
    BookOpen, BookUser, Bot, Brain, CalendarClock, CalendarDays, ChartGantt, CircleCheck, Dumbbell,
    Flag, FolderKanban, FolderOpen, GitBranch, House, Inbox as InboxIcon, MapPin, Notebook,
    Package, ReceiptText, RefreshCw, Scale, Settings as SettingsIcon, Target, Timer, Users,
    Utensils, Wallet, Waypoints, Youtube,
};
use dioxus::prelude::*;

use crate::routes::Route;

#[derive(Clone, PartialEq)]
#[allow(unpredictable_function_pointer_comparisons)]
pub struct NavTab {
    pub label: &'static str,
    pub icon: fn() -> Element,
    pub route: Route,
    /// Owning plugin's `task-plugin` catalog id (`"core"` = always
    /// shown). [`nav_tabs_for`] hides tabs whose plugin is off for the
    /// active org.
    pub plugin: &'static str,
}

fn icon_house() -> Element {
    rsx! { House { size: 16 } }
}
fn icon_inbox() -> Element {
    rsx! { InboxIcon { size: 16 } }
}
fn icon_recall() -> Element {
    rsx! { Brain { size: 16 } }
}
fn icon_contacts() -> Element {
    rsx! { BookUser { size: 16 } }
}
fn icon_projects() -> Element {
    rsx! { FolderKanban { size: 16 } }
}
fn icon_tasks() -> Element {
    rsx! { CircleCheck { size: 16 } }
}
fn icon_vault() -> Element {
    rsx! { Notebook { size: 16 } }
}
fn icon_locations() -> Element {
    rsx! { MapPin { size: 16 } }
}
fn icon_inventory() -> Element {
    rsx! { Package { size: 16 } }
}
fn icon_milestones() -> Element {
    rsx! { Flag { size: 16 } }
}
fn icon_fitness() -> Element {
    rsx! { Dumbbell { size: 16 } }
}
fn icon_mealplan() -> Element {
    rsx! { Utensils { size: 16 } }
}
fn icon_schedule() -> Element {
    rsx! { CalendarDays { size: 16 } }
}
fn icon_bookings() -> Element {
    rsx! { CalendarClock { size: 16 } }
}
fn icon_gantt() -> Element {
    rsx! { ChartGantt { size: 16 } }
}
fn icon_timer() -> Element {
    rsx! { Timer { size: 16 } }
}
fn icon_finances() -> Element {
    rsx! { Wallet { size: 16 } }
}
fn icon_invoices() -> Element {
    rsx! { ReceiptText { size: 16 } }
}
fn icon_members() -> Element {
    rsx! { Users { size: 16 } }
}
fn icon_ledger() -> Element {
    rsx! { Scale { size: 16 } }
}
fn icon_agents() -> Element {
    rsx! { Bot { size: 16 } }
}
fn icon_files() -> Element {
    rsx! { FolderOpen { size: 16 } }
}
fn icon_repos() -> Element {
    rsx! { GitBranch { size: 16 } }
}
fn icon_wiki() -> Element {
    rsx! { BookOpen { size: 16 } }
}
fn icon_connections() -> Element {
    rsx! { Waypoints { size: 16 } }
}
fn icon_watch() -> Element {
    rsx! { Youtube { size: 16 } }
}
fn icon_goals() -> Element {
    rsx! { Target { size: 18 } }
}
fn icon_settings() -> Element {
    rsx! { SettingsIcon { size: 16 } }
}
fn icon_sync() -> Element {
    rsx! { RefreshCw { size: 16 } }
}

pub fn nav_tabs() -> Vec<NavTab> {
    vec![
        NavTab {
            label: "Home",
            plugin: "core",
            icon: icon_house,
            route: Route::DashboardRoute {},
        },
        NavTab {
            label: "Inbox",
            plugin: "core",
            icon: icon_inbox,
            route: Route::InboxRoute {},
        },
        // Recall — spaced-repetition deck (adjacent to Inbox).
        NavTab {
            label: "Recall",
            plugin: "recall",
            icon: icon_recall,
            route: Route::RecallRoute {},
        },
        // Contacts — the people directory (adjacent to Recall).
        NavTab {
            label: "Contacts",
            plugin: "contacts",
            icon: icon_contacts,
            route: Route::ContactsRoute {},
        },
        NavTab {
            label: "Projects",
            plugin: "core",
            icon: icon_projects,
            route: Route::ProjectsRoute {},
        },
        NavTab {
            label: "Goals",
            plugin: "core",
            icon: icon_goals,
            route: Route::GoalsRoute {},
        },
        // Vault-views shortcut: the sidebar
        // item points at the board's VAULT ENTRY, not a bespoke
        // route — [[Views/Tasks]] and this tab open the same thing.
        NavTab {
            label: "Tasks",
            plugin: "core",
            icon: icon_tasks,
            route: Route::VaultRoute {
                path: "Views/Tasks.base".into(),
                org: String::new(),
            },
        },
        NavTab {
            label: "Vault",
            plugin: "core",
            icon: icon_vault,
            route: Route::VaultRoute {
                path: String::new(),
                org: String::new(),
            },
        },
        NavTab {
            label: "Locations",
            plugin: "home",
            icon: icon_locations,
            route: Route::LocationsRoute {},
        },
        NavTab {
            label: "Inventory",
            plugin: "home",
            icon: icon_inventory,
            route: Route::InventoryRoute {},
        },
        NavTab {
            label: "Milestones",
            plugin: "core",
            icon: icon_milestones,
            route: Route::MilestonesRoute {},
        },
        NavTab {
            label: "Fitness",
            plugin: "fitness",
            icon: icon_fitness,
            route: Route::FitnessRoute {},
        },
        NavTab {
            label: "Mealplan",
            plugin: "mealplan",
            icon: icon_mealplan,
            route: Route::MealplanRoute {},
        },
        NavTab {
            label: "Schedule",
            plugin: "scheduling",
            icon: icon_schedule,
            route: Route::ScheduleRoute {},
        },
        NavTab {
            label: "Bookings",
            plugin: "scheduling",
            icon: icon_bookings,
            route: Route::BookingsRoute {},
        },
        NavTab {
            label: "Gantt",
            plugin: "core",
            icon: icon_gantt,
            route: Route::GanttRoute {},
        },
        NavTab {
            label: "Timer",
            plugin: "core",
            icon: icon_timer,
            route: Route::TimerRoute {},
        },
        NavTab {
            label: "Finances",
            plugin: "finance",
            icon: icon_finances,
            route: Route::FinancesRoute {},
        },
        NavTab {
            label: "Invoices",
            plugin: "finance",
            icon: icon_invoices,
            route: Route::InvoicesRoute {},
        },
        NavTab {
            label: "Members",
            plugin: "core",
            icon: icon_members,
            route: Route::MembersRoute {},
        },
        NavTab {
            label: "Ledger",
            plugin: "finance",
            icon: icon_ledger,
            route: Route::LedgerRoute {},
        },
        NavTab {
            label: "Wiki",
            plugin: "wiki",
            icon: icon_wiki,
            route: Route::WikiRoute {},
        },
        NavTab {
            label: "Connections",
            plugin: "forge",
            icon: icon_connections,
            route: Route::ConnectionsRoute {},
        },
        // Bases now open inside the vault (selecting a `.base` file
        // renders its tables), so no dedicated tab — Obsidian-style.
        NavTab {
            label: "Watch",
            plugin: "fasttrackstudio",
            icon: icon_watch,
            route: Route::WatchRoute {
                v: String::new(),
                node: String::new(),
            },
        },
        NavTab {
            label: "Runners",
            plugin: "agent",
            icon: icon_agents,
            route: Route::RunnersRoute {},
        },
        NavTab {
            label: "Agents",
            plugin: "agent",
            icon: icon_agents,
            route: Route::AgentsRoute {
                session: String::new(),
            },
        },
        NavTab {
            label: "Repos",
            plugin: "forge",
            icon: icon_repos,
            route: Route::ReposRoute {},
        },
        NavTab {
            label: "Files",
            plugin: "files",
            icon: icon_files,
            route: Route::FilesRoute {},
        },
        NavTab {
            label: "Sync",
            // Under the files plugin, because that is what it syncs: an
            // org with Files turned off has no folders to keep in step
            // and should not be offered a page about them.
            plugin: "files",
            icon: icon_sync,
            route: Route::SyncRoute {},
        },
        NavTab {
            label: "Settings",
            plugin: "core",
            icon: icon_settings,
            route: Route::SettingsRoute {},
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tab reaches a nav surface, or it exists only in this file.
    ///
    /// Written after adding Sync and then squinting at a screenshot to
    /// work out whether it was there — which is not a test, and was not
    /// even an answer. A page nobody can navigate to is a page that does
    /// not exist, and the gap between "declared" and "rendered" is a
    /// plugin string away.
    #[test]
    fn the_sync_tab_is_offered_to_an_org_with_files() {
        // `None` = every plugin on, which is what an org with no
        // deny-list gets.
        let files_on = task_plugin::PluginSet::resolve(None);
        let tabs = nav_tabs_for(&files_on);
        assert!(
            tabs.iter().any(|t| t.label == "Sync"),
            "no Sync tab for a default org: {:?}",
            tabs.iter().map(|t| t.label).collect::<Vec<_>>()
        );
    }

    /// And it is gated on the thing it syncs: an org with Files off has
    /// no folders to keep in step and should not be offered a page
    /// about them.
    #[test]
    fn the_sync_tab_follows_the_files_plugin() {
        let sync = nav_tabs()
            .into_iter()
            .find(|t| t.label == "Sync")
            .expect("a Sync tab exists");
        assert_eq!(sync.plugin, "files");
    }
}

/// [`nav_tabs`] with the tabs of disabled plugins hidden — what every
/// nav surface (sidebar, rail, mobile "More", command palette) renders.
#[must_use]
pub fn nav_tabs_for(set: &task_plugin::PluginSet) -> Vec<NavTab> {
    nav_tabs()
        .into_iter()
        .chain(plugin_tabs())
        .filter(|t| set.contains(t.plugin))
        .collect()
}

/// The tabs registered apps contribute.
///
/// Built here rather than in the registry because a tab needs a
/// `Route`, and `Route` is the shell's — which is the whole reason the
/// plugin side speaks in paths. The shell does the one translation.
fn plugin_tabs() -> Vec<NavTab> {
    task_plugin_ui::registered()
        .into_iter()
        .flat_map(|app| {
            app.nav.iter().map(move |nav| NavTab {
                label: nav.label,
                icon: nav.icon,
                route: if nav.path.is_empty() {
                    Route::PluginRoute {
                        app: app.id.to_string(),
                        q: String::new(),
                    }
                } else {
                    Route::PluginPathRoute {
                        app: app.id.to_string(),
                        path: nav.path.split('/').map(str::to_string).collect(),
                        q: String::new(),
                    }
                },
                plugin: app.id,
            })
        })
        .collect()
}

/// The ACTIVE org's enabled plugin set, read from the discovery + org
/// switcher contexts the app root provides. Call from components under
/// the router; reading the signals subscribes the caller, so an org
/// switch (or discovery landing) re-renders the nav. Everything-on
/// until discovery resolves.
#[must_use]
pub fn use_active_plugins() -> task_plugin::PluginSet {
    let orgs = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let sel = use_context::<Signal<crate::orgs::OrgSelection>>();
    crate::orgs::active_plugin_set(&sel.read(), &orgs.read())
}

/// The bottom-tab-bar set: four primary destinations. Everything else
/// lives behind the "More" tab the bar appends itself.
pub fn primary_mobile_tabs() -> Vec<NavTab> {
    vec![
        NavTab {
            label: "Home",
            plugin: "core",
            icon: icon_house,
            route: Route::DashboardRoute {},
        },
        NavTab {
            label: "Tasks",
            plugin: "core",
            icon: icon_tasks,
            route: Route::TasksRoute {},
        },
        NavTab {
            label: "Projects",
            plugin: "core",
            icon: icon_projects,
            route: Route::ProjectsRoute {},
        },
        NavTab {
            label: "Schedule",
            plugin: "scheduling",
            icon: icon_schedule,
            route: Route::ScheduleRoute {},
        },
    ]
}

pub fn tabs_match(current: &Route, tab: &NavTab) -> bool {
    // Vault shortcuts must match by PATH — two tabs can both be
    // VaultRoutes (the Tasks view entry vs the vault tree itself).
    if let (Route::VaultRoute { path: cur, .. }, Route::VaultRoute { path: tab_path, .. }) =
        (current, &tab.route)
    {
        return if tab_path.is_empty() {
            // The bare Vault tab: match any vault path that isn't
            // claimed by a more specific shortcut. Keep it simple —
            // it matches everything; the shortcut match below also
            // firing is fine (exact wins visually is a later polish).
            !std::path::Path::new(cur)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("base"))
                || cur == tab_path
        } else {
            cur == tab_path
        };
    }
    std::mem::discriminant(current) == std::mem::discriminant(&tab.route)
}

pub fn route_title(route: &Route) -> &'static str {
    match route {
        Route::HomeRoute {} => "Home",
        Route::DashboardRoute {} => "Home",
        Route::InboxRoute {} => "Inbox",
        Route::RecallRoute {} => "Recall",
        Route::ContactsRoute {} => "Contacts",
        Route::ProjectsRoute {} => "Projects",
        Route::ProjectDetailRoute { .. } => "Project",
        Route::GoalsRoute {} => "Goals",
        Route::TasksRoute {} => "Tasks",
        Route::TaskDetailRoute { .. } => "Task",
        Route::VaultRoute { .. } => "Vault",
        Route::LocationsRoute {} => "Locations",
        Route::InventoryRoute {} => "Inventory",
        Route::MilestonesRoute {} => "Milestones",
        Route::FitnessRoute {} => "Fitness",
        Route::MealplanRoute {} => "Mealplan",
        Route::ShoppingRoute {} => "Shopping",
        Route::RecipeCookRoute { .. } => "Cook",
        Route::RecipeReadRoute { .. } => "Recipe",
        Route::RecipeEditRoute { .. } => "Edit recipe",
        Route::ScheduleRoute {} => "Schedule",
        Route::BookingsRoute {} => "Bookings",
        Route::GanttRoute {} => "Gantt",
        Route::TimerRoute {} => "Timer",
        Route::FinancesRoute {} => "Finances",
        Route::InvoicesRoute {} => "Invoices",
        Route::MembersRoute {} => "Members",
        Route::LedgerRoute {} => "Ledger",
        Route::WikiRoute {} => "Wiki",
        Route::ConnectionsRoute {} => "Connections",
        Route::BasesRoute {} => "Bases",
        Route::WatchRoute { .. } => "Watch",
        Route::WikiPageRoute { .. } => "Wiki page",
        Route::WikiSourcesRoute {} => "Archived sources",
        Route::WikiSourceRoute { .. } => "Source",
        Route::AgentsRoute { .. } => "Agents",
        Route::RunnersRoute {} => "Runners",
        Route::ReposRoute {} => "Repos",
        Route::FilesRoute {} => "Files",
        Route::SettingsRoute {} => "Settings",
        Route::SyncRoute {} => "Sync",
        // A registered app names its own screens. The label is
        // `&'static str` on the plugin side too, so this stays a
        // borrow rather than forcing every other arm to allocate.
        Route::PluginRoute { app, .. } => plugin_title(app, ""),
        Route::PluginPathRoute { app, path, .. } => plugin_title(app, &path.join("/")),
    }
}

/// What to call a registered app's screen.
///
/// Its own label for that path, the app's front-page label otherwise,
/// and a plain word when nothing is registered — a title bar should
/// never be the thing that reports a missing plugin.
fn plugin_title(app: &str, path: &str) -> &'static str {
    task_plugin_ui::find(app)
        .and_then(|a| {
            a.nav
                .iter()
                .find(|n| n.path == path)
                .or_else(|| a.nav.first())
                .map(|n| n.label)
        })
        .unwrap_or("App")
}
