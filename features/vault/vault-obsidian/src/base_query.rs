//! Bridge from vault pages to the Bases query engine in
//! `vault_live::bases`. Mirrors Obsidian's `base:query` CLI:
//! given a `.base` file and the page set, run each `ViewSpec` and
//! return the executed view (filtered, sorted, grouped, limited).
//!
//! Two sources of pages: a filesystem [`Vault`] snapshot
//! ([`query_view`] / [`query_all_views`]), or `(path, markdown)` pairs a
//! caller already holds — the MCP lane reads them through `VaultSync`
//! ([`rows_from_markdown`] + [`run_base`]). Either way every page
//! becomes a [`BaseRow`] with its frontmatter, tags (frontmatter ∪
//! inline `#tag`) and outgoing wikilinks, so `file.links`,
//! `file.hasLink()`, `exists()` and link-follow (`note.song.key`)
//! work the same.
//!
//! This module only does the wiring; expression evaluation (formulas,
//! link functions) is `vault_live::bases::BaseEval`. Unknown functions
//! evaluate to `null`, so an unsupported predicate shows as an empty
//! view rather than a panic.
//!
//! No CRDT, no Loro, no Dioxus — native-only, same as the rest of
//! this crate.

use uuid::Uuid;
use vault_live::bases::{self, BaseRow, ExecutedView, ParsedBase, ViewTable};

use crate::obsidian_parse::{ParsedPage, parse_page};
use crate::vault::{Vault, VaultPage};

#[derive(thiserror::Error, Debug)]
pub enum QueryError {
    #[error("base not found: {0}")]
    BaseNotFound(String),
    #[error("base parse error: {0}")]
    BaseParse(String),
    #[error("view not found: {0}")]
    ViewNotFound(String),
}

/// Convert every page in the vault into a `BaseRow` once. Callers
/// can reuse the result across multiple views in the same `.base`
/// file (or across bases) without re-serializing frontmatter.
pub fn vault_to_rows(vault: &Vault) -> Vec<BaseRow> {
    vault.pages.iter().map(page_to_row).collect()
}

/// Rows from `(vault-relative path, markdown)` pairs — for callers that
/// hold note bytes rather than a filesystem vault (the MCP lane reads
/// them through `VaultSync`). Non-`.md` paths are skipped.
pub fn rows_from_markdown<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<BaseRow> {
    files
        .into_iter()
        .filter(|(path, _)| path.ends_with(".md"))
        .map(|(path, raw)| {
            let (folder, basename) = split_rel(path);
            row_of(path, &basename, &folder, &parse_page(raw))
        })
        .collect()
}

/// Run `view` (case-insensitive name), or every view when `None`, and
/// project each into a [`ViewTable`]: visible columns plus every
/// formula's value per row.
///
/// # Errors
///
/// [`QueryError::ViewNotFound`] when `view` names no view in `base`.
pub fn run_base(
    base: &ParsedBase,
    rows: &[BaseRow],
    view: Option<&str>,
) -> Result<Vec<ViewTable>, QueryError> {
    match view {
        Some(name) => {
            let v = base
                .views
                .iter()
                .find(|v| v.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| QueryError::ViewNotFound(name.to_string()))?;
            Ok(vec![bases::view_table(base, v, rows)])
        }
        None => Ok(base
            .views
            .iter()
            .map(|v| bases::view_table(base, v, rows))
            .collect()),
    }
}

/// Look up a base by vault-relative path. Returns `None` if no
/// `.base` file at that path exists in the snapshot; otherwise
/// returns the parsed AST or, if parsing failed, the parse-error
/// string captured at load time.
#[must_use]
pub fn find_base<'a>(vault: &'a Vault, rel_path: &str) -> Option<Result<&'a ParsedBase, &'a str>> {
    vault
        .bases
        .iter()
        .find(|b| b.rel_path == rel_path)
        .map(|b| b.parsed.as_ref().map_err(String::as_str))
}

/// Run all views in a base over the vault's pages. Returns
/// `(view_name, ExecutedView)` pairs in declaration order.
pub fn query_all_views(
    vault: &Vault,
    base_rel_path: &str,
) -> Result<Vec<(String, ExecutedView)>, QueryError> {
    let base = resolve_base(vault, base_rel_path)?;
    let rows = vault_to_rows(vault);
    Ok(base
        .views
        .iter()
        .map(|v| {
            let ev = bases::execute_view(base, v, rows.clone());
            (v.name.clone(), ev)
        })
        .collect())
}

