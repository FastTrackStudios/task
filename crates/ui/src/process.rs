//! Processing — turning one captured thing into what it actually is.
//!
//! Capture is the cheap half: a line lands in the inbox
//! (`Records/inbox/<id>.md`) with no questions asked. Processing is the
//! half that makes the system trustworthy, and it is the centre of the
//! app: every capture comes back here once, and leaves as a task with
//! somewhere to live, a note, a later, or nothing.
//!
//! [`ProcessCard`] is that one decision, used by the inbox deck and by
//! quick-add (which captures, then processes on the spot). It works in
//! two [`Mode`]s, the person's choice and remembered per device:
//!
//! - **Fast** — the card and its outcomes side by side: make a task,
//!   note, later, done, delete. For an inbox you trust yourself with.
//! - **Clarify** — the questions in order: *is it actionable?* If not:
//!   note, someday, or delete. If so: *do it now* (two minutes, clock
//!   started), *waiting on someone*, *make it a task*, or *it's a
//!   project*.
//!
//! Either way, "make it a task" opens [`TaskShape`]: the capture parsed
//! the way quick-add parses (`@errands`, `#chore`, `[[Project]]`,
//! `tomorrow`, `!high`) into an editable title, project, contexts, due
//! date and estimate — so a task leaves the inbox already filed, and a
//! maintenance hour can find it.

use architect_ui::lucide_dioxus::{
    Archive, ArrowRight, Check, Clock, FileText, FolderKanban, Hourglass, ListTodo, Play, Trash2, X,
};
use chrono::{Datelike, Duration, Local, Utc};
use dioxus::prelude::*;
use inbox_proto::{InboxItem, ReviewResponse, review};
use uuid::Uuid;

use crate::stores;

// ── mode ────────────────────────────────────────────────────────────

/// How much the card asks before it acts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// t[impl process.mode.fast]
// t[impl process.mode.clarify]
pub enum Mode {
    /// Every outcome on the card at once.
    Fast,
    /// The GTD questions, one at a time.
    Clarify,
}

const MODE_KEY: &str = "task.process.mode";

impl Mode {
    /// This device's choice; Fast until one is made.
    #[must_use]
    pub fn stored() -> Self {
        match crate::local_store::get(MODE_KEY).as_deref() {
            Some("clarify") => Self::Clarify,
            _ => Self::Fast,
        }
    }

    pub fn remember(self) {
        crate::local_store::set(
            MODE_KEY,
            match self {
                Self::Fast => "fast",
                Self::Clarify => "clarify",
            },
        );
    }
}

// ── drafting ────────────────────────────────────────────────────────

/// The task a capture reads as: its first line through the quick-add
/// parser (`task_proto::capture` — tags, contexts, `[[project]]`,
/// dates, priority, recurrence), the rest as details, and the project
/// link resolved against `projects` when the line named one.
#[must_use]
pub fn draft_task(body: &str, projects: &[(Uuid, String)]) -> task_proto::TaskInfo {
    let trimmed = body.trim();
    let (first, rest) = trimmed.split_once('\n').unwrap_or((trimmed, ""));
    let mut task = task_proto::capture(first);
    if task.title.trim().is_empty() {
        task.title = first.trim().chars().take(120).collect();
    }
    if task.title.trim().is_empty() {
        "Untitled".clone_into(&mut task.title);
    }
    task.details = rest.trim().to_owned();
    task.project_id = task_proto::infer_project_id(&task.projects.0, projects);
    task
}

/// Context names offered as one-tap chips: the person's own (from the
/// tasks they have), then the ones the time blocks look for, without
/// repeats, each with its `@`.
#[must_use]
pub fn context_suggestions(existing: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let c = format!(
            "@{}",
            raw.trim().trim_start_matches('@').to_ascii_lowercase()
        );
        if c.len() > 1 && !out.contains(&c) {
            out.push(c);
        }
    };
    for c in existing {
        push(c);
    }
    for c in ["errands", "chores", "home", "calls", "computer", "out"] {
        push(c);
    }
    out
}

/// What processing an item did — the deck uses it to say so, and to
/// offer Undo where undoing is honest (the item comes back as it was).
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Became a task (or a project) — something new exists elsewhere.
    Filed(String),
    /// Became a note.
    Noted,
    /// Put back to resurface later; `before` restores it.
    Later { before: InboxItem, label: String },
    /// Marked handled; `before` restores it.
    Done { before: InboxItem },
    /// Deleted; `before` recreates it.
    Deleted { before: InboxItem },
    /// Left for another pass.
    Skipped,
}

