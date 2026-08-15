//! Convert legacy `Cookbook/*.md` recipes to cooklang
//! `Cookbook/*.cook` files.
//!
//! Usage:
//!
//! ```text
//! migrate-md-to-cook <wiki_root> [--dry-run]
//! ```
//!
//! `<wiki_root>` is typically `<org>/wiki/Knowledge/` — i.e.
//! the directory that *contains* the `Cookbook/` subdir.
//!
//! Idempotent. Reads `<wiki_root>/Cookbook/*.md`, writes
//! `<wiki_root>/Cookbook/<slug>.cook` alongside them. Leaves
//! originals in place — delete after manual review.
//!
//! Mapping:
//! - YAML frontmatter scalars → cooklang metadata block
//!   (`>> title:`, `>> servings:`, etc.).
//! - For each ingredient row, finds its name in step text
//!   and rewrites to `@name{qty%unit}`. Ingredients not
//!   mentioned in any step are appended as a final line so
//!   cooklang still picks them up.
//! - `pantry_item_id` links: **dropped**. Pantry resolution
//!   is name-based at mealprep time now.
//! - Nutrition: **dropped**. Computed from pantry data.
//! - Substitutions on the recipe row: **dropped**. Encode
//!   as pantry-side substitutes or registry rules.
//!
//! Files that don't look like recipes (`type: recipe` or
//! `recipe` in `tags:`) are skipped.

use std::path::{Path, PathBuf};

fn main() {
    let mut args = std::env::args().skip(1);
    let wiki_root = args.next().unwrap_or_else(|| {
        eprintln!("usage: migrate-md-to-cook <wiki_root> [--dry-run]");
        std::process::exit(2);
    });
    let dry_run = args.any(|a| a == "--dry-run");
    let root = PathBuf::from(&wiki_root);
    if !root.is_dir() {
        eprintln!("not a directory: {wiki_root}");
        std::process::exit(2);
    }

    let src_dir = root.join("Cookbook");
    let dst_dir = src_dir.clone();
    if !src_dir.exists() {
        eprintln!("no Cookbook/ at {wiki_root}; nothing to migrate");
        return;
    }
    if !dry_run {
        std::fs::create_dir_all(&dst_dir).expect("mkdir Cookbook/");
    }

    let mut converted = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    for entry in walkdir::WalkDir::new(&src_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("md"))
        })
    {
        let path = entry.path();
        let Ok(raw) = std::fs::read_to_string(path) else {
            eprintln!("read failed: {}", path.display());
            failed += 1;
            continue;
        };
        let Some(out) = convert(&raw) else {
            skipped += 1;
            continue;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("recipe");
        let out_path = dst_dir.join(format!("{stem}.cook"));
        if dry_run {
            println!("[dry-run] would write {}", out_path.display());
        } else {
            if let Err(e) = std::fs::write(&out_path, out) {
                eprintln!("write failed: {}: {e}", out_path.display());
                failed += 1;
                continue;
            }
            println!("wrote {}", out_path.display());
        }
        converted += 1;
    }
    println!("\nDone. converted={converted} skipped={skipped} failed={failed}");
}

fn convert(raw: &str) -> Option<String> {
    let (fm, body) = vault_entity::frontmatter::split(raw)?;
    let map: serde_yaml::Mapping = serde_yaml::from_str(fm).ok()?;
    if !looks_like_recipe(&map) {
        return None;
    }
    Some(render(&map, body))
}

fn looks_like_recipe(map: &serde_yaml::Mapping) -> bool {
    if map.get("type").and_then(|v| v.as_str()) == Some("recipe") {
        return true;
    }
    map.get("tags")
        .and_then(|v| v.as_sequence())
        .is_some_and(|seq| seq.iter().any(|v| v.as_str() == Some("recipe")))
}

