//! [`NormalizedRecipe`] — the pipeline's pivot type.
//!
//! Stage 2 (extraction) produces it; both synthesizers consume it.
//! It is deliberately dumb: free-text ingredient lines and step
//! texts, exactly as the site published them. All interpretation
//! (qty/unit parsing, inline `@ingredient{}` weaving) happens in
//! stage 3 so that the LLM and the heuristic start from the same
//! pinned source of truth.

use std::collections::BTreeMap;

use serde::Serialize;

/// A recipe as extracted from a webpage (or, in V2, from a
/// Mealie/Paprika/Tandoor export) — see the crate docs.
#[derive(Debug, Clone, Default, Serialize)]
pub struct NormalizedRecipe {
    /// Display title ("Classic Banana Bread").
    pub name: String,
    /// Short description, when the site published one.
    pub description: Option<String>,
    /// First/primary image URL.
    pub image: Option<String>,
    /// One free-text line per ingredient, site-verbatim
    /// ("1 1/2 cups all-purpose flour, sifted").
    pub ingredients: Vec<String>,
    /// One step per entry, site-verbatim. HowToSection nesting is
    /// already flattened by the extractor.
    pub steps: Vec<String>,
    /// Cooklang-convention metadata lifted from the structured data:
    /// `servings`, `prep time`, `cook time`, `time required`,
    /// `course`, `cuisine`, `diet`, `tags`, `author`, … Values are
    /// human-readable strings (durations already converted from
    /// ISO-8601 by the extractor).
    pub metadata: BTreeMap<String, String>,
    /// Where it came from. Lands in `source:` frontmatter.
    pub source_url: Option<String>,
}

impl NormalizedRecipe {
    /// `true` when there is enough material to synthesize from.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.ingredients.is_empty() && !self.steps.is_empty()
    }

    /// Compact JSON for the LLM prompt (the "pinned source of truth"
    /// block). Stable key order via `BTreeMap` + struct order.
    #[must_use]
    pub fn to_prompt_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }
}
