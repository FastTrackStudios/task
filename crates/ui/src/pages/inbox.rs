//! `/inbox` — Process: every capture, once, until the inbox is empty.
//!
//! Capture is free (the ✎ button, quick-add, the CLI, the watch, agents)
//! and lands here; this page is where each capture becomes what it
//! actually is. It is a deck: the due items — open ones whose resurface
//! date has come, and suggestions something else proposed — frozen into
//! a queue when the page opens, then one [`ProcessCard`] at a time.
//!
//! The card does the deciding (Fast or Clarify — see [`crate::process`]);
//! this page keeps the place, says what each decision did, and offers
//! Undo where undoing is honest: an item put off, marked done or deleted
//! comes back exactly as it was, and the deck steps back to it.
//!
//! State is the shared optimistic store ([`crate::stores`]), so leaving
//! needs no refetch: the store already holds every decision.

use architect_ui::prelude::*;
use chrono::Utc;
use dioxus::prelude::*;
use dioxus_primitives::toast::ToastOptions;
use inbox_proto::InboxItem;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::process::{Mode, Outcome, ProcessCard};
use crate::stores;

/// Whether an item belongs in today's deck: open and due (no resurface
/// date, or one that has come), or a suggestion awaiting a yes or no —
/// `InboxItem::is_due`, shared with the watch bridge's Suggested pick.
#[must_use]
// t[impl process.due]
pub fn is_due(item: &InboxItem, today: &str) -> bool {
    item.is_due(today)
}