/// Run one named view. View-name matching is case-insensitive,
/// matching the CLI's tolerant behavior.
pub fn query_view(
    vault: &Vault,
    base_rel_path: &str,
    view_name: &str,
) -> Result<ExecutedView, QueryError> {
    let base = resolve_base(vault, base_rel_path)?;
    let view = base
        .views
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case(view_name))
        .ok_or_else(|| QueryError::ViewNotFound(view_name.to_string()))?;
    let rows = vault_to_rows(vault);
    Ok(bases::execute_view(base, view, rows))
}

fn resolve_base<'a>(vault: &'a Vault, rel_path: &str) -> Result<&'a ParsedBase, QueryError> {
    match find_base(vault, rel_path) {
        None => Err(QueryError::BaseNotFound(rel_path.to_string())),
        Some(Err(msg)) => Err(QueryError::BaseParse(msg.to_string())),
        Some(Ok(parsed)) => Ok(parsed),
    }
}

fn page_to_row(page: &VaultPage) -> BaseRow {
    row_of(&page.rel_path, &page.basename, &page.folder, &page.parsed)
}

/// `Music/Charts.md` → (`Music`, `Charts`).
fn split_rel(rel: &str) -> (String, String) {
    let (folder, file) = rel.rsplit_once('/').unwrap_or(("", rel));
    let basename = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    (folder.to_string(), basename.to_string())
}

