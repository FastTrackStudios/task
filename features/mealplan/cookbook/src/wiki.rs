//! Bridge from `.cook` files to the wiki graph.
//!
//! The wiki feature indexes wikilinks (`[[name]]`) emanating
//! from each page so the backlinks pane on `Wiki/flour.md`
//! lists every page that mentions flour. Recipes don't write
//! `[[flour]]` — they write `@flour{500%g}` — so without a
//! bridge, recipes would be invisible to wiki backlinks.
//!
//! [`recipe_wiki_edges`] projects each cooklang `@ingredient`
//! into a wikilink edge `recipe_path → ingredient_name`.
//! Wiki indexers call this on every `.cook` file they discover
//! and feed the result into the same edge store that handles
//! markdown `[[...]]` links.

use crate::model::Recipe;

/// One wiki edge derived from a recipe.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WikiEdge {
    /// Source — the recipe path (vault-relative `.cook`).
    pub source: String,
    /// Target — the wikilink basename (e.g. `"flour"`).
    /// Wiki resolution does case-insensitive lookup against
    /// page basenames in the rest of the vault.
    pub target: String,
    /// What flavor of cooklang reference produced the edge.
    pub kind: WikiEdgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WikiEdgeKind {
    /// `@ingredient` → wikilink to the ingredient's wiki
    /// page (typically a pantry item like `[[flour]]`).
    Ingredient,
    /// `#cookware` → wikilink to the cookware's wiki page
    /// (`[[skillet]]`, etc.).
    Cookware,
    /// A reference to another recipe — `[[Hot Honey]]{6}` in the
    /// vault's own link syntax, or cooklang's `@@./hot-honey{}`.
    /// `target` is whatever the reference named: a display name for
    /// the wikilink form, a relative path for the cooklang one.
    RecipeRef,
    /// `[[wikilink]]` embedded in a step body — concept
    /// pages, techniques, related recipes. Cooklang treats
    /// `[[...]]` as plain text, so the syntax round-trips
    /// through the parser unchanged; we extract it here so
    /// the wiki graph picks up the same backlinks it would
    /// for a markdown page.
    Concept,
}

/// Project the recipe into a flat list of wiki edges. One
/// edge per `@ingredient` (deduplicated), one per
/// `#cookware`, one per `@@recipe-ref`.
#[must_use]
pub fn recipe_wiki_edges(recipe: &Recipe) -> Vec<WikiEdge> {
    let mut out: Vec<WikiEdge> = Vec::new();
    let mut seen: std::collections::HashSet<(String, WikiEdgeKind)> =
        std::collections::HashSet::new();

    for ing in recipe.ingredients.iter() {
        if ing.is_recipe_ref {
            // Recipe refs are handled below from
            // `nested_recipes` to keep `target` as the path.
            continue;
        }
        let key = (ing.name.to_ascii_lowercase(), WikiEdgeKind::Ingredient);
        if seen.insert(key) {
            out.push(WikiEdge {
                source: recipe.path.clone(),
                target: ing.name.clone(),
                kind: WikiEdgeKind::Ingredient,
            });
        }
    }

    for cw in recipe.cookware.iter() {
        let key = (cw.to_ascii_lowercase(), WikiEdgeKind::Cookware);
        if seen.insert(key) {
            out.push(WikiEdge {
                source: recipe.path.clone(),
                target: cw.clone(),
                kind: WikiEdgeKind::Cookware,
            });
        }
    }

    for path in recipe.nested_recipes.iter() {
        let key = (path.to_ascii_lowercase(), WikiEdgeKind::RecipeRef);
        if seen.insert(key) {
            out.push(WikiEdge {
                source: recipe.path.clone(),
                target: path.clone(),
                kind: WikiEdgeKind::RecipeRef,
            });
        }
        // A `[[Sauce]]{}` reference is a wikilink too, so claim it as a
        // concept as well — otherwise the scan below re-emits the same
        // target as a weaker `Concept` edge alongside this one.
        seen.insert((path.to_ascii_lowercase(), WikiEdgeKind::Concept));
    }

    // `[[wikilinks]]` embedded in step text. Cooklang passes
    // these through as plain text, so we scan the rendered
    // step strings plus the raw source as a fallback (covers
    // links sitting in metadata values or text-only sections).
    for step in recipe.steps.iter() {
        for target in scan_wikilinks(step) {
            let key = (target.to_ascii_lowercase(), WikiEdgeKind::Concept);
            if seen.insert(key) {
                out.push(WikiEdge {
                    source: recipe.path.clone(),
                    target,
                    kind: WikiEdgeKind::Concept,
                });
            }
        }
    }
    for target in scan_wikilinks(&recipe.source) {
        let key = (target.to_ascii_lowercase(), WikiEdgeKind::Concept);
        if seen.insert(key) {
            out.push(WikiEdge {
                source: recipe.path.clone(),
                target,
                kind: WikiEdgeKind::Concept,
            });
        }
    }

    out
}

