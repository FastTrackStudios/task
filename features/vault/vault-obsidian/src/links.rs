//! Wikilink index over a `Vault` snapshot.
//!
//! Mirrors the queries Obsidian's CLI surfaces:
//! - `backlinks` — pages linking TO a given page
//! - `orphans`   — pages with zero incoming links
//! - `unresolved`— wikilinks that don't resolve to any page
//! - `links`     — outgoing wikilinks from a page
//!
//! Plus `deadends` — pages with zero outgoing links.
//!
//! Pure logic over the in-memory snapshot. Anchors (`#Heading`,
//! `#^block-id`) are stripped before resolution — `[[Page#H]]`
//! resolves to `Page`.

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

use crate::obsidian_parse::FrontmatterEntry;
use regex::Regex;
use vault_live::refs::Ref;

use crate::vault::Vault;

/// Built index. Borrows from the source `Vault` for the lifetime of
/// the index, so all returned `&str`s point straight at vault data.
pub struct LinkIndex<'a> {
    vault: &'a Vault,
    /// `rel_path` -> indices into `vault.pages` of pages linking here.
    incoming: HashMap<&'a str, BTreeSet<usize>>,
    /// `rel_path` -> outgoing edges, in source order, de-duped on
    /// `(resolved_or_raw, alias)`.
    outgoing: HashMap<&'a str, Vec<OutgoingLink<'a>>>,
    /// `rel_paths` of pages that have at least one outgoing edge
    /// somewhere — body refs OR frontmatter wikilinks OR any other
    /// edge-producing source we add later. Used by `deadends()` so
    /// the calculation isn't body-refs-only. Stored separately from
    /// `outgoing` because frontmatter links don't surface in the
    /// `outgoing()` listing (frontmatter strings are owned, not
    /// 'a-lifetimed).
    has_outgoing: BTreeSet<&'a str>,
    /// All unresolved (source, raw) pairs across the vault.
    unresolved: Vec<UnresolvedLink<'a>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutgoingLink<'a> {
    pub linkpath: &'a str,
    pub resolved: Option<&'a str>,
    pub alias: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnresolvedLink<'a> {
    pub source: &'a str,
    pub linkpath: &'a str,
}

