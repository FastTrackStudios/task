//! `/inbox` — the FLAP capture queue + daily review.
//!
//! Capture anything with near-zero friction (the quick-add box), then
//! honour the "temporal contract": read the open items again later and
//! either process them into something durable, snooze them to resurface
//! on a date, or let them go. The list shows `open` items oldest-first;
//! snoozed items stay hidden until their date (toggle "Show all" to see
//! everything, including processed + archived).
//!
//! State is the shared optimistic store ([`crate::stores`]): every
//! mutation — capture, status flips, snoozes, deletes, and the focused
//! ProcessReview decisions — patches the store instantly and reconciles
//! against the server (rollback + tray notification on failure), so
//! leaving review mode needs no refetch: the store already reflects
//! every decision.

use chrono::Utc;
use dioxus::prelude::*;
use architect_ui::prelude::*;
use inbox_proto::{InboxItem, ReviewResponse, review};

use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores;

#[component]
pub fn InboxView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The org we capture into / review (first selected, or home).
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    let nav = use_navigator();
    let result = stores::use_inbox_list();
    let store = stores::use_inbox_store();
    let today = Utc::now().date_naive().to_string();

    // The inbox IS the review deck. Freeze the due set into a queue ONCE
    // (when the list first loads) so decisions — which mutate the shared
    // store optimistically — never reshuffle the cards under the cursor.
    let mut queue = use_signal(Vec::<InboxItem>::new);
    let mut seeded = use_signal(|| false);
    let seed_result = result.clone();
    use_effect(move || {
        if *seeded.peek() {
            return;
        }
        if let Some(rows) = seed_result.value() {
            let due: Vec<InboxItem> = rows
                .iter()
                .filter(|(_, it)| {
                    it.is_open()
                        && it
                            .resurface_on
                            .as_deref()
                            .is_none_or(|d| d <= today.as_str())
                })
                .map(|(_, it)| it.clone())
                .collect();
            queue.set(due);
            seeded.set(true);
        }
    });

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

    // The inbox triage IS an Experience — a full-screen, focused deck.
    // The shared chrome gives it the consistent overlay + top-bar exit;
    // `handle_esc: false` because ProcessReview binds Esc (and the triage
    // keys) itself.
    rsx! {
        task_widgets::FullscreenExperience {
            title: "Inbox",
            handle_esc: false,
            on_exit: move |()| {
                nav.push(crate::routes::Route::DashboardRoute {});
            },
            ProcessReview {
                items: queue(),
                slug,
                on_exit: move |()| {
                    nav.push(crate::routes::Route::DashboardRoute {});
                },
            }
        }
    }
}

/// One row in the review queue. Its own component so each row's action
/// closures capture just that item by value.
#[component]
fn InboxRow(item: InboxItem, slug: Memo<Option<String>>, pending: bool) -> Element {
    let muts = stores::use_inbox_mutations();
    // Hold the item in a Copy `Signal` so each action closure captures
    // only Copy handles and stays cheap to clone into the `on_click`s.
    let item = use_signal(|| item);

    let snap = item.read();
    let open = snap.is_open();
    let body = snap.body.clone();
    let kind = snap.kind.clone();
    let status = snap.status.clone();
    let created = snap.created.clone();
    let date = created.get(..10).unwrap_or(&created).to_string();
    let resurface = snap.resurface_on.clone();
    drop(snap);

    // Flip this item's status (process / archive / reopen): optimistic
    // store patch + write-through.
    let set_status = move |status: &'static str| {
        let Some(s) = slug() else { return };
        let mut next = item();
        next.status = status.to_string();
        muts.save(s, next);
    };

    // Snooze a week out — resurfaces in the daily queue then.
    let snooze = move || {
        let Some(s) = slug() else { return };
        let mut next = item();
        let until = (Utc::now().date_naive() + chrono::Duration::days(7)).to_string();
        next.resurface_on = Some(until);
        muts.save(s, next);
    };

    let delete = move || {
        let Some(s) = slug() else { return };
        muts.delete(s, item().id);
    };

    // Closed items already read as muted; layer pending on top. A
    // failed write rolls back and reports to the notification tray.
    let state_cls = if pending || !open {
        "border-border bg-card/40 opacity-60"
    } else {
        "border-border bg-card/40"
    };

    rsx! {
        div { class: "flex flex-col gap-2 rounded-lg border px-3 py-2.5 sm:flex-row sm:items-start sm:gap-3 sm:py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-1",
                Text { class: "whitespace-pre-wrap break-words text-sm", "{body}" }
                div { class: "flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground",
                    span { class: "rounded bg-muted px-1.5 py-px", "{kind}" }
                    span { "{date}" }
                    if !open {
                        span { class: "rounded bg-muted px-1.5 py-px", "{status}" }
                    }
                    if let Some(r) = resurface.as_ref() {
                        span { class: "rounded bg-muted px-1.5 py-px", "💤 {r}" }
                    }
                }
            }
            div { class: "flex shrink-0 items-center gap-1 self-end sm:self-auto",
                if open {
                    Button {
                        variant: ButtonVariant::Secondary,
                        size: ButtonSize::Small,
                        on_click: move |_| set_status(InboxItem::STATUS_PROCESSED),
                        "Process"
                    }
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        on_click: move |_| snooze(),
                        "Snooze 1w"
                    }
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        on_click: move |_| set_status(InboxItem::STATUS_ARCHIVED),
                        "Archive"
                    }
                } else {
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        on_click: move |_| set_status(InboxItem::STATUS_OPEN),
                        "Reopen"
                    }
                    Button {
                        variant: ButtonVariant::Destructive,
                        size: ButtonSize::Small,
                        on_click: move |_| delete(),
                        "Delete"
                    }
                }
            }
        }
    }
}

