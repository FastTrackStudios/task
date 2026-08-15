//! Stage 3b — deterministic (offline) synthesis.
//!
//! Guarantees, in order of importance:
//! 1. **Never drops an ingredient** — every normalized ingredient line
//!    becomes exactly one `@…{}` annotation; anything that can't be
//!    matched to a step lands in a trailing `TODO (import review)`
//!    step, so it still shows up in the parsed ingredient list.
//! 2. **Always parser-valid** — step text is sanitized of cooklang
//!    metacharacters before annotations are inserted, the result is
//!    validated, and a maximally-plain fallback rendering exists for
//!    the (defensive) case where validation still fails.
//! 3. Marked `import: needs-review` in the frontmatter so a later
//!    `task recipe polish` pass can find heuristic imports.
//!
//! Matching is first-occurrence: each ingredient's parsed name (via
//! the `ingredient` crate), then progressively shorter suffixes
//! ("ripe bananas" → "bananas" → "banana"), searched across steps at
//! word boundaries; more specific (longer) names claim spans first so
//! "peanut butter" wins over "butter".

use serde_yaml::{Mapping, Value as Yaml};

use crate::normalized::NormalizedRecipe;
use crate::validate::validate_cook;

/// Synthesize a `.cook` source without any LLM. Infallible.
#[must_use]
pub fn synthesize_heuristic(recipe: &NormalizedRecipe) -> String {
    let parsed: Vec<ParsedIngredient> = recipe.ingredients.iter().map(|l| parse_line(l)).collect();
    let steps: Vec<String> = recipe.steps.iter().map(|s| sanitize_step(s)).collect();

    let (annotated_steps, unmatched) = weave(&steps, &parsed);

    let mut body = annotated_steps.join("\n\n");
    if !unmatched.is_empty() {
        let list: Vec<String> = unmatched
            .iter()
            .map(|i| format!("@{}{}", i.safe_name(), i.amount_token()))
            .collect();
        body.push_str(&format!(
            "\n\nTODO (import review): place these unmatched ingredients where they belong — {}.",
            list.join(", ")
        ));
    }

    let source = format!("---\n{}---\n\n{}\n", frontmatter(recipe), body);
    if validate_cook("import.cook", &source).is_ok() {
        return source;
    }

    // Defensive fallback: no inline annotations at all — plain steps
    // plus one gather-step carrying every ingredient annotation.
    tracing::warn!("heuristic cooklang failed validation; falling back to plain rendering");
    let plain_steps: Vec<String> = recipe.steps.iter().map(|s| sanitize_hard(s)).collect();
    let list: Vec<String> = parsed
        .iter()
        .map(|i| format!("@{}{}", i.safe_name(), i.amount_token()))
        .collect();
    let fallback = format!(
        "---\n{}---\n\nGather the ingredients: {}.\n\n{}\n",
        frontmatter(recipe),
        list.join(", "),
        plain_steps.join("\n\n"),
    );
    if validate_cook("import.cook", &fallback).is_ok() {
        return fallback;
    }
    // Last resort: drop annotations entirely (ingredients survive as
    // plain text in the gather step — visible, if not structured).
    let plain_list: Vec<String> = parsed.iter().map(|i| i.safe_name()).collect();
    format!(
        "---\n{}---\n\nGather the ingredients: {}.\n\n{}\n",
        frontmatter(recipe),
        plain_list.join(", "),
        plain_steps.join("\n\n"),
    )
}

// ── Ingredient line parsing ──────────────────────────────────

struct ParsedIngredient {
    /// Parsed name ("all-purpose flour") — used for matching and as
    /// the annotation name when unmatched.
    name: String,
    /// Rendered cooklang amount token: `{1.5%cups}` / `{3}` / `{}`.
    amount: String,
}

impl ParsedIngredient {
    /// Name made safe for use inside `@…{}` (the braces terminate the
    /// name, so only brace/at/comment chars need scrubbing).
    fn safe_name(&self) -> String {
        sanitize_hard(&self.name)
    }
    fn amount_token(&self) -> &str {
        &self.amount
    }
}

fn parse_line(line: &str) -> ParsedIngredient {
    // Parentheticals ("(320 grams or 11.5 ounces)") routinely derail
    // the qty/name split; the primary amount never lives in one.
    let cleaned = strip_parens(line);
    let ing = ingredient::from_str(&cleaned);
    let name = ing.name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = if name.is_empty() {
        cleaned.trim().to_string()
    } else {
        name
    };

    let amount = ing.amounts.first().map_or_else(String::new, |m| {
        let (value, upper) = m.values();
        let qty = match upper {
            Some(u) => format!("{}-{}", fmt_num(value), fmt_num(u)),
            None => fmt_num(value),
        };
        match unit_str(&m.unit()) {
            Some(unit) => format!("{qty}%{unit}"),
            None => qty,
        }
    });
    ParsedIngredient {
        name,
        amount: format!("{{{amount}}}"),
    }
}

