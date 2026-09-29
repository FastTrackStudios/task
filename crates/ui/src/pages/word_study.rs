//! A wiki word, studied in the original language.
//!
//! A wiki page about a Hebrew or Greek word (`type: word`, or any page
//! with a `strongs:` field — `strongs: H2764`, or `H1121 + H430` for a
//! phrase) gets the scripture app's word study under it: the Strong's
//! lexicon entry, how often the word occurs in the installed original
//! text, and the first verses that use it — each one opening the reader
//! at that verse with the same word study showing, beside the
//! interlinear. The wiki says what the word means *here*; this says what
//! it means everywhere else.
//!
//! Needs the org's lexicon (`resources/lexicon/strongs/`) and, for the
//! concordance, an original-language edition (`resources/original/`).
//! Without them the panel says what to install rather than showing
//! nothing.

use dioxus::prelude::*;

use crate::routes::Route;

/// `H2764` → `["H2764"]`, `H1121 + H430` → `["H1121", "H430"]`.
#[must_use]
pub fn codes(field: &str) -> Vec<String> {
    field
        .split(['+', ',', ' '])
        .map(str::trim)
        .filter(|c| {
            let mut ch = c.chars();
            matches!(ch.next(), Some('H' | 'G' | 'h' | 'g'))
                && ch.next().is_some_and(|d| d.is_ascii_digit())
        })
        .map(str::to_uppercase)
        .collect()
}

/// The reader at `reference`, with `strongs`' word study open.
fn study_route(reference: &str, strongs: &str) -> Route {
    crate::routes::plugin_route(
        "scripture",
        &task_plugin_ui::LinkTarget::query(format!(
            "reference={}&strongs={}",
            task_plugin_ui::encode(reference),
            task_plugin_ui::encode(strongs)
        )),
    )
}

/// The panel: one study per Strong's code on the page.
#[component]
pub fn WordStudyPanel(org: Memo<String>, codes: Vec<String>) -> Element {
    if codes.is_empty() {
        return rsx! {};
    }
    rsx! {
        section { class: "word-study mt-8 flex flex-col gap-3", "data-testid": "word-study",
            for code in codes {
                WordStudy { key: "{code}", org, code: code.clone() }
            }
        }
    }
}

#[component]
fn WordStudy(org: Memo<String>, code: String) -> Element {
    let code_for_fetch = code.clone();
    let study = use_resource(move || {
        let slug = org();
        let code = code_for_fetch.clone();
        async move { scripture_ui::fetch_word_study(&slug, &code, 12).await }
    });
    let body = match &*study.read() {
        None => rsx! { div { class: "text-sm text-muted-foreground", "Looking up {code}…" } },
        Some(Err(_)) => rsx! {
            div { class: "text-sm text-muted-foreground",
                "No lexicon entry for {code}. The word study needs the Strong’s lexicon in this org’s resource library (resources/lexicon/strongs)."
            }
        },
        Some(Ok(w)) => {
            let first = w.occurrences.first().map(|o| o.reference.clone());
            let total = w.total_occurrences;
            let shown = w.occurrences.len();
            rsx! {
                div { class: "flex flex-wrap items-baseline gap-x-3 gap-y-1",
                    span { class: "text-2xl leading-none", lang: if code.starts_with('G') { "grc" } else { "he" }, "{w.lemma}" }
                    span { class: "italic text-muted-foreground", "{w.translit}" }
                    span { class: "rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground", "{w.normalized}" }
                    if total > 0 {
                        span { class: "ml-auto text-xs text-muted-foreground tabular-nums", "{total} verses" }
                    }
                }
                if !w.definition.trim().is_empty() {
                    p { class: "text-sm", "{w.definition.trim()}" }
                }
                if !w.kjv_def.trim().is_empty() {
                    p { class: "text-xs text-muted-foreground",
                        span { class: "font-semibold uppercase tracking-wider", "KJV " }
                        "{w.kjv_def.trim()}"
                    }
                }
                if !w.derivation.trim().is_empty() {
                    p { class: "text-xs text-muted-foreground",
                        span { class: "font-semibold uppercase tracking-wider", "From " }
                        "{w.derivation.trim()}"
                    }
                }
                if shown > 0 {
                    ul { class: "mt-1 flex flex-col divide-y divide-border/60 text-sm",
                        for o in w.occurrences.iter() {
                            li { key: "{o.osis}", class: "flex gap-3 py-1.5",
                                Link {
                                    to: study_route(&o.reference, &code),
                                    class: "w-28 shrink-0 font-medium text-foreground hover:underline",
                                    "{o.reference}"
                                }
                                span { class: "min-w-0 text-muted-foreground", "{o.text}" }
                            }
                        }
                    }
                    if total as usize > shown {
                        if let Some(r) = first.clone() {
                            Link { to: study_route(&r, &code), class: "self-start text-xs text-primary hover:underline",
                                "All {total} in the reader →"
                            }
                        }
                    }
                } else {
                    p { class: "text-xs text-muted-foreground",
                        "Occurrences need an original-language edition in the resource library (resources/original — OSHB, TAHOT, SBLGNT or TAGNT)."
                    }
                }
            }
        }
    };
    rsx! {
        div { class: "rounded-lg border border-border bg-card/50 px-4 py-3",
            div { class: "mb-2 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground", "Word study · {code}" }
            div { class: "flex flex-col gap-2", {body} }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_strongs_field_is_one_or_more_codes() {
        assert_eq!(super::codes("H2764"), ["H2764"]);
        assert_eq!(super::codes("H1121 + H430"), ["H1121", "H430"]);
        assert_eq!(super::codes("g4747, nothing"), ["G4747"]);
        assert!(super::codes("").is_empty());
    }
}
