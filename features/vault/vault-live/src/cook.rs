//! `.cook` recipes as base rows.
//!
//! Recipes are cooklang files, not markdown pages, and they live under
//! the *wiki* root rather than the vault root — so the Bases engine,
//! which walks `vault.pages`, has never been able to see them. That
//! makes the obvious thing ("a `Cookbook.base` listing every recipe")
//! impossible, even though the engine is otherwise entity-agnostic and
//! would happily filter recipes if only it were handed the rows.
//!
//! This module closes that gap with the smallest possible bridge: a
//! `.cook` file's metadata block, projected into the same frontmatter
//! JSON shape [`crate::bases::BaseRow`] already consumes. Nothing here
//! parses cooklang proper — steps, ingredients, and timers stay the
//! `cookbook` crate's job, and depending on it from here would be a
//! dependency cycle anyway (`cookbook` is built on `vault`). All we
//! need for a row is the header.
//!
//! Two dialects are in the wild and both are accepted: the classic
//! `>> key: value` block, and the YAML `---` frontmatter that
//! `task recipe import` writes. Every row is stamped `type: recipe`
//! unless the file says otherwise, so a base can filter on it the same
//! way it filters `type: meal`.

use std::path::Path;

use serde_json::{Map, Value};

/// A recipe's metadata as a frontmatter JSON object string, ready for
/// [`crate::bases::BaseRow::from_parts_full`]. Always an object —
/// a recipe with no metadata at all still yields `{"type":"recipe"}`
/// so it shows up in a `type == recipe` view rather than vanishing.
#[must_use]
pub fn cook_frontmatter_json(raw: &str) -> String {
    let mut map = yaml_frontmatter(raw).unwrap_or_else(|| meta_block(raw));

    // The discriminator bases filter on. Explicit wins, so a recipe can
    // opt into a narrower type of its own.
    map.entry("type".to_string())
        .or_insert_with(|| Value::String("recipe".into()));

    normalize_tags(&mut map);
    Value::Object(map).to_string()
}

/// YAML `---` frontmatter, when the file leads with it.
fn yaml_frontmatter(raw: &str) -> Option<Map<String, Value>> {
    let rest = raw.strip_prefix("---")?;
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))?;
    let end = rest.find("\n---")?;
    match serde_yaml::from_str::<Value>(&rest[..end]) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

/// The cooklang `>> key: value` block. These lines conventionally lead
/// the file, but cooklang permits them anywhere, so the whole source is
/// scanned rather than just the head.
fn meta_block(raw: &str) -> Map<String, Value> {
    let mut map = Map::new();
    for line in raw.lines() {
        let Some(rest) = line.trim_start().strip_prefix(">>") else {
            continue;
        };
        let Some((key, value)) = rest.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        map.insert(key.to_string(), scalar(value.trim()));
    }
    map
}

/// A `>> key: value` value as a typed JSON scalar. The cooklang block
/// is untyped text, but the Bases engine compares and sorts on real
/// types — leaving everything a string would sort `servings: 10` before
/// `servings: 2`. Coerce the way YAML would, so `.cook` rows behave
/// like the frontmatter rows beside them.
fn scalar(s: &str) -> Value {
    match s {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(i) = s.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(f) = s.parse::<f64>()
        && f.is_finite()
    {
        return Value::from(f);
    }
    Value::String(s.to_string())
}

/// Cooklang writes tags as one comma-separated string (`>> tags: quick,
/// pasta`); bases expect a list, and `tags_from_frontmatter` only reads
/// arrays. Split so tag filters work on recipes like they do on notes.
fn normalize_tags(map: &mut Map<String, Value>) {
    let Some(Value::String(s)) = map.get("tags") else {
        return;
    };
    let list: Vec<Value> = s
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| Value::String(t.to_string()))
        .collect();
    map.insert("tags".to_string(), Value::Array(list));
}

/// One `.cook` file found under a recipe root.
pub struct CookFile {
    /// Root-relative, forward-slash separated — e.g.
    /// `Knowledge/Cookbook/oatmeal.cook`. This is what a base row's
    /// `path` becomes, and what cook mode is opened with.
    pub rel_path: String,
    /// Parent folder, root-relative (`""` at the root).
    pub folder: String,
    /// Filename without the `.cook` extension.
    pub basename: String,
    /// Raw file contents.
    pub raw: String,
}

