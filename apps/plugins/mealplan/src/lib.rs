//! Mealplan, as a Task app — the cookbook, the pantry, and cooking.
//!
//! This crate began as `task-plugin-cooking`, a worked example with two
//! made-up screens, written to prove the seam before there was anything
//! real behind it. The real screens have arrived, so it stopped being
//! an example and became the app; what survives of the example is the
//! property it was written to demonstrate, and that property is checked
//! by this crate's `Cargo.toml`: **it depends on the plugin SDK and its
//! own feature crate, and on nothing else in Task.** No `task-ui`, no
//! shell, no route. If that list grows, the extension point has stopped
//! being one.
//!
//! Six screens, and the most interlinked app in Task: a recipe opens
//! cook mode, cook mode jumps to the shopping list, the list goes back
//! to the plan. Those links are written with
//! [`task_plugin_ui::href`] — the app's own paths, turned into the
//! shell's URLs by the crate they both agree on.
//!
//! ## What it claims
//!
//! `[[Recipe/Bolognese]]` is a reference and nothing else — the prefix
//! says so — and a note of that exact name would be somebody writing
//! *about* the dish. So it is claimed [`Claim::Always`] and beats a
//! page. A bare `[[Bolognese]]` might be the recipe and might be
//! somebody's note about a dinner, so it defers to the vault. The
//! difference is about the text, not the app, which is why the app is
//! the one that gets to say.

use mealplan_ui::{
    EditRecipeView, MealplanView, RecipeCookView, RecipeReadView, ShoppingView, provide_stores,
};
use task_plugin_ui::architect_ui::lucide_dioxus::{ShoppingCart, Utensils};
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{Claim, LinkTarget, PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: mealplan_ui::APP_ID,
    version: env!("CARGO_PKG_VERSION"),
    nav: &[
        PluginNav {
            label: "Mealplan",
            icon: icon_plan,
            path: "",
            rail: false,
        },
        PluginNav {
            label: "Shopping",
            icon: icon_shopping,
            path: "shopping",
            rail: false,
        },
    ],
    view,
    provide: Some(provide_stores),
    // A recipe is a `.cook` file, and a `.cook` file in the note editor
    // is raw cooklang. Claimed so it opens where it reads.
    panel: None,
    claim_file: Some(claim_file),
    // A recipe note could render as a method with its own timers right
    // in the editor — the same seam the player uses to turn a song note
    // into a player. Cook mode is that screen already; making it a
    // widget is the next thing this app wants.
    widgets: None,
    fences: None,
    claim_link: Some(claim_link),
    claim_href: Some(claim_href),
};

/// What this app makes of a wikilink. See the module docs on why there
/// are two strengths.
fn claim_link(text: &str) -> Option<Claim> {
    if let Some(dish) = text.strip_prefix("Recipe/") {
        return Some(Claim::Always(dish_target(dish)));
    }
    // A bare dish name defers to the vault. Nothing recognises one yet
    // — the recipe index would be asked here — and an app that claimed
    // every unresolved link would make every typo somebody's recipe.
    None
}

fn dish_target(dish: &str) -> LinkTarget {
    LinkTarget {
        path: "recipe/read".into(),
        query: format!("path={}", task_plugin_ui::encode(dish)),
    }
}

/// This app's own scheme, for widgets and generated notes.
fn claim_href(href: &str) -> Option<LinkTarget> {
    let dish = href.strip_prefix("recipe-open:")?.trim();
    (!dish.is_empty()).then(|| dish_target(dish))
}

/// A `.cook` file opens in the reader, not the note editor.
///
/// The shell used to know this, by extension, in three separate places
/// — a base row, a file list, the schedule overlay — and each was the
/// shell holding a fact about this app. It asks now.
///
/// Narrow on purpose: only the extension this app is the reader for.
fn claim_file(path: &str) -> Option<LinkTarget> {
    path.ends_with(".cook").then(|| LinkTarget {
        path: "recipe/read".into(),
        query: format!("path={}", task_plugin_ui::encode(path)),
    })
}

fn icon_plan() -> Element {
    rsx! { Utensils { size: 16 } }
}

fn icon_shopping() -> Element {
    rsx! { ShoppingCart { size: 16 } }
}

/// Every screen this app has.
///
/// `None` for a path it does not recognise — the shell then says so
/// itself, rather than this pretending to have a page. That is the
/// difference between a bad link and a broken app, and only this crate
/// knows which one a path is.
fn view(path: &str, query: &str) -> Option<Element> {
    // The screens are their own wasm chunk on the web, downloaded the
    // first time somebody opens this app; everything else the app
    // registers stays in the shell. A plain call everywhere else.
    task_plugin_ui::lazy_view!("mealplan", mealplan_screen, path, query)
}

fn mealplan_screen(path: &str, query: &str) -> Option<Element> {
    // Every recipe screen is addressed by its vault-relative `.cook`
    // path, which is why they all read the same parameter.
    let recipe = || task_plugin_ui::query_param(query, "path").unwrap_or_default();
    match path {
        "" => Some(rsx! { MealplanView {} }),
        "shopping" => Some(rsx! { ShoppingView {} }),
        "recipe/read" => Some(rsx! { RecipeReadView { path: recipe() } }),
        "recipe/cook" => Some(rsx! { RecipeCookView { path: recipe() } }),
        "recipe/edit" => Some(rsx! { EditRecipeView { path: recipe() } }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefixed_reference_beats_a_note_of_the_same_name() {
        let claim = claim_link("Recipe/Bolognese").expect("claimed");
        assert!(claim.beats_a_page());
        assert_eq!(
            task_plugin_ui::query_param(&claim.target().query, "path").as_deref(),
            Some("Bolognese")
        );
    }

    #[test]
    fn a_bare_name_is_left_to_the_vault() {
        assert!(claim_link("Bolognese").is_none());
        assert!(claim_link("Weekly Review").is_none());
    }

    /// A dish with an ampersand in its name is an ordinary dish, and it
    /// has to survive being put in a URL.
    #[test]
    fn a_dish_with_punctuation_survives() {
        let claim = claim_link("Recipe/Ragu & Chips").expect("claimed");
        assert_eq!(
            task_plugin_ui::query_param(&claim.target().query, "path").as_deref(),
            Some("Ragu & Chips")
        );
    }

    #[test]
    fn the_scheme_opens_a_recipe() {
        let target = claim_href("recipe-open:Bolognese").expect("claimed");
        assert_eq!(target.path, "recipe/read");
        assert!(claim_href("scripture-open:John 3:16").is_none());
        assert!(claim_href("recipe-open:").is_none());
    }
}
