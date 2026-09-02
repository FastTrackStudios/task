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

        // Contacts — the vault-backed people directory (adjacent to
        // Inbox; keep this anchor stable for clean merges).
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

        #[route("/schedule")]
        ScheduleRoute {},

        #[route("/gantt")]
        GanttRoute {},

        #[route("/timer")]
        TimerRoute {},

        #[route("/members")]
        MembersRoute {},

        // Files — the explorer over File Roots, their live trees, and
        // the Drive surface (issue #266).
        #[route("/files")]
        FilesRoute {},

        // The org's wikis, as a list you open (`wiki.many.set`).
        #[route("/wiki")]
        WikiRoute {},

        // The knowledge graph over one wiki or the vault — its own
        // surface, not the front door of the wiki.
        #[route("/graph")]
        GraphRoute {},

        // One wiki: what it is for, who edits it, and its pages.
        #[route("/wiki/w/:org/:wiki")]
        WikiHomeRoute { org: String, wiki: String },

        // One page of one wiki (wiki-root-relative path as a query
        // value, like `VaultRoute`).
        #[route("/wiki/w/:org/:wiki/page?:path")]
        WikiDocRoute { org: String, wiki: String, path: String },

        #[route("/connections")]
        ConnectionsRoute {},

        #[route("/bases")]
        BasesRoute {},

        // Deep-link to one curated wiki page (wiki-root-relative
        // path as a query value, like `VaultRoute`).
        #[route("/wiki/page?:path")]
        WikiPageRoute { path: String },

        #[route("/wiki/sources")]
        WikiSourcesRoute {},

        #[route("/wiki/source/:name")]
        WikiSourceRoute { name: String },

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
        // The far end of the central sign-in redirect: the issuer sends
        // the browser back here carrying an authorization code, which
        // the page redeems for a token before moving on.
        //
        // The path is not free to change — it is registered in the
        // issuer's `redirect_uris` for the `task` client, and authorize
        // refuses any redirect_uri that is not an exact match, before
        // the person ever reaches a login page.
        //
        // `error` is part of the contract too: OAuth reports refusals by
        // redirecting here with `error=` instead of `code=`, so a route
        // that only accepted `code` would drop a denial on the floor and
        // show an empty page.
        #[route("/auth/callback?:code&:state&:error")]
        AuthCallbackRoute { code: String, state: String, error: String },

        #[route("/app/:app?:q")]
        PluginRoute { app: String, q: String },

        #[route("/app/:app/:..path?:q")]
        PluginPathRoute { app: String, path: Vec<String>, q: String },
}

/// The URL for a target an app handed back.
///
/// Lives here rather than at any one call site because the shape is
/// this file's: four different places turn a claim into a route now (a
/// wikilink, a URL scheme, a file, a keyboard shortcut), and each one
/// spelling `/app/<id>/…` for itself is how they drift apart.
pub fn plugin_route(app: &str, target: &task_plugin_ui::LinkTarget) -> Route {
    // The app's whole query goes into the single `q` parameter,
    // encoded. Spliced in raw, an app's own `&` would end `q` and it
    // would receive only its first parameter — and would have no way to
    // tell that from a link that genuinely only had one.
    let q = task_plugin_ui::pack(&target.query);
    if target.path.is_empty() {
        Route::PluginRoute {
            app: app.to_string(),
            q,
        }
    } else {
        Route::PluginPathRoute {
            app: app.to_string(),
            path: target.path.split('/').map(str::to_string).collect(),
            q,
        }
    }
}

/// Where a vault file opens.
///
/// Ask the apps first, fall back to the note viewer. The shell used to
/// answer this itself — `if path.ends_with(".cook")`, in three separate
/// places — which was the shell holding a fact about an app, and would
/// have needed a fourth line for every app that ever added a file type.
///
/// A file nobody claims is a note, which is the right default and the
/// overwhelming case.
pub fn file_route(path: String, enabled: &task_plugin::PluginSet) -> Route {
    if let Some((app, target)) = task_plugin_ui::claim_file(&path, |id| enabled.contains(id)) {
        return plugin_route(app, &target);
    }
    Route::VaultRoute {
        path,
        org: String::new(),
    }
}

#[component]
fn PluginRoute(app: String, q: String) -> Element {
    rsx! { PluginScreen { app, path: String::new(), query: task_plugin_ui::unpack(&q) } }
}

#[component]
fn PluginPathRoute(app: String, path: Vec<String>, q: String) -> Element {
    rsx! { PluginScreen { app, path: path.join("/"), query: task_plugin_ui::unpack(&q) } }
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
        crate::plugin_gate::PluginGate { plugin: "wiki", links_ui::ConnectionsView {} }
    }
}

#[component]
fn BasesRoute() -> Element {
    rsx! { pages::bases::BasesView {} }
}

#[component]
fn MilestonesRoute() -> Element {
    rsx! { pages::milestones::MilestonesView {} }
}

#[component]
fn ScheduleRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "scheduling", pages::schedule::ScheduleView {} }
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
fn MembersRoute() -> Element {
    rsx! { pages::members::MembersView {} }
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
        crate::plugin_gate::PluginGate { plugin: "wiki", pages::wiki_index::WikiIndexView {} }
    }
}

#[component]
fn GraphRoute() -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "wiki", pages::wiki::GraphView {} }
    }
}

#[component]
fn WikiHomeRoute(org: String, wiki: String) -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "wiki", pages::wiki_home::WikiHomeView { org, wiki } }
    }
}

#[component]
fn WikiDocRoute(org: String, wiki: String, path: String) -> Element {
    rsx! {
        crate::plugin_gate::PluginGate { plugin: "wiki", pages::wiki_page::WikiPageView { org, wiki, path } }
    }
}

/// The pre-multi-wiki deep link: a page of the active org's default tier.
#[component]
fn WikiPageRoute(path: String) -> Element {
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let org = crate::orgs::active_slug(&selection.read(), &org_list.read());
    rsx! {
        crate::plugin_gate::PluginGate {
            plugin: "wiki",
            pages::wiki_page::WikiPageView { org, wiki: "knowledge".to_owned(), path }
        }
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
fn SettingsRoute() -> Element {
    rsx! { pages::settings::SettingsView {} }
}

#[component]
fn SyncRoute() -> Element {
    rsx! { pages::sync::SyncView {} }
}

#[component]
fn AuthCallbackRoute(code: String, state: String, error: String) -> Element {
    rsx! { pages::auth_callback::AuthCallbackView { code, state, error } }
}