/// How many inbox items are waiting to be processed today — the deck's
/// own rule ([`crate::pages::inbox::is_due`]), so the tab badge, Home's
/// card and the deck never disagree.
pub fn use_to_process_count() -> usize {
    let rows = stores::use_inbox_list();
    let today = Utc::now().date_naive().to_string();
    rows.value().map_or(0, |rows| {
        rows.iter()
            .filter(|(_, it)| crate::pages::inbox::is_due(it, &today))
            .count()
    })
}

// ── process now (quick-add) ─────────────────────────────────────────

/// A capture to process on the spot — quick-add's half of "one
/// capture". The item is held here, NOT yet in the inbox: the first
/// decision writes it once (already processed, or put off), so a
/// capture-then-process can't race two writes to the same file. Closing
/// without deciding is what puts it in the inbox, open — nothing typed
/// is ever lost.
#[derive(Clone, Copy)]
// t[impl process.capture.single-path]
// t[impl process.capture.process-now]
pub struct ProcessNow(pub Signal<Option<(String, InboxItem)>>);

impl ProcessNow {
    /// Capture `text` into `slug` and open processing on it.
    pub fn start(mut self, slug: String, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let item = InboxItem::capture(
            Uuid::new_v4().to_string(),
            text.to_owned(),
            "ui",
            Utc::now().to_rfc3339(),
        );
        self.0.set(Some((slug, item)));
    }
}

/// Install the [`ProcessNow`] context. Call once in the app shell.
pub fn provide_process_now() {
    use_context_provider(|| ProcessNow(Signal::new(None)));
}

/// The process-now sheet: full screen on a phone, a centred card on a
/// desk. Mounted once in the shell; shows while [`ProcessNow`] holds an
/// item.
#[component]
pub fn ProcessNowOverlay() -> Element {
    let mut now = use_context::<ProcessNow>().0;
    let muts = stores::use_inbox_mutations();
    let toast = dioxus_primitives::toast::use_toast();
    let mode = use_signal(Mode::stored);
    let Some((slug, item)) = now() else {
        return rsx! {};
    };
    // Closing undecided files it in the inbox, open.
    let mut keep = {
        let slug = slug.clone();
        let item = item.clone();
        move || {
            muts.capture(slug.clone(), item.clone());
            toast.info(
                "In your inbox".to_owned(),
                dioxus_primitives::toast::ToastOptions::new(),
            );
            now.set(None);
        }
    };
    let mut keep_bg = keep.clone();
    rsx! {
        div {
            class: "fixed inset-0 z-50 flex justify-center bg-background md:items-start md:bg-black/50 md:p-6 md:pt-[8vh]",
            onclick: move |_| keep_bg(),
            div {
                class: "flex h-full w-full flex-col overflow-hidden bg-background md:h-auto md:max-h-[84vh] md:max-w-xl md:rounded-2xl md:border md:border-border md:shadow-2xl",
                onclick: move |e| e.stop_propagation(),
                header { class: "flex items-center gap-3 px-4 pt-[max(0.75rem,env(safe-area-inset-top))] pb-2",
                    button {
                        r#type: "button",
                        class: "flex h-10 w-10 shrink-0 items-center justify-center rounded-lg text-muted-foreground hover:bg-accent hover:text-foreground",
                        aria_label: "Keep in inbox",
                        title: "Keep in inbox",
                        onclick: move |_| keep(),
                        X { size: 18 }
                    }
                    span { class: "flex-1 text-sm font-semibold text-foreground", "Process" }
                    crate::pages::inbox::ModeSwitch { mode }
                }
                div { class: "min-h-0 flex-1 overflow-y-auto px-4 pb-[calc(1.5rem+env(safe-area-inset-bottom))] md:pb-6",
                    ProcessCard {
                        key: "{item.id}",
                        item,
                        slug,
                        mode,
                        stored: false,
                        on_done: move |outcome: Outcome| {
                            match outcome {
                                Outcome::Filed(msg) => toast.success(msg, dioxus_primitives::toast::ToastOptions::new()),
                                Outcome::Noted => toast.success("Saved as a note".to_owned(), dioxus_primitives::toast::ToastOptions::new()),
                                Outcome::Later { label, .. } => toast.info(label, dioxus_primitives::toast::ToastOptions::new()),
                                Outcome::Done { .. } | Outcome::Deleted { .. } | Outcome::Skipped => {}
                            }
                            now.set(None);
                        },
                    }
                }
            }
        }
    }
}

// ── the card ────────────────────────────────────────────────────────