/// One agent-suggested capture: the proposed text + a one-tap Accept
/// (→ enters the open review queue) or Dismiss (→ deleted).
#[component]
fn SuggestedRow(item: InboxItem, slug: Memo<Option<String>>, pending: bool) -> Element {
    let muts = stores::use_inbox_mutations();
    let item = use_signal(|| item);
    let snap = item.read();
    let body = snap.body.clone();
    let source = snap.source.clone();
    drop(snap);

    let accept = move |_| {
        let Some(s) = slug() else { return };
        let mut next = item();
        next.status = InboxItem::STATUS_OPEN.to_string();
        muts.save(s, next);
    };
    let dismiss = move |_| {
        let Some(s) = slug() else { return };
        muts.delete(s, item().id);
    };

    let state_cls = if pending {
        "border-border bg-card/60 opacity-60"
    } else {
        "border-border bg-card/60"
    };

    rsx! {
        div { class: "flex flex-col gap-2 rounded-lg border px-3 py-2.5 sm:flex-row sm:items-start sm:gap-3 sm:py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                Text { class: "whitespace-pre-wrap break-words text-sm", "{body}" }
                span { class: "text-[11px] text-muted-foreground", "via {source}" }
            }
            div { class: "flex shrink-0 items-center gap-1 self-end sm:self-auto",
                Button {
                    variant: ButtonVariant::Primary,
                    size: ButtonSize::Small,
                    on_click: accept,
                    "Accept"
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    on_click: dismiss,
                    "Dismiss"
                }
            }
        }
    }
}

