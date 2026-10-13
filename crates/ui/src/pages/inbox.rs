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
    // Fast or Clarify — set from the ⋯ menu, read by the deck.
    let mut style = use_signal(Mode::stored);
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
            div { class: "-mx-4 -mt-2 pb-1 sm:-mx-8",
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
                    task_ui_core::section_header::SectionMenuItem {
                        label: "Processing style",
                        icon: rsx! { architect_ui::lucide_dioxus::SlidersHorizontal { size: 18 } },
                        note: Some(if style() == Mode::Fast { "Fast" } else { "Clarify" }.to_owned()),
                        on_click: move |()| {
                            let next = match style() {
                                Mode::Fast => Mode::Clarify,
                                Mode::Clarify => Mode::Fast,
                            };
                            next.remember();
                            style.set(next);
                        },
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
                        let (title, rest) = crate::process::split_title_body(&it.body);
                        let age = it.created.get(..10).unwrap_or(&it.created).to_owned();
                        let suggested = it.status == InboxItem::STATUS_SUGGESTED;
                        let via = (it.source != "ui" && it.source != "cli").then(|| it.source.clone());
                        rsx! {
                            li { key: "{it.id}",
                                button {
                                    r#type: "button",
                                    class: "flex w-full items-start gap-3 rounded-xl px-2 py-3 text-left active:bg-accent/60 hover:bg-accent/40",
                                    onclick: move |_| deck.set(Some(idx)),
                                    span { class: if suggested { "mt-1.5 size-2 shrink-0 rounded-full bg-primary" } else { "mt-1.5 size-2 shrink-0 rounded-full bg-muted-foreground/40" } }
                                    span { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                                        span { class: "line-clamp-2 text-[15px] leading-6 text-foreground", "{title}" }
                                        if !rest.is_empty() {
                                            span { class: "truncate text-[13px] text-muted-foreground", "{rest}" }
                                        }
                                        span { class: "flex items-center gap-1.5 text-[12px] text-muted-foreground",
                                            if suggested {
                                                span { class: "rounded-full bg-primary/15 px-1.5 text-primary", "Suggested" }
                                            }
                                            if let Some(v) = via { span { "via {v}" } }
                                            span { "{age}" }
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
                ModeSwitch { mode }
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

/// Fast / Clarify, remembered on this device.
#[component]
pub fn ModeSwitch(mode: Signal<Mode>) -> Element {
    let seg = |m: Mode, label: &'static str| {
        let on = mode() == m;
        rsx! {
            button {
                r#type: "button",
                class: if on {
                    "rounded-md bg-background px-2.5 py-1.5 text-xs font-medium text-foreground shadow-sm"
                } else {
                    "rounded-md px-2.5 py-1.5 text-xs text-muted-foreground hover:text-foreground"
                },
                aria_pressed: on.to_string(),
                title: match m {
                    Mode::Fast => "Every outcome at once",
                    Mode::Clarify => "Ask: actionable? next step?",
                },
                onclick: move |_| {
                    mode.set(m);
                    m.remember();
                },
                "{label}"
            }
        }
    };
    rsx! {
        div { class: "inline-flex shrink-0 items-center gap-0.5 rounded-lg bg-muted/50 p-0.5",
            {seg(Mode::Fast, "Fast")}
            {seg(Mode::Clarify, "Clarify")}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
