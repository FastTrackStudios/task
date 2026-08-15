//! Stage 2 — structured-data extraction.
//!
//! Runs cooklang-import's extractor chain (JSON-LD → microdata → HTML
//! classes) over HTML *we* already hold — we deliberately bypass its
//! own fetcher/pipeline so that `--from-file <saved.html>` and live
//! URLs share one code path, and so the fetch error UX stays ours.
//!
//! Quirk worth knowing: cooklang-import's internal `Recipe` model
//! lives in a `pub(crate)` module while the public `Extractor` trait
//! returns it — external code can *call* `parse` and use the value's
//! public fields through type inference, but can never write the type
//! name. Hence the inference-only style in [`extract`]; the value is
//! projected into our [`NormalizedRecipe`] before it escapes the
//! function.

use std::collections::BTreeMap;

use cooklang_import::url_to_text::html::extractors::{
    Extractor, HtmlClassExtractor, JsonLdExtractor, MicroDataExtractor, ParsingContext,
};

use crate::error::ImportError;
use crate::normalized::NormalizedRecipe;

/// Extract a [`NormalizedRecipe`] from page HTML.
///
/// `url` is used for error messages and the `source:` metadata — pass
/// the original URL when importing live, or the file path for
/// `--from-file`.
pub fn extract(html: &str, url: &str) -> Result<NormalizedRecipe, ImportError> {
    let context = ParsingContext {
        url: url.to_string(),
        document: scraper::Html::parse_document(html),
        texts: None,
    };

    // JSON-LD → microdata → HTML classes, first hit wins (same order
    // as cooklang-import's own pipeline).
    let raw = JsonLdExtractor
        .parse(&context)
        .or_else(|e| {
            tracing::debug!("json-ld extractor missed: {e}");
            MicroDataExtractor.parse(&context)
        })
        .or_else(|e| {
            tracing::debug!("microdata extractor missed: {e}");
            HtmlClassExtractor.parse(&context)
        })
        .map_err(|e| ImportError::NoRecipeFound {
            url: url.to_string(),
            detail: format!(" (last extractor said: {e})"),
        })?;

    let ingredients: Vec<String> = raw
        .ingredients
        .iter()
        .map(|i| collapse_ws(i))
        .filter(|i| !i.is_empty())
        .collect();
    let mut steps = split_steps(&raw.instructions);
    if steps.is_empty() {
        // Jetpack (WordPress.com's recipe plugin — Smitten Kitchen et
        // al.) marks up name/ingredients as microdata but never emits
        // `itemprop="recipeInstructions"`; the directions sit in a
        // `.jetpack-recipe-directions` div the generic extractors
        // can't see.
        steps = jetpack_steps(&context.document);
    }

    if ingredients.is_empty() {
        return Err(ImportError::IncompleteRecipe {
            url: url.to_string(),
            detail: "structured data carried no ingredients".into(),
        });
    }
    if steps.is_empty() {
        return Err(ImportError::IncompleteRecipe {
            url: url.to_string(),
            detail: "structured data carried no instructions".into(),
        });
    }

    let mut metadata: BTreeMap<String, String> = raw
        .metadata
        .iter()
        .map(|(k, v)| (k.clone(), collapse_ws(v)))
        .filter(|(_, v)| !v.is_empty())
        .collect();
    // The extractor stamps `source` from the URL it was handed; keep
    // ours authoritative and out of the free-form map.
    let source_url = metadata
        .remove("source")
        .or_else(|| url.starts_with("http").then(|| url.to_string()));

    let name = collapse_ws(&raw.name);
    Ok(NormalizedRecipe {
        name: if name.is_empty() {
            "Imported Recipe".into()
        } else {
            name
        },
        description: raw
            .description
            .as_deref()
            .map(collapse_ws)
            .filter(|d| !d.is_empty()),
        image: raw.image.first().cloned().filter(|i| !i.is_empty()),
        ingredients,
        steps,
        metadata,
        source_url,
    })
}

/// Instructions arrive either as `\n\n`-joined step texts (HowToStep
/// lists) or as one site-formatted blob. Prefer blank-line splits;
/// fall back to single newlines; a single paragraph stays one step.
fn split_steps(instructions: &str) -> Vec<String> {
    let blank_split: Vec<String> = instructions
        .split("\n\n")
        .map(collapse_ws)
        .filter(|s| !s.is_empty())
        .collect();
    if blank_split.len() > 1 {
        return blank_split;
    }
    let line_split: Vec<String> = instructions
        .lines()
        .map(collapse_ws)
        .filter(|s| !s.is_empty())
        .collect();
    if line_split.len() > 1 {
        return line_split;
    }
    blank_split
}

