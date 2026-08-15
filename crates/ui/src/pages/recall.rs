//! `/recall` — the spaced-repetition learning deck.
//!
//! A general, project-filterable deck of flashcards on an FSRS
//! scheduler. The stream lists cards (grouped/filtered by project via
//! chips); "Review" opens a full-screen flashcard flow — the prompt is
//! shown, Space (or a click) reveals the back, then you rate Again /
//! Hard / Good / Easy (keys 1–4). Each rating runs
//! [`spaced_repetition::review`] to reschedule the card and upserts it.
//!
//! Bible study is the seed use case (verse↔reference, first-letter,
//! cloze) but nothing here is Bible-specific: `project` and `card_type`
//! are free-form. State is the shared optimistic store
//! ([`crate::stores`]): every mutation patches the store instantly and
//! reconciles against the server, so leaving review mode needs no
//! refetch.

use architect::Id;
use architect_ui::prelude::*;
use chrono::Utc;
use dioxus::prelude::*;
use recall_proto::{CardType, RecallCard};
use spaced_repetition::Rating;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores;

const INPUT_CLS: &str = "rounded-lg border border-input bg-input/30 px-3 py-2 text-sm transition-colors \
     focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] \
     focus-visible:ring-ring/50 placeholder:text-muted-foreground";