impl<'a> LinkIndex<'a> {
    /// Build the index in one pass over all pages.
    #[must_use]
    pub fn build(vault: &'a Vault) -> Self {
        // Lookup structures.
        let by_path: HashMap<&str, usize> = vault
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.rel_path.as_str(), i))
            .collect();
        // basename -> list of (idx). Used for shortest-link resolution
        // (only resolves if exactly one match).
        let mut by_basename: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, p) in vault.pages.iter().enumerate() {
            by_basename.entry(p.basename.as_str()).or_default().push(i);
        }

        // Attachment lookups. Obsidian resolves `[[image.png]]`,
        // `[[doc.pdf]]`, `[[song.mp3]]`, etc. to vault attachments
        // (including canvas files). Keyed by full rel_path for path-y
        // links, and by filename-with-extension for shortest-link form.
        let mut by_att_path: HashMap<&str, &str> = vault
            .attachments
            .iter()
            .map(|a| (a.rel_path.as_str(), a.rel_path.as_str()))
            .collect();
        let mut by_att_basename: HashMap<&str, Vec<&str>> = HashMap::new();
        for a in &vault.attachments {
            let base = att_basename(a.rel_path.as_str());
            by_att_basename
                .entry(base)
                .or_default()
                .push(a.rel_path.as_str());
        }
        // `.base` files (Obsidian Bases) are addressable as link
        // targets too — `[[Projects.base]]` is a valid wikilink. Fold
        // them into the same "non-page" resolution maps as attachments.
        for b in &vault.bases {
            by_att_path.insert(b.rel_path.as_str(), b.rel_path.as_str());
            by_att_basename
                .entry(att_basename(b.rel_path.as_str()))
                .or_default()
                .push(b.rel_path.as_str());
        }

        let mut incoming: HashMap<&str, BTreeSet<usize>> = HashMap::new();
        let mut outgoing: HashMap<&str, Vec<OutgoingLink<'a>>> = HashMap::new();
        let mut has_outgoing: BTreeSet<&str> = BTreeSet::new();
        let mut unresolved: Vec<UnresolvedLink<'a>> = Vec::new();

        for (src_idx, page) in vault.pages.iter().enumerate() {
            let src_rel: &str = page.rel_path.as_str();
            let src_folder: &str = page.folder.as_str();
            let mut seen: BTreeSet<(Option<&str>, &str, Option<&str>)> = BTreeSet::new();
            let mut out_vec: Vec<OutgoingLink<'a>> = Vec::new();

            for block in &page.parsed.blocks {
                for r in &block.refs {
                    let (raw_linkpath, alias): (&str, Option<&str>) = match r {
                        Ref::Link(l) => (l.target_linkpath.as_str(), l.alias.as_deref()),
                        Ref::Embed(e) => (e.target_linkpath.as_str(), e.alias.as_deref()),
                        Ref::MarkdownLink(l) => (l.url.as_str(), l.alias.as_deref()),
                        _ => continue,
                    };
                    if is_external_url(raw_linkpath) {
                        continue;
                    }
                    let target_key = strip_anchor(raw_linkpath);
                    let resolved_indices =
                        resolve_all(target_key, src_folder, &by_path, &by_basename);
                    let primary_resolved: Option<&str> = match resolved_indices.first().copied() {
                        Some(i) => Some(vault.pages[i].rel_path.as_str()),
                        None => resolve_attachment(
                            target_key,
                            src_folder,
                            &by_att_path,
                            &by_att_basename,
                        ),
                    };

                    let key = (primary_resolved, raw_linkpath, alias);
                    if !seen.insert(key) {
                        continue;
                    }

                    out_vec.push(OutgoingLink {
                        linkpath: raw_linkpath,
                        resolved: primary_resolved,
                        alias,
                    });
                    has_outgoing.insert(src_rel);

                    if resolved_indices.is_empty() {
                        if primary_resolved.is_none() {
                            unresolved.push(UnresolvedLink {
                                source: src_rel,
                                linkpath: raw_linkpath,
                            });
                        }
                    } else {
                        // Credit ALL pages whose basename matches the
                        // wikilink with an incoming edge. Obsidian's
                        // graph treats ambiguous wikilinks as edges
                        // to every candidate (the resolver picks a
                        // primary for navigation, but the graph still
                        // shows the line). This is the difference
                        // between our previous "drop on ambiguity"
                        // semantics and Obsidian's "fan out" semantics.
                        for &tgt in &resolved_indices {
                            if tgt != src_idx {
                                incoming
                                    .entry(vault.pages[tgt].rel_path.as_str())
                                    .or_default()
                                    .insert(src_idx);
                            }
                        }
                    }
                }
            }

            // Frontmatter wikilinks: scan every frontmatter Value's
            // strings for `[[Page]]` shapes. Obsidian indexes these
            // as graph edges — TaskNotes-style fronts like
            // `projects: ["[[FastTrackStudio]]"]`, `parent: [[X]]`,
            // `blockedBy:` lists all become edges. Doesn't contribute
            // to the `outgoing` map (which is body-only by design)
            // but DOES count for `incoming` so backlinks/orphans
            // match Obsidian.
            let fm_links = extract_frontmatter_links(&page.parsed.frontmatter);
            if !fm_links.is_empty() {
                has_outgoing.insert(src_rel);
            }
            for raw_linkpath in fm_links {
                let target_key = strip_anchor(&raw_linkpath);
                for tgt in resolve_all(target_key, src_folder, &by_path, &by_basename) {
                    if tgt != src_idx {
                        incoming
                            .entry(vault.pages[tgt].rel_path.as_str())
                            .or_default()
                            .insert(src_idx);
                    }
                }
            }

            outgoing.insert(src_rel, out_vec);
        }

        // Bookmarks: each file-type entry in `.obsidian/bookmarks.json`
        // gets a synthetic incoming edge so user-pinned pages don't
        // show as orphans. Matches Obsidian's behavior.
        for path in read_bookmarked_paths(vault) {
            if let Some(&i) = by_path.get(path.as_str()) {
                // Reuse `usize::MAX` as a sentinel "synthetic" source
                // index — we never index into pages with it; the
                // important thing is the BTreeSet entry exists so
                // `orphans()` skips this page.
                incoming
                    .entry(vault.pages[i].rel_path.as_str())
                    .or_default()
                    .insert(usize::MAX);
            }
        }

        Self {
            vault,
            incoming,
            outgoing,
            has_outgoing,
            unresolved,
        }
    }

    /// Pages that link TO `rel_path`. Sorted by `rel_path`.
    #[must_use]
    pub fn backlinks(&self, rel_path: &str) -> Vec<&'a str> {
        let Some(set) = self.incoming.get(rel_path) else {
            return Vec::new();
        };
        let mut out: Vec<&str> = set
            .iter()
            // Skip the bookmark-sentinel; bookmarks are synthetic
            // edges that keep pages out of `orphans()` but aren't
            // real source pages to surface here.
            .filter(|&&i| i != usize::MAX)
            .map(|&i| self.vault.pages[i].rel_path.as_str())
            .collect();
        out.sort_unstable();
        out
    }

    /// Outgoing wikilinks from `rel_path`, de-duped in source order.
    #[must_use]
    pub fn outgoing(&self, rel_path: &str) -> Vec<OutgoingLink<'a>> {
        self.outgoing.get(rel_path).cloned().unwrap_or_default()
    }

    /// Pages with zero incoming links. Sorted.
    #[must_use]
    pub fn orphans(&self) -> Vec<&'a str> {
        let mut out: Vec<&str> = self
            .vault
            .pages
            .iter()
            .map(|p| p.rel_path.as_str())
            .filter(|rel| {
                self.incoming
                    .get(rel)
                    .is_none_or(|s: &BTreeSet<usize>| s.is_empty())
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// Wikilink targets that don't resolve to any page in the vault.
    #[must_use]
    pub fn unresolved(&self) -> Vec<UnresolvedLink<'a>> {
        self.unresolved.clone()
    }

    /// Pages with zero outgoing links. Counts body wikilinks,
    /// markdown links, and frontmatter wikilinks — anything that
    /// produces a graph edge.
    #[must_use]
    pub fn deadends(&self) -> Vec<&'a str> {
        let mut out: Vec<&str> = self
            .vault
            .pages
            .iter()
            .map(|p| p.rel_path.as_str())
            .filter(|rel| !self.has_outgoing.contains(rel))
            .collect();
        out.sort_unstable();
        out
    }
}

