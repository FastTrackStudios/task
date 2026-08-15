//! The app's **actions registry** — the single catalogue of things the
//! user can *do*, shared by the command palette (Ctrl+P) and the
//! keyboard-shortcut engine ([`crate::shortcuts`]).
//!
//! Each action is an [`ActionDefinition`] (id + human name + category,
//! from `actions-proto`) registered into a [`StandaloneActions`]
//! registry (`actions-standalone`). Firing an action — from a palette
//! click or a matched key sequence — calls
//! [`StandaloneActions::execute`], whose registered handler resolves the
//! id to an [`Intent`] and forwards it over a channel to the Dioxus-side
//! effect loop in [`crate::shortcuts`] (Dioxus signals + the router's
//! `Navigator` are `!Send`, so the handlers can't touch them directly —
//! they enqueue an intent the loop performs inside the runtime).
//!
//! The registry is provided via context ([`ActionsCtx`]) so the palette
//! and the shortcut dispatcher share one source of truth.

use std::sync::Arc;

use actions_proto::{ActionCategory, ActionDefinition, ActionId, ActionResult};
use actions_standalone::StandaloneActions;

use crate::routes::Route;

// ── action ids ──────────────────────────────────────────────────────
//
// `{namespace}.{group}.{action}` per the actions-proto convention.

pub const NEW_NOTE: &str = "fts.task.new_note";
pub const TIMER_START: &str = "fts.timer.start";
pub const INBOX_PROCESS: &str = "fts.inbox.process";
pub const CAPTURE_FLEETING: &str = "fts.capture.fleeting";
pub const PALETTE_TOGGLE: &str = "fts.palette.toggle";
pub const PICKER_NOTES: &str = "fts.picker.notes";
pub const SEARCH_ALL: &str = "fts.search.all";
pub const TOGGLE_SIDEBAR: &str = "fts.toggle.sidebar";
pub const TOGGLE_PANEL: &str = "fts.toggle.panel";
pub const TOGGLE_ZEN: &str = "fts.toggle.zen";
pub const NAV_HOME: &str = "fts.nav.home";
pub const NAV_INBOX: &str = "fts.nav.inbox";
pub const NAV_TASKS: &str = "fts.nav.tasks";
pub const NAV_PROJECTS: &str = "fts.nav.projects";
pub const NAV_GOALS: &str = "fts.nav.goals";
pub const NAV_TIMER: &str = "fts.nav.timer";
pub const NAV_INVOICES: &str = "fts.nav.invoices";
pub const NAV_VAULT: &str = "fts.nav.vault";
pub const NAV_SETTINGS: &str = "fts.nav.settings";

// ── intents ─────────────────────────────────────────────────────────

/// A resolved effect the Dioxus effect loop performs. The registry's
/// (`Send + Sync`) handlers can't hold Dioxus signals / the `Navigator`,
/// so they enqueue one of these instead.
#[derive(Clone, Debug)]
pub enum Intent {
    /// Navigate to a route.
    Nav(Route),
    /// Create a fresh vault note, then open it.
    NewNote,
    /// Start a timer in the active org.
    StartTimer,
    /// Open the fleeting-capture modal.
    OpenFleeting,
    /// Toggle the command palette.
    TogglePalette,
    /// Open the note omni-picker (Ctrl+O).
    OpenOmni,
    /// Open the telescope-style everything-search window.
    OpenSearch,
    /// Toggle the vault sidebar.
    ToggleSidebar,
    /// Toggle the right (backlinks) panel.
    TogglePanel,
    /// Toggle zen mode.
    ToggleZen,
}

/// Map an action id to the [`Intent`] it performs. `None` = unknown id.
#[must_use]
pub fn intent_for(action_id: &str) -> Option<Intent> {
    Some(match action_id {
        NEW_NOTE => Intent::NewNote,
        TIMER_START => Intent::StartTimer,
        CAPTURE_FLEETING => Intent::OpenFleeting,
        PALETTE_TOGGLE => Intent::TogglePalette,
        PICKER_NOTES => Intent::OpenOmni,
        SEARCH_ALL => Intent::OpenSearch,
        TOGGLE_SIDEBAR => Intent::ToggleSidebar,
        TOGGLE_PANEL => Intent::TogglePanel,
        TOGGLE_ZEN => Intent::ToggleZen,
        INBOX_PROCESS => Intent::Nav(Route::InboxRoute {}),
        NAV_HOME => Intent::Nav(Route::DashboardRoute {}),
        NAV_INBOX => Intent::Nav(Route::InboxRoute {}),
        NAV_TASKS => Intent::Nav(Route::TasksRoute {}),
        NAV_PROJECTS => Intent::Nav(Route::ProjectsRoute {}),
        NAV_GOALS => Intent::Nav(Route::GoalsRoute {}),
        NAV_TIMER => Intent::Nav(Route::TimerRoute {}),
        NAV_INVOICES => Intent::Nav(Route::InvoicesRoute {}),
        NAV_VAULT => Intent::Nav(Route::VaultRoute {
            path: String::new(),
            org: String::new(),
        }),
        NAV_SETTINGS => Intent::Nav(Route::SettingsRoute {}),
        _ => return None,
    })
}

