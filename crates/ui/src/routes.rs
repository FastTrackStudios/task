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

        #[route("/email")]
        EmailRoute {},

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

        #[route("/locations")]
        LocationsRoute {},

        #[route("/inventory")]
        InventoryRoute {},

        // `reference` deep-links straight to a passage (`John 3:16`,
        // `John 3:16-20@ESV`) — e.g. from a note's scripture chip;
        // empty opens the reader at its default position.
        #[route("/scripture?:reference")]
        ScriptureRoute { reference: String },

        #[route("/milestones")]
        MilestonesRoute {},

        #[route("/fitness")]
        FitnessRoute {},

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
}

#[component]
fn HomeRoute() -> Element {
    // Default landing = the todo list (Active + Relevant) — the
    // product's center of gravity.
    // The dashboard moved to /home.
    rsx! { pages::tasks::TasksView {} }
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
fn EmailRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "email", email_ui::EmailView {} }
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
fn LocationsRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "home", pages::locations::LocationsView {} }
    }
}

#[component]
fn InventoryRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "home", pages::inventory::InventoryView {} }
    }
}

#[component]
fn ScriptureRoute(reference: String) -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "scripture", scripture_ui::ScriptureView { reference } }
    }
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
fn FitnessRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "fitness", pages::fitness::FitnessView {} }
    }
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