#[component]
pub fn RecallView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The org we author into / review (first selected, or home).
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    // Quick-add draft.
    let mut draft_project = use_signal(String::new);
    let mut draft_front = use_signal(String::new);
    let mut draft_back = use_signal(String::new);
    let draft_type = use_signal(|| CardType::CONCEPT_QA.to_string());

    // Generate-from-note draft.
    let mut gen_path = use_signal(String::new);

    // Project filter chip (None = all decks) + review mode.
    let mut filter: Signal<Option<String>> = use_signal(|| None);
    let mut reviewing = use_signal(|| false);
    let mut queue = use_signal(Vec::<RecallCard>::new);

    let result = stores::use_recall_list();
    let store = stores::use_recall_store();
    let muts = stores::use_recall_mutations();

    let today = Utc::now().date_naive().to_string();

    let all_rows: Vec<(Id<String>, RecallCard)> = result.value().cloned().unwrap_or_default();
    let load_err = result.error().cloned();
    let first_load = result.is_waiting() && result.value().is_none();

    // Distinct project names across active cards → the filter chips.
    let mut projects: Vec<String> = all_rows
        .iter()
        .filter(|(_, c)| !c.archived)
        .map(|(_, c)| c.project.clone())
        .collect();
    projects.sort();
    projects.dedup();

    let active_filter = filter();
    let matches_filter = |c: &RecallCard| match active_filter.as_deref() {
        None => true,
        Some(p) => c.project == p,
    };

    // The stream: active (non-archived) cards under the selected deck.
    let rows: Vec<(Id<String>, RecallCard)> = all_rows
        .iter()
        .filter(|(_, c)| !c.archived && matches_filter(c))
        .cloned()
        .collect();

    // The review work set: due, non-archived cards under the filter,
    // frozen into `queue` when review starts so mutations don't
    // reshuffle it.
    let due: Vec<RecallCard> = all_rows
        .iter()
        .filter(|(_, c)| matches_filter(c) && c.in_review_queue(&today))
        .map(|(_, c)| c.clone())
        .collect();

    // Author the current draft as a fresh card.
    let mut add_card = move || {
        let front = draft_front.read().trim().to_string();
        if front.is_empty() {
            return;
        }
        let Some(s) = slug() else { return };
        let id = uuid::Uuid::new_v4().to_string();
        let created = Utc::now().to_rfc3339();
        let card = RecallCard::create(
            id,
            draft_project.read().trim(),
            draft_type.read().clone(),
            front,
            draft_back.read().trim(),
            created,
        );
        draft_front.set(String::new());
        draft_back.set(String::new());
        muts.create(s, card);
    };

    // Generate a couple of concept-qa cards from a vault note's
    // headings/paragraphs (heuristic split; see `generate_from_note`).
    let generate = move |_| {
        let Some(s) = slug() else { return };
        let path = gen_path.read().trim().to_string();
        if path.is_empty() {
            return;
        }
        let project = draft_project.read().trim().to_string();
        gen_path.set(String::new());
        spawn(async move {
            let Ok(text) = crate::feeds::fetch_note_text(&s, &path).await else {
                return;
            };
            for (front, back) in generate_from_note(&text) {
                let mut card = RecallCard::create(
                    uuid::Uuid::new_v4().to_string(),
                    project.clone(),
                    CardType::CONCEPT_QA,
                    front,
                    back,
                    Utc::now().to_rfc3339(),
                );
                card.source_note = Some(path.clone());
                muts.create(s.clone(), card);
            }
        });
    };

    // Focused review takes over the whole page.
    if reviewing() {
        return rsx! {
            ReviewDeck {
                items: queue(),
                slug,
                on_exit: move |()| reviewing.set(false),
            }
        };
    }

    rsx! {
        div { class: "mx-auto flex max-w-3xl flex-col gap-5 p-4 pb-14 sm:p-6 md:pb-6 lg:p-10",
            div { class: "flex items-center justify-between gap-3",
                Heading { level: HeadingLevel::H1, "Recall" }
                if !due.is_empty() {
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Small,
                        on_click: {
                            let q = due.clone();
                            move |_| {
                                queue.set(q.clone());
                                reviewing.set(true);
                            }
                        },
                        "Review {due.len()} →"
                    }
                } else {
                    Text { variant: TextVariant::Muted, class: "text-sm", "Nothing due" }
                }
            }
            Text {
                variant: TextVariant::Muted,
                class: "text-sm -mt-2",
                "A spaced-repetition deck on an FSRS scheduler. Add cards, filter by deck, and review what's due.",
            }

            // ── Project filter chips ───────────────────────────────
            if !projects.is_empty() {
                div { class: "flex flex-wrap items-center gap-1.5",
                    ProjectChip {
                        label: "All".to_string(),
                        active: active_filter.is_none(),
                        on_pick: move |_| filter.set(None),
                    }
                    for p in projects.clone() {
                        ProjectChip {
                            key: "{p}",
                            label: if p.is_empty() { "(no deck)".to_string() } else { p.clone() },
                            active: active_filter.as_deref() == Some(p.as_str()),
                            on_pick: {
                                let p = p.clone();
                                move |_| filter.set(Some(p.clone()))
                            },
                        }
                    }
                }
            }

            // ── Quick-add ──────────────────────────────────────────
            div { class: "flex flex-col gap-2 rounded-xl border border-border bg-card/40 p-3",
                span { class: "text-sm font-medium text-foreground", "New card" }
                div { class: "flex flex-col gap-2 sm:flex-row",
                    input {
                        class: "{INPUT_CLS} sm:w-40",
                        placeholder: "Deck / project",
                        value: "{draft_project}",
                        oninput: move |e| draft_project.set(e.value()),
                    }
                    Select {
                        value: draft_type,
                        placeholder: "Card type".to_string(),
                        class: "sm:w-48".to_string(),
                        SelectContent {
                            for (i , t) in CardType::all().iter().enumerate() {
                                SelectItem { key: "{t}", value: t.to_string(), index: i, "{t}" }
                            }
                        }
                    }
                }
                input {
                    class: "{INPUT_CLS}",
                    placeholder: "Front (prompt)",
                    value: "{draft_front}",
                    oninput: move |e| draft_front.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            add_card();
                        }
                    },
                }
                textarea {
                    class: "{INPUT_CLS} min-h-16",
                    placeholder: "Back (answer)",
                    value: "{draft_back}",
                    oninput: move |e| draft_back.set(e.value()),
                }
                div { class: "flex justify-end",
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Small,
                        on_click: move |_| add_card(),
                        "Add card"
                    }
                }
            }

            // ── Generate from a note (stub) ────────────────────────
            div { class: "flex flex-col gap-2 rounded-xl border border-dashed border-border bg-card/20 p-3",
                span { class: "text-sm font-medium text-foreground", "Generate from a note" }
                Text {
                    variant: TextVariant::Muted,
                    class: "text-xs",
                    "Splits a vault note's headings/paragraphs into concept cards (heuristic — LLM generation is future work).",
                }
                div { class: "flex flex-col gap-2 sm:flex-row",
                    input {
                        class: "{INPUT_CLS} flex-1",
                        placeholder: "Vault note path, e.g. Wiki/Grace.md",
                        value: "{gen_path}",
                        oninput: move |e| gen_path.set(e.value()),
                    }
                    Button {
                        variant: ButtonVariant::Secondary,
                        size: ButtonSize::Small,
                        on_click: generate,
                        "Generate"
                    }
                }
            }

            // ── The stream ─────────────────────────────────────────
            if first_load {
                crate::states::LoadingState {}
            } else if rows.is_empty() {
                if let Some(err) = load_err {
                    crate::states::ErrorState {
                        title: "Couldn't load deck",
                        message: err,
                        on_retry: move |()| store.reload(),
                    }
                } else {
                    crate::states::EmptyState {
                        title: "No cards yet",
                        hint: "Add a card above, or generate some from a note.",
                    }
                }
            } else {
                div { class: "flex flex-col gap-2",
                    for (id, card) in rows {
                        RecallRow { key: "{id}", pending: id.is_temp(), card, slug }
                    }
                }
            }
        }
    }
}