// ── catalogue ───────────────────────────────────────────────────────

/// Every action the app exposes, in palette display order. Shared by
/// the registry (execution) and the palette's Commands section
/// (listing + fuzzy ranking).
#[must_use]
pub fn task_action_defs() -> Vec<ActionDefinition> {
    let def = |id: &str, name: &str, desc: &str, cat: ActionCategory, hint: Option<&str>| {
        let mut d = ActionDefinition::new(id, name, desc).with_category(cat);
        if let Some(h) = hint {
            d = d.with_shortcut(h);
        }
        d
    };
    vec![
        def(
            NEW_NOTE,
            "Create new Note",
            "Create a fresh vault note and open it",
            ActionCategory::General,
            Some("Space n"),
        ),
        def(
            TIMER_START,
            "Start timer",
            "Start a time-tracking timer in the active org",
            ActionCategory::Transport,
            None,
        ),
        def(
            INBOX_PROCESS,
            "Process inbox",
            "Open the inbox to triage captured items",
            ActionCategory::General,
            Some("Space p"),
        ),
        def(
            CAPTURE_FLEETING,
            "Capture fleeting note",
            "Open the quick-capture modal",
            ActionCategory::General,
            Some("Ctrl+Shift+F"),
        ),
        def(
            PICKER_NOTES,
            "Find note…",
            "Open the fuzzy note omni-picker",
            ActionCategory::General,
            Some("Ctrl+O"),
        ),
        def(
            SEARCH_ALL,
            "Search everything…",
            "Fuzzy-search notes, tasks, and projects in one window",
            ActionCategory::General,
            Some("Space Space"),
        ),
        def(
            PALETTE_TOGGLE,
            "Command palette",
            "Toggle the command palette",
            ActionCategory::General,
            Some("Ctrl+P"),
        ),
        def(
            TOGGLE_SIDEBAR,
            "Toggle sidebar",
            "Show or hide the vault sidebar",
            ActionCategory::View,
            Some("Ctrl+\\"),
        ),
        def(
            TOGGLE_PANEL,
            "Toggle right panel",
            "Show or hide the backlinks panel",
            ActionCategory::View,
            Some("Ctrl+Shift+B"),
        ),
        def(
            TOGGLE_ZEN,
            "Toggle zen mode",
            "Hide all chrome for a focused view",
            ActionCategory::View,
            Some("Ctrl+Shift+Z"),
        ),
        def(
            NAV_HOME,
            "Go to Home",
            "Open the dashboard",
            ActionCategory::View,
            Some("g h"),
        ),
        def(
            NAV_INBOX,
            "Go to Inbox",
            "Open the inbox",
            ActionCategory::View,
            Some("g i"),
        ),
        def(
            NAV_TASKS,
            "Go to Tasks",
            "Open the task list",
            ActionCategory::View,
            Some("g t"),
        ),
        def(
            NAV_PROJECTS,
            "Go to Projects",
            "Open the projects board",
            ActionCategory::View,
            None,
        ),
        def(
            NAV_GOALS,
            "Go to Goals",
            "Open the goals view",
            ActionCategory::View,
            None,
        ),
        def(
            NAV_TIMER,
            "Go to Timer",
            "Open the time-tracking page",
            ActionCategory::View,
            None,
        ),
        def(
            NAV_INVOICES,
            "Go to Invoices",
            "Open the invoices page",
            ActionCategory::View,
            None,
        ),
        def(
            NAV_VAULT,
            "Go to Vault",
            "Open the vault tree",
            ActionCategory::View,
            None,
        ),
        def(
            NAV_SETTINGS,
            "Open Settings",
            "Open the settings page",
            ActionCategory::Settings,
            Some("Ctrl+,"),
        ),
    ]
}

// ── registry ────────────────────────────────────────────────────────

/// Shared handle on the standalone actions registry, provided via
/// context so the palette and the shortcut dispatcher execute through
/// the same instance.
#[derive(Clone)]
pub struct ActionsCtx {
    pub registry: Arc<StandaloneActions>,
}

/// Register every [`task_action_defs`] action into `registry`. Each
/// handler resolves its id to an [`Intent`] and forwards it over
/// `sink`, which the effect loop drains. Async because
/// [`StandaloneActions::register`] takes an async write lock.
pub async fn register_task_actions<S>(registry: &StandaloneActions, sink: S)
where
    S: Fn(Intent) + Send + Sync + Clone + 'static,
{
    for def in task_action_defs() {
        let sink = sink.clone();
        registry
            .register(def, move |id: &ActionId| match intent_for(id.as_str()) {
                Some(intent) => {
                    sink(intent);
                    ActionResult::success()
                }
                None => ActionResult::failure(format!("no intent for {id}")),
            })
            .await;
    }
}