fn render(map: &serde_yaml::Mapping, body: &str) -> String {
    let mut out = String::new();

    push_meta(&mut out, map, "name", "title");
    push_meta(&mut out, map, "description", "description");
    push_meta(&mut out, map, "course", "course");
    push_meta(&mut out, map, "cuisine", "cuisine");
    if let Some(n) = map.get("servings").and_then(serde_yaml::Value::as_u64) {
        out.push_str(&format!(">> servings: {n}\n"));
    }
    if let Some(n) = map.get("prepMinutes").and_then(serde_yaml::Value::as_u64) {
        out.push_str(&format!(">> prep time: {n} min\n"));
    }
    if let Some(n) = map.get("cookMinutes").and_then(serde_yaml::Value::as_u64) {
        out.push_str(&format!(">> cook time: {n} min\n"));
    }
    if let Some(seq) = map.get("tags").and_then(|v| v.as_sequence()) {
        let tags: Vec<&str> = seq
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|t| *t != "recipe")
            .collect();
        if !tags.is_empty() {
            out.push_str(&format!(">> tags: {}\n", tags.join(", ")));
        }
    }
    push_meta(&mut out, map, "source", "source");

    let ingredients = parse_ingredients(map);
    let steps = parse_steps(map, body);

    // Render steps, rewriting ingredient names inline.
    let mut placed = vec![false; ingredients.len()];
    if !steps.is_empty() {
        out.push('\n');
        for step in &steps {
            out.push_str(&rewrite_step(step, &ingredients, &mut placed));
            out.push('\n');
        }
    }
    // Ingredients not mentioned in any step → one trailing
    // step so cooklang still parses them.
    let trailing: Vec<String> = ingredients
        .iter()
        .enumerate()
        .filter_map(|(i, ing)| {
            if placed[i] {
                None
            } else {
                Some(ing.to_cooklang())
            }
        })
        .collect();
    if !trailing.is_empty() {
        out.push('\n');
        out.push_str("Also: ");
        out.push_str(&trailing.join(", "));
        out.push_str(".\n");
    }
    out
}

fn push_meta(out: &mut String, map: &serde_yaml::Mapping, key: &str, meta_key: &str) {
    if let Some(v) = map.get(key).and_then(|v| v.as_str()) {
        out.push_str(&format!(">> {meta_key}: {v}\n"));
    }
}

#[derive(Debug, Clone)]
struct Ing {
    name: String,
    qty: Option<f64>,
    unit: String,
}

impl Ing {
    fn to_cooklang(&self) -> String {
        let inner = match (self.qty, self.unit.as_str()) {
            (Some(q), "") => format!("{{{q}}}"),
            (Some(q), u) => format!("{{{q}%{u}}}"),
            (None, "") => "{}".into(),
            (None, u) => format!("{{%{u}}}"),
        };
        // Ingredient names containing spaces need {} after
        // the multi-word name in cooklang. We always emit
        // the {} form to keep the parser happy.
        format!("@{}{}", self.name, inner)
    }
}

fn parse_ingredients(map: &serde_yaml::Mapping) -> Vec<Ing> {
    let Some(seq) = map.get("ingredients").and_then(|v| v.as_sequence()) else {
        return Vec::new();
    };
    seq.iter()
        .filter_map(|row| {
            if let Some(s) = row.as_str() {
                return Some(Ing {
                    name: s.to_string(),
                    qty: None,
                    unit: String::new(),
                });
            }
            let m = row.as_mapping()?;
            let name = m.get("name").and_then(|v| v.as_str())?.to_string();
            let qty = m.get("qty").and_then(serde_yaml::Value::as_f64);
            let unit = m
                .get("unit")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            Some(Ing { name, qty, unit })
        })
        .collect()
}

fn parse_steps(map: &serde_yaml::Mapping, body: &str) -> Vec<String> {
    let from_yaml: Vec<String> = map
        .get("steps")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(std::string::ToString::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !from_yaml.is_empty() {
        return from_yaml;
    }
    // Fallback: split body markdown into paragraphs.
    body.split("\n\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(std::string::ToString::to_string)
        .collect()
}

fn rewrite_step(step: &str, ingredients: &[Ing], placed: &mut [bool]) -> String {
    let mut out = step.to_string();
    for (i, ing) in ingredients.iter().enumerate() {
        // Skip already-placed ingredients so duplicate names
        // in later steps still resolve as plain text refs.
        if placed[i] {
            continue;
        }
        if let Some(pos) = case_insensitive_find(&out, &ing.name) {
            // Don't rewrite if it's already inside an
            // existing wikilink or cooklang token.
            if is_safe_position(&out, pos, &ing.name) {
                let replacement = ing.to_cooklang();
                let end = pos + ing.name.len();
                out.replace_range(pos..end, &replacement);
                placed[i] = true;
            }
        }
    }
    out
}

fn case_insensitive_find(haystack: &str, needle: &str) -> Option<usize> {
    let lower = haystack.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    lower.find(&needle_lower)
}

fn is_safe_position(s: &str, pos: usize, _needle: &str) -> bool {
    // Avoid rewriting inside `[[wikilink]]` or right after
    // `@` (already a cooklang token).
    let before = s.get(..pos).unwrap_or("");
    if before.ends_with("[[") || before.ends_with('@') {
        return false;
    }
    true
}

#[allow(dead_code)]
fn _silence_unused_path(_: &Path) {}