/// A deck filter chip.
#[component]
fn ProjectChip(label: String, active: bool, on_pick: EventHandler<()>) -> Element {
    let cls = if active {
        "rounded-full border border-primary bg-primary/15 px-2.5 py-1 text-xs text-foreground"
    } else {
        "rounded-full border border-border bg-card/40 px-2.5 py-1 text-xs text-muted-foreground hover:text-foreground"
    };
    rsx! {
        button { r#type: "button", class: "{cls}", onclick: move |_| on_pick.call(()), "{label}" }
    }
}

/// One card in the stream: prompt + answer preview, its badges, and
/// archive / delete actions.
#[component]
fn RecallRow(card: RecallCard, slug: Memo<Option<String>>, pending: bool) -> Element {
    let muts = stores::use_recall_mutations();
    let card = use_signal(|| card);

    let snap = card.read();
    let front = snap.front.clone();
    let back = snap.back.clone();
    let project = snap.project.clone();
    let card_type = snap.card_type.clone();
    let reps = snap.reps;
    let due = snap.due.clone();
    drop(snap);

    let archive = move |_| {
        let Some(s) = slug() else { return };
        let mut next = card();
        next.archived = true;
        muts.save(s, next);
    };
    let delete = move |_| {
        let Some(s) = slug() else { return };
        muts.delete(s, card().id);
    };

    let state_cls = if pending {
        "border-border bg-card/40 opacity-60"
    } else {
        "border-border bg-card/40"
    };

    rsx! {
        div { class: "flex flex-col gap-2 rounded-lg border px-3 py-2.5 sm:flex-row sm:items-start sm:gap-3 sm:py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-1",
                Text { class: "whitespace-pre-wrap break-words text-sm font-medium", "{front}" }
                if !back.is_empty() {
                    Text { variant: TextVariant::Muted, class: "whitespace-pre-wrap break-words text-sm", "{back}" }
                }
                div { class: "flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground",
                    if !project.is_empty() {
                        span { class: "rounded bg-muted px-1.5 py-px", "{project}" }
                    }
                    span { class: "rounded bg-muted px-1.5 py-px", "{card_type}" }
                    if reps > 0 {
                        span { "{reps} reviews" }
                    }
                    if let Some(d) = due.as_ref() {
                        span { class: "rounded bg-muted px-1.5 py-px", "due {d}" }
                    } else {
                        span { class: "rounded bg-muted px-1.5 py-px", "new" }
                    }
                }
            }
            div { class: "flex shrink-0 items-center gap-1 self-end sm:self-auto",
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    on_click: archive,
                    "Archive"
                }
                Button {
                    variant: ButtonVariant::Destructive,
                    size: ButtonSize::Small,
                    on_click: delete,
                    "Delete"
                }
            }
        }
    }
}