/// Strip `#Heading` / `#^block-id` anchors from a linkpath. Anything
/// after the first `#` is treated as an anchor.
fn strip_anchor(linkpath: &str) -> &str {
    match linkpath.find('#') {
        Some(idx) => &linkpath[..idx],
        None => linkpath,
    }
}

/// Apply Obsidian-style resolution. Returns the index into
/// `vault.pages` of the resolved target, if any.
#[allow(dead_code)]
fn resolve(
    linkpath: &str,
    source_folder: &str,
    by_path: &HashMap<&str, usize>,
    by_basename: &HashMap<&str, Vec<usize>>,
) -> Option<usize> {
    let trimmed = linkpath.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 0. Relative-path resolution (`./foo`, `../foo/bar.md`) — join
    //    against the source page's folder + normalize `..` segments,
    //    then look up the result. Markdown links commonly use this
    //    form (`[lib](../lib/index.js)`); wikilinks rarely do but
    //    Obsidian accepts it.
    if trimmed.starts_with("./") || trimmed.starts_with("../") {
        if let Some(normalized) = normalize_relative(trimmed, source_folder) {
            if let Some(&i) = by_path.get(normalized.as_str()) {
                return Some(i);
            }
            let with_md = format!("{normalized}.md");
            if let Some(&i) = by_path.get(with_md.as_str()) {
                return Some(i);
            }
        }
        return None;
    }

    // 1. Exact rel_path with .md.
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    if trimmed.ends_with(".md") {
        if let Some(&i) = by_path.get(trimmed) {
            return Some(i);
        }
    }
    // 2. rel_path after appending .md.
    let with_md = format!("{trimmed}.md");
    if let Some(&i) = by_path.get(with_md.as_str()) {
        return Some(i);
    }

    // 2.5. Path-y target (`foo/bar` or `foo/bar.md`): also try
    //      resolving relative to the source page's folder. Lots of
    //      markdown READMEs use `[next](docs/next.md)`-style links
    //      that aren't vault-root-relative.
    if trimmed.contains('/') && !source_folder.is_empty() {
        let joined = format!("{source_folder}/{trimmed}");
        if let Some(&i) = by_path.get(joined.as_str()) {
            return Some(i);
        }
        let joined_md = format!("{joined}.md");
        if let Some(&i) = by_path.get(joined_md.as_str()) {
            return Some(i);
        }
    }

    // 3. Basename, only if path-y form didn't match. Treat as
    //    basename only when there's no `/` (rule 4); if there's a
    //    `/` the path-y attempts above were the only chance.
    if !trimmed.contains('/') {
        // Strip a trailing .md before basename lookup so `[[Note.md]]`
        // still resolves via basename if path lookup missed.
        let base = trimmed.strip_suffix(".md").unwrap_or(trimmed);
        if let Some(matches) = by_basename.get(base) {
            if matches.len() == 1 {
                return Some(matches[0]);
            }
            // Ambiguous (>1 page with same basename) → unresolved,
            // mirroring Obsidian's shortest-link rule which leaves
            // ambiguity to the user.
        }
    }

    None
}

