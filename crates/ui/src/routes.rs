//! Source of truth for the app's top-level routes.
//!
//! Every variant maps to a single page component in
//! [`crate::pages`]. Adding a new sidebar item is a four-step
//! change: add a `Route` variant here, add a page module, list
//! it in [`crate::nav::nav_tabs`], and add a title match arm in
//! [`crate::nav::route_title`].

use dioxus::prelude::*;

use crate::pages;
use crate::shell::app_shell::AppShell;

#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
pub enum Route {
    #[layout(AppShell)]
        #[route("/")]
        HomeRoute {},

        #[route("/home")]
        DashboardRoute {},

        #[route("/inbox")]
        InboxRoute {},

        // Recall — the spaced-repetition learning deck (adjacent to
        // Inbox; keep this anchor stable for clean merges).
        #[route("/recall")]
        RecallRoute {},

        // Contacts — the vault-backed people directory (adjacent to
        // Recall; keep this anchor stable for clean merges).
        #[route("/contacts")]
        ContactsRoute {},

        #[route("/projects")]
        ProjectsRoute {},

        #[route("/projects/:id")]
        ProjectDetailRoute { id: String },

        #[route("/goals")]
        GoalsRoute {},

        #[route("/tasks")]
        TasksRoute {},

        #[route("/tasks/:id")]
        TaskDetailRoute { id: uuid::Uuid },

        // `path` deep-links straight to a note (vault-relative path,
        // e.g. from a knowledge-graph node click); empty opens the
        // tree with nothing selected.
        #[route("/vault?:path&:org")]
        VaultRoute { path: String, org: String },

        #[route("/milestones")]
        MilestonesRoute {},

        #[route("/mealplan")]
        MealplanRoute {},

        // Deep-link straight into cook mode for one recipe (vault-relative
        // `.cook` path as a query value, like `VaultRoute`).
        #[route("/mealplan/recipe?:path")]
        RecipeCookRoute { path: String },

        // Edit a recipe's cooklang source.
        #[route("/mealplan/recipe/edit?:path")]
        RecipeEditRoute { path: String },

        // The whole recipe on one page — steps as a timeline.
        #[route("/mealplan/recipe/read?:path")]
        RecipeReadRoute { path: String },

        // The two-pass shopping run (kitchen, then store).
        #[route("/mealplan/shopping")]
        ShoppingRoute {},

        #[route("/schedule")]
        ScheduleRoute {},

        #[route("/bookings")]
        BookingsRoute {},

        #[route("/gantt")]
        GanttRoute {},

        #[route("/timer")]
        TimerRoute {},

        #[route("/finances")]
        FinancesRoute {},

        #[route("/invoices")]
        InvoicesRoute {},

        #[route("/members")]
        MembersRoute {},

        #[route("/ledger")]
        LedgerRoute {},

        #[route("/repos")]
        ReposRoute {},

        // Files — the explorer over File Roots, their live trees, and
        // the Drive surface (issue #266).
        #[route("/files")]
        FilesRoute {},

        #[route("/wiki")]
        WikiRoute {},

        #[route("/connections")]
        ConnectionsRoute {},

        #[route("/bases")]
        BasesRoute {},

        // `v` = YouTube id, `node` = the NodeRef token the timestamped
        // notes hang on (empty → the paste-a-URL landing).
        #[route("/watch?:v&:node")]
        WatchRoute { v: String, node: String },

        // Deep-link to one curated wiki page (wiki-root-relative
        // path as a query value, like `VaultRoute`).
        #[route("/wiki/page?:path")]
        WikiPageRoute { path: String },

        #[route("/wiki/sources")]
        WikiSourcesRoute {},

        #[route("/wiki/source/:name")]
        WikiSourceRoute { name: String },

        // `session` deep-links straight to one agent conversation
        // (the explorer's Agents section drives this).
        #[route("/agents?:session")]
        AgentsRoute { session: String },

        // The runner surface — everything blocking a human across
        // every project. `/agents` is the conversation UI, so this
        // gets its own path rather than displacing it.
        #[route("/runners")]
        RunnersRoute {},

