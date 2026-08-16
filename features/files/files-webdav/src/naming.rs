//! Mapping a File Root onto the URL segment a file manager shows as a
//! folder name.
//!
//! A mounted root has to *look* like the project it is, so the segment
//! is the root's name — not its uuid. Two constraints make that less
//! trivial than it sounds:
//!
//! - Root names are free text and need not be unique, but a collection
//!   cannot hold two children with the same name. Colliding names are
//!   therefore *all* disambiguated with a short id suffix, so a segment
//!   never depends on which root happened to be created first.
//! - Finder and Explorer treat names case-insensitively, so collision
//!   detection folds case even though the segments themselves keep it.
//!
//! A root's uuid is always accepted as an alternative segment
//! ([`resolve`]), which gives scripts and tests a stable address that
//! survives a rename.

use std::collections::HashMap;

use files_proto::FileRootInfo;
use uuid::Uuid;

/// Characters that must not appear in a path segment — the separator
/// and NUL are structurally impossible, the rest are the Windows
/// reserved set (Explorer refuses to create or display them, and this
/// bridge exists for Explorer).
const RESERVED: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// The segment `root` would get if its name were unique.
fn sanitize(root: &FileRootInfo) -> String {
    let cleaned: String = root
        .name
        .chars()
        .map(|c| {
            if c.is_control() || RESERVED.contains(&c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    // `.`/`..` would be eaten by path normalization, and an empty
    // segment is unaddressable — fall back to the id, which is always
    // a legal segment.
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        root.id.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Short, stable disambiguator for colliding names.
fn short_id(id: Uuid) -> String {
    id.simple().to_string()[..8].to_string()
}

/// A root and the URL segment it is addressed by — computed once per
/// request by [`segments`] and then passed around, so the mount's
/// listing and its dispatch can never disagree about what a folder is
/// called.
#[derive(Debug, Clone)]
pub struct RootSegment {
    pub segment: String,
    pub root: FileRootInfo,
}

/// Every visible root paired with the URL segment it is addressed by.
/// Order follows `roots`.
#[must_use]
pub fn segments(roots: &[FileRootInfo]) -> Vec<RootSegment> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for root in roots {
        *counts.entry(sanitize(root).to_lowercase()).or_default() += 1;
    }
    roots
        .iter()
        .map(|root| {
            let base = sanitize(root);
            let segment = if counts.get(&base.to_lowercase()).copied().unwrap_or(0) > 1 {
                format!("{base} ({})", short_id(root.id))
            } else {
                base
            };
            RootSegment {
                segment,
                root: root.clone(),
            }
        })
        .collect()
}

/// Resolve one URL segment against already-computed [`segments`]: its
/// uuid, or its name segment (case-insensitively, matching how the
/// mounting OS compares names).
#[must_use]
pub fn find<'a>(entries: &'a [RootSegment], segment: &str) -> Option<&'a RootSegment> {
    if let Ok(id) = Uuid::parse_str(segment)
        && let Some(entry) = entries.iter().find(|e| e.root.id == id)
    {
        return Some(entry);
    }
    entries
        .iter()
        .find(|e| e.segment.eq_ignore_ascii_case(segment))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use files_proto::RootFlavor;

    fn root(name: &str) -> FileRootInfo {
        FileRootInfo {
            id: Uuid::new_v4(),
            name: name.to_string(),
            path: Some(format!("/tmp/{name}")),
            flavor: RootFlavor::Media,
            created_at: Utc::now(),
            project_version: None,
        }
    }

    #[test]
    fn unique_names_keep_their_name() {
        let roots = vec![root("El Artisa"), root("Dr Jaramillo")];
        let segs = segments(&roots);
        assert_eq!(segs[0].segment, "El Artisa");
        assert_eq!(segs[1].segment, "Dr Jaramillo");
    }

    #[test]
    fn colliding_names_are_all_suffixed() {
        let roots = vec![root("Mix"), root("mix")];
        let segs = segments(&roots);
        assert!(segs[0].segment.starts_with("Mix ("), "{}", segs[0].segment);
        assert!(segs[1].segment.starts_with("mix ("), "{}", segs[1].segment);
        assert_ne!(segs[0].segment, segs[1].segment);
        // Both remain resolvable by their own segment.
        for entry in &segs {
            assert_eq!(
                find(&segs, &entry.segment).unwrap().root.id,
                entry.root.id,
                "{} resolves to itself",
                entry.segment
            );
        }
    }

    #[test]
    fn reserved_characters_are_replaced() {
        let roots = vec![root("A/B:C")];
        assert_eq!(segments(&roots)[0].segment, "A-B-C");
    }

    #[test]
    fn a_root_is_always_addressable_by_uuid() {
        let roots = vec![root("Mix")];
        let segs = segments(&roots);
        let by_id = find(&segs, &roots[0].id.to_string()).expect("uuid segment resolves");
        assert_eq!(by_id.root.id, roots[0].id);
    }

    #[test]
    fn an_unnameable_root_falls_back_to_its_id() {
        let roots = vec![root("   ")];
        assert_eq!(segments(&roots)[0].segment, roots[0].id.to_string());
    }
}