/// Resolve to **every** matching page (vs `resolve`'s single best
/// match). Mirrors Obsidian's graph semantics: a wikilink with an
/// ambiguous basename creates an edge to every candidate page, not
/// just one. The navigation layer still picks a primary, but the
/// link-graph counts them all.
fn resolve_all(
    linkpath: &str,
    source_folder: &str,
    by_path: &HashMap<&str, usize>,
    by_basename: &HashMap<&str, Vec<usize>>,
) -> Vec<usize> {
    let trimmed = linkpath.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    // Relative paths are unambiguous (one normalized result).
    if trimmed.starts_with("./") || trimmed.starts_with("../") {
        if let Some(normalized) = normalize_relative(trimmed, source_folder) {
            if let Some(&i) = by_path.get(normalized.as_str()) {
                return vec![i];
            }
            let with_md = format!("{normalized}.md");
            if let Some(&i) = by_path.get(with_md.as_str()) {
                return vec![i];
            }
        }
        return Vec::new();
    }

    // Exact path-y matches first (always unique).
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    if trimmed.ends_with(".md") {
        if let Some(&i) = by_path.get(trimmed) {
            return vec![i];
        }
    }
    let with_md = format!("{trimmed}.md");
    if let Some(&i) = by_path.get(with_md.as_str()) {
        return vec![i];
    }
    if trimmed.contains('/') && !source_folder.is_empty() {
        let joined = format!("{source_folder}/{trimmed}");
        if let Some(&i) = by_path.get(joined.as_str()) {
            return vec![i];
        }
        let joined_md = format!("{joined}.md");
        if let Some(&i) = by_path.get(joined_md.as_str()) {
            return vec![i];
        }
    }

    // Basename — return ALL matches so the graph treats ambiguous
    // names as multi-edges (Obsidian behavior). The caller's
    // `primary_resolved` picks the first for navigation; `incoming`
    // gets all.
    if !trimmed.contains('/') {
        let base = trimmed.strip_suffix(".md").unwrap_or(trimmed);
        if let Some(matches) = by_basename.get(base) {
            return matches.clone();
        }
    }

    Vec::new()
}

/// Normalize a `./foo` / `../foo/bar` relative path against
/// `source_folder` and collapse `..` segments. Returns `None` if the
/// path tries to escape the vault root (e.g. too many `..`).
fn normalize_relative(linkpath: &str, source_folder: &str) -> Option<String> {
    let mut segments: Vec<&str> = if source_folder.is_empty() {
        Vec::new()
    } else {
        source_folder.split('/').collect()
    };
    for piece in linkpath.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    Some(segments.join("/"))
}

