//! Top-level Tasks app. Owns the view-mode toggle + the
//! currently-open detail panel, and the board's keyboard model (`?`
//! lists it); everything else is dumb.

use architect_ui::lucide_dioxus::{Keyboard, X};
use architect_ui::prelude::*;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::TaskInfo;
use crate::TaskMutation;
use crate::display::TaskDisplay;

use super::detail::TaskDetail;
use super::kanban::KanbanBoard;
use super::list::TaskList;
use super::quick_add::QuickAdd;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    List,
    Kanban,
}

impl ViewMode {
    fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Kanban => "Kanban",
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct TasksAppProps {
    pub tasks: Vec<TaskInfo>,
    /// Non-project belonging per task (see
    /// [`super::row::AnchorChip`]) — what keeps a row from reading as
    /// a bare title when it has no project.
    #[props(default)]
    pub anchors: Vec<(Uuid, super::row::AnchorChip)>,
    /// Open tasks that belong to nothing at all. Rendered above the
    /// list as the triage strip rather than mixed into it; see
    /// [`super::triage::TriageStrip`].
    #[props(default)]
    pub triage: Vec<TaskInfo>,
    pub on_event: EventHandler<TaskMutation>,
    /// One capture path: when set, the quick-add line goes here (the
    /// page captures it and processes it on the spot) instead of being
    /// created as a task directly.
    #[props(default)]
    pub on_capture: Option<EventHandler<String>>,
    #[props(default)]
    pub initial_view: Option<ViewMode>,
    /// When set, the quick-edit sheet offers "Open full view" and
    /// emits the task id — the page layer routes it (`/tasks/:id`).
    #[props(default)]
    pub on_open_full: Option<EventHandler<Uuid>>,
    /// Page-owned controls (filter chips) rendered inline in the
    /// header row — one row of chrome, not two.
    #[props(default)]
    pub header_extra: Option<Element>,
    /// `(id, title)` project choices forwarded to the quick-add's
    /// picker (empty = no picker).
    #[props(default)]
    pub projects: Vec<(uuid::Uuid, String)>,
    /// `(task id, org label)` when the board spans several orgs, so a
    /// row says whose it is. Empty in a single org.
    #[props(default)]
    pub org_of: Vec<(Uuid, String)>,
    /// `(user id, name)` of the people a task can be given to — the
    /// detail sheet's owner picker. Empty = no picker.
    #[props(default)]
    pub people: Vec<(String, String)>,
    /// How many tasks the page's filters hold back, and why — the
    /// header says so instead of "0 done".
    #[props(default)]
    pub hidden_note: Option<String>,
    /// Remember the List / Kanban choice (the page persists it).
    #[props(default)]
    pub on_view_change: Option<EventHandler<ViewMode>>,
    /// Open tasks the Relevant filter holds back, and the contexts they
    /// wait for (`@home`, `@evening`) — said at the foot of the list,
    /// not only in a tooltip.
    #[props(default)]
    pub held_count: usize,
    /// The page heading. Empty for none — the Tasks list doesn't name
    /// itself; the tab bar already says where you are.
    #[props(default = "Tasks".to_string())]
    pub title: String,
    /// Rendered where the list ends, after what Relevant holds back.
    #[props(default)]
    pub footer: Option<Element>,
    #[props(default)]
    pub held_by: Vec<String>,
    /// "Show them": the page turns the Relevant filter off.
    #[props(default)]
    pub on_show_held: Option<EventHandler<()>>,
    /// Hosted inside another page (a project's board) rather than being
    /// the page: no own H1 — the host already named the context — and
    /// no `max-w` centering, the host owns the layout. The counts and
    /// the list/kanban toggle stay: they are the board's, not the
    /// page's.
    #[props(default = false)]
    pub embedded: bool,
}

#[component]
pub fn TasksApp(props: TasksAppProps) -> Element {
    let mut view = use_signal(|| props.initial_view.unwrap_or_default());
    let mut open_id: Signal<Option<Uuid>> = use_signal(|| None);

    // Triage rows open the same detail as list rows.
    let find_task = {
        let tasks = props.tasks.clone();
        let triage = props.triage.clone();
        move |id: Uuid| -> Option<TaskInfo> {
            tasks
                .iter()
                .chain(triage.iter())
                .find(|t| t.id == id)
                .cloned()
        }
    };
    let selected: Option<TaskInfo> = open_id.read().and_then(&find_task);

    // ── Keyboard ────────────────────────────────────────────────────
    // The board's keys act on the focused row, which the app-wide
    // keymap (`ui::shortcuts`) has no notion of — so they live here.
    // The listener runs in the bubble phase: an app sequence (`g t`,
    // `space …`) is consumed in capture first and never reaches it.
    // Moving focus is pure DOM (document order is the visual order);
    // actions come back as `[key, task id]`.
    let mut help_open = use_signal(|| false);
    let on_key = use_callback({
        let find_task = find_task.clone();
        move |(key, id): (String, Option<Uuid>)| {
            if key == "?" {
                help_open.toggle();
                return;
            }
            let Some(t) = id.and_then(&find_task) else {
                return;
            };
            let today = chrono::Local::now().date_naive();
            let set_due = |day: chrono::NaiveDate| {
                let mut next = t.clone();
                next.due = Some(day.to_string());
                props.on_event.call(TaskMutation::Update { task: next });
            };
            let status = |status: &str| {
                props.on_event.call(TaskMutation::SetStatus {
                    id: t.id,
                    status: status.to_owned(),
                });
            };
            match key.as_str() {
                "x" => status(task_proto::click_transition(&t.status, None)),
                "s" => status(task_proto::track_transition(&t.status)),
                "e" => open_id.set(Some(t.id)),
                "t" => set_due(today),
                "T" => set_due(today + chrono::Duration::days(1)),
                "1" | "2" | "3" | "4" => {
                    let priority = ["critical", "high", "normal", "low"]
                        [key.parse::<usize>().unwrap_or(3) - 1];
                    props.on_event.call(TaskMutation::SetPriority {
                        id: t.id,
                        priority: priority.to_owned(),
                    });
                }
                // Kanban: the focused card to the next / previous column.
                "left" | "right" => {
                    let order = crate::display::BOARD_ORDER;
                    if let Some(i) = order.iter().position(|s| *s == t.status_enum()) {
                        let j = if key == "left" {
                            i.checked_sub(1)
                        } else {
                            Some(i + 1)
                        };
                        if let Some(next) = j.and_then(|j| order.get(j)) {
                            status(next.as_str());
                        }
                    }
                }
                _ => {}
            }
        }
    });
    let embedded = props.embedded;
    use_effect(move || {
        if embedded {
            return;
        }
        spawn(async move {
            let mut keys = dioxus::document::eval(BOARD_KEYS);
            while let Ok(msg) = keys.recv::<Vec<String>>().await {
                let key = msg.first().cloned().unwrap_or_default();
                let id = msg.get(1).and_then(|s| Uuid::parse_str(s).ok());
                on_key.call((key, id));
            }
        });
    });
    use_drop(|| {
        let _ = dioxus::document::eval("window.__taskBoardKeys && window.__taskBoardKeys.abort();");
    });

    // Progress counts route through state groups (is_done), not
    // the Status enum, so custom completed-group names count. The
    // triage strip's tasks are in `tasks` too, so they count once.
    let done = props.tasks.iter().filter(|t| t.is_done()).count();
    let open_count = props.tasks.len() - done;
    // Time tracked today across the board — the day's receipt.
    let now = chrono::Utc::now();
    let today = chrono::Local::now().date_naive();
    let tracked_today: i64 = props
        .tasks
        .iter()
        .flat_map(|t| t.time_entries.iter())
        .filter(|e| chrono::DateTime::<chrono::Local>::from(e.start_time).date_naive() == today)
        .map(|e| {
            (e.end_time.unwrap_or(now) - e.start_time)
                .num_seconds()
                .max(0)
        })
        .sum();

    let shell = if props.embedded {
        "relative flex flex-col gap-3 h-full"
    } else {
        "relative mx-auto flex max-w-6xl flex-col gap-3 p-3 sm:p-5 lg:px-8 lg:py-6 h-full"
    };
    rsx! {
        div {
            class: "{shell}",
            // The key listener stands down while the task sheet is open.
            "data-board-busy": "{open_id.read().is_some()}",
            div { class: "flex flex-wrap items-center gap-x-3 gap-y-2",
                // Title and counts as one block: stacked on a phone, so a
                // long count line ("57 open · 193 done hidden") wraps under
                // the title instead of pushing the view switch onto a row
                // of its own; side by side from `sm` up, as before.
                div { class: "flex min-w-0 flex-col gap-0.5 sm:flex-row sm:items-center sm:gap-3",
                    if !props.embedded && !props.title.is_empty() {
                        Heading { level: HeadingLevel::H1, "{props.title}" }
                    }
                    // Said only when there's something to say: "0 open"
                    // over an empty list is the empty state twice.
                    if open_count > 0 || done > 0 || tracked_today > 0 {
                        span { class: "text-xs tabular-nums text-muted-foreground",
                            "{open_count} open"
                            if done > 0 {
                                " · {done} done"
                            }
                            if let Some(note) = props.hidden_note.clone() {
                                " · {note}"
                            }
                            if tracked_today > 0 {
                                " · {crate::display::duration_label(tracked_today)} tracked today"
                            }
                        }
                    }
                }
                if let Some(extra) = props.header_extra.clone() {
                    {extra}
                }
                // No view switch when embedded: a kanban board in a
                // 384px sidebar is columns nobody can read — the panel
                // is a list, the full Tasks page is where boards live.
                div { class: if props.embedded { "hidden" } else { "ml-auto flex items-center gap-1" },
                    div { class: "inline-flex items-center gap-0.5 rounded-lg bg-muted/40 p-0.5 text-xs",
                        for v in [ViewMode::List, ViewMode::Kanban] {
                            {
                                let active = v == view();
                                // Touch-sized below `sm`, compact on desktop.
                                let cls = if active {
                                    "bg-background text-foreground shadow-sm font-medium px-3 py-1.5 sm:px-2.5 sm:py-0.5 rounded-md transition-colors"
                                } else {
                                    "text-muted-foreground hover:text-foreground px-3 py-1.5 sm:px-2.5 sm:py-0.5 rounded-md transition-colors"
                                };
                                rsx! {
                                    button {
                                        key: "{v.label()}",
                                        r#type: "button",
                                        class: "{cls}",
                                        aria_pressed: active.to_string(),
                                        onclick: move |_| {
                                            view.set(v);
                                            if let Some(remember) = props.on_view_change {
                                                remember.call(v);
                                            }
                                        },
                                        "{v.label()}"
                                    }
                                }
                            }
                        }
                    }
                    button {
                        r#type: "button",
                        class: "hidden h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:inline-flex",
                        title: "Keyboard shortcuts (?)",
                        aria_label: "Keyboard shortcuts",
                        aria_expanded: "{help_open()}",
                        onclick: move |_| help_open.toggle(),
                        Keyboard { size: 16 }
                    }
                }
            }
            // Hidden on phones: the page layer renders a sticky
            // bottom action bar (thumb reach) that opens the same
            // QuickAdd in a bottom sheet instead.
            div { class: "hidden sm:block",
                QuickAdd {
                    projects: props.projects.clone(),
                    on_create: move |task: TaskInfo| match props.on_capture {
                        Some(capture) => capture.call(task.title),
                        None => props.on_event.call(TaskMutation::Create { task }),
                    },
                }
            }
            // Bottom room on phones: the capture button floats over the
            // list's end, and the last line must scroll clear of it.
            div { class: "flex flex-1 min-h-0 flex-col gap-4 overflow-y-auto pb-20 md:pb-0",
                match view() {
                    ViewMode::List => rsx! {
                        TaskList {
                            tasks: props.tasks.clone(),
                            anchors: props.anchors.clone(),
                            org_of: props.org_of.clone(),
                            on_event: props.on_event,
                            // The checkbox completes (or reopens) —
                            // `task_proto::click_transition`.
                            on_toggle: {
                                let find_task = find_task.clone();
                                move |id: Uuid| {
                                    if let Some(t) = find_task(id) {
                                        let status = task_proto::click_transition(&t.status, None).to_string();
                                        props
                                            .on_event
                                            .call(TaskMutation::SetStatus { // Start / stop work — `task_proto::track_transition`.
                                                id,
                                                status,
                                            });
                                    }
                                }
                            },
                            // Start / stop work — `task_proto::track_transition`.
                            on_track: {
                                let find_task = find_task.clone();
                                move |id: Uuid| {
                                    if let Some(t) = find_task(id) {
                                        let status = task_proto::track_transition(&t.status).to_string();
                                        props
                                            .on_event
                                            .call(TaskMutation::SetStatus {
                                                id,
                                                status,
                                            });
                                    }
                                }
                            },
                            on_open: move |id: Uuid| open_id.set(Some(id)),
                        }
                    },
                    ViewMode::Kanban => rsx! {
                        KanbanBoard {
                            tasks: props.tasks.clone(),
                            on_open: move |id: Uuid| open_id.set(Some(id)),
                            on_set_status: move |(id, status): (Uuid, String)| {
                                props
                                    .on_event
                                    .call(TaskMutation::SetStatus {
                                        id,
                                        status,
                                    });
                            },
                        }
                    },
                }
                // What Relevant is holding back, said where the list
                // ends — not only as a count in the header.
                if props.held_count > 0 {
                    div { class: "flex flex-wrap items-center gap-x-2 gap-y-1 border-t border-border/60 px-2 pt-3 text-sm text-muted-foreground",
                        span { {held_phrase(props.held_count, &props.held_by)} }
                        if let Some(show) = props.on_show_held {
                            button {
                                r#type: "button",
                                class: "rounded-md px-2 py-1 text-sm font-medium text-primary transition-colors hover:bg-primary/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                                onclick: move |_| show.call(()),
                                "Show them"
                            }
                        }
                    }
                }
                // Whatever the page wants said where the list ends (the
                // Tasks list's way to project work).
                if let Some(footer) = props.footer.clone() {
                    {footer}
                }
                // Unfiled work, after the list: it is housekeeping, and
                // the tasks themselves already sit under their dates.
                super::triage::TriageStrip {
                    tasks: props.triage.clone(),
                    projects: props.projects.clone(),
                    on_event: props.on_event,
                    on_open: move |id: Uuid| open_id.set(Some(id)),
                }
            }
            if let Some(t) = selected {
                TaskDetail {
                    task: t,
                    on_event: props.on_event,
                    on_close: move |()| open_id.set(None),
                    on_open_full: props.on_open_full,
                    projects: props.projects.clone(),
                    people: props.people.clone(),
                }
            }
            if help_open() {
                ShortcutsPanel { on_close: move |()| help_open.set(false) }
            }
        }
    }
}

/// "2 tasks wait for @errands or @home." — the foot-of-list line for
/// what the Relevant filter holds back.
fn held_phrase(count: usize, by: &[String]) -> String {
    let subject = if count == 1 {
        "1 task waits"
    } else {
        &format!("{count} tasks wait")
    };
    let place = match by {
        [] => "for another time or place".to_owned(),
        [one] => format!("for {one}"),
        [a, b] => format!("for {a} or {b}"),
        [a, b, c] => format!("for {a}, {b} or {c}"),
        [a, b, c, rest @ ..] => format!("for {a}, {b}, {c} or {} more", rest.len()),
    };
    format!("{subject} {place}.")
}

/// Every board key, in the order a person learns them: `(keys, what
/// joins them, action)` — "/" for alternatives, "–" for a range, "+"
/// for a chord.
const SHORTCUTS: &[(&[&str], &str, &str)] = &[
    (&["j", "k"], "/", "Next / previous task"),
    (&["Enter"], "", "Open the task"),
    (&["x"], "", "Complete — Undo is in the toast"),
    (&["s"], "", "Start or stop working"),
    (&["t"], "", "Due today"),
    (&["Shift", "t"], "+", "Due tomorrow"),
    (&["1", "4"], "–", "Priority, urgent to low"),
    (&["Shift", "← →"], "+", "Move a card between columns"),
    (&["n"], "", "New task"),
    (&["?"], "", "Show or hide this list"),
];

#[component]
fn ShortcutsPanel(on_close: EventHandler<()>) -> Element {
    rsx! {
        div {
            role: "dialog",
            aria_label: "Keyboard shortcuts",
            "data-board-help": "true",
            class: "fixed bottom-4 right-4 z-50 w-80 rounded-lg border border-border bg-popover p-4 text-sm text-popover-foreground shadow-[0_12px_32px_-12px_rgb(0_0_0/0.5)]",
            div { class: "mb-3 flex items-center justify-between",
                h2 { class: "font-medium", "Keyboard shortcuts" }
                button {
                    r#type: "button",
                    class: "flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                    aria_label: "Close",
                    onclick: move |_| on_close.call(()),
                    X { size: 15 }
                }
            }
            dl { class: "grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-2",
                for (keys, joiner, label) in SHORTCUTS.iter() {
                    dt { key: "{label}", class: "flex items-center gap-1",
                        for (i, k) in keys.iter().enumerate() {
                            if i > 0 {
                                span { class: "text-xs text-muted-foreground", "{joiner}" }
                            }
                            Kbd { "{k}" }
                        }
                    }
                    dd { class: "text-muted-foreground", "{label}" }
                }
            }
        }
    }
}

/// The board's key listener (see the Keyboard block in [`TasksApp`]).
/// One per page: a re-mount aborts the previous listener.
const BOARD_KEYS: &str = r#"
const ac = new AbortController();
if (window.__taskBoardKeys) window.__taskBoardKeys.abort();
window.__taskBoardKeys = ac;
const rows = () => [...document.querySelectorAll('[data-task-row]')].filter((r) => r.offsetParent !== null);
const rowOf = (el) => (el && el.closest ? el.closest('[data-task-row]') : null);
const focusRow = (r) => {
  if (!r) return;
  const f = r.matches('[data-row-focus]') ? r : r.querySelector('[data-row-focus]');
  (f || r).focus({ preventScroll: true });
  r.scrollIntoView({ block: 'nearest' });
};
document.addEventListener('keydown', (e) => {
  if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey) return;
  const t = e.target;
  if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
  if (document.querySelector('[data-board-help]')) {
    if (e.key === '?' || e.key === 'Escape') { e.preventDefault(); dioxus.send(['?']); }
    return;
  }
  if (document.querySelector('[data-board-busy="true"], [aria-modal="true"], [role="dialog"]')) return;
  const list = rows();
  const cur = rowOf(document.activeElement);
  const i = cur ? list.indexOf(cur) : -1;
  const k = e.key;
  const move = (d) => {
    e.preventDefault();
    if (!list.length) return;
    const j = i < 0 ? (d > 0 ? 0 : list.length - 1) : Math.min(Math.max(i + d, 0), list.length - 1);
    focusRow(list[j]);
  };
  if (k === 'j' || (k === 'ArrowDown' && cur && !e.shiftKey)) return move(1);
  if (k === 'k' || (k === 'ArrowUp' && cur && !e.shiftKey)) return move(-1);
  if (k === '?') { e.preventDefault(); dioxus.send(['?']); return; }
  if (k === 'n') {
    const q = document.querySelector('input[aria-label="New task"]');
    if (q && q.offsetParent !== null) { e.preventDefault(); q.focus(); }
    return;
  }
  if (!cur) return;
  let key = null;
  if (['x', 's', 'e', 't', 'T', '1', '2', '3', '4'].includes(k)) key = k;
  if (e.shiftKey && cur.hasAttribute('data-board-card')) {
    if (k === 'ArrowLeft') key = 'left';
    if (k === 'ArrowRight') key = 'right';
  }
  if (!key) return;
  e.preventDefault();
  // Completing moves on down the list; every other action (reopening
  // included) keeps the same task in hand — its row may re-render in
  // another section.
  const box = cur.querySelector('[role="checkbox"]');
  const completing = key === 'x' && (!box || box.getAttribute('aria-checked') !== 'true');
  const next = completing ? list[i + 1] || list[i - 1] : cur;
  const nextId = next ? next.dataset.taskRow : null;
  dioxus.send([key, cur.dataset.taskRow]);
  if (key !== 'e' && nextId) {
    // Once after the optimistic re-render, and again if the server's
    // reply re-rendered the row and focus fell back to the page.
    const refocus = () => focusRow(document.querySelector(`[data-task-row="${nextId}"]`));
    setTimeout(refocus, 250);
    setTimeout(() => {
      if (document.activeElement === document.body) refocus();
    }, 1000);
  }
}, { signal: ac.signal });
await new Promise(() => {});
"#;

#[cfg(test)]
mod tests {
    use super::held_phrase;

    #[test]
    fn the_held_line_names_what_it_waits_for() {
        assert_eq!(held_phrase(1, &["@home".into()]), "1 task waits for @home.");
        assert_eq!(
            held_phrase(2, &["@errands".into(), "@home".into()]),
            "2 tasks wait for @errands or @home."
        );
        assert_eq!(
            held_phrase(3, &[]),
            "3 tasks wait for another time or place."
        );
        let many: Vec<String> = ["@a", "@b", "@c", "@d", "@e"].map(String::from).to_vec();
        assert_eq!(
            held_phrase(6, &many),
            "6 tasks wait for @a, @b, @c or 2 more."
        );
    }
}