/// Where the card is in its questions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Fast: all outcomes. Clarify: "is it actionable?".
    Decide,
    /// Clarify, actionable: what kind of next step.
    Actionable,
    /// Clarify, not actionable: note, someday, or gone.
    NotActionable,
    /// Choosing when it comes back.
    Later,
    /// Shaping the task.
    Shape(Shape),
}

/// What kind of task the shaping form makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Task,
    Waiting,
}

/// One inbox item, processed. `on_done` fires once with what happened;
/// the card does every write itself, through the shared stores.
#[component]
pub fn ProcessCard(
    item: InboxItem,
    slug: String,
    mode: Signal<Mode>,
    on_done: EventHandler<Outcome>,
    /// Offer "Skip" (the deck does; a single fresh capture doesn't).
    #[props(default)]
    skippable: bool,
    /// Whether the item is in the inbox yet. A fresh capture from
    /// quick-add isn't ([`ProcessNow`]): deleting it writes nothing.
    #[props(default = true)]
    stored: bool,
) -> Element {
    let muts = stores::use_inbox_mutations();
    // Callers key the card by item id, so each item starts here.
    let mut step = use_signal(|| Step::Decide);

    let suggested = item.status == InboxItem::STATUS_SUGGESTED;
    let kind = item.kind.clone();
    let source = item.source.clone();
    let date = item.created.get(..10).unwrap_or(&item.created).to_string();
    let body = item.body.clone();

    // Every terminal decision, in one place.
    let decide = {
        let item = item.clone();
        let slug = slug.clone();
        move |outcome: Decision| {
            let item = item.clone();
            let slug = slug.clone();
            match outcome {
                // t[impl process.outcome.note]
                Decision::Note => {
                    let (title, _) = split_title_body(&item.body);
                    let path = format!(
                        "Wiki/Atomic/{}-{}.md",
                        slugify(&title),
                        item.id.get(..6).unwrap_or("note")
                    );
                    let md = atomic_markdown(&title, &item.body, &Utc::now().to_rfc3339());
                    muts.promote_to_note(slug, item, path, md);
                    on_done.call(Outcome::Noted);
                }
                // t[impl process.outcome.later]
                Decision::Later(resp) => {
                    let before = item.clone();
                    let (interval, ease, resurface_on, reviews) =
                        review(&item, resp, Utc::now().date_naive());
                    let label = format!("Back in {}", fmt_interval(interval));
                    let mut next = item;
                    // A suggestion put off is still accepted into the inbox.
                    next.status = InboxItem::STATUS_OPEN.to_string();
                    next.interval = interval;
                    next.ease = ease;
                    next.resurface_on = Some(resurface_on);
                    next.reviews = reviews;
                    muts.save(slug, next);
                    on_done.call(Outcome::Later { before, label });
                }
                // t[impl process.outcome.done]
                Decision::Done => {
                    let before = item.clone();
                    let mut done = item;
                    done.status = InboxItem::STATUS_PROCESSED.to_string();
                    muts.save(slug, done);
                    on_done.call(Outcome::Done { before });
                }
                // t[impl process.outcome.delete]
                Decision::Delete => {
                    let before = item.clone();
                    if stored {
                        muts.delete(slug, item.id);
                    }
                    on_done.call(Outcome::Deleted { before });
                }
                // t[impl process.outcome.now]
                Decision::DoNow => {
                    let mut task = draft_task(&item.body, &[]);
                    task.status = "in-progress".into();
                    task.time_entries =
                        task_proto::model::TimeEntries(vec![task_proto::model::TimeEntry {
                            start_time: Utc::now(),
                            end_time: None,
                        }]);
                    let title = task.title.clone();
                    muts.process_into_task(slug, item, task);
                    on_done.call(Outcome::Filed(format!("Started: {title}")));
                }
                // t[impl process.outcome.project]
                Decision::Project => {
                    let (title, _) = split_title_body(&item.body);
                    muts.process_into_project(slug, item, stores::draft_project(title.clone()));
                    on_done.call(Outcome::Filed(format!("New project: {title}")));
                }
            }
        }
    };
    // `Callback` so the many buttons below can each hold a copy.
    let decide = use_callback(decide);

    // t[impl process.suggestion]
    let accept_suggestion = {
        let item = item.clone();
        let slug = slug.clone();
        move |_| {
            let mut next = item.clone();
            next.status = InboxItem::STATUS_OPEN.to_string();
            muts.save(slug.clone(), next);
        }
    };

    // Keys for the decision steps (never while typing in the task form):
    // Fast — t task, l later, n note, c/d done, x delete, space skip;
    // Clarify — y yes, n no; then the same letters as their tiles.
    let on_key = move |e: KeyboardEvent| {
        let Key::Character(c) = e.key() else { return };
        let picked = match (step(), mode(), c.as_str()) {
            (Step::Decide, Mode::Fast, "t") => Some(Pick::Step(Step::Shape(Shape::Task))),
            (Step::Decide, Mode::Fast, "l") => Some(Pick::Step(Step::Later)),
            (Step::Decide, Mode::Fast, "n") => Some(Pick::Decide(Decision::Note)),
            (Step::Decide, Mode::Fast, "c" | "d") => Some(Pick::Decide(Decision::Done)),
            (Step::Decide, Mode::Fast, "x") => Some(Pick::Decide(Decision::Delete)),
            (Step::Decide, _, " ") if skippable => Some(Pick::Skip),
            (Step::Decide, Mode::Clarify, "y") => Some(Pick::Step(Step::Actionable)),
            (Step::Decide, Mode::Clarify, "n") => Some(Pick::Step(Step::NotActionable)),
            (Step::Actionable, _, "d") => Some(Pick::Decide(Decision::DoNow)),
            (Step::Actionable, _, "t") => Some(Pick::Step(Step::Shape(Shape::Task))),
            (Step::Actionable, _, "w") => Some(Pick::Step(Step::Shape(Shape::Waiting))),
            (Step::Actionable, _, "p") => Some(Pick::Decide(Decision::Project)),
            (Step::NotActionable, _, "n") => Some(Pick::Decide(Decision::Note)),
            (Step::NotActionable, _, "s") => {
                Some(Pick::Decide(Decision::Later(ReviewResponse::Easy)))
            }
            (Step::NotActionable, _, "x") => Some(Pick::Decide(Decision::Delete)),
            _ => None,
        };
        if let Some(p) = picked {
            e.prevent_default();
            match p {
                Pick::Step(s) => step.set(s),
                Pick::Decide(d) => decide.call(d),
                Pick::Skip => on_done.call(Outcome::Skipped),
            }
        }
    };

    rsx! {
        div {
            class: "flex min-h-0 w-full flex-col gap-4 outline-none",
            tabindex: "0",
            autofocus: true,
            onkeydown: on_key,
            // ── the capture ──
            article { class: "flex flex-col gap-3 rounded-2xl border border-border bg-card p-5 shadow-sm sm:p-6",
                div { class: "flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px] font-semibold uppercase tracking-[0.16em] text-muted-foreground",
                    if suggested {
                        span { class: "rounded-full bg-primary/15 px-2 py-0.5 normal-case tracking-normal text-primary",
                            "Suggested"
                        }
                    }
                    span { "{kind}" }
                    span { class: "text-border", "·" }
                    span { "{date}" }
                    if source != "ui" && source != "cli" {
                        span { class: "text-border", "·" }
                        span { "via {source}" }
                    }
                }
                p { class: "whitespace-pre-wrap break-words text-lg font-medium leading-relaxed text-foreground sm:text-xl",
                    "{body}"
                }
                if suggested && step() == Step::Decide {
                    div { class: "flex flex-wrap items-center gap-2 border-t border-border/60 pt-3 text-sm",
                        span { class: "text-muted-foreground", "Something suggested this. Keep it?" }
                        button {
                            r#type: "button",
                            class: "ml-auto rounded-lg px-3 py-1.5 text-muted-foreground hover:bg-accent active:bg-accent",
                            onclick: move |_| decide.call(Decision::Delete),
                            "Dismiss"
                        }
                        button {
                            r#type: "button",
                            class: "rounded-lg bg-primary/15 px-3 py-1.5 font-medium text-primary hover:bg-primary/25",
                            onclick: accept_suggestion,
                            "Keep in inbox"
                        }
                    }
                }
            }

            // ── the decision ──
            match step() {
                Step::Decide => match mode() {
                    Mode::Fast => rsx! {
                        div { class: "grid grid-cols-2 gap-2 sm:grid-cols-3",
                            Choice { icon: rsx! { ListTodo { size: 18 } }, label: "Make a task", hint: "File it with a project, context and date", primary: true,
                                on_pick: move |()| step.set(Step::Shape(Shape::Task)) }
                            Choice { icon: rsx! { Clock { size: 18 } }, label: "Later", hint: "Bring it back when it matters",
                                on_pick: move |()| step.set(Step::Later) }
                            Choice { icon: rsx! { FileText { size: 18 } }, label: "Note", hint: "Keep it as a note",
                                on_pick: move |()| decide.call(Decision::Note) }
                            Choice { icon: rsx! { Check { size: 18 } }, label: "Done", hint: "Already handled",
                                on_pick: move |()| decide.call(Decision::Done) }
                            Choice { icon: rsx! { Trash2 { size: 18 } }, label: "Delete", hint: "Not worth keeping", danger: true,
                                on_pick: move |()| decide.call(Decision::Delete) }
                            if skippable {
                                Choice { icon: rsx! { ArrowRight { size: 18 } }, label: "Skip", hint: "Decide on another pass",
                                    on_pick: move |()| on_done.call(Outcome::Skipped) }
                            }
                        }
                    },
                    Mode::Clarify => rsx! {
                        Question { text: "Is this actionable?" }
                        div { class: "grid grid-cols-2 gap-2",
                            Choice { icon: rsx! { Check { size: 18 } }, label: "Yes", hint: "There's something to do", primary: true,
                                on_pick: move |()| step.set(Step::Actionable) }
                            Choice { icon: rsx! { X { size: 18 } }, label: "No", hint: "Reference, an idea, or nothing",
                                on_pick: move |()| step.set(Step::NotActionable) }
                        }
                        if skippable {
                            SkipLink { on_skip: move |()| on_done.call(Outcome::Skipped) }
                        }
                    },
                },
                Step::Actionable => rsx! {
                    Question { text: "What's the next step?", on_back: move |()| step.set(Step::Decide) }
                    div { class: "grid grid-cols-2 gap-2",
                        Choice { icon: rsx! { Play { size: 18 } }, label: "Do it now", hint: "Two minutes or less — the clock starts", primary: true,
                            on_pick: move |()| decide.call(Decision::DoNow) }
                        Choice { icon: rsx! { ListTodo { size: 18 } }, label: "Make it a task", hint: "Give it a context and a date",
                            on_pick: move |()| step.set(Step::Shape(Shape::Task)) }
                        Choice { icon: rsx! { Hourglass { size: 18 } }, label: "Waiting on someone", hint: "Track it until they come back",
                            on_pick: move |()| step.set(Step::Shape(Shape::Waiting)) }
                        Choice { icon: rsx! { FolderKanban { size: 18 } }, label: "It's a project", hint: "More than one step",
                            on_pick: move |()| decide.call(Decision::Project) }
                    }
                },
                Step::NotActionable => rsx! {
                    Question { text: "Keep it?", on_back: move |()| step.set(Step::Decide) }
                    div { class: "grid grid-cols-3 gap-2",
                        Choice { icon: rsx! { FileText { size: 18 } }, label: "Note", hint: "Reference", primary: true,
                            on_pick: move |()| decide.call(Decision::Note) }
                        Choice { icon: rsx! { Archive { size: 18 } }, label: "Someday", hint: "Maybe, later",
                            on_pick: move |()| decide.call(Decision::Later(ReviewResponse::Easy)) }
                        Choice { icon: rsx! { Trash2 { size: 18 } }, label: "Delete", hint: "Let it go", danger: true,
                            on_pick: move |()| decide.call(Decision::Delete) }
                    }
                },
                Step::Later => {
                    let today = Utc::now().date_naive();
                    let soon = fmt_interval(review(&item, ReviewResponse::Hard, today).0);
                    let mid = fmt_interval(review(&item, ReviewResponse::Good, today).0);
                    let far = fmt_interval(review(&item, ReviewResponse::Easy, today).0);
                    rsx! {
                        Question { text: "When should it come back?", on_back: move |()| step.set(Step::Decide) }
                        div { class: "grid grid-cols-3 gap-2",
                            Choice { label: "Soon", hint: "{soon}", primary: true,
                                on_pick: move |()| decide.call(Decision::Later(ReviewResponse::Hard)) }
                            Choice { label: "Maybe", hint: "{mid}",
                                on_pick: move |()| decide.call(Decision::Later(ReviewResponse::Good)) }
                            Choice { label: "Someday", hint: "{far}",
                                on_pick: move |()| decide.call(Decision::Later(ReviewResponse::Easy)) }
                        }
                    }
                }
                Step::Shape(shape) => rsx! {
                    TaskShape {
                        key: "{item.id}",
                        item: item.clone(),
                        slug: slug.clone(),
                        shape: shape == Shape::Waiting,
                        on_back: move |()| step.set(Step::Decide),
                        on_done,
                    }
                },
            }
        }
    }
}

