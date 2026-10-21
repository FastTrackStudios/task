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
    Archive, ArrowRight, Check, Clock, FileText, FolderKanban, Hourglass, Play, Trash2, X,
};
use chrono::{Datelike, Duration, Local, Utc};
use dioxus::prelude::*;
use inbox_proto::{InboxItem, ReviewResponse, review};
use uuid::Uuid;

use crate::stores;

// ── mode ────────────────────────────────────────────────────────────

/// How much the card asks before it acts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// How many inbox items are waiting to be processed today — the inbox
/// lane's one due queue (`inbox_proto::due_queue`, what
/// `Inbox::due_queue` serves the brief and the watch counts), so the tab
/// badge, Home's card, the deck and the brief never disagree.
pub fn use_to_process_count() -> usize {
    let rows = stores::use_inbox_list();
    let today = Utc::now().date_naive().to_string();
    rows.value().map_or(0, |rows| {
        inbox_proto::due_queue(rows.iter().map(|(_, it)| it), &today).len()
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
    let undo_slug = slug.clone();
    rsx! {
        div {
            class: "fixed inset-0 z-50 flex justify-center bg-background md:items-start md:bg-black/50 md:p-6 md:pt-[8vh]",
            onclick: move |_| keep_bg(),
            div {
                "data-testid": "process-now",
                role: "dialog",
                aria_label: "Process this capture",
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
                }
                div { class: "min-h-0 flex-1 overflow-y-auto px-4 pb-[calc(1.5rem+env(safe-area-inset-bottom))] md:pb-6",
                    ProcessCard {
                        key: "{item.id}",
                        item,
                        slug,
                        mode,
                        stored: false,
                        on_done: {
                            let slug = undo_slug.clone();
                            move |outcome: Outcome| {
                                use dioxus_primitives::toast::ToastOptions;
                                // t[impl process.undo]
                                // Undo files the capture in the inbox as it
                                // was typed — open — so nothing is lost
                                // however the decision went. A delete wrote
                                // nothing, so its undo creates the file.
                                let undo = |before: InboxItem, stored: bool| {
                                    let slug = slug.clone();
                                    move || muts.carry(slug.clone(), inbox_proto::decision::undo(&before), stored)
                                };
                                match outcome {
                                    Outcome::Filed(msg) => toast.success(msg, ToastOptions::new()),
                                    Outcome::Noted => toast.success("Saved as a note".to_owned(), ToastOptions::new()),
                                    Outcome::Later { label, before } => {
                                        toast.info(label, ToastOptions::new().action("Undo", undo(before, true)));
                                    }
                                    Outcome::Done { before } => {
                                        toast.success("Marked done".to_owned(), ToastOptions::new().action("Undo", undo(before, true)));
                                    }
                                    Outcome::Deleted { before } => {
                                        toast.info("Deleted".to_owned(), ToastOptions::new().action("Undo", undo(before, false)));
                                    }
                                    Outcome::Skipped => {}
                                }
                                now.set(None);
                            }
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
    /// What it is: every outcome, one tap each.
    Decide,
    /// Choosing when it comes back.
    Later,
    /// Shaping the task and filing it in a project.
    Shape(Shape),
    /// Rewriting it as a note in your own words, under a parent.
    Rewrite,
    /// Doing it now: the task exists and its clock is running.
    Doing,
}

/// What kind of task the shaping form makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Task,
    Waiting,
}

/// One inbox item, processed. `on_done` fires once with what happened;
/// the card does every write itself, through the shared stores.
// t[impl process.card]
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
    let me = use_context::<crate::auth::AuthCtx>()
        .active
        .read()
        .as_ref()
        .map(|a| (a.user_id.to_string(), a.name.clone()));
    // Callers key the card by item id, so each item starts here.
    let mut step = use_signal(|| Step::Decide);
    // The task "do it now" made, while its clock runs on this card.
    let mut doing = use_signal(|| None::<task_proto::TaskInfo>);
    let task_muts = stores::use_task_mutations();
    let _ = mode;
    // "Do it now", finished or paused: the clock stops, the task records
    // how it ended, and the card moves on.
    let finish = {
        let slug = slug.clone();
        use_callback(move |status: &'static str| {
            let Some(mut t) = doing.peek().clone() else {
                return;
            };
            let now = Utc::now();
            for e in &mut t.time_entries.0 {
                if e.end_time.is_none() {
                    e.end_time = Some(now);
                }
            }
            t.status = status.to_owned();
            task_muts.apply(&slug, task_ui::TaskMutation::Update { task: t.clone() });
            let what = if status == "done" { "Done" } else { "Paused" };
            on_done.call(Outcome::Filed(format!("{what}: {}", t.title)));
        })
    };
    // t[impl process.outcome.note]
    // Writing the note the rewrite step produced.
    let save_note = {
        let slug = slug.clone();
        let item = item.clone();
        use_callback(move |(path, md): (String, String)| {
            muts.promote_to_note(slug.clone(), item.clone(), path, md);
            on_done.call(Outcome::Noted);
        })
    };

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
                // t[impl process.outcome.reference]
                Decision::Note => {
                    let path = inbox_proto::decision::reference_path(&item);
                    let md =
                        inbox_proto::decision::reference_markdown(&item, &Utc::now().to_rfc3339());
                    muts.promote_to_note(slug, item, path, md);
                    on_done.call(Outcome::Noted);
                }
                // t[impl process.outcome.later]
                Decision::Later(resp) => {
                    let before = item.clone();
                    // A suggestion put off is still accepted into the inbox.
                    let write =
                        inbox_proto::Decision::Later(resp).apply(&item, Utc::now().date_naive());
                    let label = match &write {
                        inbox_proto::Write::Save(next) => {
                            format!("Back in {}", fmt_interval(next.interval))
                        }
                        inbox_proto::Write::Remove(_) => String::new(),
                    };
                    muts.carry(slug, write, stored);
                    on_done.call(Outcome::Later { before, label });
                }
                // t[impl process.outcome.done]
                Decision::Done => {
                    let before = item.clone();
                    let write = inbox_proto::Decision::Done.apply(&item, Utc::now().date_naive());
                    muts.carry(slug, write, stored);
                    on_done.call(Outcome::Done { before });
                }
                // t[impl process.outcome.delete]
                Decision::Delete => {
                    let before = item.clone();
                    // A fresh capture that was never stored writes nothing.
                    let write = inbox_proto::Decision::Delete.apply(&item, Utc::now().date_naive());
                    muts.carry(slug, write, stored);
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
                    let task = task_ui::with_assignee(&task, me.clone());
                    muts.process_into_task(slug, item, task.clone());
                    // The card stays on it, clock running, until it is
                    // done or paused (`Step::Doing`).
                    doing.set(Some(task));
                    step.set(Step::Doing);
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
            let write = inbox_proto::Decision::Keep.apply(&item, Utc::now().date_naive());
            muts.carry(slug.clone(), write, stored);
        }
    };

    // Keys for the decision steps (never while typing in the task form):
    // Fast — t task, l later, n note, c/d done, x delete, space skip;
    // Clarify — y yes, n no; then the same letters as their tiles.
    let on_key = move |e: KeyboardEvent| {
        let Key::Character(c) = e.key() else { return };
        let picked = match (step(), c.as_str()) {
            (Step::Decide, "d") => Some(Pick::Decide(Decision::DoNow)),
            (Step::Decide, "p" | "t") => Some(Pick::Step(Step::Shape(Shape::Task))),
            (Step::Decide, "s" | "l") => Some(Pick::Step(Step::Later)),
            (Step::Decide, "n") => Some(Pick::Step(Step::Rewrite)),
            (Step::Decide, "r") => Some(Pick::Decide(Decision::Note)),
            (Step::Decide, "x") => Some(Pick::Decide(Decision::Delete)),
            (Step::Decide, " ") if skippable => Some(Pick::Skip),
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
                // The temporal contract: read it, then decide in one tap.
                // Do it now (two minutes, the clock starts), file it as the
                // next step of a project, snooze it, or make it knowledge —
                // an idea rewritten in your own words under a parent note,
                // or a fact kept verbatim. Or let it go.
                Step::Decide => rsx! {
                    div { class: "grid grid-cols-2 gap-2",
                        Choice { icon: rsx! { Play { size: 18 } }, label: "Do it now", hint: "Two minutes — the clock starts", primary: true,
                            on_pick: move |()| decide.call(Decision::DoNow) }
                        Choice { icon: rsx! { FolderKanban { size: 18 } }, label: "Add to a project", hint: "Its next step, with a date",
                            on_pick: move |()| step.set(Step::Shape(Shape::Task)) }
                        Choice { icon: rsx! { FileText { size: 18 } }, label: "Write a note", hint: "In your own words, under a parent",
                            on_pick: move |()| step.set(Step::Rewrite) }
                        Choice { icon: rsx! { Clock { size: 18 } }, label: "Snooze", hint: "Read it again later",
                            on_pick: move |()| step.set(Step::Later) }
                    }
                    div { class: "flex flex-wrap items-center gap-1",
                        button {
                            r#type: "button",
                            class: "flex h-10 items-center gap-1.5 rounded-full px-3 text-[14px] text-muted-foreground active:bg-accent hover:bg-accent/50",
                            onclick: move |_| decide.call(Decision::Note),
                            Archive { size: 15 }
                            "Keep as reference"
                        }
                        button {
                            r#type: "button",
                            class: "flex h-10 items-center gap-1.5 rounded-full px-3 text-[14px] text-muted-foreground active:bg-accent hover:bg-accent/50",
                            onclick: move |_| decide.call(Decision::Project),
                            FolderKanban { size: 15 }
                            "It's a project"
                        }
                        button {
                            r#type: "button",
                            class: "flex h-10 items-center gap-1.5 rounded-full px-3 text-[14px] text-muted-foreground active:bg-accent hover:bg-accent/50",
                            onclick: move |_| decide.call(Decision::Done),
                            Check { size: 15 }
                            "Already done"
                        }
                        button {
                            r#type: "button",
                            class: "flex h-10 items-center gap-1.5 rounded-full px-3 text-[14px] text-rose-400 active:bg-rose-500/10 hover:bg-rose-500/10",
                            onclick: move |_| decide.call(Decision::Delete),
                            Trash2 { size: 15 }
                            "Delete"
                        }
                        if skippable {
                            button {
                                r#type: "button",
                                class: "ml-auto flex h-10 items-center gap-1.5 rounded-full px-3 text-[14px] text-muted-foreground active:bg-accent hover:bg-accent/50",
                                onclick: move |_| on_done.call(Outcome::Skipped),
                                "Skip"
                                ArrowRight { size: 15 }
                            }
                        }
                    }
                },
                // t[impl process.outcome.now]
                Step::Doing => {
                    let task = doing.read().clone();
                    let title = task.as_ref().map(|t| t.title.clone()).unwrap_or_default();
                    rsx! {
                        div { class: "flex flex-col gap-3 rounded-2xl border border-primary/30 bg-primary/10 p-4",
                            div { class: "flex items-center gap-3",
                                span { class: "relative flex size-3",
                                    span { class: "absolute inline-flex size-full animate-ping rounded-full bg-primary/60" }
                                    span { class: "relative inline-flex size-3 rounded-full bg-primary" }
                                }
                                span { class: "min-w-0 flex-1 truncate text-[16px] font-medium text-foreground", "Doing: {title}" }
                                crate::process::ElapsedClock { since: task.as_ref().and_then(|t| t.time_entries.0.last().map(|e| e.start_time)) }
                            }
                            p { class: "text-[13px] text-muted-foreground",
                                "It's a task now, and the clock is running. Finish it, or pause and it stays on your list."
                            }
                            div { class: "grid grid-cols-2 gap-2",
                                button {
                                    r#type: "button",
                                    class: "flex h-12 items-center justify-center gap-2 rounded-xl bg-primary text-[15px] font-semibold text-primary-foreground active:opacity-90",
                                    onclick: move |_| finish.call("done"),
                                    Check { size: 18 }
                                    "Done"
                                }
                                button {
                                    r#type: "button",
                                    class: "flex h-12 items-center justify-center gap-2 rounded-xl bg-foreground/[0.08] text-[15px] font-medium text-foreground active:bg-foreground/[0.12]",
                                    onclick: move |_| finish.call("todo"),
                                    Hourglass { size: 18 }
                                    "Pause"
                                }
                            }
                        }
                    }
                }
                Step::Rewrite => rsx! {
                    NoteRewrite {
                        key: "{item.id}",
                        item: item.clone(),
                        slug: slug.clone(),
                        on_back: move |()| step.set(Step::Decide),
                        on_save: move |v| save_note.call(v),
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

/// Time since `since`, ticking each second — the clock "do it now"
/// started.
#[component]
pub fn ElapsedClock(since: Option<chrono::DateTime<Utc>>) -> Element {
    let mut now = use_signal(Utc::now);
    use_future(move || async move {
        loop {
            architect::sleep(architect::Duration::from_secs(1)).await;
            now.set(Utc::now());
        }
    });
    let secs = since.map_or(0, |s| (now() - s).num_seconds().max(0));
    let text = if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    };
    rsx! { span { class: "shrink-0 font-mono text-[15px] tabular-nums text-primary", "{text}" } }
}

/// A capture rewritten as a note in your own words — the step that turns
/// a fleeting thought into knowledge. The title is the note's name (a
/// sentence that says what it means, not an id); the body is yours; the
/// original words are kept beneath as a quote, the paper trail back to
/// the thought; and the note is placed under one parent note (its
/// `folder`), so related thinking sits together in the tree.
#[component]
fn NoteRewrite(
    item: InboxItem,
    slug: String,
    on_back: EventHandler<()>,
    on_save: EventHandler<(String, String)>,
) -> Element {
    let (first, rest) = split_title_body(&item.body);
    let mut title = use_signal(|| first.clone());
    let mut body = use_signal(|| rest.clone());
    let mut parent = use_signal(|| None::<(String, String)>);
    let mut query = use_signal(String::new);
    let pages_slug = slug.clone();
    let pages = use_resource(move || {
        let slug = pages_slug.clone();
        async move {
            crate::pages::vault::rpc::fetch_folder_index(slug, "default".to_owned())
                .await
                .unwrap_or_default()
        }
    });
    let q = query().to_lowercase();
    let matches: Vec<(String, String)> = pages
        .read()
        .as_ref()
        .map(|ps| {
            ps.iter()
                .filter(|p| p.path.ends_with(".md") && !p.path.starts_with("Records/"))
                .filter(|p| q.is_empty() || p.title.to_lowercase().contains(&q))
                .take(6)
                .map(|p| (p.basename.clone(), p.title.clone()))
                .collect()
        })
        .unwrap_or_default();
    let original = item.body.clone();
    let can_save = !title.read().trim().is_empty();
    let field = "w-full rounded-xl bg-foreground/[0.06] px-3.5 text-[16px] text-foreground outline-none placeholder:text-muted-foreground/70 focus:bg-foreground/[0.09]";

    rsx! {
        div { class: "flex flex-col gap-3",
            div { class: "flex items-center gap-2",
                button {
                    r#type: "button",
                    class: "flex size-9 items-center justify-center rounded-full text-muted-foreground active:bg-accent",
                    aria_label: "Back",
                    onclick: move |_| on_back.call(()),
                    ArrowRight { size: 16, class: "rotate-180" }
                }
                span { class: "text-[15px] font-semibold text-foreground", "Write it in your own words" }
            }
            input {
                class: "{field} h-12 font-medium",
                placeholder: "What it means, as a sentence",
                aria_label: "Note title",
                value: "{title}",
                oninput: move |e| title.set(e.value()),
            }
            textarea {
                class: "{field} min-h-32 py-3 leading-6",
                placeholder: "Explain it so you'd understand it in a year.",
                aria_label: "Note body",
                value: "{body}",
                oninput: move |e| body.set(e.value()),
            }
            // ── the parent ──
            div { class: "flex flex-col gap-2 rounded-2xl bg-foreground/[0.04] p-3",
                span { class: "text-[13px] font-medium text-muted-foreground", "Place it under" }
                if let Some((_, ptitle)) = parent() {
                    div { class: "flex items-center gap-2",
                        span { class: "flex h-9 min-w-0 items-center gap-1.5 rounded-full bg-primary/15 px-3 text-[14px] font-medium text-primary",
                            FileText { size: 14 }
                            span { class: "truncate", "{ptitle}" }
                        }
                        button {
                            r#type: "button",
                            class: "text-[13px] text-muted-foreground",
                            onclick: move |_| parent.set(None),
                            "Change"
                        }
                    }
                } else {
                    input {
                        class: "{field} h-10 text-[15px]",
                        placeholder: "Find a related note…",
                        aria_label: "Parent note",
                        value: "{query}",
                        oninput: move |e| query.set(e.value()),
                    }
                    div { class: "flex flex-col",
                        for (base, ptitle) in matches {
                            button {
                                key: "{base}",
                                r#type: "button",
                                class: "flex min-h-10 items-center gap-2 rounded-lg px-2 text-left text-[15px] text-foreground active:bg-accent",
                                onclick: move |_| parent.set(Some((base.clone(), ptitle.clone()))),
                                FileText { size: 14, class: "shrink-0 text-muted-foreground" }
                                span { class: "truncate", "{ptitle}" }
                            }
                        }
                    }
                }
            }
            // ── the original, kept ──
            div { class: "rounded-xl border-l-2 border-border px-3 py-1 text-[14px] leading-5 text-muted-foreground",
                span { class: "block pb-1 text-[11px] font-semibold uppercase tracking-[0.12em]", "Original capture" }
                span { class: "whitespace-pre-wrap", "{original}" }
            }
            button {
                r#type: "button",
                class: "flex h-12 items-center justify-center gap-2 rounded-xl bg-primary text-[15px] font-semibold text-primary-foreground active:opacity-90 disabled:opacity-40",
                disabled: !can_save,
                onclick: move |_| {
                    let t = title.peek().trim().to_owned();
                    let p = parent.peek().clone().map(|(b, _)| b);
                    let path = format!("{}.md", note_file_name(&t));
                    let md = rewritten_note(&body.peek(), &original, p.as_deref(), &Utc::now().to_rfc3339());
                    on_save.call((path, md));
                },
                Check { size: 18 }
                "Save note"
            }
        }
    }
}

// The note shape and its names live with the decision, so every
// surface that processes writes the same file.
pub use inbox_proto::decision::{note_file_name, rewritten_note, slugify, split_title_body};

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
    // Whoever processes it: the task lands on their list.
    let me = use_context::<crate::auth::AuthCtx>()
        .active
        .read()
        .as_ref()
        .map(|a| (a.user_id.to_string(), a.name.clone()));
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
            // Yours: what you process lands on your Tasks list.
            let task = task_ui::with_assignee(&task, me.clone());
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
