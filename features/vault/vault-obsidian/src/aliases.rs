//! Frontmatter alias aggregation. Mirrors Obsidian's `aliases` CLI.

use crate::vault::{Vault, VaultPage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasEntry<'a> {
    /// Vault-relative page path.
    pub page: &'a str,
    pub alias: String,
}

/// All `aliases:` (and singular `alias:`) frontmatter values across
/// the vault, flattened to `(page, alias)` pairs. Sorted by alias
/// then by page for deterministic output.
#[must_use]
pub fn list_aliases(vault: &Vault) -> Vec<AliasEntry<'_>> {
    let mut out = Vec::new();
    for p in &vault.pages {
        collect_page_aliases(p, &mut out);
    }
    out.sort_by(|a, b| a.alias.cmp(&b.alias).then(a.page.cmp(b.page)));
    out
}

/// Total alias count across the vault.
#[must_use]
pub fn total_aliases(vault: &Vault) -> usize {
    vault
        .pages
        .iter()
        .map(|p| {
            let mut v = Vec::new();
            collect_page_aliases(p, &mut v);
            v.len()
        })
        .sum()
}

fn collect_page_aliases<'a>(page: &'a VaultPage, out: &mut Vec<AliasEntry<'a>>) {
    for e in &page.parsed.frontmatter {
        if e.key != "aliases" && e.key != "alias" {
            continue;
        }
        push_value(&page.rel_path, &e.value, out);
    }
}

fn push_value<'a>(rel: &'a str, v: &serde_json::Value, out: &mut Vec<AliasEntry<'a>>) {
    match v {
        serde_json::Value::String(s) => {
            // Comma-separated is the legacy form Obsidian accepts.
            if s.contains(',') {
                for piece in s.split(',') {
                    let t = piece.trim();
                    if !t.is_empty() {
                        out.push(AliasEntry {
                            page: rel,
                            alias: t.to_string(),
                        });
                    }
                }
            } else {
                let t = s.trim();
                if !t.is_empty() {
                    out.push(AliasEntry {
                        page: rel,
                        alias: t.to_string(),
                    });
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                match item {
                    serde_json::Value::String(s) => {
                        let t = s.trim();
                        if !t.is_empty() {
                            out.push(AliasEntry {
                                page: rel,
                                alias: t.to_string(),
                            });
                        }
                    }
                    other => {
                        if let Some(s) = other.as_str() {
                            let t = s.trim();
                            if !t.is_empty() {
                                out.push(AliasEntry {
                                    page: rel,
                                    alias: t.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
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
    fn list_aliases_handles_sequence_string_and_csv() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("seq.md"),
            "---\naliases:\n  - Zeta\n  - Alpha\n---\n",
        );
        touch(&dir.path().join("one.md"), "---\naliases: Mono\n---\n");
        touch(
            &dir.path().join("csv.md"),
            "---\naliases: \"foo, bar\"\n---\n",
        );
        touch(&dir.path().join("sing.md"), "---\nalias: Solo\n---\n");
        touch(&dir.path().join("none.md"), "---\ntitle: nope\n---\n");

        let v = Vault::open(dir.path()).unwrap();
        let entries = list_aliases(&v);
        let aliases: Vec<&str> = entries.iter().map(|e| e.alias.as_str()).collect();
        // sorted alphabetically
        assert_eq!(aliases, vec!["Alpha", "Mono", "Solo", "Zeta", "bar", "foo"]);

        let alpha = entries.iter().find(|e| e.alias == "Alpha").unwrap();
        assert_eq!(alpha.page, "seq.md");
        let solo = entries.iter().find(|e| e.alias == "Solo").unwrap();
        assert_eq!(solo.page, "sing.md");
    }

    #[test]
    fn total_aliases_counts_all_forms() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("a.md"),
            "---\naliases:\n  - one\n  - two\n---\n",
        );
        touch(&dir.path().join("b.md"), "---\naliases: \"x, y, z\"\n---\n");
        touch(&dir.path().join("c.md"), "---\nalias: lone\n---\n");
        let v = Vault::open(dir.path()).unwrap();
        assert_eq!(total_aliases(&v), 6);
    }

    #[test]
    fn empty_vault_has_zero_aliases() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("plain.md"), "no fm here\n");
        let v = Vault::open(dir.path()).unwrap();
        assert_eq!(total_aliases(&v), 0);
        assert!(list_aliases(&v).is_empty());
    }
}