fn row_of(rel_path: &str, basename: &str, folder: &str, parsed: &ParsedPage) -> BaseRow {
    // Preserve frontmatter key order. `serde_json::Map` iterates in
    // insertion order when the `preserve_order` feature is on; even
    // without it, the executor uses `IndexMap` internally — but to
    // be defensive we serialize manually so the key order in the
    // JSON string we hand to `BaseRow::from_parts_full` matches the
    // parsed frontmatter sequence.
    let mut obj = serde_json::Map::with_capacity(parsed.frontmatter.len());
    for entry in &parsed.frontmatter {
        // Last write wins on duplicate keys, matching how Obsidian
        // collapses duplicate frontmatter keys.
        obj.insert(entry.key.clone(), entry.value.clone());
    }
    let fm_json = serde_json::Value::Object(obj).to_string();

    // Inline `#tag` markers get merged into `file.tags` so
    // `file.tags.contains("chart")` filters match the way Obsidian's
    // bases engine does (frontmatter ∪ inline body tags). Body
    // `[[wikilinks]]` join the frontmatter links behind `file.links`.
    let mut inline_tags: Vec<String> = Vec::new();
    let mut body_links: Vec<&str> = Vec::new();
    for block in &parsed.blocks {
        for r in &block.refs {
            match r {
                vault_live::refs::Ref::Tag(t) => {
                    let joined = t.path.join("/");
                    if !joined.is_empty() && !inline_tags.iter().any(|x| x == &joined) {
                        inline_tags.push(joined);
                    }
                }
                vault_live::refs::Ref::Link(l) => body_links.push(&l.target_linkpath),
                _ => {}
            }
        }
    }

    let ext = rel_path
        .rsplit_once('.')
        .map(|(_, e)| e.to_string())
        .unwrap_or_default();

    BaseRow::from_parts_full(
        Uuid::new_v4(),
        basename,
        rel_path,
        folder,
        ext,
        &fm_json,
        &inline_tags,
    )
    .with_links(body_links)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn touch(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn synthetic_vault_chart_filter() {
        let dir = tempfile::tempdir().unwrap();
        // Three pages — two with `kind: chart`, one with `kind: demo`.
        // The current `execute_view` evaluator only does scalar
        // comparisons on note properties (see `cmp_values`), so we
        // express the filter as an equality on a scalar key rather
        // than a `tags.contains` array probe. The bridge being
        // tested here is the same either way.
        touch(
            &dir.path().join("Alpha.md"),
            "---\nkind: chart\ntags: [chart]\n---\nbody\n",
        );
        touch(
            &dir.path().join("Beta.md"),
            "---\nkind: chart\ntags: [chart]\n---\nbody\n",
        );
        touch(
            &dir.path().join("Gamma.md"),
            "---\nkind: demo\ntags: [demo]\n---\nbody\n",
        );
        touch(
            &dir.path().join("Charts.base"),
            "filters:\n  and:\n    - 'note.kind == \"chart\"'\nviews:\n  - type: list\n    name: All Charts\n",
        );

        let v = Vault::open(dir.path()).expect("open");
        let ev = query_view(&v, "Charts.base", "all charts").expect("query");
        assert_eq!(ev.groups.len(), 1, "no group_by => single bucket");
        let (label, rows) = &ev.groups[0];
        assert_eq!(label, "");
        let names: std::collections::HashSet<_> = rows.iter().map(|r| r.basename.clone()).collect();
        assert!(names.contains("Alpha"), "got {names:?}");
        assert!(names.contains("Beta"), "got {names:?}");
        assert!(!names.contains("Gamma"), "got {names:?}");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn query_all_views_returns_each_named_view() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a.md"), "---\ntags: [chart]\n---\n");
        touch(
            &dir.path().join("Multi.base"),
            "views:\n  - type: list\n    name: First\n  - type: table\n    name: Second\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let results = query_all_views(&v, "Multi.base").unwrap();
        let names: Vec<_> = results.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["First", "Second"]);
    }

    /// The gigs recipe over in-memory notes: a body wikilink, a
    /// frontmatter link followed into the song, formulas per row.
    #[test]
    fn markdown_rows_carry_links_and_run_with_formula_values() {
        let files = [
            ("Songs/Song A.md", "---\nkey: G\n---\n# Song A\n"),
            ("Songs/Song B.md", "---\nkey: D\n---\n"),
            (
                "Gigs/Friday.md",
                "---\nsong: \"[[Song A]]\"\nfee: 400\n---\nLoad-in at [[The Hall|the hall]]. #paid\n",
            ),
            ("Gigs/Saturday.md", "---\nsong: \"[[Song B]]\"\nfee: 250\n---\n"),
            ("Gigs.base", "views: []\n"),
        ];
        let rows = rows_from_markdown(files.iter().map(|(p, s)| (*p, *s)));
        assert_eq!(rows.len(), 4, "the .base file is not a row");
        let friday = rows.iter().find(|r| r.basename == "Friday").unwrap();
        assert_eq!(friday.folder, "Gigs");
        assert_eq!(friday.path, "Gigs/Friday.md");
        assert!(friday.links.iter().any(|l| l == "Song A"), "{:?}", friday.links);
        assert!(friday.links.iter().any(|l| l == "The Hall"), "{:?}", friday.links);
        assert!(friday.tags.iter().any(|t| t == "paid"));

        let base = vault_live::bases::parse(
            r#"
filters:
  and:
    - file.inFolder("Gigs")
formulas:
  song_key: "note.song.key"
  cut: "fee * 0.1"
views:
  - type: table
    name: At the hall
    filters:
      and:
        - file.hasLink("The Hall")
    order: [file.name, formula.song_key]
  - type: table
    name: In D
    filters:
      and:
        - 'song.key == "D"'
"#,
        )
        .unwrap();
        let tables = run_base(&base, &rows, None).unwrap();
        assert_eq!(tables.len(), 2);
        let hall = &tables[0].groups[0].rows;
        assert_eq!(hall.len(), 1);
        assert_eq!(hall[0].name, "Friday");
        assert_eq!(hall[0].values["formula.song_key"], serde_json::json!("G"));
        assert_eq!(hall[0].formulas["cut"], serde_json::json!(40.0));
        let in_d = run_base(&base, &rows, Some("in d")).unwrap();
        assert_eq!(in_d.len(), 1);
        assert_eq!(in_d[0].groups[0].rows[0].name, "Saturday");
        assert!(matches!(
            run_base(&base, &rows, Some("Nope")),
            Err(QueryError::ViewNotFound(_))
        ));
    }

    #[test]
    fn missing_base_is_error() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("only.md"), "x\n");
        let v = Vault::open(dir.path()).unwrap();
        assert!(matches!(
            query_view(&v, "Nope.base", "x"),
            Err(QueryError::BaseNotFound(_))
        ));
    }

    #[test]
    fn missing_view_is_error() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("B.base"),
            "views:\n  - type: list\n    name: Only\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        assert!(matches!(
            query_view(&v, "B.base", "Other"),
            Err(QueryError::ViewNotFound(_))
        ));
    }

    /// Smoke test against a real Observatory-style vault. Gated on
    /// `TASK_OBS_VAULT` env var — set it to a vault root to enable.
    #[test]
    #[ignore = "requires TASK_OBS_VAULT env var pointing at an Observatory vault"]
    fn smoke_observatory_charts_base() {
        let Ok(root) = std::env::var("TASK_OBS_VAULT") else {
            eprintln!("TASK_OBS_VAULT not set; skipping");
            return;
        };
        let v = Vault::open(Path::new(&root)).expect("open vault");
        let results = match query_all_views(&v, "Charts.base") {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Charts.base unavailable: {e}");
                return;
            }
        };
        for (name, ev) in results {
            let total: usize = ev.groups.iter().map(|(_, r)| r.len()).sum();
            eprintln!("{name}: {total} rows ({} groups)", ev.groups.len());
        }
    }
}
