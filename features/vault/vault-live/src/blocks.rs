//! Cross-vault block index. Scans every page in a [`Vault`]
//! once, finding every `id:: <uuid>` property line, and builds a
//! `uuid → (page index, byte offset)` map.
//!
//! Used to resolve:
//! - `((uuid))` block references in any page → the target page +
//!   the block's anchor offset for content lookup.
//! - `{{embed ((uuid))}}` block embeds — same resolution path,
//!   different render.
//!
//! The build is O(total markdown bytes). For small / medium
//! vaults a single in-memory rebuild on watcher events is fine;
//! a future slice can incrementalize per-file.

use std::collections::HashMap;

use uuid::Uuid;

use crate::vault::{Vault, VaultPage};

/// Where a block lives.
#[derive(Clone, Debug)]
pub struct BlockLocation {
    /// Index of the page in `vault.pages`.
    pub page_idx: usize,
    /// Byte offset (within `page.raw`) of the start of the
    /// block's content line — the line above the `id:: …` line.
    pub anchor: usize,
    /// Byte range of the `id:: <uuid>` line itself, including
    /// its trailing newline when present. Mutate ops use this
    /// to update / remove the id without disturbing the rest of
    /// the page.
    pub id_line: std::ops::Range<usize>,
}

/// `uuid → location` map across the whole vault.
#[derive(Clone, Debug, Default)]
pub struct BlockIndex {
    map: HashMap<Uuid, BlockLocation>,
}

impl BlockIndex {
    /// Walk the vault and collect every `id:: <uuid>` line.
    #[must_use]
    pub fn build(vault: &Vault) -> Self {
        let mut map = HashMap::new();
        for (page_idx, page) in vault.pages.iter().enumerate() {
            for loc in scan_page_block_ids(page) {
                if let Some((uuid, loc)) = wrap(page_idx, loc, &page.raw) {
                    map.insert(uuid, loc);
                }
            }
        }
        Self { map }
    }

    #[must_use]
    pub fn lookup(&self, uuid: &Uuid) -> Option<&BlockLocation> {
        self.map.get(uuid)
    }

    #[must_use]
    pub fn lookup_str(&self, uuid: &str) -> Option<&BlockLocation> {
        let parsed = Uuid::parse_str(uuid).ok()?;
        self.lookup(&parsed)
    }

    /// First-line content of the block (for `((uuid))` chip
    /// previews and `{{embed}}` cards).
    #[must_use]
    pub fn block_preview<'v>(&self, vault: &'v Vault, uuid: &Uuid) -> Option<&'v str> {
        let loc = self.map.get(uuid)?;
        let page = vault.pages.get(loc.page_idx)?;
        let raw = &page.raw;
        let end = raw[loc.anchor..]
            .find('\n')
            .map_or(raw.len(), |n| loc.anchor + n);
        Some(&raw[loc.anchor..end])
    }

    /// String-keyed [`Self::block_preview`] — the shape the links layer
    /// injects into `resources::resolve_with` to turn a
    /// `block:<uuid>` / `note:p.md#^<uuid>` reference into a preview.
    /// Returns an owned `String` (a closure can't borrow the vault).
    #[must_use]
    pub fn preview_str(&self, vault: &Vault, uuid: &str) -> Option<String> {
        let parsed = Uuid::parse_str(uuid).ok()?;
        self.block_preview(vault, &parsed).map(str::to_string)
    }

    /// How many blocks across the whole vault have ids.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Internal: walk one page, yielding `(uuid_str, id_line_range)`
/// pairs. The `anchor` offset of the OWNING block is computed
/// by `wrap` below using the page's raw text.
fn scan_page_block_ids(page: &VaultPage) -> Vec<(String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let bytes = page.raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let line_start = i;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        let line_end = i;
        let nl_end = if i < bytes.len() { i + 1 } else { i };
        let line = &page.raw[line_start..line_end];
        if let Some(uuid) = parse_id_line(line) {
            out.push((uuid.to_string(), line_start..nl_end));
        }
        i = nl_end;
    }
    out
}

/// Compute the block's content anchor (the first non-blank line
/// above the id-line) and the typed UUID. The anchor handles
/// the common case of `<content>\nid:: <uuid>` and tolerates
/// any number of blank lines between them.
fn wrap(
    page_idx: usize,
    (uuid_str, id_line): (String, std::ops::Range<usize>),
    raw: &str,
) -> Option<(Uuid, BlockLocation)> {
    let parsed = Uuid::parse_str(&uuid_str).ok()?;
    let anchor = find_block_anchor(raw, id_line.start);
    Some((
        parsed,
        BlockLocation {
            page_idx,
            anchor,
            id_line,
        },
    ))
}

