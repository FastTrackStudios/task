//! Resolve a `NodeRef` anchor against a resource's sidecar — the
//! read-path counterpart of [`crate::build`]. Given a link endpoint
//! pointing into a resource, say *what* it points at and *how to reach
//! it* (seek to a second, jump to a PDF region, highlight a lyric line).
//! Logseq's `open-block-ref!` does the same job.

use links_proto::{Anchor, NodeKind, NodeRef};

use crate::types::{AnnotationFile, Geometry};

/// What an anchor resolves to, enriched with sidecar geometry/label.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// The whole resource (no anchor).
    Whole,
    /// Seek the recording to `secs`.
    Seek { secs: u32 },
    /// Play a clip — a timestamp range (`start..end`, seconds).
    Clip { start: u32, end: u32 },
    /// The Nth original-language word (verse word study).
    Word { index: usize },
    /// A vault/note block. `preview` is the block's first line when a
    /// lookup was supplied (backed by the vault's `BlockIndex`).
    Block { id: String, preview: Option<String> },
    /// A PDF region — `geometry` present when the sidecar has it.
    Region {
        page: u32,
        geometry: Option<Geometry>,
    },
    /// A text/lyric span — `label`/`text` from the sidecar when present.
    Span { label: String, text: String },
}

/// Resolve `node`'s anchor against `file` (the resource's sidecar),
/// without block previews. See [`resolve_with`] to supply a `BlockIndex`
/// lookup.
#[must_use]
pub fn resolve(file: &AnnotationFile, node: &NodeRef) -> Resolved {
    resolve_with(file, node, |_| None)
}

/// Resolve `node`, using `block_preview(uuid) -> Option<preview>` to fill
/// in block content. The caller wires `block_preview` to the live vault's
/// `BlockIndex` (`index.preview_str(&vault, uuid)`), keeping this crate
/// free of a vault dependency — Logseq's `open-block-ref` resolution,
/// dependency-injected.
#[must_use]
pub fn resolve_with(
    file: &AnnotationFile,
    node: &NodeRef,
    block_preview: impl Fn(&str) -> Option<String>,
) -> Resolved {
    // A block *node* (`block:uuid`) — the id is the block uuid itself.
    if node.kind == NodeKind::Block {
        return Resolved::Block {
            preview: block_preview(&node.id),
            id: node.id.clone(),
        };
    }
    match node.anchor_kind() {
        Anchor::Whole => Resolved::Whole,
        Anchor::Timestamp(secs) => Resolved::Seek { secs },
        Anchor::Clip { start, end } => Resolved::Clip { start, end },
        Anchor::Word(index) => Resolved::Word { index },
        // A block *anchor* (`note:page.md#^uuid`).
        Anchor::Block(id) => Resolved::Block {
            preview: block_preview(&id),
            id,
        },
        Anchor::Region { page, .. } => {
            let geometry = file.get(&node.anchor).and_then(|a| a.geometry.clone());
            Resolved::Region { page, geometry }
        }
        Anchor::Span(anchor_label) => {
            // Prefer the sidecar's human label + captured text; fall back
            // to the raw anchor string when the resource has no sidecar row.
            let (label, text) = file.get(&node.anchor).map_or_else(
                || (anchor_label.clone(), String::new()),
                |a| {
                    let label = if a.label.is_empty() {
                        anchor_label.clone()
                    } else {
                        a.label.clone()
                    };
                    (label, a.text.clone())
                },
            );
            Resolved::Span { label, text }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Annotation;

    #[test]
    fn resolves_timestamp_and_span() {
        let mut file = AnnotationFile::new("keep-on-finding-more");
        file.upsert(Annotation {
            anchor: "chorus.L1".into(),
            label: "You are the maker".into(),
            text: "You are the maker".into(),
            color: None,
            geometry: None,
        });

        let seek = resolve(&file, &NodeRef::song("keep-on-finding-more").at(90));
        assert_eq!(seek, Resolved::Seek { secs: 90 });

        let span = resolve(
            &file,
            &NodeRef::song("keep-on-finding-more").with_anchor("chorus.L1"),
        );
        assert_eq!(
            span,
            Resolved::Span {
                label: "You are the maker".into(),
                text: "You are the maker".into(),
            }
        );

        // Unknown span anchor resolves to an empty-text span (still navigable).
        let unknown = resolve(
            &file,
            &NodeRef::song("keep-on-finding-more").with_anchor("verse9.L9"),
        );
        assert!(matches!(unknown, Resolved::Span { text, .. } if text.is_empty()));
    }

    #[test]
    fn resolves_block_node_and_anchor_via_lookup() {
        use links_proto::NodeKind;
        let file = AnnotationFile::new("x");
        // Stand-in for the vault BlockIndex lookup.
        let lookup = |uuid: &str| (uuid == "abc").then(|| "the block's first line".to_string());

        // A block *node* (`block:abc`).
        let node = NodeRef::new(NodeKind::Block, "abc");
        assert_eq!(
            resolve_with(&file, &node, lookup),
            Resolved::Block {
                id: "abc".into(),
                preview: Some("the block's first line".into())
            }
        );

        // A block *anchor* on a note (`note:journal.md#^abc`).
        let anchored = NodeRef::new(NodeKind::Note, "journal.md").with_anchor("^abc");
        assert_eq!(
            resolve_with(&file, &anchored, lookup),
            Resolved::Block {
                id: "abc".into(),
                preview: Some("the block's first line".into())
            }
        );

        // Plain `resolve` supplies no lookup → no preview.
        assert_eq!(
            resolve(&file, &node),
            Resolved::Block {
                id: "abc".into(),
                preview: None
            }
        );
    }
}
