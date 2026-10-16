//! The composer — making an issue, Linear's way, with Task's
//! natural-language title.
//!
//! A title, a description, and the property chips under them (Status,
//! Priority, Assignee, Project, Labels — the shared [`PropertyPicker`]),
//! a ⋯ menu for the rest (due date, repeat, a link, sub-issues created
//! right there), and Create with a "Create more" switch.
//!
//! What Linear doesn't do and this does: the title is read as you type.
//!
//! - `@na…` opens the people list under the title, filtered by what
//!   follows the `@`; ↑ / ↓ / Enter (or Tab) pick, Esc dismisses. Picking
//!   removes the `@…` from the title and sets the Assignee chip — the
//!   assignment that puts an issue on someone's Tasks list.
//! - `#la…` does the same for labels (and offers to create one).
//! - `[[Project]]`, `tomorrow`, `fri`, `every week`, `!high` are read by
//!   the quick-add parser (`task_proto::capture`) and shown as chips the
//!   moment they're typed; a chip set by hand wins over the text.
//!
//! On a phone it is a full-screen sheet; on a desk, a card. ⌘↩ creates.

use architect_ui::lucide_dioxus::{CalendarDays, Clock, Link2, ListTree, Plus, Repeat, Tag, X};
use dioxus::prelude::*;
use uuid::Uuid;

use super::issue_view::with_assignee;
use super::picker::{PickOption, PropertyPicker, filtered};
use super::properties::{
    AssigneeChip, CHIP_ROW_CLASS, CHIP_ROW_QUIET_CLASS, Hotkey, LabelsChip, PersonDot,
    PriorityChip, ProjectChip, StatusChip, label_color,
};
use crate::TaskInfo;

/// The `@` / `#` word being typed at the end of the title, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    Person(String),
    Label(String),
}

/// What the end of `title` is asking for: the last word, when it
/// starts with `@` or `#` and the title doesn't end in a space.
#[must_use]
pub fn trigger_at_end(title: &str) -> Option<Trigger> {
    if title.ends_with(char::is_whitespace) {
        return None;
    }
    let word = title.rsplit(char::is_whitespace).next()?;
    match word.strip_prefix('@') {
        Some(q) => Some(Trigger::Person(q.to_owned())),
        None => word.strip_prefix('#').map(|q| Trigger::Label(q.to_owned())),
    }
}

/// `title` without its last word (the trigger just resolved), trimmed
/// of the space before it.
#[must_use]
pub fn drop_last_word(title: &str) -> String {
    match title.rfind(char::is_whitespace) {
        Some(i) => format!("{} ", title[..i].trim_end()),
        None => String::new(),
    }
}

/// The issue the composer's state describes: the title through the
/// quick-add parser, then every chip set by hand on top of it.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn compose(
    title: &str,
    description: &str,
    status: &str,
    priority: Option<&str>,
    assignee: Option<(String, String)>,
    project: Option<(Uuid, String)>,
    labels: &[String],
    due: Option<&str>,
    recurrence: Option<&str>,
    projects: &[(Uuid, String)],
) -> TaskInfo {
    let mut t = task_proto::capture(title);
    if t.title.trim().is_empty() {
        t.title = title.trim().to_owned();
    }
    t.details = description.trim().to_owned();
    t.status = status.to_owned();
    if let Some(p) = priority {
        t.priority = p.to_owned();
    }
    for l in labels {
        if !t.tags.iter().any(|g| g.eq_ignore_ascii_case(l)) {
            t.tags.0.push(l.clone());
        }
    }
    match project {
        Some((id, name)) => {
            t.project_id = Some(id);
            t.projects = task_proto::model::StringList(vec![format!("[[{name}]]")]);
        }
        None => t.project_id = task_proto::infer_project_id(&t.projects.0, projects),
    }
    if let Some(d) = due {
        t.due = Some(d.to_owned());
    }
    if let Some(r) = recurrence {
        t.recurrence = Some(r.to_owned());
        t.recurrence_anchor = Some("scheduled".to_owned());
    }
    with_assignee(&t, assignee)
}

/// What a capture becomes. Nobody picks Note or Task: a capture is a
/// note until it is given something a task has — a person, a date, a
/// priority, a project, a label, a repeat, a subtask — and then it is a
/// task. A project is the one deliberate choice, being more structured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureKind {
    /// A fleeting thought into the inbox, processed later.
    #[default]
    Note,
    /// Something to do — it has a who, a when, a where or a priority.
    Task,
    Project,
}

impl CaptureKind {
    /// What the switch at the top shows it as.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "Capture",
            Self::Task => "Task",
            Self::Project => "Project",
        }
    }
}