        #[route("/settings")]
        SettingsRoute {},

        #[route("/sync")]
        SyncRoute {},

        // Every registered app lives under here, and this is the only
        // route the shell has that it does not own the inside of. A
        // plugin cannot add a variant to this enum — it is one enum in
        // one crate — so instead of thirty typed routes nobody outside
        // can write, there is one catch-all and a registry
        // (`task_ui_core::plugin`) that says which app answers.
        //
        // Stringly-typed exactly here, and nowhere else: the shell's
        // own routes stay typed, and an app's internal paths are its
        // own business.
        // `q` carries a deep link the app parses itself — a scripture
        // reference, a note path. The shell does not know what any
        // app's parameters mean and must not pretend to.
        //
        // It is **one** parameter holding an encoded query, not the
        // query itself, and that is load-bearing: an app's query is
        // `dish=X&tx=Y`, and spliced in raw the `&` would end `q` and
        // the app would silently receive only its first parameter.
        // `plugin_route` encodes going in, `PluginScreen` decodes
        // coming out.
        #[route("/app/:app?:q")]
        PluginRoute { app: String, q: String },

        #[route("/app/:app/:..path?:q")]
        PluginPathRoute { app: String, path: Vec<String>, q: String },
}

#[component]
fn PluginRoute(app: String, q: String) -> Element {
    rsx! { PluginScreen { app, path: String::new(), query: task_plugin_ui::decode(&q) } }
}

#[component]
fn PluginPathRoute(app: String, path: Vec<String>, q: String) -> Element {
    rsx! { PluginScreen { app, path: path.join("/"), query: task_plugin_ui::decode(&q) } }
}

/// One registered app's screen, or an honest account of why there is
/// none.
///
/// Three different nothings, told apart on purpose. An app nobody
/// registered is not installed in this build. One that is registered
/// but off for this org is a setting somebody can change. One that is
/// on and does not recognise the path is a bad link. Collapsing them
/// into a single "not found" would send a person looking in the wrong
/// place every time.
#[component]
fn PluginScreen(app: String, path: String, query: String) -> Element {
    let enabled = crate::nav::use_active_plugins();

    let Some(registered) = task_plugin_ui::find(&app) else {
        return rsx! {
            pages::missing::Missing {
                title: "That app is not installed",
                detail: "Nothing in this build registers `{app}`.",
            }
        };
    };
    if !enabled.contains(registered.id) {
        return rsx! {
            pages::missing::Missing {
                title: "That app is turned off",
                detail: "`{app}` is not enabled for this organisation. Settings can turn it on.",
            }
        };
    }
    match (registered.view)(&path, &query) {
        Some(view) => view,
        None => rsx! {
            pages::missing::Missing {
                title: "No such screen",
                detail: "`{app}` has no page at `{path}`.",
            }
        },
    }
}

#[component]
fn HomeRoute() -> Element {
    // Default landing = the Home dashboard (project cards + Today) —
    // the day's overview first, the todo list one click away at
    // /tasks. `/home` renders the same view, so both spellings land in
    // one place; a different start page is a preference
    // (`StartPageRedirect`).
    rsx! { pages::home::HomeView {} }
}

#[component]
fn DashboardRoute() -> Element {
    rsx! { pages::home::HomeView {} }
}

#[component]
fn InboxRoute() -> Element {
    rsx! { pages::inbox::InboxView {} }
}

#[component]
fn RecallRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "recall", pages::recall::RecallView {} }
    }
}

#[component]
fn ContactsRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "contacts", pages::contacts::ContactsView {} }
    }
}

#[component]
fn ProjectsRoute() -> Element {
    rsx! { pages::projects::ProjectsView {} }
}

#[component]
fn ProjectDetailRoute(id: String) -> Element {
    rsx! { pages::project_detail::ProjectDetailView { id } }
}

#[component]
fn GoalsRoute() -> Element {
    // Store-driven: the shared goal store is hydrated by
    // `use_goal_list` and kept live by the app-root `GoalService`
    // stream subscription (see `stores!`), so edits made anywhere
    // land on this page without a refetch.
    let goals = crate::stores::use_goal_list();
    let rows: Option<Vec<goal_proto::Goal>> = goals
        .value()
        .map(|rows| rows.iter().map(|(_, r)| r.goal.clone()).collect());
    let error = goals.error().cloned();
    rsx! { goal_ui::GoalsScreen { rows, error } }
}