/// Step texts from a Jetpack recipe-directions container: one step
/// per `<li>`/`<p>`, whole-container text as a last resort.
fn jetpack_steps(doc: &scraper::Html) -> Vec<String> {
    let container = scraper::Selector::parse(".jetpack-recipe-directions, .e-instructions")
        .expect("static selector");
    let Some(el) = doc.select(&container).next() else {
        return Vec::new();
    };
    let items = scraper::Selector::parse("li, p").expect("static selector");
    let steps: Vec<String> = el
        .select(&items)
        .map(|item| collapse_ws(&item.text().collect::<String>()))
        .filter(|s| !s.is_empty())
        .collect();
    if !steps.is_empty() {
        return steps;
    }
    let whole = collapse_ws(&el.text().collect::<String>());
    if whole.is_empty() {
        Vec::new()
    } else {
        vec![whole]
    }
}

/// Collapse all whitespace runs (incl. newlines) to single spaces.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON_LD_PAGE: &str = r#"<!DOCTYPE html><html><head>
<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "Recipe",
  "name": "Test Banana Bread",
  "description": "Moist and easy.",
  "image": ["https://img.test/banana.jpg"],
  "recipeYield": "8 servings",
  "prepTime": "PT15M",
  "cookTime": "PT60M",
  "recipeIngredient": [
    "1 1/2 cups all-purpose flour",
    "3 ripe bananas, mashed",
    "1/2 cup butter, melted",
    "1 tsp baking soda"
  ],
  "recipeInstructions": [
    {"@type": "HowToStep", "text": "Preheat the oven to 350F."},
    {"@type": "HowToStep", "text": "Mash the bananas with the melted butter."},
    {"@type": "HowToStep", "text": "Fold in the flour and baking soda, then bake for 60 minutes."}
  ]
}
</script></head><body><h1>Test Banana Bread</h1></body></html>"#;

    #[test]
    fn json_ld_roundtrip() {
        let r = extract(JSON_LD_PAGE, "https://example.test/banana").unwrap();
        assert_eq!(r.name, "Test Banana Bread");
        assert_eq!(r.description.as_deref(), Some("Moist and easy."));
        assert_eq!(r.image.as_deref(), Some("https://img.test/banana.jpg"));
        assert_eq!(r.ingredients.len(), 4);
        assert_eq!(r.steps.len(), 3);
        assert!(r.steps[0].contains("Preheat"));
        assert_eq!(
            r.metadata.get("servings").map(String::as_str),
            Some("8 servings")
        );
        assert!(r.metadata.contains_key("prep time"), "{:?}", r.metadata);
        assert_eq!(r.source_url.as_deref(), Some("https://example.test/banana"));
    }

    #[test]
    fn no_recipe_markup_is_a_clear_error() {
        let err = extract(
            "<html><body><p>blog spam</p></body></html>",
            "https://x.test/",
        )
        .unwrap_err();
        assert!(matches!(err, ImportError::NoRecipeFound { .. }), "{err}");
    }

    /// Jetpack-style page: microdata name + ingredients but no
    /// `recipeInstructions` itemprop — directions only as a
    /// `.jetpack-recipe-directions` div (Smitten Kitchen's shape).
    const JETPACK_PAGE: &str = r#"<!DOCTYPE html><html><body>
<div class="jetpack-recipe" itemscope itemtype="https://schema.org/Recipe">
  <h3 itemprop="name">Blueberry Muffin Loaf</h3>
  <ul class="jetpack-recipe-ingredients">
    <li class="jetpack-recipe-ingredient" itemprop="recipeIngredient">5 tablespoons butter</li>
    <li class="jetpack-recipe-ingredient" itemprop="recipeIngredient">1 cup blueberries</li>
  </ul>
  <div class="jetpack-recipe-directions e-instructions">
    <p>Heat oven to 350F and butter a loaf pan.</p>
    <p>Fold the blueberries into the batter with the melted butter and bake.</p>
  </div>
</div></body></html>"#;

    #[test]
    fn jetpack_directions_fallback() {
        let r = extract(JETPACK_PAGE, "https://example.test/jetpack").unwrap();
        assert_eq!(r.name, "Blueberry Muffin Loaf");
        assert_eq!(r.ingredients.len(), 2);
        assert_eq!(r.steps.len(), 2);
        assert!(r.steps[0].contains("Heat oven"), "{:?}", r.steps);
    }

    #[test]
    fn step_splitting_prefers_blank_lines() {
        assert_eq!(split_steps("a\n\nb\n\nc").len(), 3);
        assert_eq!(split_steps("a\nb\nc").len(), 3);
        assert_eq!(split_steps("one paragraph only").len(), 1);
    }
}