/// Render the `ingredient` crate's unit as a cooklang-friendly short
/// string. `None` for count-like pseudo-units ("whole") where a bare
/// quantity reads better.
fn unit_str(unit: &ingredient::unit::Unit) -> Option<String> {
    use ingredient::unit::Unit;
    let s = match unit {
        Unit::Gram => "g".into(),
        Unit::Kilogram => "kg".into(),
        Unit::Liter => "l".into(),
        Unit::Milliliter => "ml".into(),
        Unit::Teaspoon => "tsp".into(),
        Unit::Tablespoon => "tbsp".into(),
        Unit::Cup => "cup".into(),
        Unit::Quart => "quart".into(),
        Unit::FluidOunce => "fl oz".into(),
        Unit::Ounce => "oz".into(),
        Unit::Pound => "lb".into(),
        Unit::Other(s) => {
            let s = s.trim();
            if s.is_empty() || s.eq_ignore_ascii_case("whole") {
                return None;
            }
            sanitize_hard(s)
        }
        other => sanitize_hard(&format!("{other:?}").to_lowercase()),
    };
    Some(s)
}

fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

// ── First-occurrence weaving ─────────────────────────────────

/// Insert each ingredient's annotation at its first word-boundary
/// occurrence across `steps`. Returns the annotated steps and the
/// ingredients that never matched.
fn weave<'a>(
    steps: &[String],
    ingredients: &'a [ParsedIngredient],
) -> (Vec<String>, Vec<&'a ParsedIngredient>) {
    // (step idx, byte range, ingredient idx)
    let mut claims: Vec<(usize, std::ops::Range<usize>, usize)> = Vec::new();
    let mut unmatched: Vec<&ParsedIngredient> = Vec::new();

    // Longest names first so "peanut butter" claims before "butter".
    let mut order: Vec<usize> = (0..ingredients.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(ingredients[i].name.len()));

    for ing_idx in order {
        let ing = &ingredients[ing_idx];
        let mut found = false;
        'candidates: for cand in candidates(&ing.name) {
            for (step_idx, step) in steps.iter().enumerate() {
                if let Some(range) = find_word(step, &cand, |r| {
                    !claims
                        .iter()
                        .any(|(s, c, _)| *s == step_idx && overlaps(c, r))
                }) {
                    claims.push((step_idx, range, ing_idx));
                    found = true;
                    break 'candidates;
                }
            }
        }
        if !found {
            unmatched.push(ing);
        }
    }
    // Restore ingredient-list order for the TODO block.
    unmatched.sort_by_key(|i| std::ptr::from_ref::<ParsedIngredient>(*i) as usize);

    let mut out = Vec::with_capacity(steps.len());
    for (step_idx, step) in steps.iter().enumerate() {
        let mut spans: Vec<&(usize, std::ops::Range<usize>, usize)> =
            claims.iter().filter(|(s, _, _)| *s == step_idx).collect();
        spans.sort_by_key(|(_, r, _)| r.start);

        let mut built = String::with_capacity(step.len() + 16 * spans.len());
        let mut cursor = 0usize;
        for (_, range, ing_idx) in spans {
            built.push_str(&step[cursor..range.start]);
            built.push('@');
            built.push_str(&step[range.clone()]);
            built.push_str(ingredients[*ing_idx].amount_token());
            cursor = range.end;
        }
        built.push_str(&step[cursor..]);
        out.push(built);
    }
    (out, unmatched)
}

/// Candidate search strings for an ingredient name, most → least
/// specific: the comma-trimmed name, then suffixes with leading words
/// dropped (each also in naive singular form), then a last-resort pass
/// over the individual words minus prep-adjective noise ("Finely
/// grated zest from one lemon" still finds "zest"). All lowercase,
/// min 3 chars.
fn candidates(name: &str) -> Vec<String> {
    // Trailing modifiers (", cold is fine") would otherwise pin every
    // suffix to words that never appear in steps.
    let base = name.split(',').next().unwrap_or(name);
    let words: Vec<&str> = base.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut push = |cand: String| {
        if cand.len() >= 3 && !out.contains(&cand) {
            out.push(cand);
        }
    };
    for start in 0..words.len() {
        let cand = words[start..].join(" ").to_lowercase();
        let singular = cand.strip_suffix('s').unwrap_or(&cand).to_string();
        push(cand);
        push(singular);
    }
    for w in &words {
        let w = w
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        if w.len() >= 4 && !STOPWORDS.contains(&w.as_str()) {
            let singular = w.strip_suffix('s').unwrap_or(&w).to_string();
            push(w.clone());
            push(singular);
        }
    }
    out
}