/// What the text says it is: `project: …` (or `new project …`) is a
/// project; a line that files or schedules itself — `[[Project]]`,
/// `@context`, a date, a repeat, `!priority` — is a task; anything else
/// is a note.
#[must_use]
// t[impl process.capture.as]
pub fn kind_from_text(text: &str) -> CaptureKind {
    let first = text.trim().lines().next().unwrap_or("").trim();
    let lower = first.to_lowercase();
    if lower.starts_with("project:") || lower.starts_with("new project ") {
        return CaptureKind::Project;
    }
    let t = task_proto::capture(first);
    let set = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
    if !t.projects.is_empty()
        || !t.contexts.is_empty()
        || set(&t.due)
        || set(&t.scheduled)
        || set(&t.recurrence)
        || (t.priority != "normal" && t.priority != "none" && !t.priority.is_empty())
    {
        CaptureKind::Task
    } else {
        CaptureKind::Note
    }
}

/// A project's name from what was typed: the first line, without a
/// leading `project:` / `new project`.
#[must_use]
pub fn project_title(text: &str) -> String {
    let first = text.trim().lines().next().unwrap_or("").trim();
    let lower = first.to_lowercase();
    let rest = if lower.starts_with("project:") {
        &first["project:".len()..]
    } else if lower.starts_with("new project ") {
        &first["new project ".len()..]
    } else {
        first
    };
    rest.trim().to_owned()
}

/// Installed while the composer is open (see [`Composer`]). Idempotent.
const KEEP_KEYBOARD_JS: &str = r#"
if (!window.__composerKeep) {
  window.__composerKeep = true;
  document.addEventListener('focusout', () => {
    setTimeout(() => {
      const t = document.getElementById('composer-title');
      if (!t) return;
      const a = document.activeElement;
      if (!a || a === document.body) t.focus({ preventScroll: true });
    }, 0);
  }, true);
}
"#;

/// The days the Due picker offers, one tap each: `(YYYY-MM-DD, label)`,
/// and "No date" to clear it.
fn due_options() -> Vec<PickOption> {
    use chrono::Datelike as _;
    let today = chrono::Local::now().date_naive();
    let days_to_monday = (7 - today.weekday().num_days_from_monday()) % 7;
    let next_monday = today
        + chrono::Duration::days(i64::from(if days_to_monday == 0 {
            7
        } else {
            days_to_monday
        }));
    let days_to_sat = (5 + 7 - today.weekday().num_days_from_monday()) % 7;
    let weekend = today + chrono::Duration::days(i64::from(days_to_sat));
    let opt = |d: chrono::NaiveDate, label: &str| {
        PickOption::new(
            d.to_string(),
            format!("{label} · {}", d.format("%a %-d %b")),
        )
    };
    vec![
        opt(today, "Today"),
        opt(today + chrono::Duration::days(1), "Tomorrow"),
        opt(weekend, "This weekend"),
        opt(next_monday, "Next week"),
        opt(today + chrono::Duration::days(14), "In two weeks"),
        PickOption::new("", "No date"),
    ]
}

/// The repeats the More menu offers, as RRULE bodies.
const REPEATS: &[(&str, &str)] = &[
    ("Every day", "FREQ=DAILY"),
    ("Every weekday", "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"),
    ("Every week", "FREQ=WEEKLY"),
    ("Every month", "FREQ=MONTHLY"),
];

#[derive(Props, Clone, PartialEq)]
pub struct ComposerProps {
    /// `(user id, name)` — the signed-in person first; they are the
    /// default assignee, as in Linear's "My issues".
    pub people: Vec<(String, String)>,
    #[props(default)]
    pub projects: Vec<(Uuid, String)>,
    #[props(default)]
    pub known_labels: Vec<String>,
    /// Text to start from (a capture being processed, a quick-add line).
    #[props(default)]
    pub initial_title: String,
    /// Where it lands, shown at the top ("ACME Audio").
    #[props(default)]
    pub place: String,
    /// Make it: the issue, then its sub-issue titles.
    pub on_create: EventHandler<(TaskInfo, Vec<String>)>,
    pub on_close: EventHandler<()>,
    /// What it opens as. With `on_note` and `on_project` set, a switch
    /// at the top lets the capture be any of the three.
    #[props(default = CaptureKind::Task)]
    pub kind: CaptureKind,
    /// Write it down: the whole text, into the inbox.
    #[props(default)]
    pub on_note: Option<EventHandler<String>>,
    /// Make a project: `(name, description)`.
    #[props(default)]
    pub on_project: Option<EventHandler<(String, String)>>,
    /// On a note: start a timer with the text as what it is for.
    #[props(default)]
    pub on_timer: Option<EventHandler<String>>,
}