/// Full-screen flashcard flow: walk a frozen queue one card at a time.
/// The front is shown; Space (or click) reveals the back; rate 1–4 to
/// reschedule via FSRS and advance. Decisions are optimistic store
/// mutations, so exiting just exits.
#[component]
fn ReviewDeck(
    items: Vec<RecallCard>,
    slug: Memo<Option<String>>,
    on_exit: EventHandler<()>,
) -> Element {
    let muts = stores::use_recall_mutations();
    // Hold the frozen queue in a Copy signal so the per-rating +
    // keyboard closures stay Copy (a moved `Vec` capture wouldn't be).
    let items = use_signal(|| items);
    let mut cursor = use_signal(|| 0usize);
    let mut revealed = use_signal(|| false);
    let total = items.read().len();
    let idx = cursor();

    if idx >= total {
        return rsx! {
            div { class: "mx-auto flex max-w-2xl flex-col items-center gap-4 p-6 pt-[12vh] text-center lg:p-10",
                div { class: "text-5xl", "🎉" }
                Heading { level: HeadingLevel::H2, "Deck clear" }
                Text { variant: TextVariant::Muted, "You've reviewed everything due." }
                Button {
                    variant: ButtonVariant::Primary,
                    on_click: move |_| on_exit.call(()),
                    "Back to deck"
                }
            }
        };
    }

    let card = items.read()[idx].clone();
    let front = render_front(&card);
    let back = card.back.clone();
    let project = card.project.clone();
    let card_type = card.card_type.clone();
    let pct = ((idx as f32) / (total.max(1) as f32) * 100.0).round() as i32;
    let today = Utc::now().date_naive();

    // Rate the current card, reschedule, and advance to the next.
    let mut rate = move |rating: Rating| {
        if let Some(s) = slug() {
            muts.review(s, items.read()[idx].clone(), rating, today);
        }
        revealed.set(false);
        cursor += 1;
    };

    // Keyboard: Space reveals; 1–4 rate (once revealed).
    let on_key = move |e: KeyboardEvent| match e.key() {
        Key::Character(c) if c == " " => {
            e.prevent_default();
            revealed.set(true);
        }
        Key::Character(c) if revealed() => match c.as_str() {
            "1" => rate(Rating::Again),
            "2" => rate(Rating::Hard),
            "3" => rate(Rating::Good),
            "4" => rate(Rating::Easy),
            _ => {}
        },
        _ => {}
    };

    rsx! {
        div {
            class: "mx-auto flex max-w-2xl flex-col gap-4 p-4 outline-none sm:p-6 lg:p-10",
            tabindex: 0,
            autofocus: true,
            onkeydown: on_key,
            onmounted: move |e| {
                spawn(async move {
                    let _ = e.set_focus(true).await;
                });
            },
            // Progress + exit.
            div { class: "flex items-center justify-between",
                Text { variant: TextVariant::Muted, class: "text-sm", "Reviewing {idx + 1} of {total}" }
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    on_click: move |_| on_exit.call(()),
                    "Exit"
                }
            }
            div { class: "h-1 w-full overflow-hidden rounded-full bg-muted",
                div { class: "h-full rounded-full bg-primary transition-all", style: "width: {pct}%" }
            }

            // The prompt (click anywhere to reveal).
            div {
                class: "flex min-h-40 cursor-pointer flex-col items-center justify-center gap-3 rounded-xl border border-border bg-card/40 p-6 text-center",
                onclick: move |_| revealed.set(true),
                div { class: "flex flex-wrap items-center justify-center gap-2 text-[11px] text-muted-foreground",
                    if !project.is_empty() {
                        span { class: "rounded bg-muted px-1.5 py-px", "{project}" }
                    }
                    span { class: "rounded bg-muted px-1.5 py-px", "{card_type}" }
                }
                Text { class: "whitespace-pre-wrap break-words text-lg font-medium", "{front}" }
                if revealed() {
                    div { class: "mt-2 w-full border-t border-border pt-3",
                        Text { class: "whitespace-pre-wrap break-words text-base", "{back}" }
                    }
                } else {
                    Text { variant: TextVariant::Muted, class: "text-xs", "Press Space or click to reveal" }
                }
            }

            // Ratings (enabled once revealed).
            if revealed() {
                div { class: "flex flex-wrap gap-2",
                    Button {
                        variant: ButtonVariant::Destructive,
                        class: "min-h-11 flex-1",
                        on_click: move |_| rate(Rating::Again),
                        "Again (1)"
                    }
                    Button {
                        variant: ButtonVariant::Outline,
                        class: "min-h-11 flex-1",
                        on_click: move |_| rate(Rating::Hard),
                        "Hard (2)"
                    }
                    Button {
                        variant: ButtonVariant::Secondary,
                        class: "min-h-11 flex-1",
                        on_click: move |_| rate(Rating::Good),
                        "Good (3)"
                    }
                    Button {
                        variant: ButtonVariant::Primary,
                        class: "min-h-11 flex-1",
                        on_click: move |_| rate(Rating::Easy),
                        "Easy (4)"
                    }
                }
            } else {
                div { class: "flex justify-center",
                    Button {
                        variant: ButtonVariant::Primary,
                        class: "min-h-11 w-full sm:w-auto",
                        on_click: move |_| revealed.set(true),
                        "Reveal"
                    }
                }
            }
        }
    }
}

