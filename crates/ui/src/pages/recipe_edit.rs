//! Recipe authoring — edit a recipe's cooklang `.cook` source.
//!
//! The source is the source of truth: the server writes it verbatim and
//! re-parses, so saving here is what produces the structured steps +
//! timers that [`super::cook_mode`] consumes. A live structural preview
//! (step blocks, with `@ingredient` / `#cookware` / `~timer` tokens
//! cleaned up client-side) gives quick feedback without a round-trip;
//! the authoritative parse still happens server-side on save.

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{Save, X};
use architect_ui::prelude::*;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::routes::Route;
use crate::stores;

const COOK_HINT: &str = "@ingredient{200%g} · #cookware · ~timer{10%min} · >> key: value (metadata) · blank line = new step";

#[component]
pub fn EditRecipeView(path: String) -> Element {
    let nav = use_navigator();
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    let recipes = stores::use_recipe_list();
    let store = stores::use_recipe_store();
    let muts = stores::use_recipe_mutations();

    let target = path.clone();
    let recipe = recipes.value().and_then(|rows| {
        rows.iter()
            .find(|(_, r)| r.path == target)
            .map(|(_, r)| r.clone())
    });

    // Source buffer, seeded from the recipe once it resolves.
    let mut source = use_signal(String::new);
    let mut seeded = use_signal(|| false);
    if let Some(r) = &recipe {
        if !seeded() {
            source.set(r.source.clone());
            seeded.set(true);
        }
    }

    let Some(recipe) = recipe else {
        return rsx! {
            div { class: "mx-auto flex max-w-md flex-col gap-3 p-8 text-center",
                if recipes.is_waiting() {
                    crate::states::LoadingState {}
                } else if let Some(err) = recipes.error() {
                    crate::states::ErrorState {
                        title: "Couldn't load the recipe",
                        message: err.clone(),
                        on_retry: move |()| store.reload(),
                    }
                } else {
                    crate::states::EmptyState { title: "Recipe not found", hint: "It may have been moved or renamed." }
                }
            }
        };
    };

    let cook_path = path.clone();
    let to_cook = use_callback(move |()| {
        nav.push(Route::RecipeCookRoute {
            path: cook_path.clone(),
        });
    });

    let save = {
        let recipe = recipe.clone();
        move |()| {
            let Some(s) = slug() else { return };
            let mut updated = recipe.clone();
            updated.source = source.read().clone();
            muts.update(s, updated);
            to_cook.call(());
        }
    };

    let blocks = preview_blocks(&source.read());

    rsx! {
        div { class: "fixed inset-0 z-50 flex flex-col bg-background text-foreground",
            header { class: "flex items-center gap-3 border-b border-border px-3 py-2 pt-[calc(0.5rem+env(safe-area-inset-top,0px))]",
                button {
                    class: "flex size-10 shrink-0 items-center justify-center rounded-lg text-muted-foreground hover:bg-muted hover:text-foreground",
                    aria_label: "Cancel",
                    onclick: move |_| to_cook.call(()),
                    X { size: 20 }
                }
                div { class: "flex min-w-0 flex-1 flex-col",
                    Heading { level: HeadingLevel::H2, class: "truncate text-base font-semibold", "Edit {recipe.name}" }
                    span { class: "truncate text-xs text-muted-foreground", "{recipe.path}" }
                }
                Button {
                    size: ButtonSize::Small,
                    on_click: move |_| save(()),
                    Save { size: 14 }
                    "Save"
                }
            }

            div { class: "flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-3 pb-[calc(2rem+env(safe-area-inset-bottom,0px))] lg:flex-row",
                // Source editor.
                div { class: "flex min-h-[40vh] flex-1 flex-col gap-1.5 lg:min-h-0",
                    label { class: "text-xs font-semibold uppercase tracking-wide text-muted-foreground", "Cooklang source" }
                    textarea {
                        class: "min-h-[40vh] flex-1 resize-none rounded-xl border border-input bg-input/30 p-3 font-mono text-sm leading-relaxed outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 lg:min-h-0",
                        spellcheck: false,
                        value: "{source}",
                        oninput: move |e| source.set(e.value()),
                    }
                    span { class: "text-[11px] text-muted-foreground", "{COOK_HINT}" }
                }
                // Live structural preview.
                div { class: "flex flex-1 flex-col gap-1.5",
                    label { class: "text-xs font-semibold uppercase tracking-wide text-muted-foreground", "Preview" }
                    div { class: "flex flex-1 flex-col gap-2 rounded-xl border border-border bg-card/40 p-3",
                        if blocks.is_empty() {
                            Text { variant: TextVariant::Muted, class: "text-sm", "Write a step below the metadata to see it here." }
                        } else {
                            for (i, block) in blocks.iter().enumerate() {
                                div { key: "{i}", class: "flex gap-2",
                                    span { class: "flex size-6 shrink-0 items-center justify-center rounded-full border border-border text-xs font-semibold text-muted-foreground", "{i + 1}" }
                                    p { class: "text-sm leading-relaxed text-foreground", "{block}" }
                                }
                            }
                        }
                        span { class: "mt-1 text-[11px] text-muted-foreground", "Approximate — the exact steps + timers are parsed on save." }
                    }
                }
            }
        }
    }
}