fn parse_id_line(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let after = trimmed.strip_prefix("id:: ")?;
    let candidate = after.trim();
    if candidate.len() != 36 {
        return None;
    }
    if !candidate.is_ascii() {
        return None;
    }
    let bytes = candidate.as_bytes();
    for (idx, b) in bytes.iter().enumerate() {
        let is_hyphen = matches!(idx, 8 | 13 | 18 | 23);
        if is_hyphen {
            if *b != b'-' {
                return None;
            }
        } else if !b.is_ascii_hexdigit() {
            return None;
        }
    }
    Some(candidate)
}

/// Walk back from the id-line's offset to the first non-blank
/// line above. Returns the start byte of that line.
fn find_block_anchor(raw: &str, id_line_start: usize) -> usize {
    if id_line_start == 0 {
        return 0;
    }
    let prefix = &raw[..id_line_start];
    let mut end = id_line_start;
    while end > 0 {
        let prev_nl = prefix[..end - 1].rfind('\n').map_or(0, |n| n + 1);
        let line = &raw[prev_nl..end - 1];
        if !line.trim().is_empty() {
            return prev_nl;
        }
        end = prev_nl;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::VaultPage;
    use std::time::SystemTime;

    fn page(raw: &str) -> VaultPage {
        VaultPage {
            rel_path: "p.md".into(),
            basename: "p".into(),
            folder: String::new(),
            raw: raw.to_string(),
            mtime: SystemTime::UNIX_EPOCH,
        }
    }

    fn vault_with(pages: Vec<VaultPage>) -> Vault {
        Vault {
            bases: Vec::new(),
            property_types: Default::default(),
            root: ".".into(),
            pages,
        }
    }

    #[test]
    fn indexes_a_single_id_line() {
        let uuid = "5f9c1234-abcd-4ef0-8123-fedcba012345";
        let v = vault_with(vec![page(&format!(
            "First block content\nid:: {uuid}\n\nNext block"
        ))]);
        let idx = BlockIndex::build(&v);
        assert_eq!(idx.len(), 1);
        let parsed = Uuid::parse_str(uuid).unwrap();
        let loc = idx.lookup(&parsed).unwrap();
        assert_eq!(loc.page_idx, 0);
        assert_eq!(loc.anchor, 0);
    }

    #[test]
    fn block_preview_returns_anchor_line() {
        let uuid = "5f9c1234-abcd-4ef0-8123-fedcba012345";
        let v = vault_with(vec![page(&format!("Hello there\nid:: {uuid}\nmore"))]);
        let idx = BlockIndex::build(&v);
        let preview = idx
            .block_preview(&v, &Uuid::parse_str(uuid).unwrap())
            .unwrap();
        assert_eq!(preview, "Hello there");
    }

    #[test]
    fn ignores_malformed_id_lines() {
        let v = vault_with(vec![page(
            "Block 1\nid:: not-a-uuid\n\nBlock 2\nid:: 11111111-1111-1111-1111-111111111111",
        )]);
        let idx = BlockIndex::build(&v);
        assert_eq!(idx.len(), 1);
    }

    #[test]
    fn preview_str_resolves_by_uuid_string() {
        let uuid = "5f9c1234-abcd-4ef0-8123-fedcba012345";
        let v = vault_with(vec![page(&format!(
            "Cast your cares on Him\nid:: {uuid}\nmore"
        ))]);
        let idx = BlockIndex::build(&v);
        assert_eq!(
            idx.preview_str(&v, uuid).as_deref(),
            Some("Cast your cares on Him")
        );
        assert_eq!(idx.preview_str(&v, "not-a-uuid"), None);
    }

    #[test]
    fn cross_page_lookup() {
        let uuid_a = "11111111-1111-1111-1111-111111111111";
        let uuid_b = "22222222-2222-2222-2222-222222222222";
        let v = vault_with(vec![
            page(&format!("Page A block\nid:: {uuid_a}")),
            page(&format!("Page B block\nid:: {uuid_b}")),
        ]);
        let idx = BlockIndex::build(&v);
        assert_eq!(idx.lookup_str(uuid_a).unwrap().page_idx, 0);
        assert_eq!(idx.lookup_str(uuid_b).unwrap().page_idx, 1);
    }
}