/// The prompt text for a card, respecting its type. First-letter cards
/// mask the front to each word's initial; everything else shows the
/// front verbatim.
fn render_front(card: &RecallCard) -> String {
    if card.card_type == CardType::FIRST_LETTER {
        first_letters(&card.front)
    } else {
        card.front.clone()
    }
}

/// Replace every word with its first character — the "first-letter"
/// memory aid (e.g. "For God so loved" → "F G s l").
fn first_letters(s: &str) -> String {
    s.split_whitespace()
        .map(|w| w.chars().next().map(String::from).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Heuristic note → concept cards. Splits on ATX markdown headings:
/// each heading becomes a card front, the text up to the next heading
/// its back. Capped so one note doesn't flood the deck.
///
// FUTURE: LLM generation — replace this heading/paragraph heuristic
// with a model that reads the note and writes real Q&A / cloze cards.
fn generate_from_note(text: &str) -> Vec<(String, String)> {
    const MAX_CARDS: usize = 3;
    let mut out: Vec<(String, String)> = Vec::new();
    let mut heading: Option<String> = None;
    let mut body: Vec<&str> = Vec::new();

    let flush = |out: &mut Vec<(String, String)>, heading: &Option<String>, body: &[&str]| {
        if let Some(h) = heading {
            let back = body.join("\n").trim().to_string();
            if !back.is_empty() {
                out.push((format!("What is {h}?"), back));
            }
        }
    };

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('#') {
            // New heading — flush the previous section.
            flush(&mut out, &heading, &body);
            if out.len() >= MAX_CARDS {
                return out;
            }
            heading = Some(rest.trim_start_matches('#').trim().to_string());
            body.clear();
        } else if !trimmed.is_empty() {
            body.push(trimmed);
        }
    }
    flush(&mut out, &heading, &body);
    out.truncate(MAX_CARDS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_letters_masks_words() {
        assert_eq!(first_letters("For God so loved"), "F G s l");
    }

    #[test]
    fn generate_splits_on_headings() {
        let note = "# Grace\nUnmerited favor from God.\n\n## Faith\nTrust in what is unseen.\n";
        let cards = generate_from_note(note);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].0, "What is Grace?");
        assert_eq!(cards[0].1, "Unmerited favor from God.");
        assert_eq!(cards[1].0, "What is Faith?");
    }
}