#[component]
fn TasksRoute() -> Element {
    rsx! { pages::tasks::TasksView {} }
}

#[component]
fn TaskDetailRoute(id: uuid::Uuid) -> Element {
    rsx! { pages::task_detail::TaskDetailPage { id } }
}

#[component]
fn VaultRoute(path: String, org: String) -> Element {
    rsx! { pages::vault::VaultView { initial_path: path, initial_org: org } }
}

#[component]
fn ConnectionsRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "forge", links_ui::ConnectionsView {} }
    }
}

#[component]
fn BasesRoute() -> Element {
    rsx! { pages::bases::BasesView {} }
}

#[component]
fn WatchRoute(v: String, node: String) -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "fasttrackstudio", pages::watch::WatchView { v, node } }
    }
}

#[component]
fn MilestonesRoute() -> Element {
    rsx! { pages::milestones::MilestonesView {} }
}

#[component]
fn MealplanRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "mealplan", pages::mealplan::MealplanView {} }
    }
}

#[component]
fn RecipeCookRoute(path: String) -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "mealplan", pages::cook_mode::RecipeCookView { path } }
    }
}

#[component]
fn RecipeReadRoute(path: String) -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "mealplan", pages::recipe_read::RecipeReadView { path } }
    }
}

#[component]
fn ShoppingRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "mealplan", pages::shopping::ShoppingView {} }
    }
}

#[component]
fn RecipeEditRoute(path: String) -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "mealplan", pages::recipe_edit::EditRecipeView { path } }
    }
}

#[component]
fn ScheduleRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "scheduling", pages::schedule::ScheduleView {} }
    }
}

#[component]
fn BookingsRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "scheduling", pages::bookings::BookingsView {} }
    }
}

#[component]
fn GanttRoute() -> Element {
    rsx! { pages::gantt::GanttView {} }
}

#[component]
fn TimerRoute() -> Element {
    rsx! { pages::timer::TimerView {} }
}

#[component]
fn FinancesRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "finance", pages::finances::FinancesView {} }
    }
}

#[component]
fn InvoicesRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "finance", pages::invoices::InvoicesView {} }
    }
}

#[component]
fn MembersRoute() -> Element {
    rsx! { pages::members::MembersView {} }
}

#[component]
fn LedgerRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "finance", pages::ledger::LedgerView {} }
    }
}

#[component]
fn ReposRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "forge", pages::repos::ReposView {} }
    }
}

#[component]
fn FilesRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "files", pages::files::FilesView {} }
    }
}

#[component]
fn WikiRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "wiki", pages::wiki::WikiView {} }
    }
}

#[component]
fn WikiPageRoute(path: String) -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "wiki", pages::wiki_page::WikiPageView { path } }
    }
}

#[component]
fn WikiSourcesRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "wiki", pages::wiki_source::WikiSourcesView {} }
    }
}

#[component]
fn WikiSourceRoute(name: String) -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "wiki", pages::wiki_source::WikiSourceView { name } }
    }
}

#[component]
fn RunnersRoute() -> Element {
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    // The fleet view is still per-org on the wire — one runner
    // registry per org — so it reads the first selected org, the same
    // way the agents page picks its active one.
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
            .unwrap_or_default()
    });
    rsx! {
        crate::plugin_gate::PluginGate {
            plugin: "agent",
            div { class: "p-4",
                pages::agent_surface::AgentSurfaceView {
                    slug: slug(),
                    project: None,
                    heading: false,
                }
            }
        }
    }
}

#[component]
fn AgentsRoute(session: String) -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "agent", pages::agents::AgentsView { session } }
    }
}

#[component]
fn SettingsRoute() -> Element {
    rsx! { pages::settings::SettingsView {} }
}

#[component]
fn SyncRoute() -> Element {
    rsx! { pages::sync::SyncView {} }
}