/// Resolve a wikilink against vault attachments. Mirrors page
/// resolution, but keyed on attachment `rel_paths` and filename-with-
/// extension basenames (e.g. `image.png`, `slides.pdf`).
fn resolve_attachment<'a>(
    linkpath: &str,
    source_folder: &str,
    by_att_path: &HashMap<&'a str, &'a str>,
    by_att_basename: &HashMap<&str, Vec<&'a str>>,
) -> Option<&'a str> {
    let trimmed = linkpath.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Relative path (`./img.png`, `../assets/foo.pdf`): normalize
    // against the source folder before lookup.
    if trimmed.starts_with("./") || trimmed.starts_with("../") {
        if let Some(normalized) = normalize_relative(trimmed, source_folder) {
            if let Some(&rel) = by_att_path.get(normalized.as_str()) {
                return Some(rel);
            }
        }
        return None;
    }
    // Path-y form: full rel_path match (e.g. `Folder/img.png`).
    if let Some(&rel) = by_att_path.get(trimmed) {
        return Some(rel);
    }
    // Path-y but vault-root-relative might be source-folder-relative
    // instead.
    if trimmed.contains('/') && !source_folder.is_empty() {
        let joined = format!("{source_folder}/{trimmed}");
        if let Some(&rel) = by_att_path.get(joined.as_str()) {
            return Some(rel);
        }
    }
    // Basename form: only resolves if exactly one attachment shares
    // the filename. Mirrors Obsidian's shortest-link rule for pages.
    if !trimmed.contains('/') {
        if let Some(matches) = by_att_basename.get(trimmed) {
            if matches.len() == 1 {
                return Some(matches[0]);
            }
        }
    }
    None
}

/// Filename-with-extension component of a vault-relative attachment
/// path. `Folder/Sub/img.png` -> `img.png`.
fn att_basename(rel: &str) -> &str {
    match rel.rfind('/') {
        Some(idx) => &rel[idx + 1..],
        None => rel,
    }
}

/// Compiled wikilink regex for scanning frontmatter strings. We
/// reuse `crate::obsidian_parse::LINK_REGEX` so the pattern
/// stays in lockstep with the body parser.
fn frontmatter_link_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(crate::obsidian_parse::LINK_REGEX).expect("LINK_REGEX is valid"))
}

/// Walk every frontmatter Value and yield linkpath targets found
/// inside `[[wikilink]]` patterns. Frontmatter is parsed as YAML
/// then stored as JSON values, so we recurse into Arrays and
/// Objects and apply the wikilink regex to any String we find.
///
/// Obsidian indexes these the same as body wikilinks for the
/// backlink graph — `projects: ["[[FastTrackStudio]]"]` gives
/// FastTrackStudio.md a backlink from the page declaring it.
fn extract_frontmatter_links(fm: &[FrontmatterEntry]) -> Vec<String> {
    let mut out = Vec::new();
    for entry in fm {
        scan_value(&entry.value, &mut out);
    }
    out
}

fn scan_value(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => {
            for caps in frontmatter_link_re().captures_iter(s) {
                if let Some(target) = caps.get(1) {
                    out.push(target.as_str().to_string());
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                scan_value(v, out);
            }
        }
        serde_json::Value::Object(obj) => {
            for v in obj.values() {
                scan_value(v, out);
            }
        }
        _ => {}
    }
}

/// Read `.obsidian/bookmarks.json` and return the vault-relative
/// paths of file-type bookmarks (recurses into groups). Other
/// bookmark kinds (url, search, …) are ignored.
fn read_bookmarked_paths(vault: &Vault) -> Vec<String> {
    let path = vault.root.join(".obsidian/bookmarks.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(json): Result<serde_json::Value, _> = serde_json::from_str(&raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(items) = json.get("items").and_then(|v| v.as_array()) {
        walk_bookmarks(items, &mut out);
    }
    out
}

fn walk_bookmarks(items: &[serde_json::Value], out: &mut Vec<String>) {
    for item in items {
        let Some(ty) = item.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        match ty {
            "file" => {
                if let Some(p) = item.get("path").and_then(|v| v.as_str()) {
                    out.push(p.to_string());
                }
            }
            "group" => {
                if let Some(nested) = item.get("items").and_then(|v| v.as_array()) {
                    walk_bookmarks(nested, out);
                }
            }
            _ => {}
        }
    }
}