/// Words too generic (units, prep adjectives, glue) to identify an
/// ingredient on their own in the word-level fallback.
const STOPWORDS: [&str; 36] = [
    "with",
    "from",
    "into",
    "over",
    "plus",
    "more",
    "optional",
    "taste",
    "fresh",
    "frozen",
    "cold",
    "warm",
    "room",
    "temperature",
    "large",
    "small",
    "medium",
    "extra",
    "finely",
    "coarsely",
    "grated",
    "chopped",
    "minced",
    "sliced",
    "diced",
    "melted",
    "softened",
    "unsalted",
    "salted",
    "ground",
    "whole",
    "light",
    "dark",
    "heavy",
    "divided",
    "packed",
];

/// Drop `( … )` groups (non-nested is all recipes use).
fn strip_parens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Case-insensitive word-boundary search; `free` filters out ranges
/// already claimed.
fn find_word(
    haystack: &str,
    needle: &str,
    free: impl Fn(&std::ops::Range<usize>) -> bool,
) -> Option<std::ops::Range<usize>> {
    let hay_lc = haystack.to_lowercase();
    let mut from = 0usize;
    while let Some(pos) = hay_lc[from..].find(needle) {
        let start = from + pos;
        let end = start + needle.len();
        // `to_lowercase` can shift byte offsets for non-ASCII text;
        // guard before slicing the original.
        let boundary_ok = haystack.is_char_boundary(start)
            && haystack.is_char_boundary(end)
            && haystack[start..end].eq_ignore_ascii_case(needle);
        let before_ok = start == 0
            || haystack[..start]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_alphanumeric());
        let after_ok = end >= haystack.len()
            || haystack[end..]
                .chars()
                .next()
                .is_some_and(|c| !c.is_alphanumeric());
        let range = start..end;
        if boundary_ok && before_ok && after_ok && free(&range) {
            return Some(range);
        }
        from = start + needle.len().max(1);
        if from >= hay_lc.len() {
            break;
        }
    }
    None
}

