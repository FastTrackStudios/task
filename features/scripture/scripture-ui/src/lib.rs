//! `/scripture` — the Bible reader + study experience.
//!
//! Reads the org's installed scripture (the read-only Resources Library
//! spine) via [`scripture_proto::ScriptureService`]. Pick a translation
//! and book, page through chapters, and read verses. Each verse carries
//! its stable OSIS id as the element `id`, so it's a permalink anchor —
//! and the route's `reference` param deep-links straight to a passage
//! (`/scripture?reference=John 3:16`), which is where a note's
//! `[[John 3:16]]` chip lands.
//!
//! Clicking a verse opens the STUDY PANEL — interlinear original text
//! (TAGNT / TAHOT / SBLGNT / OSHB), Strong's word tokens, OpenBible
//! cross-references and topics — with word-level drill-down into the
//! full lexicon + concordance word study.
//!
//! Read-only: there's no editing here. Verses come from `ChapterView`
//! DTOs; the heavy `VerseId`/`Book` logic stays server-side except for
//! reference *parsing*, which the proto crate ships wasm-clean.

use std::collections::BTreeMap;

use architect_ui::prelude::*;
use dioxus::prelude::*;
use scripture_proto::{
    Book, ChapterView, ComparisonView, ScriptureRef, VerseBacklinks, WordStudyReport,
};

use task_ui_core::feeds;
use task_ui_core::nav::use_note_href;
use task_ui_core::orgs::{OrgMeta, OrgSelection};

const CTRL_CLS: &str = "rounded-lg border border-input bg-input/30 px-3 py-2 text-sm transition-colors \
     focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] \
     focus-visible:ring-ring/50";

/// Where a backlink goes: a vault note to its page; a media source
/// (`sermon:<slug>`) to the FastTrackStudio watch screen, opened at the
/// second the verse was named.
fn backlink_href(
    n: &scripture_proto::VerseBacklink,
    note_href: &Callback<String, String>,
) -> String {
    if n.source_kind.is_empty() {
        return note_href.call(n.note_path.clone());
    }
    task_plugin_ui::href(
        "fasttrackstudio",
        "",
        &format!("node={}&t={}", task_plugin_ui::encode(&n.note_path), n.secs),
    )
}

/// `Title` for a note; `Title · MM:SS` for a media source.
fn backlink_label(n: &scripture_proto::VerseBacklink) -> String {
    if n.source_kind.is_empty() {
        return n.note_title.clone();
    }
    let (m, s) = (n.secs / 60, n.secs % 60);
    let stamp = if m >= 60 {
        format!("{}:{:02}:{s:02}", m / 60, m % 60)
    } else {
        format!("{m}:{s:02}")
    };
    format!("{} · {stamp}", n.note_title)
}

/// The study panel's tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StudyTab {
    Interlinear,
    Words,
    CrossRefs,
    Topics,
}