/// Focused daily-review ("process") mode: walk a frozen queue of open
/// items one at a time and decide what each becomes — a Task, an atomic
/// note, a snooze, done, or gone. Mirrors the FLAP processing ritual.
/// Every decision is an optimistic store mutation (rollback + tray
/// notification on failure) and the cursor advances immediately; the
/// queue is a snapshot so it never reshuffles under you.
#[component]
fn ProcessReview(
    items: Vec<InboxItem>,
    slug: Memo<Option<String>>,
    on_exit: EventHandler<()>,
) -> Element {
    let muts = stores::use_inbox_mutations();
    let mut cursor = use_signal(|| 0usize);
    let total = items.len();
    let idx = cursor();

    // One dispatcher for every decision — a Copy `Callback`, so the
    // buttons AND the keyboard both fire it. The item is passed in each
    // call (never a stale capture). Every decision advances the cursor.
    let act = use_callback(move |(item, action): (InboxItem, Act)| {
        let Some(s) = slug() else {
            cursor += 1;
            return;
        };
        match action {
            Act::Task => {
                let (title, details) = split_title_body(&item.body);
                muts.promote_to_task(s, item, title, details);
            }
            Act::Note => {
                let (title, _) = split_title_body(&item.body);
                let path = format!(
                    "Wiki/Atomic/{}-{}.md",
                    slugify(&title),
                    item.id.get(..6).unwrap_or("note")
                );
                let md = atomic_markdown(&title, &item.body, &Utc::now().to_rfc3339());
                muts.promote_to_note(s, item, path, md);
            }
            Act::Done => {
                let mut done = item;
                done.status = InboxItem::STATUS_PROCESSED.to_string();
                muts.save(s, done);
            }
            Act::Delete => muts.delete(s, item.id),
            Act::Skip => {}
            Act::Rate(resp) => {
                // Spaced-repetition reschedule (obsidian-SR SM-2): urgency
                // escalates the interval; the item stays open and resurfaces
                // on its new due date.
                let today = Utc::now().date_naive();
                let (interval, ease, resurface_on, reviews) = review(&item, resp, today);
                let mut next = item;
                next.interval = interval;
                next.ease = ease;
                next.resurface_on = Some(resurface_on);
                next.reviews = reviews;
                muts.save(s, next);
            }
        }
        cursor += 1;
    });

    if idx >= total {
        return rsx! {
            div { class: "flex h-full flex-col items-center justify-center gap-4 px-6 text-center",
                div { class: "text-6xl", "🃏" }
                Heading { level: HeadingLevel::H2, "Deck cleared" }
                Text { variant: TextVariant::Muted, class: "max-w-md",
                    "Every due card is processed. Come back tomorrow — the schedule resurfaces only what still matters."
                }
                Button {
                    variant: ButtonVariant::Primary,
                    on_click: move |_| on_exit.call(()),
                    "Back to inbox"
                }
            }
        };
    }

    let item = items[idx].clone();
    let body = item.body.clone();
    let kind = item.kind.clone();
    let source = item.source.clone();
    let date = item.created.get(..10).unwrap_or(&item.created).to_string();
    let reviews = item.reviews;
    let interval = item.interval;
    let pct = ((idx as f32) / (total.max(1) as f32) * 100.0).round() as i32;

    // Preview where each urgency rating would push the item.
    let today = Utc::now().date_naive();
    let iv_urgent = review(&item, ReviewResponse::Hard, today).0;
    let iv_maybe = review(&item, ReviewResponse::Good, today).0;
    let iv_someday = review(&item, ReviewResponse::Easy, today).0;

    rsx! {
        div {
            class: "flex h-full flex-col text-foreground outline-none",
            tabindex: "0",
            autofocus: true,
            onkeydown: {
                let item = item.clone();
                move |e: KeyboardEvent| {
                    // Esc leaves the deck; letters/digits drive the triage.
                    if e.key() == Key::Escape {
                        e.prevent_default();
                        on_exit.call(());
                        return;
                    }
                    let a = if let Key::Character(c) = e.key() {
                        match c.as_str() {
                            "1" => Some(Act::Rate(ReviewResponse::Hard)),
                            "2" => Some(Act::Rate(ReviewResponse::Good)),
                            "3" => Some(Act::Rate(ReviewResponse::Easy)),
                            "c" | "d" => Some(Act::Done),
                            "t" => Some(Act::Task),
                            "n" => Some(Act::Note),
                            "x" => Some(Act::Delete),
                            " " => Some(Act::Skip),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    if let Some(a) = a {
                        e.prevent_default();
                        act.call((item.clone(), a));
                    }
                }
            },

            // ── header: leave · progress · position ──
            header { class: "flex items-center gap-4 px-5 py-3 sm:px-8",
                button {
                    r#type: "button",
                    class: "flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
                    title: "Exit review (Esc)",
                    onclick: move |_| on_exit.call(()),
                    architect_ui::lucide_dioxus::X { size: 16 }
                }
                div { class: "h-1 flex-1 overflow-hidden rounded-full bg-muted",
                    div { class: "h-full rounded-full bg-primary transition-all duration-300", style: "width: {pct}%" }
                }
                span { class: "shrink-0 font-mono text-xs tabular-nums text-muted-foreground", "{idx + 1} / {total}" }
            }

            // ── the card, centered, deck stacked behind it ──
            div { class: "relative flex flex-1 items-center justify-center px-4 sm:px-8",
                div { class: "pointer-events-none absolute left-1/2 top-1/2 h-[min(58vh,30rem)] w-full max-w-2xl -translate-x-1/2 -translate-y-1/2",
                    div { class: "absolute inset-0 translate-y-4 scale-[0.955] rounded-3xl border border-border/40 bg-card/30" }
                    div { class: "absolute inset-0 translate-y-2 scale-[0.978] rounded-3xl border border-border/50 bg-card/50" }
                }
                article {
                    key: "{item.id}",
                    class: "relative flex h-[min(58vh,30rem)] w-full max-w-2xl flex-col rounded-3xl border border-border bg-card p-8 shadow-xl sm:p-10",
                    // eyebrow: what/when/where + the due-date stamp
                    div { class: "flex items-center gap-2 text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                        span { "{kind}" }
                        span { class: "text-border", "·" }
                        span { "{date}" }
                        if source != "ui" && source != "cli" {
                            span { class: "text-border", "·" }
                            span { "via {source}" }
                        }
                        if reviews > 0 {
                            span { class: "ml-auto rounded border border-border/70 px-2 py-0.5 font-mono text-[10px] normal-case tracking-normal text-muted-foreground",
                                "seen {reviews}\u{d7} · {fmt_interval(interval)}"
                            }
                        }
                    }
                    // the capture — the hero of the card
                    div { class: "mt-6 flex-1 overflow-y-auto",
                        p { class: "whitespace-pre-wrap break-words text-2xl font-medium leading-relaxed text-foreground sm:text-[1.75rem] sm:leading-relaxed",
                            "{body}"
                        }
                    }
                }
            }

            // ── decision bar ──
            footer { class: "flex flex-col gap-3 border-t border-border/60 bg-card/30 px-4 py-4 sm:px-8",
                // Temperature triage: warm = keep near, cool = file far.
                div { class: "mx-auto grid w-full max-w-2xl grid-cols-3 gap-3",
                    {urgency_button(act, item.clone(), ReviewResponse::Hard, "Urgent", iv_urgent, "1",
                        "border-rose-500/50 bg-rose-500/10 text-rose-200 hover:border-rose-500/70 hover:bg-rose-500/20")}
                    {urgency_button(act, item.clone(), ReviewResponse::Good, "Maybe", iv_maybe, "2",
                        "border-amber-500/50 bg-amber-500/10 text-amber-200 hover:border-amber-500/70 hover:bg-amber-500/20")}
                    {urgency_button(act, item.clone(), ReviewResponse::Easy, "Someday", iv_someday, "3",
                        "border-indigo-500/50 bg-indigo-500/10 text-indigo-200 hover:border-indigo-500/70 hover:bg-indigo-500/20")}
                }
                // Handle it now.
                div { class: "mx-auto flex w-full max-w-2xl flex-wrap items-center justify-center gap-1.5 text-sm",
                    {act_chip(act, item.clone(), Act::Done, "Complete", "c", "hover:bg-emerald-500/15 hover:text-emerald-300")}
                    {act_chip(act, item.clone(), Act::Task, "→ Task", "t", "hover:bg-accent hover:text-foreground")}
                    {act_chip(act, item.clone(), Act::Note, "→ Note", "n", "hover:bg-accent hover:text-foreground")}
                    {act_chip(act, item.clone(), Act::Delete, "Delete", "x", "hover:bg-destructive/15 hover:text-destructive")}
                }
            }
        }
    }
}

/// A decision the review dispatcher can carry out.
#[derive(Clone)]
enum Act {
    Task,
    Note,
    Done,
    Delete,
    Skip,
    Rate(ReviewResponse),
}

/// A big temperature-graded urgency button: label, the interval it would
/// schedule, and its key hint. Colour encodes how far it files the card.
fn urgency_button(
    act: Callback<(InboxItem, Act)>,
    item: InboxItem,
    resp: ReviewResponse,
    label: &str,
    interval: i64,
    key: &str,
    color: &str,
) -> Element {
    let label = label.to_string();
    let key = key.to_string();
    rsx! {
        button {
            r#type: "button",
            class: "flex min-h-[4.5rem] flex-col items-center justify-center gap-0.5 rounded-2xl border text-center transition-colors {color}",
            onclick: move |_| act.call((item.clone(), Act::Rate(resp))),
            span { class: "text-base font-semibold leading-none", "{label}" }
            span { class: "text-xs opacity-80", "{fmt_interval(interval)}" }
            span { class: "mt-1 rounded bg-foreground/10 px-1.5 font-mono text-[10px] opacity-80", "{key}" }
        }
    }
}

/// A small secondary action chip (Complete / Task / Note / Delete).
fn act_chip(
    act: Callback<(InboxItem, Act)>,
    item: InboxItem,
    action: Act,
    label: &str,
    key: &str,
    hover: &str,
) -> Element {
    let label = label.to_string();
    let key = key.to_string();
    rsx! {
        button {
            r#type: "button",
            class: "flex items-center gap-1.5 rounded-lg px-3 py-2 text-muted-foreground transition-colors {hover}",
            onclick: move |_| act.call((item.clone(), action.clone())),
            span { "{label}" }
            span { class: "rounded bg-muted px-1 font-mono text-[10px]", "{key}" }
        }
    }
}

/// Human-friendly interval label ("today", "1d", "3w", "2mo").
fn fmt_interval(days: i64) -> String {
    if days <= 0 {
        "today".to_string()
    } else if days == 1 {
        "1d".to_string()
    } else if days < 7 {
        format!("{days}d")
    } else if days < 30 {
        format!("{}w", (days as f64 / 7.0).round() as i64)
    } else if days < 365 {
        format!("{}mo", (days as f64 / 30.0).round() as i64)
    } else {
        format!("{:.1}y", days as f64 / 365.0)
    }
}

/// First non-empty line (capped) as the title; the remainder as the
/// body. Used to seed a promoted Task's title + details.
fn split_title_body(body: &str) -> (String, String) {
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
fn slugify(s: &str) -> String {
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

/// Markdown for a promoted atomic note: frontmatter (title / `atomic`
/// type + tag / created) over the verbatim capture as the body.
fn atomic_markdown(title: &str, body: &str, created: &str) -> String {
    let esc = title.replace('"', "'");
    format!(
        "---\ntitle: \"{esc}\"\ntype: atomic\ntags:\n  - atomic\ncreated: {created}\n---\n\n{body}\n"
    )
}