#[component]
pub fn InboxView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The org we review (first selected, or home).
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    let nav = use_navigator();
    let known = use_known_people(slug);
    let result = stores::use_inbox_list();
    let store = stores::use_inbox_store();

    // Freeze the due set ONCE (when the list first loads) so decisions —
    // which patch the store optimistically — never reshuffle the cards.
    // Oldest first: the thing ignored longest is decided first.
    let mut queue = use_signal(Vec::<InboxItem>::new);
    let mut seeded = use_signal(|| false);
    let mut fleeting = crate::chrome::use_fleeting_open();
    // `Some(i)`: the deck is open, from row `i`.
    let mut deck = use_signal(|| None::<usize>);
    // Seeded on the first render that HAS rows. (An effect made once at
    // mount froze whatever the list held then — empty, now that the app
    // opens on this page before the inbox has loaded — and never ran
    // again, so the deck and the list stayed empty under a badge of 5.)
    // Written once (guarded by `seeded`), so it can't loop.
    if !*seeded.peek() {
        if let Some(rows) = result.value() {
            let today = Utc::now().date_naive().to_string();
            let mut due: Vec<InboxItem> = rows
                .iter()
                .filter(|(_, it)| is_due(it, &today))
                .map(|(_, it)| it.clone())
                .collect();
            due.sort_by(|a, b| a.created.cmp(&b.created));
            queue.set(due);
            seeded.set(true);
        }
    } else if let Some(rows) = result.value() {
        // Captured since: appended at the end, so the order already on
        // screen (and the deck's place in it) never moves. Written only
        // when something is new, so it can't loop.
        let today = Utc::now().date_naive().to_string();
        let fresh: Vec<InboxItem> = {
            let have = queue.peek();
            let mut v: Vec<InboxItem> = rows
                .iter()
                .filter(|(_, it)| is_due(it, &today) && !have.iter().any(|q| q.id == it.id))
                .map(|(_, it)| it.clone())
                .collect();
            v.sort_by(|a, b| a.created.cmp(&b.created));
            v
        };
        if !fresh.is_empty() {
            queue.write().extend(fresh);
        }
    }

    if result.is_waiting() && result.value().is_none() {
        return rsx! { crate::states::LoadingState {} };
    }
    if let Some(err) = result.error() {
        return rsx! {
            crate::states::ErrorState {
                title: "Couldn't reach the inbox",
                message: err.clone(),
                on_retry: move |()| store.reload(),
            }
        };
    }

    // Nothing due: an ordinary page, inside the app's chrome, offering
    // the two things a person does next. Decided from the loaded rows,
    // not the seeded queue (the queue fills in an effect).
    let today = Utc::now().date_naive().to_string();
    let nothing_due = queue.read().is_empty()
        && result
            .value()
            .is_some_and(|rows| !rows.iter().any(|(_, it)| is_due(it, &today)));
    if nothing_due {
        return rsx! {
            div { class: "mx-auto flex max-w-md flex-col items-center gap-4 px-6 py-24 text-center",
                Heading { level: HeadingLevel::H2, "Inbox clear" }
                Text { variant: TextVariant::Muted,
                    "Nothing waiting to be processed. Captures land here until you decide what they are."
                }
                div { class: "flex flex-wrap justify-center gap-2",
                    Button {
                        variant: ButtonVariant::Primary,
                        on_click: move |_| fleeting.set(true),
                        "Capture"
                    }
                    Button {
                        variant: ButtonVariant::Outline,
                        on_click: move |_| {
                            nav.push(crate::routes::Route::TasksRoute {});
                        },
                        "Go to tasks"
                    }
                }
            }
        };
    }

    // The deck, over the list, from the row you tapped (or the top).
    if let Some(start) = deck() {
        return rsx! {
            task_widgets::FullscreenExperience {
                title: "Process",
                handle_esc: true,
                on_exit: move |()| deck.set(None),
                ProcessDeck {
                    items: queue(),
                    slug,
                    start,
                    on_exit: move |()| deck.set(None),
                }
            }
        };
    }

    // The list — a page in the shell, like Linear's Inbox: what's
    // waiting, oldest first; tap one to process it, or Process for all.
    let store_rows = result.value().cloned().unwrap_or_default();
    let today = Utc::now().date_naive().to_string();
    let rows: Vec<(usize, InboxItem)> = queue()
        .into_iter()
        .enumerate()
        // Still due as the store has it now (processed ones drop out).
        .filter(|(_, q)| {
            store_rows
                .iter()
                .any(|(_, it)| it.id == q.id && is_due(it, &today))
        })
        .collect();
    let left = rows.len();
    // With captures waiting the main action is processing them; with
    // none, it is capturing one.
    let header_icon = if left > 0 {
        rsx! { architect_ui::lucide_dioxus::Play { size: 17 } }
    } else {
        rsx! { architect_ui::lucide_dioxus::SquarePen { size: 19 } }
    };
    let header_label = if left > 0 {
        "Process the inbox"
    } else {
        "Capture"
    }
    .to_owned();
    rsx! {
        div { class: "mx-auto flex w-full max-w-3xl flex-col px-4 pt-5 pb-6 sm:px-8",
            // The section's name, its main action said in words —
            // "Process 5" — and ⋯ for the rest (Linear's top bar).
            div { class: "-mx-4 -mt-5 pb-1 sm:-mx-8",
                task_ui_core::section_header::SectionHeader {
                    title: "Inbox".to_owned(),
                    action: header_icon,
                    action_label: header_label,
                    action_text: (left > 0).then(|| format!("Process {left}")),
                    on_action: move |()| {
                        if left > 0 {
                            deck.set(Some(0));
                        } else {
                            fleeting.set(true);
                        }
                    },
                    task_ui_core::section_header::SectionMenuItem {
                        label: "Capture",
                        icon: rsx! { architect_ui::lucide_dioxus::SquarePen { size: 18 } },
                        on_click: move |()| fleeting.set(true),
                    }
                }
            }
            if rows.is_empty() {
                p { class: "py-24 text-center text-[15px] text-muted-foreground",
                    "Inbox zero. Everything you captured has a place."
                }
            }
            ul { class: "flex flex-col",
                for (idx, it) in rows.into_iter() {
                    {
                        // t[impl process.capture.marks]
                        let (title, rest) = crate::process::split_title_body(&it.body);
                        let people = known();
                        let mut m = marks(&title, &people);
                        let rest_m = marks(rest.lines().next().unwrap_or_default(), &people);
                        let rest = rest_m.text.clone();
                        let title = std::mem::take(&mut m.text);
                        m.merge(rest_m);
                        let Marks { tags, contexts, people, .. } = m;
                        let with = format!(
                            "With {}",
                            people.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
                        );
                        let kind = CaptureKind::of(&it);
                        let age = ago(&it.created, Utc::now());
                        // A suggestion also says who proposed it.
                        let from = (kind == CaptureKind::Suggested
                            && it.source != "ui"
                            && it.source != "cli")
                            .then(|| format!("from {}", it.source));
                        rsx! {
                            li { key: "{it.id}",
                                button {
                                    r#type: "button",
                                    class: "flex w-full items-start gap-3 rounded-xl px-2 py-2.5 text-left active:bg-accent/60 hover:bg-accent/40",
                                    onclick: move |_| deck.set(Some(idx)),
                                    {kind.tile()}
                                    span { class: "flex min-w-0 flex-1 flex-col gap-0.5 pt-0.5",
                                        span { class: "flex items-baseline gap-2",
                                            span { class: "line-clamp-2 min-w-0 flex-1 text-[16px] leading-[22px] text-foreground", "{title}" }
                                            span { class: "shrink-0 text-[13px] tabular-nums text-muted-foreground", "{age}" }
                                        }
                                        if !rest.is_empty() {
                                            span { class: "line-clamp-1 text-[14px] leading-5 text-muted-foreground", "{rest}" }
                                        }
                                        span { class: "mt-0.5 flex min-w-0 items-center gap-1.5 text-[13px] text-muted-foreground",
                                            span { class: "flex min-w-0 flex-1 flex-wrap items-center gap-1.5",
                                                span { class: if kind == CaptureKind::Suggested { "font-medium text-primary" } else { "" }, "{kind.label()}" }
                                                if let Some(f) = from { span { "· {f}" } }
                                                for t in tags {
                                                    span {
                                                        key: "t-{t}",
                                                        "data-testid": "inbox-tag",
                                                        class: "rounded-md px-1.5 py-px text-[12px] font-medium leading-[18px] {tag_tint(&t)}",
                                                        "#{t}"
                                                    }
                                                }
                                                for c in contexts {
                                                    span {
                                                        key: "c-{c}",
                                                        "data-testid": "inbox-context",
                                                        class: "inline-flex items-center gap-1 rounded-full border border-border px-2 py-px text-[12px] leading-[18px] text-foreground/80",
                                                        architect_ui::lucide_dioxus::AtSign { size: 11 }
                                                        "{c}"
                                                    }
                                                }
                                            }
                                            if !people.is_empty() {
                                                span {
                                                    "data-testid": "inbox-people",
                                                    class: "flex shrink-0 -space-x-1.5",
                                                    "aria-label": "{with}",
                                                    for p in people.into_iter().take(3) {
                                                        span { key: "{p.name}", class: "rounded-full ring-2 ring-background",
                                                            task_ui_core::avatar::Avatar { name: p.name, email: p.email, size: 20 }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The deck: a frozen queue, a cursor, and the card for the item under
/// it. Every outcome advances; Undo steps back.
#[component]
fn ProcessDeck(
    items: Vec<InboxItem>,
    slug: Memo<Option<String>>,
    /// The row the deck opens on (the one tapped in the list).
    #[props(default)]
    start: usize,
    on_exit: EventHandler<()>,
) -> Element {
    let muts = stores::use_inbox_mutations();
    let toast = use_toast();
    let mut cursor = use_signal(move || start);
    let mode = use_signal(Mode::stored);
    let total = items.len();
    let idx = cursor();

    let Some(slug_now) = slug() else {
        return rsx! { crate::states::LoadingState {} };
    };

    if idx >= total {
        return rsx! {
            div { class: "flex h-full flex-col items-center justify-center gap-4 px-6 text-center",
                div { class: "flex size-14 items-center justify-center rounded-2xl bg-primary/15 text-primary",
                    architect_ui::lucide_dioxus::Check { size: 28 }
                }
                Heading { level: HeadingLevel::H2, "Inbox processed" }
                Text { variant: TextVariant::Muted, class: "max-w-md",
                    "Everything you captured has a place. What you put off comes back when it's due."
                }
                Button {
                    variant: ButtonVariant::Primary,
                    on_click: move |_| on_exit.call(()),
                    "Done"
                }
            }
        };
    }

    let item = items[idx].clone();
    let pct = ((idx as f32) / (total.max(1) as f32) * 100.0).round() as i32;
    let positions: Vec<String> = items.iter().map(|i| i.id.clone()).collect();

    let on_done = {
        let slug_now = slug_now.clone();
        move |outcome: Outcome| {
            // t[impl process.undo]
            // Undo puts the item back as it was and steps the deck to it.
            let undo = |before: InboxItem| {
                let slug = slug_now.clone();
                let at = positions.iter().position(|id| *id == before.id);
                move || {
                    muts.save(slug.clone(), before.clone());
                    if let Some(at) = at {
                        // A copy per call: toast actions are `Fn`.
                        let mut c = cursor;
                        c.set(at);
                    }
                }
            };
            match outcome {
                Outcome::Filed(msg) => toast.success(msg, ToastOptions::new()),
                Outcome::Noted => toast.success("Saved as a note".to_owned(), ToastOptions::new()),
                Outcome::Later { before, label } => {
                    toast.info(label, ToastOptions::new().action("Undo", undo(before)));
                }
                Outcome::Done { before } => {
                    toast.success(
                        "Marked done".to_owned(),
                        ToastOptions::new().action("Undo", undo(before)),
                    );
                }
                Outcome::Deleted { before } => {
                    toast.info(
                        "Deleted".to_owned(),
                        ToastOptions::new().action("Undo", undo(before)),
                    );
                }
                Outcome::Skipped => {}
            }
            cursor += 1;
        }
    };

    rsx! {
        div { class: "flex h-full flex-col text-foreground",
            // ── header: leave · progress · mode ──
            header { class: "flex items-center gap-3 px-4 pt-[max(0.75rem,env(safe-area-inset-top))] pb-3 sm:px-8",
                button {
                    r#type: "button",
                    class: "flex h-10 w-10 shrink-0 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
                    title: "Leave (Esc)",
                    aria_label: "Leave processing",
                    onclick: move |_| on_exit.call(()),
                    architect_ui::lucide_dioxus::X { size: 18 }
                }
                div { class: "flex min-w-0 flex-1 flex-col gap-1",
                    div { class: "h-1 overflow-hidden rounded-full bg-muted",
                        div { class: "h-full rounded-full bg-primary transition-all duration-300", style: "width: {pct}%" }
                    }
                    span { class: "font-mono text-[11px] tabular-nums text-muted-foreground", "{idx + 1} of {total}" }
                }
            }
            // ── the card ──
            div { class: "flex min-h-0 flex-1 justify-center overflow-y-auto px-4 pb-[calc(1.5rem+env(safe-area-inset-bottom))] sm:px-8",
                div { class: "flex w-full max-w-xl flex-col py-2",
                    ProcessCard {
                        key: "{item.id}",
                        item,
                        slug: slug_now,
                        mode,
                        skippable: true,
                        on_done,
                    }
                }
            }
        }
    }
}

/// What a capture is, at a glance: where it came from or what kind of
/// note it is, as an icon and a word. The icon leads the row where a bare
/// dot used to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureKind {
    /// Proposed by something else (mail ingestion, an agent) — a yes or no.
    Suggested,
    Email,
    Watch,
    Agent,
    /// A highlight from something read or watched.
    Literature,
    /// Notes taken live.
    Lecture,
    /// Your own passing thought — the default.
    Thought,
}

impl CaptureKind {
    #[must_use]
    pub fn of(item: &InboxItem) -> Self {
        if item.status == InboxItem::STATUS_SUGGESTED {
            return Self::Suggested;
        }
        match item.source.as_str() {
            "email" | "mail" => return Self::Email,
            "watch" => return Self::Watch,
            "agent" | "mcp" | "llm" => return Self::Agent,
            "readwise" | "kindle" => return Self::Literature,
            _ => {}
        }
        match item.kind.as_str() {
            "literature" => Self::Literature,
            "lecture" => Self::Lecture,
            _ => Self::Thought,
        }
    }

    /// The word for it in the row's meta line.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Suggested => "Suggested",
            Self::Email => "Email",
            Self::Watch => "Watch",
            Self::Agent => "Agent",
            Self::Literature => "Reading",
            Self::Lecture => "Lecture",
            Self::Thought => "Thought",
        }
    }

    /// The icon tile's colours: a soft wash and its ink.
    fn tint(self) -> &'static str {
        match self {
            Self::Suggested => "bg-primary/15 text-primary",
            Self::Email => "bg-sky-500/15 text-sky-400",
            Self::Watch => "bg-emerald-500/15 text-emerald-400",
            Self::Agent => "bg-violet-500/15 text-violet-400",
            Self::Literature => "bg-amber-500/15 text-amber-400",
            Self::Lecture => "bg-rose-500/15 text-rose-400",
            Self::Thought => "bg-foreground/[0.07] text-muted-foreground",
        }
    }

    /// The icon tile for this kind.
    pub fn tile(self) -> Element {
        use architect_ui::lucide_dioxus as i;
        let icon = match self {
            Self::Suggested => rsx! { i::Sparkles { size: 16 } },
            Self::Email => rsx! { i::Mail { size: 16 } },
            Self::Watch => rsx! { i::Watch { size: 16 } },
            Self::Agent => rsx! { i::Bot { size: 16 } },
            Self::Literature => rsx! { i::BookOpen { size: 16 } },
            Self::Lecture => rsx! { i::Presentation { size: 16 } },
            Self::Thought => rsx! { i::Lightbulb { size: 16 } },
        };
        rsx! {
            span {
                class: "flex size-8 shrink-0 items-center justify-center rounded-[10px] {self.tint()}",
                "aria-label": "{self.label()}",
                {icon}
            }
        }
    }
}

/// Someone a capture can be about: an org member or a contact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub email: String,
}

impl Person {
    /// The `@handle` that names them: their first name, lowercased.
    fn handle(&self) -> String {
        self.name
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_lowercase()
    }
}

/// A line of a capture, read for what it is marked with: `#tags`,
/// `@contexts`, and the people it mentions — by `@handle` or by first
/// name. `text` is the line as the row shows it: tags and contexts
/// lifted out into badges, an `@handle` said as the person's name.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub text: String,
    pub tags: Vec<String>,
    pub contexts: Vec<String>,
    pub people: Vec<Person>,
}

impl Marks {
    fn merge(&mut self, other: Marks) {
        for t in other.tags {
            push_unique(&mut self.tags, t);
        }
        for c in other.contexts {
            push_unique(&mut self.contexts, c);
        }
        for p in other.people {
            push_unique(&mut self.people, p);
        }
    }
}

fn push_unique<T: PartialEq>(v: &mut Vec<T>, x: T) {
    if !v.contains(&x) {
        v.push(x);
    }
}

/// Read a line for its marks (see [`Marks`]).
#[must_use]
pub fn marks(line: &str, known: &[Person]) -> Marks {
    let mut out = Marks::default();
    let mut words: Vec<String> = Vec::new();
    let by_handle = |h: &str| known.iter().find(|p| p.handle() == h).cloned();
    for word in line.split_whitespace() {
        // Matched without trailing punctuation, which stays in the text.
        let core = word.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
        let tail = &word[core.len()..];
        let name_of = |s: &str| {
            s.chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '/')
                && s.chars().any(char::is_alphabetic)
        };
        if let Some(tag) = core.strip_prefix('#').filter(|t| name_of(t)) {
            push_unique(&mut out.tags, tag.to_owned());
            continue;
        }
        if let Some(at) = core.strip_prefix('@').filter(|t| name_of(t)) {
            if let Some(p) = by_handle(&at.to_lowercase()) {
                words.push(format!("{}{tail}", p.handle_display()));
                push_unique(&mut out.people, p);
            } else {
                push_unique(&mut out.contexts, at.to_owned());
            }
            continue;
        }
        // A first name said plainly ("Reply to Casey") — capitalised,
        // as names are written, so "sam" the word never matches.
        let bare = core.strip_suffix("'s").unwrap_or(core);
        if bare.len() >= 3 && bare.starts_with(char::is_uppercase) {
            if let Some(p) = by_handle(&bare.to_lowercase()) {
                push_unique(&mut out.people, p);
            }
        }
        words.push(word.to_owned());
    }
    out.text = words.join(" ");
    out
}

impl Person {
    /// The name as a sentence says it: the first name, as written.
    fn handle_display(&self) -> String {
        self.name
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned()
    }
}

/// Everyone a capture in this org might mention: its members and its
/// contacts, less the person signed in. Empty until they load.
fn use_known_people(slug: Memo<Option<String>>) -> Memo<Vec<Person>> {
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();
    let contacts = stores::use_contact_list();
    let members = use_resource(move || {
        let slug = slug();
        let token = account.read().as_ref().map(|a| a.token.clone());
        async move {
            match slug {
                Some(s) => crate::feeds::fetch_org_members(&s, token.unwrap_or_default())
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        }
    });
    use_memo(move || {
        let me = account.read().as_ref().map(|a| a.email.to_lowercase());
        let mut people: Vec<Person> = Vec::new();
        let mut add = |name: &str, email: &str| {
            let name = name.trim();
            if name.is_empty() || me.as_deref() == Some(&email.to_lowercase()) {
                return;
            }
            if !people.iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
                people.push(Person {
                    name: name.to_owned(),
                    email: email.to_owned(),
                });
            }
        };
        if let Some(list) = &*members.read() {
            for m in list {
                add(&m.name, &m.email);
            }
        }
        if let Some(rows) = contacts.value() {
            for (_, c) in rows {
                add(&c.full_name, c.primary_email().unwrap_or_default());
            }
        }
        people
    })
}

/// A `#tag`'s badge colours, the same for the same tag everywhere.
fn tag_tint(tag: &str) -> &'static str {
    const TINTS: [&str; 6] = [
        "bg-sky-500/15 text-sky-400",
        "bg-amber-500/15 text-amber-400",
        "bg-emerald-500/15 text-emerald-400",
        "bg-violet-500/15 text-violet-400",
        "bg-rose-500/15 text-rose-400",
        "bg-teal-500/15 text-teal-400",
    ];
    let h = tag.to_lowercase().bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    TINTS[h as usize % TINTS.len()]
}

/// How long ago, the way a list says it: "now", "5m", "3h", "Yesterday",
/// "4d", then the date.
#[must_use]
pub fn ago(created: &str, now: chrono::DateTime<Utc>) -> String {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(created) else {
        return created.get(..10).unwrap_or(created).to_owned();
    };
    let at = at.with_timezone(&Utc);
    let mins = (now - at).num_minutes().max(0);
    match mins {
        0 => "now".to_owned(),
        1..=59 => format!("{mins}m"),
        60..=1439 => format!("{}h", mins / 60),
        1440..=2879 => "Yesterday".to_owned(),
        2880..=10079 => format!("{}d", mins / 1440),
        _ => at
            .with_timezone(&chrono::Local)
            .format("%b %-d")
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_shows_where_it_came_from() {
        let mut i = InboxItem::capture("x", "body", "email", "2026-10-01T00:00:00Z");
        assert_eq!(CaptureKind::of(&i), CaptureKind::Email);
        i.source = "watch".into();
        assert_eq!(CaptureKind::of(&i), CaptureKind::Watch);
        i.source = "ui".into();
        assert_eq!(CaptureKind::of(&i), CaptureKind::Thought);
        i.kind = "literature".into();
        assert_eq!(CaptureKind::of(&i), CaptureKind::Literature);
        i.status = InboxItem::STATUS_SUGGESTED.into();
        assert_eq!(
            CaptureKind::of(&i),
            CaptureKind::Suggested,
            "a suggestion says so first"
        );
    }

    fn cast() -> Vec<Person> {
        ["Sam Reed", "Casey"]
            .iter()
            .map(|n| Person {
                name: (*n).to_owned(),
                email: String::new(),
            })
            .collect()
    }

    #[test]
    fn tags_and_contexts_become_badges_and_leave_the_text() {
        let m = marks("Descale the espresso machine #chore @home.", &cast());
        assert_eq!(m.text, "Descale the espresso machine");
        assert_eq!(m.tags, vec!["chore"]);
        assert_eq!(m.contexts, vec!["home"]);
        assert!(m.people.is_empty());
        // Not marks: a number, an email address, a lone sign.
        let m = marks("Fix #12 for a@b.test # now", &cast());
        assert!(m.tags.is_empty() && m.contexts.is_empty());
        assert_eq!(m.text, "Fix #12 for a@b.test # now");
    }

    #[test]
    fn people_are_found_by_handle_or_by_name() {
        let m = marks("Ask @sam to check the patchbay @office", &cast());
        assert_eq!(m.text, "Ask Sam to check the patchbay");
        assert_eq!(m.people.len(), 1);
        assert_eq!(m.people[0].name, "Sam Reed");
        assert_eq!(
            m.contexts,
            vec!["office"],
            "a handle nobody owns is a context"
        );

        let m = marks("Fold in Casey's notes, and sam the word", &cast());
        assert_eq!(m.people.len(), 1, "a name is capitalised: {m:?}");
        assert_eq!(m.people[0].name, "Casey");
        assert_eq!(m.text, "Fold in Casey's notes, and sam the word");
    }

    #[test]
    fn ages_read_like_a_list() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(ago("2026-10-15T11:55:00Z", now), "5m");
        assert_eq!(ago("2026-10-15T09:00:00Z", now), "3h");
        assert_eq!(ago("2026-10-14T10:00:00Z", now), "Yesterday");
        assert_eq!(ago("2026-10-11T12:00:00Z", now), "4d");
    }

    fn item(status: &str, resurface: Option<&str>) -> InboxItem {
        let mut i = InboxItem::capture("x", "body", "ui", "2026-10-01");
        i.status = status.into();
        i.resurface_on = resurface.map(str::to_owned);
        i
    }

    #[test]
    fn the_deck_holds_due_open_items_and_suggestions() {
        let today = "2026-10-12";
        assert!(is_due(&item("open", None), today));
        assert!(is_due(&item("open", Some("2026-10-12")), today));
        assert!(!is_due(&item("open", Some("2026-10-20")), today), "put off");
        assert!(
            is_due(&item("suggested", None), today),
            "suggestions get a yes or no"
        );
        assert!(!is_due(&item("processed", None), today));
        assert!(!is_due(&item("archived", None), today));
    }
}