/// True for URLs that point outside the vault — http(s):, mailto:,
/// tel:, ftp:, ws(s):, file:, scheme-prefixed in general. Markdown
/// links containing these can't resolve to a vault entry, so the
/// link-index treats them as "ignore" rather than "unresolved".
fn is_external_url(linkpath: &str) -> bool {
    // Strict prefix list — covers the realistic schemes Obsidian
    // emits without false-positiving on `note:foo` style relative
    // paths (which don't appear in practice but a generic `scheme:`
    // detector would mis-match).
    let lc = linkpath.trim_start();
    for prefix in [
        "http://", "https://", "mailto:", "tel:", "ftp://", "ftps://", "ws://", "wss://",
        "file://", "data:",
    ] {
        if lc.starts_with(prefix) {
            return true;
        }
    }
    false
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

    fn mini_vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        // A links to B (by basename), to Folder/C (by path), to
        // Missing (unresolved), embeds D, and self-links A.
        touch(
            &dir.path().join("A.md"),
            "Has [[B]] and [[Folder/C]] and [[Missing]] and ![[D]] and [[A]] and [[B#Heading]]\n",
        );
        touch(&dir.path().join("B.md"), "Plain page with no links.\n");
        touch(&dir.path().join("Folder/C.md"), "C links back to [[A]]\n");
        touch(&dir.path().join("D.md"), "Embeddable.\n");
        // Orphan + deadend.
        touch(&dir.path().join("Lonely.md"), "Nothing here.\n");
        let v = Vault::open(dir.path()).expect("open");
        (dir, v)
    }

    #[test]
    fn backlinks_lists_sources_pointing_at_target() {
        let (_dir, v) = mini_vault();
        let idx = LinkIndex::build(&v);
        let bl = idx.backlinks("B.md");
        assert_eq!(bl, vec!["A.md"]);
        let bl_c = idx.backlinks("Folder/C.md");
        assert_eq!(bl_c, vec!["A.md"]);
        let bl_a = idx.backlinks("A.md");
        // C links to A; self-link is not counted.
        assert_eq!(bl_a, vec!["Folder/C.md"]);
    }

    #[test]
    fn outgoing_is_deduped_in_source_order_with_resolution() {
        let (_dir, v) = mini_vault();
        let idx = LinkIndex::build(&v);
        let out = idx.outgoing("A.md");
        // [[B]] [[Folder/C]] [[Missing]] ![[D]] [[A]] [[B#Heading]]
        // The parser pulls `#Heading` into a separate `heading` field,
        // so [[B#Heading]] has the same `target_linkpath` ("B") as
        // [[B]] and dedupes away.
        let raws: Vec<&str> = out.iter().map(|o| o.linkpath).collect();
        assert_eq!(raws, vec!["B", "Folder/C", "Missing", "D", "A"]);
        let resolved: Vec<Option<&str>> = out.iter().map(|o| o.resolved).collect();
        assert_eq!(
            resolved,
            vec![
                Some("B.md"),
                Some("Folder/C.md"),
                None,
                Some("D.md"),
                Some("A.md"),
            ]
        );
    }

    #[test]
    fn orphans_have_zero_incoming() {
        let (_dir, v) = mini_vault();
        let idx = LinkIndex::build(&v);
        let orph = idx.orphans();
        // B, C, D, A all have an incoming link. Lonely is the only
        // orphan. (A has an incoming from C; self-link doesn't count
        // as incoming but A still has C → A.)
        assert!(orph.contains(&"Lonely.md"));
        assert!(!orph.contains(&"A.md"));
        assert!(!orph.contains(&"B.md"));
        assert!(!orph.contains(&"Folder/C.md"));
        assert!(!orph.contains(&"D.md"));
    }

    #[test]
    fn unresolved_collects_bad_targets() {
        let (_dir, v) = mini_vault();
        let idx = LinkIndex::build(&v);
        let un = idx.unresolved();
        assert_eq!(un.len(), 1);
        assert_eq!(un[0].source, "A.md");
        assert_eq!(un[0].linkpath, "Missing");
    }

    #[test]
    fn deadends_are_pages_with_no_outgoing() {
        let (_dir, v) = mini_vault();
        let idx = LinkIndex::build(&v);
        let dead = idx.deadends();
        assert!(dead.contains(&"B.md"));
        assert!(dead.contains(&"D.md"));
        assert!(dead.contains(&"Lonely.md"));
        assert!(!dead.contains(&"A.md"));
        assert!(!dead.contains(&"Folder/C.md"));
    }

    #[test]
    fn ambiguous_basename_fans_out_to_all_candidates() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("X/Note.md"), "x\n");
        touch(&dir.path().join("Y/Note.md"), "y\n");
        touch(&dir.path().join("Src.md"), "See [[Note]] and [[X/Note]]\n");
        let v = Vault::open(dir.path()).unwrap();
        let idx = LinkIndex::build(&v);
        let out = idx.outgoing("Src.md");
        // [[Note]] resolves to *some* candidate (the navigation
        // primary is the first-listed); [[X/Note]] is exact path.
        assert_eq!(out[0].linkpath, "Note");
        assert!(
            out[0].resolved.is_some(),
            "ambiguous basename should still surface a primary target"
        );
        assert_eq!(out[1].linkpath, "X/Note");
        assert_eq!(out[1].resolved, Some("X/Note.md"));
        // Both X/Note.md and Y/Note.md should have Src.md as a
        // backlink — Obsidian's "ambiguous edges fan out" semantics.
        assert!(idx.backlinks("X/Note.md").contains(&"Src.md"));
        assert!(idx.backlinks("Y/Note.md").contains(&"Src.md"));
    }

    #[test]
    fn attachment_resolves_by_basename() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Assets/photo.png"), "binary");
        touch(&dir.path().join("Src.md"), "See ![[photo.png]]\n");
        let v = Vault::open(dir.path()).unwrap();
        let idx = LinkIndex::build(&v);
        let out = idx.outgoing("Src.md");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].linkpath, "photo.png");
        assert_eq!(out[0].resolved, Some("Assets/photo.png"));
        assert!(idx.unresolved().is_empty());
    }

    #[test]
    fn attachment_resolves_by_folder_path() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Assets/Sub/diagram.pdf"), "binary");
        touch(&dir.path().join("Src.md"), "[[Assets/Sub/diagram.pdf]]\n");
        let v = Vault::open(dir.path()).unwrap();
        let idx = LinkIndex::build(&v);
        let out = idx.outgoing("Src.md");
        assert_eq!(out[0].resolved, Some("Assets/Sub/diagram.pdf"));
        assert!(idx.unresolved().is_empty());
    }

    #[test]
    fn attachment_unresolved_when_no_match() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Src.md"), "![[ghost.png]]\n");
        let v = Vault::open(dir.path()).unwrap();
        let idx = LinkIndex::build(&v);
        let un = idx.unresolved();
        assert_eq!(un.len(), 1);
        assert_eq!(un[0].linkpath, "ghost.png");
    }

    #[test]
    fn orphans_and_deadends_exclude_attachments() {
        let dir = tempfile::tempdir().unwrap();
        // Attachment exists but no page links to it; should NOT appear
        // in orphans (orphans is a page-only concept) and should NOT
        // appear in deadends (deadends is a page-only concept).
        touch(&dir.path().join("Assets/loner.png"), "binary");
        touch(&dir.path().join("OnlyPage.md"), "I link nothing.\n");
        let v = Vault::open(dir.path()).unwrap();
        let idx = LinkIndex::build(&v);

        let orph = idx.orphans();
        assert!(orph.contains(&"OnlyPage.md"));
        assert!(
            !orph.iter().any(|r| std::path::Path::new(r)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("png"))),
            "attachments leaked into orphans: {orph:?}"
        );

        let dead = idx.deadends();
        assert!(dead.contains(&"OnlyPage.md"));
        assert!(
            !dead.iter().any(|r| std::path::Path::new(r)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("png"))),
            "attachments leaked into deadends: {dead:?}"
        );
    }

    #[test]
    fn ambiguous_attachment_basename_does_not_resolve() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("A/img.png"), "x");
        touch(&dir.path().join("B/img.png"), "x");
        touch(
            &dir.path().join("Src.md"),
            "![[img.png]] and ![[A/img.png]]\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let idx = LinkIndex::build(&v);
        let out = idx.outgoing("Src.md");
        assert_eq!(out[0].linkpath, "img.png");
        assert_eq!(out[0].resolved, None);
        assert_eq!(out[1].linkpath, "A/img.png");
        assert_eq!(out[1].resolved, Some("A/img.png"));
    }

    #[test]
    fn explicit_md_suffix_resolves() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Target.md"), "t\n");
        touch(&dir.path().join("Src.md"), "[[Target.md]]\n");
        let v = Vault::open(dir.path()).unwrap();
        let idx = LinkIndex::build(&v);
        let out = idx.outgoing("Src.md");
        assert_eq!(out[0].resolved, Some("Target.md"));
    }
}
