//! Frontmatter property aggregation. Mirrors Obsidian's `properties`
//! CLI surface.

use std::collections::BTreeSet;

use crate::vault::{Vault, VaultPage};

/// Obsidian's reserved frontmatter keys that are case-folded to a
/// canonical lowercase form regardless of how the YAML spells them.
const RESERVED_LOWERCASE_KEYS: &[&str] = &["aliases", "tags", "cssclasses"];

/// Normalize a frontmatter key the way Obsidian does for the
/// properties pane: reserved keys (`aliases`, `tags`, `cssclasses`)
/// are forced to lowercase; everything else is left untouched.
fn normalize_key(key: &str) -> String {
    let lower = key.to_ascii_lowercase();
    if RESERVED_LOWERCASE_KEYS.iter().any(|k| *k == lower) {
        lower
    } else {
        key.to_string()
    }
}

/// Load extra property keys declared in `.obsidian/types.json`. These
/// are plugin-injected property declarations (`LanguageTool`'s `lt-*`,
/// Tasks plugin's `TQ_*`, etc.) that Obsidian exposes via the
/// properties pane even on vaults where no page sets them.
fn read_types_json_keys(vault: &Vault) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let path = vault.root.join(".obsidian").join("types.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return out;
    };
    let Ok(json): Result<serde_json::Value, _> = serde_json::from_slice(&bytes) else {
        return out;
    };
    if let Some(types) = json.get("types").and_then(|v| v.as_object()) {
        for k in types.keys() {
            out.insert(normalize_key(k));
        }
    }
    out
}

/// Alphabetized list of distinct frontmatter keys used anywhere in
/// the vault. Includes:
/// - top-level frontmatter keys (case-folded for reserved names)
/// - plugin-declared keys from `.obsidian/types.json` (the type bag
///   Obsidian's properties pane surfaces even when no page sets them)
///
/// Obsidian's `properties` CLI does NOT expand nested objects or
/// list-of-objects into dotted paths — only top-level keys are
/// listed, so we mirror that.
#[must_use]
pub fn list_property_keys(vault: &Vault) -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for p in &vault.pages {
        for e in &p.parsed.frontmatter {
            set.insert(normalize_key(&e.key));
        }
    }
    set.extend(read_types_json_keys(vault));
    set.into_iter().collect()
}

/// Sample values seen for `key`. Caps at `max_samples`; preserves
/// document order (first occurrence wins). Duplicate values are
/// suppressed by JSON-stringified identity so e.g. two pages with
/// `status: active` only contribute one sample.
#[must_use]
pub fn property_values(vault: &Vault, key: &str, max_samples: usize) -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    if max_samples == 0 {
        return out;
    }
    for p in &vault.pages {
        for e in &p.parsed.frontmatter {
            if e.key != key {
                continue;
            }
            let sig = serde_json::to_string(&e.value).unwrap_or_default();
            if seen.insert(sig) {
                out.push(e.value.clone());
                if out.len() >= max_samples {
                    return out;
                }
            }
        }
    }
    out
}

/// Read one property from one page.
#[must_use]
pub fn read_property<'a>(page: &'a VaultPage, key: &str) -> Option<&'a serde_json::Value> {
    page.parsed
        .frontmatter
        .iter()
        .find(|e| e.key == key)
        .map(|e| &e.value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Vault;
    use std::fs;
    use std::path::Path;

    fn touch(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn list_keys_is_alphabetized_and_distinct() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("a.md"),
            "---\nstatus: active\ntags: [x]\n---\n",
        );
        touch(
            &dir.path().join("b.md"),
            "---\nstatus: done\nauthor: c\n---\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let keys = list_property_keys(&v);
        assert_eq!(
            keys,
            vec!["author".to_string(), "status".into(), "tags".into()]
        );
    }

    #[test]
    fn property_values_dedups_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a.md"), "---\nstatus: active\n---\n");
        touch(&dir.path().join("b.md"), "---\nstatus: active\n---\n");
        touch(&dir.path().join("c.md"), "---\nstatus: done\n---\n");
        touch(&dir.path().join("d.md"), "---\nstatus: pending\n---\n");
        let v = Vault::open(dir.path()).unwrap();
        let all = property_values(&v, "status", 10);
        let as_strs: BTreeSet<_> = all
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        assert_eq!(as_strs.len(), 3);
        assert!(as_strs.contains("active"));
        assert!(as_strs.contains("done"));
        assert!(as_strs.contains("pending"));

        let capped = property_values(&v, "status", 2);
        assert_eq!(capped.len(), 2);
    }

    #[test]
    fn nested_objects_are_not_expanded() {
        // Obsidian's properties CLI only lists top-level keys; the
        // properties pane treats nested-object subkeys as values, not
        // separately enumerable property names.
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("a.md"),
            "---\nrecurrence:\n  freq: WEEKLY\n  interval: 2\n---\n",
        );
        touch(
            &dir.path().join("b.md"),
            "---\ntimeEntries:\n  - startTime: 09:00\n    endTime: 10:00\n---\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let keys = list_property_keys(&v);
        assert!(keys.contains(&"recurrence".to_string()));
        assert!(keys.contains(&"timeEntries".to_string()));
        assert!(!keys.contains(&"recurrence.freq".to_string()));
        assert!(!keys.contains(&"timeEntries.startTime".to_string()));
    }

    #[test]
    fn reserved_keys_are_case_folded() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("a.md"),
            "---\nAliases: [foo]\nTags: [x]\n---\n",
        );
        touch(&dir.path().join("b.md"), "---\naliases: [bar]\n---\n");
        let v = Vault::open(dir.path()).unwrap();
        let keys = list_property_keys(&v);
        assert!(keys.contains(&"aliases".to_string()));
        assert!(keys.contains(&"tags".to_string()));
        assert!(!keys.contains(&"Aliases".to_string()));
        assert!(!keys.contains(&"Tags".to_string()));
    }

    #[test]
    fn types_json_keys_are_merged_in() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a.md"), "---\nstatus: active\n---\n");
        touch(
            &dir.path().join(".obsidian/types.json"),
            r#"{"types":{"lt-language":"text","TQ_explain":"checkbox","Aliases":"aliases"}}"#,
        );
        let v = Vault::open(dir.path()).unwrap();
        let keys = list_property_keys(&v);
        assert!(keys.contains(&"status".to_string()));
        assert!(keys.contains(&"lt-language".to_string()));
        assert!(keys.contains(&"TQ_explain".to_string()));
        // Reserved key in types.json is also normalized.
        assert!(keys.contains(&"aliases".to_string()));
        assert!(!keys.contains(&"Aliases".to_string()));
    }

    #[test]
    fn read_property_returns_value() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("p.md"),
            "---\nstatus: active\ncount: 3\n---\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let p = v.page("p.md").unwrap();
        assert_eq!(
            read_property(p, "status"),
            Some(&serde_json::json!("active"))
        );
        assert_eq!(
            read_property(p, "count").and_then(serde_json::Value::as_i64),
            Some(3)
        );
        assert!(read_property(p, "missing").is_none());
    }
}