/// A recipe called for by another recipe, written in the vault's own
/// link syntax: `[[Hot Honey]]{6}`.
#[derive(Debug, Clone, PartialEq)]
pub struct RecipeLink {
    /// The wikilink target — a recipe's display name or file stem.
    pub target: String,
    /// Servings of the referenced recipe. `None` for `[[X]]{}`, which
    /// means one whole batch of it.
    pub servings: Option<f64>,
}

/// One wikilink as it appears in a string, with the byte span it
/// occupies so a renderer can replace the markup with the display text
/// instead of showing `[[Sauce]]{}` to someone who is cooking.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkSpan {
    /// What the link points at, pipe-alias stripped.
    pub target: String,
    /// What to show: the alias if the link carried one, else the target.
    pub display: String,
    /// Servings, for the braced form. `None` means one whole batch.
    pub servings: Option<f64>,
    /// Whether the trailing `{…}` was present — see [`scan_recipe_links`]
    /// for why that brace is the thing that makes it a recipe.
    pub is_recipe: bool,
    /// Byte offset of the opening `[`.
    pub start: usize,
    /// Byte length of the whole run, including `{…}` when braced.
    pub len: usize,
}

/// Find every wikilink in `s`, braced or bare, with its span.
///
/// This is the one bracket walk; [`scan_recipe_links`] and
/// [`scan_wikilinks`] are filters over it. Recording spans here rather
/// than re-finding the markup downstream is what lets the reader render
/// a link as its display text — the parser knows exactly which bytes
/// were markup, and nothing else has to guess.
#[must_use]
pub fn scan_links(s: &str) -> Vec<LinkSpan> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let rest = &s[i + 2..];
            if let Some(end) = rest.find("]]") {
                let inner = rest[..end].trim();
                let target = inner.split('|').next().unwrap_or(inner).trim();
                let display = inner
                    .split_once('|')
                    .map_or(target, |(_, alias)| alias.trim());
                // `[[…]]` is 4 bytes of delimiter around `end` of inner.
                let mut len = 2 + end + 2;
                let mut servings = None;
                let mut is_recipe = false;
                if let Some(brace) = rest[end + 2..].strip_prefix('{')
                    && let Some(close) = brace.find('}')
                {
                    let qty = brace[..close].trim();
                    servings = if qty.is_empty() {
                        None
                    } else {
                        qty.parse::<f64>().ok()
                    };
                    is_recipe = true;
                    len += 1 + close + 1;
                }
                if !target.is_empty() && !target.contains('\n') {
                    out.push(LinkSpan {
                        target: target.to_string(),
                        display: if display.is_empty() {
                            target.to_string()
                        } else {
                            display.to_string()
                        },
                        servings,
                        is_recipe,
                        start: i,
                        len,
                    });
                }
                i += 2 + end + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Find every `[[target]]{…}` in `s` — a recipe reference in vault
/// link form.
///
/// The trailing brace is what makes it a reference, and it is not
/// decoration. Recipes link to plenty of things that are not recipes —
/// concepts, techniques, a "see also" pointing at another dish — and
/// treating a bare `[[Garlic Pasta]]` as a reference would quietly
/// add spaghetti to the shopping list of whatever mentioned it. So a
/// bare wikilink stays an ordinary wiki edge, and only the braced form
/// pulls a recipe in.
#[must_use]
pub fn scan_recipe_links(s: &str) -> Vec<RecipeLink> {
    scan_links(s)
        .into_iter()
        .filter(|l| l.is_recipe)
        .map(|l| RecipeLink {
            target: l.target,
            servings: l.servings,
        })
        .collect()
}

/// Find every `[[target]]` in `s`. Targets are returned
/// trimmed; nested or escaped brackets are not supported
/// (matches Obsidian-style wikilink semantics).
fn scan_wikilinks(s: &str) -> Vec<String> {
    scan_links(s).into_iter().map(|l| l.target).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_cook;

    #[test]
    fn emits_one_edge_per_unique_ingredient() {
        let src = "\
>> title: Pasta
Cook @flour{200%g} and toss with @flour{50%g} for dusting and @salt{1%tsp}.
";
        let r = parse_cook("Cookbook/Pasta.cook", src).unwrap();
        let edges = recipe_wiki_edges(&r);
        let names: Vec<&str> = edges
            .iter()
            .filter(|e| e.kind == WikiEdgeKind::Ingredient)
            .map(|e| e.target.as_str())
            .collect();
        assert_eq!(names, vec!["flour", "salt"]);
    }

    #[test]
    fn extracts_concept_wikilinks_from_step_bodies() {
        let src = "\
>> title: Carbonara

Whisk @eggs{2} per [[mise en place]] guidelines.
Cook @pancetta{100%g} until crisp - see [[render fat]] for technique.
Reference [[render fat]] again to dedupe.
";
        let r = parse_cook("Cookbook/Carbonara.cook", src).unwrap();
        let edges = recipe_wiki_edges(&r);
        let concepts: Vec<&str> = edges
            .iter()
            .filter(|e| e.kind == WikiEdgeKind::Concept)
            .map(|e| e.target.as_str())
            .collect();
        // Order: first-occurrence; dedup case-insensitive.
        assert!(concepts.contains(&"mise en place"));
        assert!(concepts.contains(&"render fat"));
        // Dedup: "render fat" appears twice in source but
        // only once in edges.
        assert_eq!(concepts.iter().filter(|t| **t == "render fat").count(), 1);
    }

    #[test]
    fn a_recipe_link_is_one_edge_not_two() {
        let src = "Serve @rice{100%g} with [[Hot Honey]]{6}.";
        let r = parse_cook("Cookbook/Bowl.cook", src).unwrap();
        let edges = recipe_wiki_edges(&r);
        let hits: Vec<&WikiEdge> = edges
            .iter()
            .filter(|e| e.target.eq_ignore_ascii_case("Hot Honey"))
            .collect();
        assert_eq!(hits.len(), 1, "one edge per link, got {hits:?}");
        assert_eq!(
            hits[0].kind,
            WikiEdgeKind::RecipeRef,
            "and it should be the specific kind, not a generic concept"
        );
    }

    #[test]
    fn link_spans_cover_the_whole_markup_run() {
        // The reader draws `display` over `[start, start+len)`, so if the
        // span is short by even the trailing `{}` the brackets survive
        // on screen — which is the bug this exists to prevent.
        let s = "Make a batch of [[Low-Cal Taco Sauce]]{} and chill it.";
        let links = scan_links(s);
        assert_eq!(links.len(), 1);
        let l = &links[0];
        assert_eq!(&s[l.start..l.start + l.len], "[[Low-Cal Taco Sauce]]{}");
        assert_eq!(l.display, "Low-Cal Taco Sauce");
        assert!(l.is_recipe);
        assert_eq!(l.servings, None);
    }

    #[test]
    fn bare_and_braced_links_are_told_apart() {
        let s = "See [[mise en place]] then make [[Hot Honey]]{6}.";
        let links = scan_links(s);
        assert_eq!(links.len(), 2);
        assert!(!links[0].is_recipe, "a bare link is not a recipe pull");
        assert_eq!(
            &s[links[0].start..links[0].start + links[0].len],
            "[[mise en place]]"
        );
        assert!(links[1].is_recipe);
        assert_eq!(links[1].servings, Some(6.0));
        assert_eq!(
            &s[links[1].start..links[1].start + links[1].len],
            "[[Hot Honey]]{6}"
        );
    }

    #[test]
    fn alias_displays_the_alias_but_links_the_target() {
        let l = &scan_links("Use [[saute|sautéing]] here.")[0];
        assert_eq!(l.target, "saute");
        assert_eq!(l.display, "sautéing");
    }

    #[test]
    fn spans_survive_multibyte_text_before_them() {
        // Byte offsets, not char offsets — an em-dash earlier in the
        // step must not shift the span off the markup.
        let s = "Brown the beef — then make [[Sauce]]{}.";
        let l = &scan_links(s)[0];
        assert_eq!(&s[l.start..l.start + l.len], "[[Sauce]]{}");
    }

    #[test]
    fn wikilink_with_pipe_alias() {
        let src = "Use [[saute|sautéing]] on medium heat.";
        let r = parse_cook("Cookbook/X.cook", src).unwrap();
        let edges = recipe_wiki_edges(&r);
        assert!(
            edges
                .iter()
                .any(|e| e.kind == WikiEdgeKind::Concept && e.target == "saute"),
            "got {edges:?}"
        );
    }

    #[test]
    fn includes_cookware_and_recipe_refs() {
        let src = "\
>> title: Pizza

Make @@./Shared/Dough{} on a #stone{}.
";
        let r = parse_cook("Cookbook/Pizza.cook", src).unwrap();
        let edges = recipe_wiki_edges(&r);
        assert!(
            edges
                .iter()
                .any(|e| e.kind == WikiEdgeKind::Cookware && e.target == "stone")
        );
        assert!(
            edges
                .iter()
                .any(|e| e.kind == WikiEdgeKind::RecipeRef && e.target.ends_with("Dough"))
        );
    }
}