#[component]
pub fn Composer(props: ComposerProps) -> Element {
    let mut title = use_signal(|| props.initial_title.clone());
    let mut description = use_signal(String::new);
    let mut status = use_signal(|| "open".to_owned());
    let mut priority = use_signal(|| None::<String>);
    // A new task starts as yours; a capture starts with nobody, so
    // choosing someone is what makes it a task.
    let me = (props.kind == CaptureKind::Task)
        .then(|| props.people.first().cloned())
        .flatten();
    let mut assignee = use_signal(move || me.clone());
    let mut project = use_signal(|| None::<(Uuid, String)>);
    let mut labels = use_signal(Vec::<String>::new);
    let mut due = use_signal(|| None::<String>);
    let mut recurrence = use_signal(|| None::<String>);
    let mut subs = use_signal(Vec::<String>::new);
    let mut sub_draft = use_signal(String::new);
    let mut adding_sub = use_signal(|| false);
    let mut repeat_open = use_signal(|| false);
    let mut details_open = use_signal(|| false);
    let mut details_focus = use_signal(|| false);
    // Property chips render as rows down the left on a phone.
    use_context_provider(|| super::properties::ChipRows);
    let mut due_open = use_signal(|| false);
    let mut more_open = use_signal(|| false);
    let mut show_labels = use_signal(|| false);
    let mut labels_key = use_signal(|| None::<Hotkey>);
    let mut link_draft = use_signal(|| None::<String>);
    let mut create_more = use_signal(|| false);
    let mut cursor = use_signal(|| 0usize);
    // What it becomes: picked by hand, else read from the text while it
    // opened as a note (a plain thought stays a note).
    let opened_as = props.kind;
    // A project typed into the Project picker that does not exist yet —
    // made along with the task. (There is no Capture | Project switch:
    // a new project comes from here, or from the project list.)
    let mut new_project = use_signal(|| None::<String>);
    let switchable = props.on_note.is_some();
    // Given anything a task has, a capture is a task.
    let given = status() != "open"
        || priority().is_some()
        || assignee().is_some()
        || project().is_some()
        || new_project().is_some()
        || !labels().is_empty()
        || due().is_some()
        || recurrence().is_some()
        || !subs().is_empty();
    let text_kind = kind_from_text(&title.read());
    let tasky = opened_as == CaptureKind::Task || given || text_kind == CaptureKind::Task;
    let kind = if !switchable {
        opened_as
    } else if tasky {
        CaptureKind::Task
    } else {
        CaptureKind::Note
    };
    // Yours unless you say otherwise — once it is a task.
    let default_assignee = props.people.first().cloned();
    let default_assignee_chip = default_assignee.clone();

    // What the title reads as right now (for the parsed chips).
    let parsed = task_proto::capture(&title.read());
    let trigger = trigger_at_end(&title.read());
    let people_opts: Vec<PickOption> = props
        .people
        .iter()
        .map(|(id, n)| {
            PickOption::new(id.clone(), n.clone())
                .icon(rsx! { PersonDot { name: n.clone(), size: 16 } })
        })
        .collect();
    let label_opts: Vec<PickOption> = props
        .known_labels
        .iter()
        .map(|l| PickOption::new(l.clone(), l.clone()).dot(label_color(l)))
        .collect();
    let suggestions: Vec<PickOption> = match &trigger {
        Some(Trigger::Person(q)) => filtered(&people_opts, q)
            .into_iter()
            .map(|(_, o)| o)
            .take(6)
            .collect(),
        Some(Trigger::Label(q)) => {
            let mut hits: Vec<PickOption> = filtered(&label_opts, q)
                .into_iter()
                .map(|(_, o)| o)
                .take(6)
                .collect();
            if !q.is_empty() && !props.known_labels.iter().any(|l| l.eq_ignore_ascii_case(q)) {
                hits.push(
                    PickOption::new(format!("+{q}"), format!("Create “{q}”")).dot(label_color(q)),
                );
            }
            hits
        }
        None => Vec::new(),
    };
    let at = (*cursor.read()).min(suggestions.len().saturating_sub(1));

    let people_c = props.people.clone();
    let resolve = move |opt: PickOption| {
        match trigger_at_end(&title.peek()) {
            Some(Trigger::Person(_)) => {
                let who = people_c.iter().find(|(id, _)| *id == opt.id).cloned();
                assignee.set(who);
            }
            Some(Trigger::Label(_)) => {
                let name = opt.id.strip_prefix('+').unwrap_or(&opt.id).to_owned();
                let mut l = labels();
                if !l.iter().any(|x| x.eq_ignore_ascii_case(&name)) {
                    l.push(name);
                }
                labels.set(l);
            }
            None => {}
        }
        let next = drop_last_word(&title.peek());
        title.set(next);
        cursor.set(0);
    };

    let projects = props.projects.clone();
    let create = {
        let projects = projects.clone();
        move || {
            let t = title.peek().trim().to_owned();
            if t.is_empty() {
                return;
            }
            match kind {
                CaptureKind::Note => {
                    if let Some(on_note) = props.on_note {
                        let d = description.peek().trim().to_owned();
                        on_note.call(if d.is_empty() {
                            t
                        } else {
                            format!("{t}\n\n{d}")
                        });
                    }
                    props.on_close.call(());
                    return;
                }
                CaptureKind::Project => {
                    if let Some(on_project) = props.on_project {
                        on_project.call((project_title(&t), description.peek().trim().to_owned()));
                    }
                    props.on_close.call(());
                    return;
                }
                CaptureKind::Task => {}
            }
            let mut task = compose(
                &t,
                &description.peek(),
                &status.peek(),
                priority.peek().as_deref(),
                assignee.peek().clone().or_else(|| default_assignee.clone()),
                project.peek().clone(),
                &labels.peek(),
                due.peek().as_deref(),
                recurrence.peek().as_deref(),
                &projects,
            );
            // A project named in the picker that does not exist yet is
            // made now, and the task filed under it by name.
            if let (None, Some(name)) = (project.peek().clone(), new_project.peek().clone()) {
                if let Some(on_project) = props.on_project {
                    on_project.call((name.clone(), String::new()));
                }
                task.project_id = None;
                task.projects = task_proto::model::StringList(vec![format!("[[{name}]]")]);
            }
            props.on_create.call((task, subs.peek().clone()));
            if *create_more.peek() {
                title.set(String::new());
                description.set(String::new());
                subs.set(Vec::new());
            } else {
                props.on_close.call(());
            }
        }
    };
    let mut create_key = create.clone();
    let mut create_btn = create.clone();
    let mut create_enter = create.clone();
    let (title_hint, detail_hint, action_label) = match kind {
        CaptureKind::Note => (
            "Get it out of your head…",
            "More, if there is more…",
            "Capture",
        ),
        CaptureKind::Task => ("Task title", "Add description…", "Create task"),
        CaptureKind::Project => ("Project name", "What it's for…", "Create project"),
    };
    let mut resolve_key = resolve.clone();
    let suggestions_key = suggestions.clone();

    // The chips the title's text produced, shown until a hand-set chip
    // replaces them.
    let parsed_due = due().is_none().then(|| parsed.due.clone()).flatten();
    let parsed_project = project()
        .is_none()
        .then(|| parsed.projects.first().cloned())
        .flatten()
        .map(|p| p.trim_start_matches("[[").trim_end_matches("]]").to_owned());
    let parsed_repeat = recurrence()
        .is_none()
        .then(|| parsed.recurrence.clone())
        .flatten();
    // Labels chosen by hand, then any the title's `#tags` add.
    let chosen_labels: Vec<String> = {
        let mut l = labels();
        for t in parsed.tags.iter().filter(|t| *t != "task") {
            if !l.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                l.push(t.clone());
            }
        }
        l
    };
    let parsed_priority = priority()
        .is_none()
        .then(|| Some(parsed.priority.clone()).filter(|p| p != "normal" && p != "none"))
        .flatten();

    // Keep the keyboard up while the properties are used: a row tapped
    // or a picker closed leaves focus on nothing, which drops the
    // keyboard — so focus goes back to the title, unless it went to
    // another field (a picker's search, the details).
    use_effect(|| {
        let _ = document::eval(KEEP_KEYBOARD_JS);
    });

    rsx! {
        div {
            // A phone: the visible viewport — the space above the keyboard —
            // so the chips and the create row ride on top of it (`--vv-*`,
            // kept by the shell) instead of under it.
            class: "fixed inset-x-0 top-[var(--vv-top,0px)] z-50 flex h-[var(--vv-h,100dvh)] justify-center bg-popover md:inset-0 md:h-auto md:items-start md:bg-black/50 md:p-6 md:pt-[10vh]",
            onclick: move |_| props.on_close.call(()),
            div {
                class: "flex h-full w-full flex-col overflow-hidden bg-popover md:h-auto md:max-h-[80vh] md:max-w-2xl md:rounded-xl md:border md:border-border md:bg-popover md:shadow-2xl",
                onclick: move |e| e.stop_propagation(),
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Enter && (e.modifiers().meta() || e.modifiers().ctrl()) {
                        e.prevent_default();
                        create_key();
                    } else if e.key() == Key::Escape {
                        e.prevent_default();
                        props.on_close.call(());
                    }
                },
                // ── where ──
                header { class: "flex items-center gap-2 px-4 pt-[max(0.75rem,env(safe-area-inset-top))] pb-1 text-[13px] text-muted-foreground md:pt-3",
                    if !props.place.is_empty() {
                        span { class: "rounded-md bg-muted/60 px-1.5 py-0.5 font-medium text-foreground/80", "{props.place}" }
                        span { "›" }
                    }
                    // What it is right now: a capture, until it is given
                    // something a task has — then a task. Nothing to pick.
                    span {
                        "data-testid": "composer-kind",
                        class: if kind == CaptureKind::Task { "rounded-full bg-primary/15 px-2.5 py-0.5 text-[13px] font-semibold text-primary" } else { "rounded-full bg-foreground/[0.07] px-2.5 py-0.5 text-[13px] font-semibold text-foreground/80" },
                        match kind {
                            CaptureKind::Task if switchable => "Task",
                            CaptureKind::Task => "New task",
                            CaptureKind::Project => "New project",
                            CaptureKind::Note => "Capture",
                        }
                    }
                    button {
                        r#type: "button",
                        class: "ml-auto flex size-9 items-center justify-center rounded-md hover:bg-accent hover:text-foreground md:size-7",
                        aria_label: "Close",
                        onclick: move |_| props.on_close.call(()),
                        X { size: 16 }
                    }
                }
                div { class: "flex min-h-0 flex-1 flex-col overflow-y-auto px-4 pb-2",
                    // ── title, read as you type ──
                    div { class: "relative",
                        textarea {
                            aria_label: "{title_hint}",
                            id: "composer-title",
                            rows: "1",
                            class: "w-full resize-none bg-transparent pt-2 text-[20px] font-semibold leading-7 tracking-[-0.1px] text-foreground outline-none [field-sizing:content] placeholder:text-muted-foreground md:text-[18px]",
                            placeholder: "{title_hint}",
                            value: "{title}",
                            autofocus: true,
                            onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                            oninput: move |e| {
                                title.set(e.value());
                                cursor.set(0);
                            },
                            onkeydown: move |e: KeyboardEvent| {
                                if suggestions_key.is_empty() {
                                    if e.key() == Key::Enter && !e.modifiers().meta() && !e.modifiers().ctrl() {
                                        // The title is one line. On a quick
                                        // note, Enter is done — write it down.
                                        e.prevent_default();
                                        if kind == CaptureKind::Note && !e.modifiers().shift() {
                                            create_enter();
                                        }
                                    }
                                    return;
                                }
                                let n = suggestions_key.len();
                                match e.key() {
                                    Key::ArrowDown => { e.prevent_default(); cursor.set((at + 1) % n); }
                                    Key::ArrowUp => { e.prevent_default(); cursor.set((at + n - 1) % n); }
                                    Key::Enter | Key::Tab => {
                                        e.prevent_default();
                                        if let Some(o) = suggestions_key.get(at).cloned() {
                                            resolve_key(o);
                                        }
                                    }
                                    Key::Escape => {
                                        e.prevent_default();
                                        e.stop_propagation();
                                        title.set(format!("{} ", title.peek()));
                                    }
                                    _ => {}
                                }
                            },
                        }
                        if !suggestions.is_empty() {
                            div { class: "absolute inset-x-0 top-full z-20 mt-1 flex flex-col rounded-lg border border-border bg-popover p-1 shadow-xl md:max-w-xs",
                                for (i, o) in suggestions.iter().cloned().enumerate() {
                                    {
                                        let mut resolve = resolve.clone();
                                        let pick = o.clone();
                                        rsx! {
                                            button {
                                                key: "{o.id}",
                                                r#type: "button",
                                                class: if i == at { "flex min-h-10 items-center gap-2.5 rounded-md bg-accent px-2.5 text-left text-[14px] md:min-h-8 md:text-[13px]" } else { "flex min-h-10 items-center gap-2.5 rounded-md px-2.5 text-left text-[14px] md:min-h-8 md:text-[13px]" },
                                                onmouseenter: move |_| cursor.set(i),
                                                onclick: move |_| resolve(pick.clone()),
                                                if let Some(icon) = o.icon.clone() { {icon} }
                                                if let Some(c) = o.dot.as_ref() { span { class: "size-2.5 rounded-full", style: "background: {c};" } }
                                                span { class: "truncate", "{o.label}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if details_open() || !description.read().is_empty() {
                    textarea {
                        aria_label: "Description",
                        onmounted: move |e| async move { if *details_focus.peek() { let _ = e.set_focus(true).await; } },
                        class: "mt-1 min-h-20 w-full resize-none bg-transparent text-[15px] font-[450] leading-6 tracking-[-0.1px] text-foreground/90 outline-none [field-sizing:content] placeholder:text-muted-foreground",
                        placeholder: "{detail_hint}",
                        value: "{description}",
                        oninput: move |e| description.set(e.value()),
                    }
                    } else {
                        button {
                            r#type: "button",
                            "data-testid": "composer-details",
                            class: "mt-1 flex items-center gap-1.5 self-start rounded-full px-0.5 py-1 text-[15px] text-muted-foreground active:text-foreground",
                            onclick: move |_| { details_focus.set(true); details_open.set(true); },
                            Plus { size: 15 }
                            "Add details"
                        }
                    }

                    // ── sub-issues ──
                    if !subs.read().is_empty() || adding_sub() {
                        div { class: "mt-2 flex flex-col rounded-lg border border-border/70 p-2",
                            span { class: "px-1 pb-1 text-[12px] font-medium text-muted-foreground", "Subtasks {subs.read().len()}" }
                            for (i, s) in subs.read().iter().cloned().enumerate() {
                                div { key: "{i}", class: "flex min-h-9 items-center gap-2 px-1 text-[14px]",
                                    super::properties::StatusIcon { status: "open".to_owned() }
                                    span { class: "min-w-0 flex-1 truncate", "{s}" }
                                    button {
                                        r#type: "button",
                                        class: "flex size-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent",
                                        aria_label: "Remove subtask",
                                        onclick: move |_| { let mut v = subs(); v.remove(i); subs.set(v); },
                                        X { size: 13 }
                                    }
                                }
                            }
                            if adding_sub() {
                                input {
                                    r#type: "text",
                                    class: "min-h-9 bg-transparent px-1 text-[14px] outline-none placeholder:text-muted-foreground",
                                    placeholder: "Subtask — Enter to add",
                                    value: "{sub_draft}",
                                    onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                                    oninput: move |e| sub_draft.set(e.value()),
                                    onkeydown: move |e: KeyboardEvent| match e.key() {
                                        Key::Enter if !e.modifiers().meta() => {
                                            e.prevent_default();
                                            let t = sub_draft.peek().trim().to_owned();
                                            if !t.is_empty() {
                                                let mut v = subs();
                                                v.push(t);
                                                subs.set(v);
                                                sub_draft.set(String::new());
                                            }
                                        }
                                        Key::Escape => { e.prevent_default(); e.stop_propagation(); adding_sub.set(false); }
                                        _ => {}
                                    },
                                }
                            }
                        }
                    }
                    if let Some(l) = link_draft() {
                        input {
                            r#type: "url",
                            class: "mt-2 min-h-9 rounded-md border border-border bg-transparent px-2 text-[14px] outline-none",
                            placeholder: "Paste a link — Enter to add",
                            value: "{l}",
                            onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                            oninput: move |e| link_draft.set(Some(e.value())),
                            onkeydown: move |e: KeyboardEvent| {
                                if e.key() == Key::Enter {
                                    e.prevent_default();
                                    let url = link_draft.peek().clone().unwrap_or_default();
                                    if !url.trim().is_empty() {
                                        let d = description.peek().clone();
                                        description.set(if d.trim().is_empty() { url.trim().to_owned() } else { format!("{}\n\n{}", d.trim_end(), url.trim()) });
                                    }
                                    link_draft.set(None);
                                } else if e.key() == Key::Escape {
                                    e.prevent_default();
                                    e.stop_propagation();
                                    link_draft.set(None);
                                }
                            },
                        }
                    }
                }

                // ── the chips ──
                // Rides on top of the keyboard; each chip opens its picker
                // above it. Wrapped, not a sideways scroller: iOS clips a
                // fixed sheet to a scrolling parent, which hid the pickers.
                div { class: if kind != CaptureKind::Project { "flex flex-col items-stretch gap-0.5 px-2 pb-1 pt-1 md:flex-row md:flex-wrap md:items-center md:gap-1.5 md:border-t md:border-border/60 md:px-4 md:py-2.5" } else { "hidden" },
                    StatusChip { status: status(), on_change: move |s: String| status.set(s) }
                    PriorityChip {
                        priority: priority().or(parsed_priority.clone()).unwrap_or_else(|| "none".to_owned()),
                        on_change: move |p: String| priority.set(Some(p)),
                    }
                    AssigneeChip {
                        // Once it is a task, it shows who it goes to: you,
                        // unless someone else was chosen.
                        assignee: assignee()
                            .or_else(|| (kind == CaptureKind::Task).then(|| default_assignee_chip.clone()).flatten())
                            .map(|(id, _)| id),
                        people: props.people.clone(),
                        on_change: {
                            let people = props.people.clone();
                            move |id: Option<String>| assignee.set(id.and_then(|id| people.iter().find(|(p, _)| *p == id).cloned()))
                        },
                    }
                    ProjectChip {
                        project: project().map(|(id, _)| id).or_else(|| {
                            parsed_project.as_ref().and_then(|n| projects.iter().find(|(_, t)| t.eq_ignore_ascii_case(n)).map(|(id, _)| *id))
                        }),
                        projects: projects.clone(),
                        on_change: {
                            let projects = projects.clone();
                            move |id: Option<Uuid>| project.set(id.and_then(|id| projects.iter().find(|(p, _)| *p == id).cloned()))
                        },
                        // Type a name no project has and press Enter: it is made
                        // with the task.
                        on_create: props.on_project.map(|_| EventHandler::new(move |name: String| {
                            project.set(None);
                            new_project.set(Some(name));
                        })),
                        pending: new_project(),
                    }
                    // The rest only once set — chosen from More.
                    if !chosen_labels.is_empty() || show_labels() {
                        LabelsChip {
                            chosen: chosen_labels.clone(),
                            known: props.known_labels.clone(),
                            hotkey: Some(labels_key),
                            on_toggle: move |name: String| {
                                let mut l = labels();
                                if let Some(i) = l.iter().position(|x| x.eq_ignore_ascii_case(&name)) { l.remove(i); } else { l.push(name); }
                                labels.set(l);
                            },
                        }
                    }
                    if let Some(d) = due().or(parsed_due.clone()) {
                        button {
                            r#type: "button",
                            class: CHIP_ROW_CLASS,
                            title: "Due date",
                            onclick: move |_| due_open.set(true),
                            span { class: "text-muted-foreground", CalendarDays { size: 14 } }
                            {d.get(..10).and_then(|s| s.parse::<chrono::NaiveDate>().ok()).map_or(d.clone(), |x| x.format("%b %-d").to_string())}
                        }
                    }
                    if let Some(r) = recurrence().or(parsed_repeat.clone()) {
                        button {
                            r#type: "button",
                            class: CHIP_ROW_CLASS,
                            title: "Repeat",
                            onclick: move |_| repeat_open.set(true),
                            span { class: "text-muted-foreground", Repeat { size: 14 } }
                            {super::time::recurrence_summary(&r)}
                        }
                    }
                    // More: labels, a date, a repeat, a subtask, a link.
                    div { class: "relative",
                        button {
                            r#type: "button",
                            "data-testid": "composer-more",
                            class: CHIP_ROW_QUIET_CLASS,
                            aria_label: "More properties",
                            onclick: move |_| more_open.set(true),
                            Plus { size: 14 }
                            "More"
                        }
                        PropertyPicker {
                            open: more_open(),
                            title: "Add…",
                            options: vec![
                                PickOption::new("labels", "Labels").icon(rsx! { Tag { size: 15 } }),
                                PickOption::new("due", "Due date").icon(rsx! { CalendarDays { size: 15 } }),
                                PickOption::new("repeat", "Repeat").icon(rsx! { Repeat { size: 15 } }),
                                PickOption::new("subtask", "Subtask").icon(rsx! { ListTree { size: 15 } }),
                                PickOption::new("link", "Link").icon(rsx! { Link2 { size: 15 } }),
                            ],
                            number_from: None,
                            on_pick: move |what: String| match what.as_str() {
                                "labels" => {
                                    show_labels.set(true);
                                    labels_key.set(Some(Hotkey::Labels));
                                }
                                "due" => due_open.set(true),
                                "repeat" => repeat_open.set(true),
                                "subtask" => adding_sub.set(true),
                                "link" => link_draft.set(Some(String::new())),
                                _ => {}
                            },
                            on_close: move |()| more_open.set(false),
                        }
                        // When: the common days, one tap each.
                        PropertyPicker {
                            open: due_open(),
                            title: "Due…",
                            options: due_options(),
                            selected: vec![due().unwrap_or_default()],
                            number_from: None,
                            on_pick: move |d: String| due.set(Some(d).filter(|d| !d.is_empty())),
                            on_close: move |()| due_open.set(false),
                        }
                        PropertyPicker {
                            open: repeat_open(),
                            title: "Repeat…",
                            options: std::iter::once(PickOption::new("", "Doesn't repeat"))
                                .chain(REPEATS.iter().map(|(label, rule)| PickOption::new(*rule, *label)))
                                .collect::<Vec<_>>(),
                            selected: vec![recurrence().unwrap_or_default()],
                            number_from: None,
                            on_pick: move |rule: String| recurrence.set(Some(rule).filter(|r| !r.is_empty())),
                            on_close: move |()| repeat_open.set(false),
                        }
                    }
                }

                // Start a timer instead — the swipe-up on the capture button, for
                // whoever has not found it yet.
                if let (CaptureKind::Note, Some(on_timer)) = (kind, props.on_timer) {
                    div { class: "px-2 pb-1",
                        button {
                            r#type: "button",
                            "data-testid": "composer-timer",
                            class: CHIP_ROW_QUIET_CLASS,
                            title: "Start a timer with this as its description",
                            onclick: move |_| {
                                on_timer.call(title.peek().trim().to_owned());
                                props.on_close.call(());
                            },
                            span { Clock { size: 16 } }
                            "Start a timer"
                        }
                    }
                }
                // ── create ──
                footer { class: "flex items-center justify-end gap-3 border-t border-border/60 px-4 py-2.5 pb-[calc(0.625rem+env(safe-area-inset-bottom))] md:pb-2.5 [.kb-open_&]:pb-2.5",
                    label { class: if kind == CaptureKind::Task { "ml-auto flex items-center gap-2 text-[13px] text-muted-foreground" } else { "hidden" },
                        input {
                            r#type: "checkbox",
                            checked: create_more(),
                            onchange: move |e| create_more.set(e.checked()),
                        }
                        "Create more"
                    }
                    button {
                        r#type: "button",
                        class: "flex h-11 flex-1 items-center justify-center rounded-xl bg-primary px-4 text-[16px] font-semibold text-primary-foreground disabled:opacity-50 md:h-8 md:flex-none md:rounded-lg md:text-[13px]",
                        disabled: title.read().trim().is_empty(),
                        onclick: move |_| create_btn(),
                        "{action_label}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_word_says_what_it_wants() {
        assert_eq!(
            trigger_at_end("Bounce stems @co"),
            Some(Trigger::Person("co".into()))
        );
        assert_eq!(
            trigger_at_end("Bounce stems #mix"),
            Some(Trigger::Label("mix".into()))
        );
        assert_eq!(
            trigger_at_end("Bounce stems @"),
            Some(Trigger::Person(String::new()))
        );
        assert_eq!(
            trigger_at_end("Bounce stems @cody "),
            None,
            "a space closes it"
        );
        assert_eq!(
            trigger_at_end("email@example.com"),
            None,
            "only at a word's start"
        );
        assert_eq!(drop_last_word("Bounce stems @co"), "Bounce stems ");
        assert_eq!(drop_last_word("@co"), "");
    }

    #[test]
    // t[verify process.capture.as]
    fn a_capture_reads_as_a_note_a_task_or_a_project() {
        assert_eq!(
            kind_from_text("Idea: a stems-only bonus edition"),
            CaptureKind::Note
        );
        assert_eq!(
            kind_from_text("Pick up gaff tape @errands"),
            CaptureKind::Task
        );
        assert_eq!(
            kind_from_text("Bounce stems [[Crescendum Album]]"),
            CaptureKind::Task
        );
        assert_eq!(kind_from_text("Call the venue tomorrow"), CaptureKind::Task);
        assert_eq!(
            kind_from_text("project: Live session EP"),
            CaptureKind::Project
        );
        assert_eq!(
            kind_from_text("New project Remix pack"),
            CaptureKind::Project
        );
        assert_eq!(
            project_title("project:  Live session EP\nfour songs"),
            "Live session EP"
        );
        assert_eq!(project_title("Remix pack"), "Remix pack");
    }

    #[test]
    fn chips_set_by_hand_win_over_the_title_and_the_title_fills_the_rest() {
        let pid = Uuid::new_v4();
        let projects = vec![(pid, "Crescendum Album".to_owned())];
        let t = compose(
            "Bounce stems for the client [[Crescendum Album]] tomorrow !high",
            "",
            "open",
            Some("critical"),
            Some(("u1".into(), "Cody".into())),
            None,
            &["mixing".to_owned()],
            None,
            None,
            &projects,
        );
        assert_eq!(t.title, "Bounce stems for the client");
        assert_eq!(t.priority, "critical", "the hand-set chip");
        assert_eq!(t.project_id, Some(pid), "from [[...]]");
        assert!(t.due.is_some(), "from 'tomorrow'");
        assert!(t.tags.iter().any(|g| g == "mixing"));
        assert_eq!(
            super::super::issue_view::assignee_of(&t).as_deref(),
            Some("u1")
        );
    }
}