/// Split a `.cook` source into readable step blocks for the preview:
/// drop `>>` metadata lines, split on blank lines, and strip cooklang
/// token punctuation so `@olive oil{2%tbsp}` reads as "olive oil",
/// `~{10%min}` as "10 min", etc. Approximate — the server parse is
/// authoritative.
fn preview_blocks(source: &str) -> Vec<String> {
    let body: String = source
        .lines()
        .filter(|l| !l.trim_start().starts_with(">>"))
        .collect::<Vec<_>>()
        .join("\n");
    body.split("\n\n")
        .map(|b| clean_tokens(b.trim()))
        .filter(|b| !b.is_empty())
        .collect()
}

/// Strip cooklang `@`/`#`/`~` markers and `{…}` amounts, keeping the
/// human-readable word(s). `~{10%min}` → "10 min"; `@olive oil{}` →
/// "olive oil"; `#pan` → "pan".
fn clean_tokens(block: &str) -> String {
    let chars: Vec<char> = block.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '@' || c == '#' || c == '~' {
            i += 1;
            // A `{…}` before the next newline / marker means a (possibly
            // multi-word) name; otherwise it's a bare single word.
            let mut j = i;
            let mut brace = None;
            while j < chars.len() {
                match chars[j] {
                    '{' => {
                        brace = Some(j);
                        break;
                    }
                    '\n' | '@' | '#' | '~' => break,
                    _ => j += 1,
                }
            }
            if let Some(b) = brace {
                let name: String = chars[i..b].iter().collect::<String>().trim().to_string();
                let mut k = b + 1;
                let mut amount = String::new();
                while k < chars.len() && chars[k] != '}' {
                    amount.push(chars[k]);
                    k += 1;
                }
                i = (k + 1).min(chars.len());
                if name.is_empty() {
                    // Bare timer like `~{10%min}` — show the amount.
                    out.push_str(amount.replace('%', " ").trim());
                } else {
                    out.push_str(&name);
                }
            } else {
                // Single word: up to whitespace or sentence punctuation.
                let start = i;
                while i < chars.len() && !chars[i].is_whitespace() && !"{}.,;:!?".contains(chars[i])
                {
                    i += 1;
                }
                out.extend(&chars[start..i]);
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_drops_metadata_and_splits_steps() {
        let src = ">> title: Tea\n>> servings: 1\n\nBoil @water{500%ml} in a #kettle.\n\nSteep for ~{4%min}.";
        let b = preview_blocks(src);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0], "Boil water in a kettle.");
        assert_eq!(b[1], "Steep for 4 min.");
    }

    #[test]
    fn cleans_multiword_and_amount_tokens() {
        assert_eq!(
            clean_tokens("Add @olive oil{2%tbsp} and #large pan{}."),
            "Add olive oil and large pan."
        );
        assert_eq!(clean_tokens("Rest ~{30%seconds}."), "Rest 30 seconds.");
    }
}