/// What a key on the card picked.
#[derive(Clone, Copy)]
enum Pick {
    Step(Step),
    Decide(Decision),
    Skip,
}

/// A terminal decision the card carries out itself.
#[derive(Clone, Copy, Debug)]
enum Decision {
    Note,
    Later(ReviewResponse),
    Done,
    Delete,
    DoNow,
    Project,
}

/// The question a clarify step asks, with a way back to the last one.
#[component]
fn Question(text: &'static str, on_back: Option<EventHandler<()>>) -> Element {
    rsx! {
        div { class: "flex items-center gap-2",
            if let Some(back) = on_back {
                button {
                    r#type: "button",
                    class: "rounded-md px-2 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground",
                    onclick: move |_| back.call(()),
                    "‹ Back"
                }
            }
            h2 { class: "text-base font-semibold text-foreground", "{text}" }
        }
    }
}

#[component]
fn SkipLink(on_skip: EventHandler<()>) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "self-center rounded-md px-3 py-2 text-xs text-muted-foreground hover:bg-accent hover:text-foreground",
            onclick: move |_| on_skip.call(()),
            "Skip for now"
        }
    }
}

/// One outcome, as a tile big enough to hit with a thumb.
#[component]
fn Choice(
    label: &'static str,
    hint: String,
    on_pick: EventHandler<()>,
    #[props(default)] icon: Option<Element>,
    #[props(default)] primary: bool,
    #[props(default)] danger: bool,
) -> Element {
    let tone = if primary {
        "border-primary/50 bg-primary/10 hover:bg-primary/20"
    } else if danger {
        "border-border/70 bg-card/60 hover:border-destructive/50 hover:bg-destructive/10"
    } else {
        "border-border/70 bg-card/60 hover:border-border hover:bg-accent/40"
    };
    rsx! {
        button {
            r#type: "button",
            class: "flex min-h-[4.5rem] flex-col items-start justify-center gap-1 rounded-xl border px-3 py-2.5 text-left transition-colors active:bg-accent/50 {tone}",
            onclick: move |_| on_pick.call(()),
            span { class: "flex items-center gap-2 text-sm font-medium text-foreground",
                if let Some(icon) = icon {
                    span { class: if danger { "text-destructive" } else if primary { "text-primary" } else { "text-muted-foreground" }, {icon} }
                }
                "{label}"
            }
            span { class: "text-xs leading-snug text-muted-foreground", "{hint}" }
        }
    }
}