impl StudyTab {
    const ALL: [(StudyTab, &'static str); 4] = [
        (StudyTab::Interlinear, "Interlinear"),
        (StudyTab::Words, "Words"),
        (StudyTab::CrossRefs, "Cross-refs"),
        (StudyTab::Topics, "Topics"),
    ];
}

/// The verse the study panel is anchored on.
#[derive(Clone, PartialEq, Eq)]
struct SelectedVerse {
    /// OSIS id, `John.3.16` — the RPC key.
    osis: String,
    /// Display reference, `John 3:16`.
    display: String,
}

#[component]
pub fn ScriptureView(reference: String) -> Element {
    // Backlinks link out to the vault note. The shell owns the router,
    // so it hands the href builder down (see `task_ui_core::nav`).
    let note_href = use_note_href();
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let slug = use_memo(move || {
        task_ui_core::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    let mut translation = use_signal(|| "WEB".to_string());
    let mut book = use_signal(|| "John".to_string());
    let mut chapter = use_signal(|| 1u16);

    // Study state.
    let mut selected = use_signal(|| None::<SelectedVerse>);
    let mut study_tab = use_signal(|| StudyTab::Interlinear);
    let mut strongs_sel = use_signal(|| None::<String>);
    let mut topic_sel = use_signal(|| None::<String>);
    // OSIS id to scroll to once the chapter renders.
    let mut anchor = use_signal(|| None::<String>);

    // Compare panel state.
    let mut compare_ref = use_signal(String::new);
    let mut compare_tx = use_signal(String::new);
    let mut compare_query = use_signal(|| None::<(String, Vec<String>)>);

    // Deep link: `?reference=John 3:16-20@ESV` positions the reader
    // (translation qualifier included) and pre-selects the start verse.
    // Re-runs when the route param changes (chip → chip navigation).
    use_effect(use_reactive!(|(reference,)| {
        let Ok(scref) = ScriptureRef::parse(&reference) else {
            return;
        };
        if let Some(tx) = &scref.translation {
            translation.set(tx.clone());
        }
        let start = scref.range.start;
        book.set(start.book.name().to_string());
        chapter.set(start.chapter);
        selected.set(Some(SelectedVerse {
            osis: start.osis(),
            display: start.to_string(),
        }));
        anchor.set(Some(start.osis()));
        strongs_sel.set(None);
        topic_sel.set(None);
    }));

    // Installed translations for the picker.
    let translations = use_resource(move || async move {
        match slug() {
            Some(s) => fetch_translations(&s).await.unwrap_or_default(),
            None => Vec::new(),
        }
    });
    let tx_list = translations.read().clone().unwrap_or_default();

    // The current chapter — re-fetches whenever a picker changes.
    let view = use_resource(move || async move {
        let s = slug()?;
        fetch_chapter(&s, &translation(), &book(), chapter())
            .await
            .ok()
    });
    let pending = view.read().is_none();
    let chapter_view: Option<ChapterView> = view.read().clone().flatten();
    let chapter_count = chapter_view.as_ref().map_or(1, |c| c.chapter_count);

    // Scroll the deep-linked verse into view once its chapter is up.
    use_effect(move || {
        let Some(osis) = anchor() else { return };
        let rendered = view
            .read()
            .as_ref()
            .and_then(|v| v.as_ref())
            .is_some_and(|c| c.verses.iter().any(|v| v.osis == osis));
        if rendered {
            anchor.set(None);
            let _ = dioxus::document::eval(&format!(
                "setTimeout(() => document.getElementById('{osis}')?.scrollIntoView({{block: 'center', behavior: 'smooth'}}), 50);"
            ));
        }
    });

    // Per-verse backlinks for this chapter (vault notes that link a
    // verse, including span links like `[[John 3:16-20]]`). Independent
    // of translation, so it doesn't re-fetch when the edition changes.
    let backlinks = use_resource(move || async move {
        let s = slug()?;
        fetch_chapter_backlinks(&s, &book(), chapter()).await.ok()
    });
    let backlink_map: BTreeMap<u16, VerseBacklinks> = backlinks
        .read()
        .clone()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .map(|b| (b.verse, b))
        .collect();

    // ── Study panel resources (all keyed on the selected verse) ──
    let orig_editions = use_resource(move || async move {
        let s = slug()?;
        fetch_original_editions(&s).await.ok()
    });
    let editions = orig_editions.read().clone().flatten().unwrap_or_default();
    let mut edition = use_signal(String::new);
    // Default the interlinear edition once the list lands.
    use_effect(move || {
        let list = orig_editions.read().clone().flatten().unwrap_or_default();
        if edition.peek().is_empty() {
            if let Some(first) = list.first() {
                edition.set(first.id.clone());
            }
        }
    });

    let interlinear = use_resource(move || async move {
        if study_tab() != StudyTab::Interlinear {
            return None;
        }
        let s = slug()?;
        let sel = selected()?;
        let ed = edition();
        if ed.is_empty() {
            return None;
        }
        Some(fetch_interlinear(&s, &ed, &sel.osis).await)
    });

    let word_tokens = use_resource(move || async move {
        if study_tab() != StudyTab::Words {
            return None;
        }
        let s = slug()?;
        let sel = selected()?;
        Some(fetch_word_tokens(&s, &translation(), &sel.osis).await)
    });

    let cross_refs = use_resource(move || async move {
        if study_tab() != StudyTab::CrossRefs {
            return None;
        }
        let s = slug()?;
        let sel = selected()?;
        Some(fetch_cross_refs(&s, &sel.osis, 1).await)
    });

    let topics = use_resource(move || async move {
        if study_tab() != StudyTab::Topics {
            return None;
        }
        let s = slug()?;
        let sel = selected()?;
        Some(fetch_topics_of(&s, &sel.osis).await)
    });

    let topic_verses = use_resource(move || async move {
        let s = slug()?;
        let topic = topic_sel()?;
        Some(fetch_verses_for_topic(&s, &topic, 40).await)
    });

    // The word-study drill-down (lexicon + concordance).
    let word_study = use_resource(move || async move {
        let s = slug()?;
        let code = strongs_sel()?;
        Some(fetch_word_study(&s, &code, 60).await)
    });

    // Jump to a display reference (`John 3:16` / OSIS / range start),
    // used by cross-ref and concordance rows.
    let jump = use_callback(move |reference: String| {
        let Ok(scref) = ScriptureRef::parse(&reference) else {
            return;
        };
        let start = scref.range.start;
        book.set(start.book.name().to_string());
        chapter.set(start.chapter);
        selected.set(Some(SelectedVerse {
            osis: start.osis(),
            display: start.to_string(),
        }));
        anchor.set(Some(start.osis()));
    });

    // The on-demand translation comparison.
    let comparison = use_resource(move || async move {
        let (reference, txs) = compare_query()?;
        let s = slug()?;
        fetch_comparison(&s, &reference, txs).await.ok()
    });
    let comparison_view: Option<ComparisonView> = comparison.read().clone().flatten();

    let books: Vec<&'static str> = (1..=66)
        .filter_map(Book::from_ordinal)
        .map(Book::name)
        .collect();

    let selected_osis = selected.read().as_ref().map(|s| s.osis.clone());
    let study_report: Option<Result<WordStudyReport, String>> = word_study.read().clone().flatten();

    rsx! {
        div { class: "mx-auto flex max-w-6xl flex-col gap-5 p-4 sm:p-6 lg:p-10",
            // ── Controls ──
            div { class: "flex flex-wrap items-center gap-3",
                Heading { level: HeadingLevel::H1, "Scripture" }
                Select {
                    value: translation,
                    placeholder: "Translation".to_string(),
                    SelectContent {
                        for (i, t) in tx_list.iter().enumerate() {
                            SelectItem { key: "{t.id}", value: "{t.id}", index: i, "{t.id}" }
                        }
                    }
                }
                Select {
                    value: book,
                    placeholder: "Book".to_string(),
                    on_change: move |_v: String| chapter.set(1),
                    SelectContent {
                        for (i, b) in books.iter().enumerate() {
                            SelectItem { key: "{b}", value: "{b}", index: i, "{b}" }
                        }
                    }
                }
                div { class: "flex items-center gap-2",
                    Button {
                        variant: ButtonVariant::Outline,
                        disabled: chapter() <= 1,
                        on_click: move |_| {
                            let c = chapter();
                            if c > 1 {
                                chapter.set(c - 1);
                            }
                        },
                        "Prev"
                    }
                    Text { class: "min-w-16 text-center text-sm", "Chapter {chapter}" }
                    Button {
                        variant: ButtonVariant::Outline,
                        disabled: chapter() >= chapter_count as u16,
                        on_click: move |_| chapter.set(chapter() + 1),
                        "Next"
                    }
                }
            }

            div { class: "flex flex-col gap-6 lg:flex-row lg:items-start",
                // ── Reading pane ──
                div { class: "min-w-0 flex-1",
                    if pending {
                        task_ui_core::states::LoadingState {}
                    } else if let Some(c) = chapter_view {
                        div { class: "flex flex-col gap-1",
                            div { class: "flex items-baseline justify-between gap-3",
                                Heading { level: HeadingLevel::H2, "{c.book_name} {c.chapter}" }
                                Text { variant: TextVariant::Muted, class: "text-xs", "{c.translation}" }
                            }
                            // Sources that name the whole chapter (a sermon's
                            // "Romans 8", a note's `[[Romans 8]]`) — verse 0.
                            if let Some(bl) = backlink_map.get(&0) {
                                div { class: "mt-2 flex flex-col gap-0.5 border-l border-border pl-3",
                                    span { class: "text-xs font-semibold uppercase tracking-[0.16em] text-muted-foreground",
                                        "Mentions this chapter"
                                    }
                                    for n in bl.notes.iter() {
                                        Link {
                                            key: "{n.note_path}#t:{n.secs}",
                                            to: backlink_href(n, &note_href),
                                            class: "text-xs text-muted-foreground hover:text-foreground",
                                            title: "{n.excerpt}",
                                            "↳ {backlink_label(n)}"
                                        }
                                    }
                                }
                            }
                            div { class: "mt-3 flex flex-col gap-2 leading-relaxed",
                                for v in c.verses.iter() {
                                    {
                                        let is_sel = selected_osis.as_deref() == Some(v.osis.as_str());
                                        let osis = v.osis.clone();
                                        let display = format!("{} {}:{}", c.book_name, c.chapter, v.verse);
                                        rsx! {
                                            div { key: "{v.osis}", class: "flex flex-col",
                                                p {
                                                    id: "{v.osis}",
                                                    class: if is_sel {
                                                        "-mx-2 cursor-pointer scroll-mt-20 rounded-md bg-primary/10 px-2 ring-1 ring-primary/30"
                                                    } else {
                                                        "-mx-2 cursor-pointer scroll-mt-20 rounded-md px-2 hover:bg-muted/40"
                                                    },
                                                    onclick: move |_| {
                                                        if selected.peek().as_ref().map(|s| s.osis.as_str())
                                                            == Some(osis.as_str())
                                                        {
                                                            selected.set(None);
                                                        } else {
                                                            selected.set(Some(SelectedVerse {
                                                                osis: osis.clone(),
                                                                display: display.clone(),
                                                            }));
                                                            strongs_sel.set(None);
                                                            topic_sel.set(None);
                                                        }
                                                    },
                                                    span {
                                                        class: "mr-2 select-none align-super text-xs font-semibold text-muted-foreground",
                                                        "{v.verse}"
                                                    }
                                                    span { "{v.text}" }
                                                    if let Some(bl) = backlink_map.get(&v.verse) {
                                                        span {
                                                            class: "ml-2 select-none align-super text-xs text-primary",
                                                            title: "{bl.notes.len()} linked note(s)",
                                                            "🔗{bl.notes.len()}"
                                                        }
                                                    }
                                                }
                                                // Linked notes — click through to the note; a
                                                // sermon opens at the moment it named the verse.
                                                if let Some(bl) = backlink_map.get(&v.verse) {
                                                    div { class: "ml-6 mt-1 flex flex-col gap-0.5 border-l border-border pl-3",
                                                        for n in bl.notes.iter() {
                                                            Link {
                                                                key: "{n.note_path}#t:{n.secs}",
                                                                to: backlink_href(n, &note_href),
                                                                class: "text-xs text-muted-foreground hover:text-foreground",
                                                                title: "{n.excerpt}",
                                                                "↳ {backlink_label(n)}"
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
                    } else {
                        task_ui_core::states::EmptyState {
                            title: "Nothing to show",
                            hint: "Pick a translation and book — install a corpus into the org's resource library if the list is empty.",
                        }
                    }
                }

                // ── Study panel ──
                if let Some(sel) = selected.read().clone() {
                    div { class: "flex w-full flex-col gap-3 rounded-xl border border-border bg-card/50 p-4 lg:sticky lg:top-4 lg:max-h-[85vh] lg:w-[26rem] lg:shrink-0 lg:overflow-y-auto",
                        div { class: "flex items-center justify-between gap-2",
                            Heading { level: HeadingLevel::H3, "Study · {sel.display}" }
                            Button {
                                variant: ButtonVariant::Ghost,
                                on_click: move |_| {
                                    selected.set(None);
                                    strongs_sel.set(None);
                                    topic_sel.set(None);
                                },
                                "✕"
                            }
                        }
                        // Tab bar.
                        div { class: "flex flex-wrap gap-1",
                            for (tab, label) in StudyTab::ALL {
                                Button {
                                    key: "{label}",
                                    variant: if study_tab() == tab { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                                    on_click: move |_| {
                                        study_tab.set(tab);
                                        topic_sel.set(None);
                                    },
                                    "{label}"
                                }
                            }
                        }

                        // ── Word-study drill-down overlays every tab ──
                        if let Some(code) = strongs_sel() {
                            div { class: "flex flex-col gap-2 rounded-lg border border-border bg-background/60 p-3",
                                div { class: "flex items-center justify-between gap-2",
                                    Text { class: "text-sm font-semibold", "Word study · {code}" }
                                    Button {
                                        variant: ButtonVariant::Ghost,
                                        on_click: move |_| strongs_sel.set(None),
                                        "‹ back"
                                    }
                                }
                                match study_report {
                                    None => rsx! { task_ui_core::states::LoadingState {} },
                                    Some(Err(ref e)) => rsx! {
                                        Text { variant: TextVariant::Muted, class: "text-xs", "{e}" }
                                    },
                                    Some(Ok(ref r)) => rsx! {
                                        div { class: "flex items-baseline gap-2",
                                            span { class: "text-xl", "{r.lemma}" }
                                            Text { variant: TextVariant::Muted, class: "text-sm italic", "{r.translit}" }
                                            Text { variant: TextVariant::Muted, class: "text-xs", "{r.normalized}" }
                                        }
                                        if !r.definition.is_empty() {
                                            Text { class: "text-sm leading-relaxed", "{r.definition}" }
                                        }
                                        if !r.kjv_def.is_empty() {
                                            Text { variant: TextVariant::Muted, class: "text-xs",
                                                "KJV: {r.kjv_def}"
                                            }
                                        }
                                        if !r.derivation.is_empty() {
                                            Text { variant: TextVariant::Muted, class: "text-xs",
                                                "Derivation: {r.derivation}"
                                            }
                                        }
                                        Text { class: "mt-1 text-xs font-semibold",
                                            "{r.total_occurrences} occurrence(s)"
                                        }
                                        div { class: "flex flex-col gap-1",
                                            for occ in r.occurrences.iter() {
                                                div {
                                                    key: "{occ.osis}",
                                                    class: "cursor-pointer rounded-md px-2 py-1 text-xs hover:bg-muted/40",
                                                    onclick: {
                                                        let target = occ.reference.clone();
                                                        move |_| jump(target.clone())
                                                    },
                                                    span { class: "mr-1 font-semibold text-primary", "{occ.reference}" }
                                                    span { class: "text-muted-foreground", "{occ.text}" }
                                                }
                                            }
                                        }
                                    },
                                }
                            }
                        } else {
                            // ── Tab bodies ──
                            match study_tab() {
                                StudyTab::Interlinear => rsx! {
                                    if editions.is_empty() {
                                        Text { variant: TextVariant::Muted, class: "text-xs",
                                            "No original-language editions installed — add TAGNT / TAHOT (STEPBible), SBLGNT, or OSHB to the org's resource library."
                                        }
                                    } else {
                                        Select {
                                            value: edition,
                                            placeholder: "Edition".to_string(),
                                            SelectContent {
                                                for (i, e) in editions.iter().enumerate() {
                                                    SelectItem { key: "{e.id}", value: "{e.id}", index: i,
                                                        "{e.name} ({e.language})"
                                                    }
                                                }
                                            }
                                        }
                                        match interlinear.read().clone().flatten() {
                                            None => rsx! { task_ui_core::states::LoadingState {} },
                                            Some(Err(e)) => rsx! {
                                                Text { variant: TextVariant::Muted, class: "text-xs", "{e}" }
                                            },
                                            Some(Ok(words)) => rsx! {
                                                div { class: "flex flex-wrap gap-2",
                                                    for (i, w) in words.iter().enumerate() {
                                                        div {
                                                            key: "{i}",
                                                            class: if w.strong.is_empty() {
                                                                "flex flex-col rounded-md border border-border/60 px-2 py-1"
                                                            } else {
                                                                "flex cursor-pointer flex-col rounded-md border border-border/60 px-2 py-1 hover:border-primary/60 hover:bg-primary/5"
                                                            },
                                                            onclick: {
                                                                let strong = w.strong.clone();
                                                                move |_| {
                                                                    if !strong.is_empty() {
                                                                        strongs_sel.set(Some(strong.clone()));
                                                                    }
                                                                }
                                                            },
                                                            title: "{w.morph}",
                                                            span { class: "text-lg leading-tight", "{w.word}" }
                                                            span { class: "text-[0.65rem] italic text-muted-foreground", "{w.translit}" }
                                                            span { class: "text-xs", "{w.gloss}" }
                                                            if !w.strong.is_empty() {
                                                                span { class: "text-[0.6rem] text-primary", "{w.strong}" }
                                                            }
                                                        }
                                                    }
                                                }
                                            },
                                        }
                                    }
                                },
                                StudyTab::Words => rsx! {
                                    match word_tokens.read().clone().flatten() {
                                        None => rsx! { task_ui_core::states::LoadingState {} },
                                        Some(Err(e)) => rsx! {
                                            Text { variant: TextVariant::Muted, class: "text-xs", "{e}" }
                                        },
                                        Some(Ok(tokens)) => rsx! {
                                            if tokens.is_empty() {
                                                Text { variant: TextVariant::Muted, class: "text-xs",
                                                    "No Strong's tags in {translation} for this verse — try the interlinear."
                                                }
                                            }
                                            div { class: "flex flex-wrap gap-1.5",
                                                for (i, t) in tokens.iter().enumerate() {
                                                    div {
                                                        key: "{i}",
                                                        class: "flex cursor-pointer flex-col rounded-md border border-border/60 px-2 py-1 hover:border-primary/60 hover:bg-primary/5",
                                                        onclick: {
                                                            let strong = t.strongs.clone();
                                                            move |_| strongs_sel.set(Some(strong.clone()))
                                                        },
                                                        span { class: "text-sm font-medium", "{t.surface}" }
                                                        span { class: "text-[0.65rem] italic text-muted-foreground",
                                                            "{t.lemma} · {t.translit}"
                                                        }
                                                        span { class: "text-xs text-muted-foreground", "{t.gloss}" }
                                                        span { class: "text-[0.6rem] text-primary", "{t.strongs}" }
                                                    }
                                                }
                                            }
                                        },
                                    }
                                },
                                StudyTab::CrossRefs => rsx! {
                                    match cross_refs.read().clone().flatten() {
                                        None => rsx! { task_ui_core::states::LoadingState {} },
                                        Some(Err(e)) => rsx! {
                                            Text { variant: TextVariant::Muted, class: "text-xs", "{e}" }
                                        },
                                        Some(Ok(refs)) => rsx! {
                                            if refs.is_empty() {
                                                Text { variant: TextVariant::Muted, class: "text-xs",
                                                    "No cross-references — install the OpenBible cross-reference set."
                                                }
                                            }
                                            div { class: "flex flex-col gap-1",
                                                for r in refs.iter() {
                                                    div {
                                                        key: "{r.osis}",
                                                        class: "flex cursor-pointer items-center justify-between gap-2 rounded-md px-2 py-1 text-sm hover:bg-muted/40",
                                                        onclick: {
                                                            let target = r.reference.clone();
                                                            move |_| jump(target.clone())
                                                        },
                                                        span { class: "text-primary", "{r.reference}" }
                                                        span { class: "text-xs text-muted-foreground", "▲{r.votes}" }
                                                    }
                                                }
                                            }
                                        },
                                    }
                                },
                                StudyTab::Topics => rsx! {
                                    match topics.read().clone().flatten() {
                                        None => rsx! { task_ui_core::states::LoadingState {} },
                                        Some(Err(e)) => rsx! {
                                            Text { variant: TextVariant::Muted, class: "text-xs", "{e}" }
                                        },
                                        Some(Ok(tags)) => rsx! {
                                            if tags.is_empty() {
                                                Text { variant: TextVariant::Muted, class: "text-xs",
                                                    "No topics — install the OpenBible topic set."
                                                }
                                            }
                                            div { class: "flex flex-wrap gap-1.5",
                                                for t in tags.iter() {
                                                    Button {
                                                        key: "{t.topic}",
                                                        variant: if topic_sel.read().as_deref() == Some(t.topic.as_str()) {
                                                            ButtonVariant::Secondary
                                                        } else {
                                                            ButtonVariant::Outline
                                                        },
                                                        on_click: {
                                                            let topic = t.topic.clone();
                                                            move |_| topic_sel.set(Some(topic.clone()))
                                                        },
                                                        "{t.topic} ▲{t.votes}"
                                                    }
                                                }
                                            }
                                        },
                                    }
                                    if topic_sel.read().is_some() {
                                        match topic_verses.read().clone().flatten() {
                                            None => rsx! { task_ui_core::states::LoadingState {} },
                                            Some(Err(e)) => rsx! {
                                                Text { variant: TextVariant::Muted, class: "text-xs", "{e}" }
                                            },
                                            Some(Ok(refs)) => rsx! {
                                                div { class: "mt-1 flex flex-col gap-1 border-t border-border pt-2",
                                                    for r in refs.iter() {
                                                        div {
                                                            key: "{r.osis}",
                                                            class: "flex cursor-pointer items-center justify-between gap-2 rounded-md px-2 py-1 text-sm hover:bg-muted/40",
                                                            onclick: {
                                                                let target = r.reference.clone();
                                                                move |_| jump(target.clone())
                                                            },
                                                            span { class: "text-primary", "{r.reference}" }
                                                            span { class: "text-xs text-muted-foreground", "▲{r.votes}" }
                                                        }
                                                    }
                                                }
                                            },
                                        }
                                    }
                                },
                            }
                        }
                    }
                }
            }

            // ── Compare translations ──
            div { class: "mt-6 flex max-w-3xl flex-col gap-3 border-t border-border pt-4",
                Heading { level: HeadingLevel::H2, "Compare translations" }
                div { class: "flex flex-wrap items-center gap-2",
                    input {
                        class: CTRL_CLS,
                        placeholder: "John 3:16-18 or Genesis 4:3-Exodus 15:17",
                        value: "{compare_ref}",
                        oninput: move |e| compare_ref.set(e.value()),
                    }
                    input {
                        class: CTRL_CLS,
                        placeholder: "translations (optional, e.g. WEB, BSB)",
                        value: "{compare_tx}",
                        oninput: move |e| compare_tx.set(e.value()),
                    }
                    Button {
                        variant: ButtonVariant::Primary,
                        on_click: move |_| {
                            let r = compare_ref.read().trim().to_string();
                            if r.is_empty() {
                                return;
                            }
                            let txs: Vec<String> = compare_tx
                                .read()
                                .split([',', ' '])
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_uppercase())
                                .collect();
                            compare_query.set(Some((r, txs)));
                        },
                        "Compare"
                    }
                }
                if let Some(cv) = comparison_view {
                    div { class: "overflow-x-auto",
                        table { class: "w-full border-collapse text-sm",
                            thead {
                                tr {
                                    th { class: "border-b border-border px-2 py-1 text-left align-bottom text-xs text-muted-foreground",
                                        "{cv.reference}"
                                    }
                                    for t in cv.translations.iter() {
                                        th {
                                            key: "{t}",
                                            class: "border-b border-border px-2 py-1 text-left align-bottom text-xs font-semibold",
                                            "{t}"
                                        }
                                    }
                                }
                            }
                            tbody {
                                for row in cv.rows.iter() {
                                    tr { key: "{row.reference}",
                                        td { class: "whitespace-nowrap border-b border-border/40 px-2 py-1 align-top text-xs text-muted-foreground",
                                            "{row.reference}"
                                        }
                                        for (ci, cell) in row.cells.iter().enumerate() {
                                            td {
                                                key: "{ci}",
                                                class: "border-b border-border/40 px-2 py-1 align-top leading-relaxed",
                                                "{cell}"
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

// ── data ────────────────────────────────────────────────────────────
//
// This slice's RPCs live with the page that calls them, not in the
// shell's `feeds` module — that is the point of the split. `feeds!` and
// the fan-out helpers come from `task-ui-core`; see its `feeds` module
// for the shape.

feeds! {
    scripture_proto::ScriptureServiceClient {
        /// Installed Bible translations for the org (bundled editions first).
        fetch_translations() -> Vec<scripture_proto::TranslationInfo>
            = translations() as "translations";
    }
}

/// One chapter of one translation. `book` accepts any spelling.
pub async fn fetch_chapter(
    slug: &str,
    translation: &str,
    book: &str,
    chapter: u16,
) -> Result<scripture_proto::ChapterView, String> {
    let client =
        task_ui_core::vox_clients::establish_for::<scripture_proto::ScriptureServiceClient>(slug)
            .await?;
    // The generated vox client takes owned `String` args.
    client
        .chapter(translation.to_owned(), book.to_owned(), chapter)
        .await
        .map_err(|e| format!("{slug}: chapter {book} {chapter}: {e:?}"))
}

feeds! {
    scripture_proto::ScriptureServiceClient {
        /// Compare a verse/range across translations (empty list ⇒ all).
        fetch_comparison(reference: &str, translations: Vec<String>) -> scripture_proto::ComparisonView
            = compare(reference.to_owned(), translations) as format!("compare {reference}");

        /// Per-verse backlinks for a chapter — vault notes that link each verse.
        fetch_chapter_backlinks(book: &str, chapter: u16) -> Vec<scripture_proto::VerseBacklinks>
            = chapter_backlinks(book.to_owned(), chapter) as format!("backlinks {book} {chapter}");

        /// Installed original-language editions (TAGNT / TAHOT / SBLGNT / OSHB).
        fetch_original_editions() -> Vec<scripture_proto::OrigEditionInfo>
            = original_editions() as "original editions";

        /// Word-by-word interlinear of a verse in an original-language edition.
        fetch_interlinear(edition: &str, reference: &str) -> Vec<scripture_proto::InterlinearWord>
            = interlinear(edition.to_owned(), reference.to_owned()) as format!("interlinear {edition} {reference}");

        /// Strong's-tagged breakdown of a verse in an English translation.
        fetch_word_tokens(translation: &str, reference: &str) -> Vec<scripture_proto::WordToken>
            = word_study(translation.to_owned(), reference.to_owned()) as format!("word tokens {reference}");

        /// Full word study for a Strong's code: lexicon entry + concordance.
        fetch_word_study(strongs: &str, limit: u32) -> scripture_proto::WordStudyReport
            = study(strongs.to_owned(), limit) as format!("word study {strongs}");

        /// Cross-references from a verse (votes-desc, `min_votes` filters noise).
        fetch_cross_refs(reference: &str, min_votes: i32) -> Vec<scripture_proto::WeightedRef>
            = cross_refs(reference.to_owned(), min_votes) as format!("cross refs {reference}");

        /// Topics a verse is tagged with (votes-desc).
        fetch_topics_of(reference: &str) -> Vec<scripture_proto::TopicTag>
            = topics_of(reference.to_owned()) as format!("topics {reference}");

        /// Verses about a topic (votes-desc, capped at `limit`; 0 ⇒ default).
        fetch_verses_for_topic(topic: &str, limit: u32) -> Vec<scripture_proto::WeightedRef>
            = verses_for_topic(topic.to_owned(), limit) as format!("topic verses {topic}");
    }
}