fn overlaps(a: &std::ops::Range<usize>, b: &std::ops::Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

// ── Sanitization ─────────────────────────────────────────────

/// Make free text safe to host `@…{}` insertions: strip cooklang
/// metacharacters, neutralize comments, keep everything else.
fn sanitize_step(s: &str) -> String {
    let s = s.replace("--", "–").replace("[-", "(-").replace("-]", "-)");
    let s: String = s
        .chars()
        .filter(|c| !matches!(c, '@' | '#' | '~'))
        .map(|c| match c {
            '{' => '(',
            '}' => ')',
            c => c,
        })
        .collect();
    // `>` / `=` / `[` only matter at line starts; steps are single
    // lines, so trim them off the front.
    s.trim_start_matches(['>', '=', '[', ' '])
        .trim()
        .to_string()
}

/// Even stricter: also drop braces/brackets entirely (used inside
/// annotation names and the plain fallback).
fn sanitize_hard(s: &str) -> String {
    let s = s.replace("--", "–");
    s.chars()
        .filter(|c| !matches!(c, '@' | '#' | '~' | '{' | '}' | '[' | ']'))
        .collect::<String>()
        .trim()
        .to_string()
}

// ── Frontmatter ──────────────────────────────────────────────

/// Cooklang-convention metadata keys, emitted in this order when
/// present in the normalized metadata.
const KNOWN_KEYS: [&str; 9] = [
    "servings",
    "prep time",
    "cook time",
    "time required",
    "course",
    "cuisine",
    "diet",
    "tags",
    "author",
];

fn frontmatter(recipe: &NormalizedRecipe) -> String {
    let mut map = Mapping::new();
    map.insert(Yaml::from("title"), Yaml::from(recipe.name.as_str()));
    if let Some(d) = &recipe.description {
        map.insert(Yaml::from("description"), Yaml::from(d.as_str()));
    }
    for key in KNOWN_KEYS {
        if let Some(v) = recipe.metadata.get(key) {
            map.insert(Yaml::from(key), Yaml::from(v.as_str()));
        }
    }
    for (k, v) in &recipe.metadata {
        if !KNOWN_KEYS.contains(&k.as_str()) {
            map.insert(Yaml::from(k.as_str()), Yaml::from(v.as_str()));
        }
    }
    if let Some(s) = &recipe.source_url {
        map.insert(Yaml::from("source"), Yaml::from(s.as_str()));
    }
    if let Some(i) = &recipe.image {
        map.insert(Yaml::from("image"), Yaml::from(i.as_str()));
    }
    // The marker `task recipe polish` (and humans) will look for.
    map.insert(Yaml::from("import"), Yaml::from("needs-review"));
    serde_yaml::to_string(&map).unwrap_or_else(|_| format!("title: {}\n", recipe.name))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::synthesize::coverage;

    fn banana_bread() -> NormalizedRecipe {
        NormalizedRecipe {
            name: "Banana Bread".into(),
            description: Some("Moist and easy.".into()),
            image: Some("https://img.test/banana.jpg".into()),
            ingredients: vec![
                "1 1/2 cups all-purpose flour".into(),
                "3 ripe bananas".into(),
                "1/2 cup butter, melted".into(),
                "1 tsp baking soda".into(),
                "1 pinch salt".into(),
                "2 large eggs".into(),
            ],
            steps: vec![
                "Preheat the oven to 350F (175C).".into(),
                "Mash the bananas with the melted butter, then beat in the eggs.".into(),
                "Fold in the flour and baking soda; bake for 60 minutes.".into(),
            ],
            metadata: BTreeMap::from([
                ("servings".to_string(), "8".to_string()),
                ("prep time".to_string(), "15 minutes".to_string()),
            ]),
            source_url: Some("https://example.test/banana".into()),
        }
    }

    #[test]
    fn always_parser_valid() {
        let src = synthesize_heuristic(&banana_bread());
        validate_cook("t.cook", &src).expect("heuristic output must parse");
    }

    #[test]
    fn never_drops_ingredients() {
        let r = banana_bread();
        let src = synthesize_heuristic(&r);
        let cov = coverage(&r, &src);
        assert!(cov.complete(), "unmatched: {:?}\n{src}", cov.unmatched);
    }

    #[test]
    fn unmatched_ingredients_land_in_todo_block() {
        let r = banana_bread();
        let src = synthesize_heuristic(&r);
        // "salt" never appears in a step → must be in the TODO step,
        // still as an @annotation.
        assert!(src.contains("TODO (import review)"), "{src}");
        assert!(src.contains("@salt"), "{src}");
    }

    #[test]
    fn first_occurrence_only_and_amounts_attached() {
        let src = synthesize_heuristic(&banana_bread());
        // bananas matched once, with the count.
        assert_eq!(src.matches("@banana").count(), 1, "{src}");
        assert!(
            src.contains("@flour{1.5%cup") || src.contains("@flour{1.5%cups"),
            "{src}"
        );
    }

    #[test]
    fn marks_needs_review_and_carries_metadata() {
        let src = synthesize_heuristic(&banana_bread());
        assert!(src.contains("import: needs-review"), "{src}");
        assert!(src.contains("title: Banana Bread"), "{src}");
        assert!(src.contains("source: https://example.test/banana"), "{src}");
        assert!(src.contains("servings:"), "{src}");
    }

    #[test]
    fn hostile_step_text_is_sanitized() {
        let r = NormalizedRecipe {
            name: "Hostile".into(),
            ingredients: vec!["1 cup rice".into()],
            steps: vec![
                "Add rice -- the @good kind -- to a #pot{} and stir ~5 minutes [- joke -].".into(),
            ],
            ..Default::default()
        };
        let src = synthesize_heuristic(&r);
        validate_cook("t.cook", &src).expect("sanitized output must parse");
        let cov = coverage(&r, &src);
        assert!(cov.complete(), "{src}");
    }

    #[test]
    fn parentheticals_and_trailing_modifiers_still_match() {
        // Real lines from smittenkitchen.com that the naive matcher
        // missed: parenthetical weights + ", cold is fine" modifier.
        let r = NormalizedRecipe {
            name: "Loaf".into(),
            ingredients: vec![
                "8 tablespoons (1/2 cup or 115 grams) unsalted butter, cold is fine".into(),
                "Finely grated zest from one lemon".into(),
            ],
            steps: vec!["Melt the butter in a bowl, then whisk in the sugar and zest.".into()],
            ..Default::default()
        };
        let src = synthesize_heuristic(&r);
        validate_cook("t.cook", &src).unwrap();
        assert!(src.contains("@butter{8%tbsp}"), "{src}");
        assert!(src.contains("@zest"), "{src}");
        assert!(!src.contains("TODO"), "{src}");
    }

    #[test]
    fn specific_names_win_over_generic() {
        let r = NormalizedRecipe {
            name: "PB".into(),
            ingredients: vec!["1 cup peanut butter".into(), "2 tbsp butter".into()],
            steps: vec!["Melt the butter, then stir in the peanut butter.".into()],
            ..Default::default()
        };
        let src = synthesize_heuristic(&r);
        validate_cook("t.cook", &src).unwrap();
        assert!(src.contains("@peanut butter{1%cup}"), "{src}");
        assert!(src.contains("@butter{2%tbsp}"), "{src}");
    }
}