// ── shaping the task ────────────────────────────────────────────────

/// The task a capture becomes, pre-filled from what it said and editable
/// before it's made: title, project, contexts, due date, estimate — and,
/// for "waiting on someone", who.
#[component]
fn TaskShape(
    item: InboxItem,
    slug: String,
    /// `true` = a Waiting task ("waiting on someone").
    shape: bool,
    on_back: EventHandler<()>,
    on_done: EventHandler<Outcome>,
) -> Element {
    let waiting = shape;
    let muts = stores::use_inbox_mutations();
    let task_store = stores::use_task_store();
    let project_store = stores::use_project_store();

    let projects: Vec<(Uuid, String)> = project_store
        .list()
        .iter()
        .filter(|r| r.slug == slug && !r.project.archived)
        .map(|r| (r.project.id, r.project.title.clone()))
        .collect();
    let existing_contexts: Vec<String> = {
        let mut seen: Vec<String> = Vec::new();
        for r in task_store.list().iter() {
            for c in r.task.contexts.iter() {
                if !seen.contains(c) {
                    seen.push(c.clone());
                }
            }
        }
        seen
    };

    let draft = use_signal({
        let body = item.body.clone();
        let projects = projects.clone();
        move || draft_task(&body, &projects)
    });
    let mut title = use_signal(|| draft.peek().title.clone());
    let mut details = use_signal(|| draft.peek().details.clone());
    let mut project = use_signal(|| draft.peek().project_id);
    let mut contexts = use_signal(|| draft.peek().contexts.0.clone());
    let mut due = use_signal(|| draft.peek().due.clone().unwrap_or_default());
    let mut estimate = use_signal(|| draft.peek().time_estimate);
    let mut who = use_signal(String::new);

    let chips = {
        let mut all = context_suggestions(&existing_contexts);
        for c in contexts.read().iter() {
            let c = format!("@{}", c.trim_start_matches('@'));
            if !all.contains(&c) {
                all.insert(0, c);
            }
        }
        all
    };
    let today = Local::now().date_naive();
    // The coming Saturday (today, on a Saturday).
    let to_saturday = (5 - i64::from(today.weekday().num_days_from_monday())).rem_euclid(7);
    let weekend = today + Duration::days(to_saturday);
    let dates = [
        ("Today", today.to_string()),
        ("Tomorrow", (today + Duration::days(1)).to_string()),
        ("Weekend", weekend.to_string()),
        ("Next week", (today + Duration::days(7)).to_string()),
    ];

    // t[impl process.outcome.task]
    // t[impl process.outcome.waiting]
    let save = {
        let item = item.clone();
        let slug = slug.clone();
        let projects = projects.clone();
        move |_| {
            let mut task = draft();
            let t = title.read().trim().to_owned();
            if t.is_empty() {
                return;
            }
            task.title = t.clone();
            let mut body = details.read().trim().to_owned();
            if waiting {
                task.status = "waiting".into();
                let w = who.read().trim().to_owned();
                if !w.is_empty() {
                    body = if body.is_empty() {
                        format!("Waiting on {w}.")
                    } else {
                        format!("Waiting on {w}.\n\n{body}")
                    };
                }
            }
            task.details = body;
            task.contexts = task_proto::model::StringList(contexts());
            task.due = Some(due()).filter(|d| !d.is_empty());
            task.time_estimate = estimate();
            task.project_id = project();
            // The wikilink rides along so vault views that read
            // `projects:` agree with the id.
            task.projects = task_proto::model::StringList(
                project()
                    .and_then(|id| projects.iter().find(|(p, _)| *p == id))
                    .map(|(_, name)| vec![format!("[[{name}]]")])
                    .unwrap_or_default(),
            );
            muts.process_into_task(slug.clone(), item.clone(), task);
            on_done.call(Outcome::Filed(if waiting {
                format!("Waiting: {t}")
            } else {
                format!("Task added: {t}")
            }));
        }
    };

    const CHIP_ON: &str =
        "rounded-full border border-primary/60 bg-primary/15 px-3 py-1.5 text-sm text-foreground";
    const CHIP_OFF: &str = "rounded-full border border-border/70 px-3 py-1.5 text-sm text-muted-foreground hover:text-foreground";
    const FIELD: &str = "w-full rounded-lg border border-border bg-background px-3 py-2 text-sm text-foreground outline-none focus-visible:border-primary";

    let heading = if waiting {
        "Waiting on someone"
    } else {
        "Make it a task"
    };
    rsx! {
        div { class: "flex flex-col gap-4",
            Question { text: heading, on_back: move |()| on_back.call(()) }
            textarea {
                aria_label: "Title",
                rows: "1",
                class: "w-full resize-none border-b border-border bg-transparent py-1 text-lg font-medium leading-snug text-foreground outline-none [field-sizing:content] focus-visible:border-primary",
                value: "{title}",
                oninput: move |e| title.set(e.value()),
            }
            if waiting {
                input {
                    r#type: "text",
                    class: "{FIELD}",
                    placeholder: "Who or what are you waiting on?",
                    value: "{who}",
                    oninput: move |e| who.set(e.value()),
                }
            }
            if !projects.is_empty() {
                Field { label: "Project",
                    select {
                        class: "{FIELD}",
                        onchange: move |e| project.set(e.value().parse::<Uuid>().ok()),
                        option { value: "", selected: project().is_none(), "No project" }
                        for (id, name) in projects.iter() {
                            option { key: "{id}", value: "{id}", selected: project() == Some(*id), "{name}" }
                        }
                    }
                }
            }
            Field { label: "Context",
                div { class: "flex flex-wrap gap-1.5",
                    for c in chips.into_iter() {
                        {
                            let on = contexts.read().iter().any(|x| x.trim_start_matches('@').eq_ignore_ascii_case(c.trim_start_matches('@')));
                            let c2 = c.clone();
                            rsx! {
                                button {
                                    key: "{c}",
                                    r#type: "button",
                                    class: if on { CHIP_ON } else { CHIP_OFF },
                                    aria_pressed: on.to_string(),
                                    onclick: move |_| {
                                        let name = c2.trim_start_matches('@').to_ascii_lowercase();
                                        let mut list = contexts();
                                        if let Some(i) = list.iter().position(|x| x.trim_start_matches('@').eq_ignore_ascii_case(&name)) {
                                            list.remove(i);
                                        } else {
                                            list.push(format!("@{name}"));
                                        }
                                        contexts.set(list);
                                    },
                                    "{c}"
                                }
                            }
                        }
                    }
                }
            }
            Field { label: "Due",
                div { class: "flex flex-wrap gap-1.5",
                    for (label, value) in dates.into_iter() {
                        {
                            let on = due() == value;
                            rsx! {
                                button {
                                    key: "{label}",
                                    r#type: "button",
                                    class: if on { CHIP_ON } else { CHIP_OFF },
                                    onclick: move |_| due.set(if on { String::new() } else { value.clone() }),
                                    "{label}"
                                }
                            }
                        }
                    }
                    input {
                        r#type: "date",
                        aria_label: "Due date",
                        class: "rounded-full border border-border/70 bg-transparent px-3 py-1 text-sm text-muted-foreground",
                        value: "{due}",
                        onchange: move |e| due.set(e.value()),
                    }
                }
            }
            Field { label: "How long",
                div { class: "flex flex-wrap gap-1.5",
                    for (label, mins) in [("5m", 5u32), ("15m", 15), ("30m", 30), ("1h", 60), ("2h", 120)] {
                        {
                            let on = estimate() == Some(mins);
                            rsx! {
                                button {
                                    key: "{label}",
                                    r#type: "button",
                                    class: if on { CHIP_ON } else { CHIP_OFF },
                                    onclick: move |_| estimate.set(if on { None } else { Some(mins) }),
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }
            details { class: "text-sm",
                summary { class: "cursor-pointer py-1 text-muted-foreground", "Notes" }
                textarea {
                    class: "{FIELD} mt-2 min-h-24",
                    value: "{details}",
                    oninput: move |e| details.set(e.value()),
                }
            }
            button {
                r#type: "button",
                class: "flex min-h-12 items-center justify-center gap-2 rounded-xl bg-primary px-4 text-sm font-semibold text-primary-foreground active:bg-primary/85 disabled:opacity-50",
                disabled: title.read().trim().is_empty(),
                onclick: save,
                if waiting { "Save as waiting" } else { "Add task" }
            }
        }
    }
}

#[component]
fn Field(label: &'static str, children: Element) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1.5",
            span { class: "text-xs font-medium text-muted-foreground", "{label}" }
            {children}
        }
    }
}

// ── shared helpers ──────────────────────────────────────────────────

/// Human-friendly interval label ("today", "1d", "3w", "2mo").
#[must_use]
pub fn fmt_interval(days: i64) -> String {
    if days <= 0 {
        "today".to_string()
    } else if days == 1 {
        "tomorrow".to_string()
    } else if days < 7 {
        format!("{days} days")
    } else if days < 30 {
        format!("{} weeks", (days as f64 / 7.0).round() as i64)
    } else if days < 365 {
        format!("{} months", (days as f64 / 30.0).round() as i64)
    } else {
        format!("{:.1} years", days as f64 / 365.0)
    }
}

/// First non-empty line (capped) as the title; the remainder as the
/// body.
#[must_use]
pub fn split_title_body(body: &str) -> (String, String) {
    let trimmed = body.trim();
    let (first, rest) = trimmed.split_once('\n').unwrap_or((trimmed, ""));
    let title: String = first.trim().chars().take(120).collect();
    let title = if title.is_empty() {
        "Untitled".to_string()
    } else {
        title
    };
    (title, rest.trim().to_string())
}

/// Kebab-case a title into a vault-safe filename stem.
#[must_use]
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let capped: String = out.trim_matches('-').chars().take(60).collect();
    let capped = capped.trim_matches('-').to_string();
    if capped.is_empty() {
        "note".to_string()
    } else {
        capped
    }
}

/// Markdown for a promoted atomic note: frontmatter over the verbatim
/// capture.
#[must_use]
pub fn atomic_markdown(title: &str, body: &str, created: &str) -> String {
    let esc = title.replace('"', "'");
    format!(
        "---\ntitle: \"{esc}\"\ntype: atomic\ntags:\n  - atomic\ncreated: {created}\n---\n\n{body}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_drafts_the_task_quick_add_would_make() {
        let errands = Uuid::new_v4();
        let projects = vec![(errands, "House".to_owned())];
        let t = draft_task(
            "Return the library books @errands #chore [[House]] tomorrow\nThey're on the hall table.",
            &projects,
        );
        assert_eq!(t.title, "Return the library books");
        assert!(t.contexts.iter().any(|c| c == "@errands"));
        assert!(t.tags.iter().any(|g| g == "chore"));
        assert_eq!(t.project_id, Some(errands));
        assert!(t.due.is_some(), "tomorrow parsed");
        assert_eq!(t.details, "They're on the hall table.");
    }

    #[test]
    fn a_bare_capture_keeps_its_words() {
        let t = draft_task("   call mum   ", &[]);
        assert_eq!(t.title, "call mum");
        assert!(t.contexts.is_empty() && t.project_id.is_none());
    }

    #[test]
    fn context_chips_lead_with_the_persons_own_and_never_repeat() {
        let chips = context_suggestions(&["@Studio".into(), "@errands".into()]);
        assert_eq!(chips[0], "@studio");
        assert_eq!(chips.iter().filter(|c| *c == "@errands").count(), 1);
        assert!(chips.contains(&"@chores".to_owned()));
    }
}