/// Every `.cook` file under `root`, recursively. Unreadable files and
/// directories are skipped rather than failing the whole scan — one bad
/// recipe shouldn't blank a base view.
#[must_use]
pub fn scan_cook_files(root: &Path) -> Vec<CookFile> {
    let mut out = Vec::new();
    collect(root, root, &mut out);
    out
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<CookFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Same skip rule the vault walker uses: dotted dirs are tooling
        // state (`.git`, `.obsidian`), never content.
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("cook") {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel_path = rel.to_string_lossy().replace('\\', "/");
        let folder = rel_path
            .rsplit_once('/')
            .map_or(String::new(), |(f, _)| f.to_string());
        let basename = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        out.push(CookFile {
            rel_path,
            folder,
            basename,
            raw,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(raw: &str) -> Map<String, Value> {
        match serde_json::from_str(&cook_frontmatter_json(raw)) {
            Ok(Value::Object(m)) => m,
            _ => panic!("expected a JSON object"),
        }
    }

    #[test]
    fn reads_the_cooklang_metadata_block() {
        let m = fm(">> title: Overnight Oats\n>> servings: 2\n\nMix @oats{50%g}.");
        assert_eq!(m["title"], Value::String("Overnight Oats".into()));
        assert_eq!(m["servings"], Value::from(2), "numbers sort as numbers");
    }

    #[test]
    fn scalars_coerce_like_yaml_would() {
        let m = fm(">> servings: 4\n>> rating: 4.5\n>> favourite: true\n>> course: main");
        assert_eq!(m["servings"], Value::from(4));
        assert_eq!(m["rating"], Value::from(4.5));
        assert_eq!(m["favourite"], Value::Bool(true));
        assert_eq!(m["course"], Value::String("main".into()));
    }

    #[test]
    fn stamps_type_recipe_so_bases_can_filter() {
        assert_eq!(fm("Just one step.")["type"], Value::String("recipe".into()));
    }

    #[test]
    fn explicit_type_is_not_overwritten() {
        let m = fm(">> type: dessert\n\nBake it.");
        assert_eq!(m["type"], Value::String("dessert".into()));
    }

    #[test]
    fn reads_yaml_frontmatter_from_imports() {
        let m = fm("---\ntitle: Cookies\nimport: needs-review\n---\n\nBake @flour{200%g}.");
        assert_eq!(m["title"], Value::String("Cookies".into()));
        assert_eq!(m["import"], Value::String("needs-review".into()));
        assert_eq!(m["type"], Value::String("recipe".into()));
    }

    #[test]
    fn comma_separated_tags_become_a_list() {
        let m = fm(">> tags: quick, pasta\n\nBoil it.");
        assert_eq!(
            m["tags"],
            Value::Array(vec![
                Value::String("quick".into()),
                Value::String("pasta".into())
            ])
        );
    }

    #[test]
    fn scan_walks_nested_folders_and_skips_dotted_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("Cookbook/Shared")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("Cookbook/oatmeal.cook"), ">> title: Oatmeal").unwrap();
        std::fs::write(root.join("Cookbook/Shared/dough.cook"), ">> title: Dough").unwrap();
        std::fs::write(root.join("Cookbook/notes.md"), "not a recipe").unwrap();
        std::fs::write(root.join(".git/config.cook"), ">> title: Nope").unwrap();

        let mut found: Vec<String> = scan_cook_files(root)
            .into_iter()
            .map(|c| c.rel_path)
            .collect();
        found.sort();
        assert_eq!(
            found,
            vec![
                "Cookbook/Shared/dough.cook".to_string(),
                "Cookbook/oatmeal.cook".to_string()
            ]
        );
    }

    #[test]
    fn scan_reports_folder_and_basename() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("Cookbook")).unwrap();
        std::fs::write(tmp.path().join("Cookbook/garlic-pasta.cook"), ">> title: X").unwrap();
        let found = scan_cook_files(tmp.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].folder, "Cookbook");
        assert_eq!(found[0].basename, "garlic-pasta");
    }
}
