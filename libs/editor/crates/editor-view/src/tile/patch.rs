//! Serializable tile-tree description for the imperative DOM
//! patcher.
//!
//! The Dioxus reconciler can't safely manage a contenteditable's
//! contents — async batching + node-replace-on-attribute-change
//! interact badly with browser-side text editing. So we bypass
//! it: Dioxus renders an empty `<div contenteditable>` as a
//! Mount point, and EVERY state change feeds through this
//! serialization to an imperative JS patcher that mutates the
//! DOM directly. Same pattern CM6 uses.
//!
//! What this module produces: a `Patch` tree — a JSON-friendly
//! description of exactly what the editor's content DOM should
//! look like for the current state. Compact field names (`tag`,
//! `attrs`, `kids`, `text`) keep the per-render payload small.
//!
//! The JS-side patcher (`editor.rs::patcher_script`) diffs this
//! against the live DOM, matches elements by `data-tile-pos`,
//! updates text nodes in place, and adds/removes children
//! minimally.

use serde::{Deserialize, Serialize};

use crate::tile::arena::{Arena, TileId};
use crate::tile::line::line_extra_classes;
use crate::tile::mark::mark_spec_of;
use crate::tile::pos::pos_at_start;
use crate::tile::text::text_of;
use crate::tile::{TileBody, TileKind};

/// One node in the imperative patch description. Either an
/// element (with tag + attributes + children) or a literal
/// text node.
///
/// Serialized as `{tag, attrs, kids}` / `{text}` for the JS
/// patcher.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum Patch {
    Element {
        /// HTML tag — `div`, `span`, `br`, etc.
        tag: String,
        /// Key/value attribute pairs. Order isn't significant
        /// to the patcher; it diffs by key.
        attrs: Vec<(String, String)>,
        /// Child patches. Empty vec is fine.
        kids: Vec<Patch>,
    },
    Text {
        /// The text content this node holds.
        text: String,
    },
}

impl Patch {
    fn element(tag: &str, attrs: Vec<(String, String)>, kids: Vec<Patch>) -> Self {
        Self::Element {
            tag: tag.to_string(),
            attrs,
            kids,
        }
    }

    fn text(s: impl Into<String>) -> Self {
        Self::Text { text: s.into() }
    }
}

/// Build the patch description for the entire tile tree.
/// Returns the *children* of the editor root — the patcher
/// applies them to the existing `<div data-editor-id>` mount
/// element rather than replacing it.
#[must_use]
pub fn build_patch(arena: &Arena, root: TileId) -> Vec<Patch> {
    let root_tile = arena.get(root);
    let mut out = Vec::new();
    for &child in &root_tile.children {
        if let Some(p) = render_node(arena, child) {
            out.push(p);
        }
    }
    out
}

/// Does this patch sub-tree contain any non-widget text node?
/// Widgets are rendered `contenteditable="false"` so the browser
/// doesn't treat them as cursor targets — without an actual
/// text descendant a line stays unreachable via line motions.
fn patch_has_text(p: &Patch) -> bool {
    match p {
        Patch::Text { text } => !text.is_empty(),
        Patch::Element { tag, attrs, kids } => {
            if tag == "br" {
                return true;
            }
            let is_widget = attrs
                .iter()
                .any(|(k, v)| k == "class" && v.contains("editor-widget"));
            if is_widget {
                return false;
            }
            kids.iter().any(patch_has_text)
        }
    }
}

fn render_node(arena: &Arena, tile: TileId) -> Option<Patch> {
    let t = arena.get(tile);
    let pos = pos_at_start(arena, tile);
    let tid = tile.0;
    match t.kind {
        TileKind::Doc => {
            // Doc handled at the root level — see build_patch.
            None
        }
        TileKind::Line => {
            let mut kids = Vec::new();
            for &c in &t.children {
                if let Some(p) = render_node(arena, c) {
                    kids.push(p);
                }
            }
            // The browser's native arrow / line-by-line caret
            // motion (also what `Selection.modify('move', dir,
            // 'line')` uses for vim j/k) refuses to land on a
            // line that has no text descendant — a line whose
            // sole child is an absolute-positioned widget reads
            // as "no cursor target here" and gets skipped. Add
            // a `<br>` so there's always a place for the caret
            // to anchor.
            let has_text_anchor = kids.iter().any(patch_has_text);
            if !has_text_anchor {
                kids.push(Patch::element("br", Vec::new(), Vec::new()));
            }
            let extras = line_extra_classes(t);
            let class = if extras.is_empty() {
                "cm-line".to_string()
            } else {
                let mut s = String::from("cm-line");
                for c in extras {
                    s.push(' ');
                    s.push_str(c);
                }
                s
            };
            Some(Patch::element(
                "div",
                vec![
                    ("class".into(), class),
                    ("data-tile-id".into(), tid.to_string()),
                    ("data-tile-pos".into(), pos.to_string()),
                    ("data-tile-len".into(), t.length.to_string()),
                ],
                kids,
            ))
        }
        TileKind::Text => {
            let text = text_of(t).to_string();
            Some(Patch::element(
                "span",
                vec![
                    ("data-tile-id".into(), tid.to_string()),
                    ("data-tile-pos".into(), pos.to_string()),
                ],
                vec![Patch::text(text)],
            ))
        }
        TileKind::Mark => {
            let spec = mark_spec_of(t).clone();
            let class = spec.class;
            let mut kids = Vec::new();
            for &c in &t.children {
                if let Some(p) = render_node(arena, c) {
                    kids.push(p);
                }
            }
            let mut attrs = vec![
                ("class".into(), class),
                ("data-tile-id".into(), tid.to_string()),
                ("data-tile-pos".into(), pos.to_string()),
            ];
            // Extra spec attrs (e.g. `data-href` for links). The
            // JS-side click handler in editor.rs reads these.
            attrs.extend(spec.attrs);
            Some(Patch::element("span", attrs, kids))
        }
        TileKind::WidgetBuffer => {
            // Zero-width `<img>` anchor bracketing an inline point
            // widget — gives the browser caret an editable-side stop
            // next to `contenteditable=false` content. Carries
            // `data-tile-len="0"` so the JS bridge's offset math counts
            // it as zero doc bytes. CM6's `.cm-widgetBuffer`
            // (`tile.ts:488-505`).
            Some(Patch::element(
                "img",
                vec![
                    ("class".into(), "cm-widgetBuffer".into()),
                    ("aria-hidden".into(), "true".into()),
                    ("data-tile-id".into(), tid.to_string()),
                    ("data-tile-pos".into(), pos.to_string()),
                    ("data-tile-len".into(), "0".into()),
                ],
                Vec::new(),
            ))
        }
        TileKind::Widget => {
            let html = match &t.body {
                TileBody::Widget { html } => html.clone(),
                _ => String::new(),
            };
            if html.is_empty() {
                None
            } else {
                Some(Patch::element(
                    "span",
                    vec![
                        ("class".into(), "editor-widget".into()),
                        ("contenteditable".into(), "false".into()),
                        ("data-tile-id".into(), tid.to_string()),
                        ("data-tile-pos".into(), pos.to_string()),
                        // `priorSiblingBytes` in the JS bridge
                        // reads this to know how many doc bytes
                        // this widget consumes (markers etc.)
                        // when summing offsets for a downstream
                        // text node.
                        ("data-tile-len".into(), t.length.to_string()),
                        ("data-widget-html".into(), html),
                    ],
                    Vec::new(),
                ))
            }
        }
        TileKind::BlockWrapper => {
            // Same shape as Line for v1 — no block-level
            // decorations carry their own DOM yet.
            let mut kids = Vec::new();
            for &c in &t.children {
                if let Some(p) = render_node(arena, c) {
                    kids.push(p);
                }
            }
            Some(Patch::element(
                "div",
                vec![
                    ("data-tile-id".into(), tid.to_string()),
                    ("data-tile-pos".into(), pos.to_string()),
                ],
                kids,
            ))
        }
    }
}

#[cfg(test)]
#[allow(clippy::match_wildcard_for_single_variants)]
mod tests {
    use super::*;
    use crate::tile::build::build_tiles;
    use editor_state::Decoration;

    #[test]
    fn plain_text_produces_line_and_span() {
        let (arena, root) = build_tiles("hello", &[]);
        let patch = build_patch(&arena, root);
        // One LineTile child with one span child.
        assert_eq!(patch.len(), 1);
        match &patch[0] {
            Patch::Element { tag, kids, .. } => {
                assert_eq!(tag, "div");
                assert_eq!(kids.len(), 1);
                match &kids[0] {
                    Patch::Element { tag, kids, .. } => {
                        assert_eq!(tag, "span");
                        match &kids[0] {
                            Patch::Text { text } => assert_eq!(text, "hello"),
                            _ => panic!("expected Text"),
                        }
                    }
                    _ => panic!("expected span"),
                }
            }
            _ => panic!("expected div"),
        }
    }

    #[test]
    fn point_widget_renders_buffer_imgs_around_span() {
        // A zero-width widget renders as
        // `<img cm-widgetBuffer><span editor-widget><img cm-widgetBuffer>`
        // so the caret has an editable-side anchor on each side.
        let decs = vec![Decoration::widget(2, "<span class=\"kf-inlay\">G</span>")];
        let (arena, root) = build_tiles("abcd", &decs);
        let patch = build_patch(&arena, root);
        let kids = match &patch[0] {
            Patch::Element { tag, kids, .. } if tag == "div" => kids,
            _ => panic!("expected line div"),
        };
        let tags: Vec<&str> = kids
            .iter()
            .map(|k| match k {
                Patch::Element { tag, .. } => tag.as_str(),
                Patch::Text { .. } => "#text",
            })
            .collect();
        assert_eq!(tags, vec!["span", "img", "span", "img", "span"]);
        // The buffers carry the CM6 class + zero doc-length so the JS
        // bridge counts them as no bytes.
        for i in [1usize, 3] {
            match &kids[i] {
                Patch::Element { attrs, .. } => {
                    assert!(attrs
                        .iter()
                        .any(|(k, v)| k == "class" && v == "cm-widgetBuffer"));
                    assert!(attrs.iter().any(|(k, v)| k == "data-tile-len" && v == "0"));
                }
                _ => panic!("expected buffer img"),
            }
        }
    }

    #[test]
    fn empty_line_emits_br() {
        let (arena, root) = build_tiles("", &[]);
        let patch = build_patch(&arena, root);
        assert_eq!(patch.len(), 1);
        match &patch[0] {
            Patch::Element { tag, kids, .. } => {
                assert_eq!(tag, "div");
                assert_eq!(kids.len(), 1);
                match &kids[0] {
                    Patch::Element { tag, .. } => assert_eq!(tag, "br"),
                    _ => panic!("expected br"),
                }
            }
            _ => panic!("expected div"),
        }
    }

    #[test]
    fn mark_with_class() {
        let decs = vec![Decoration::mark(0..3, "md-bold")];
        let (arena, root) = build_tiles("hi!", &decs);
        let patch = build_patch(&arena, root);
        // Walk: Line > Mark > Text
        let line = &patch[0];
        if let Patch::Element { kids, .. } = line {
            if let Patch::Element { tag, attrs, .. } = &kids[0] {
                assert_eq!(tag, "span");
                let class = attrs.iter().find(|(k, _)| k == "class").unwrap();
                assert_eq!(class.1, "md-bold");
            } else {
                panic!("expected mark span");
            }
        }
    }

    #[test]
    fn line_decoration_extends_class_attr() {
        let decs = vec![Decoration::line(0, "md-h1")];
        let (arena, root) = build_tiles("# title", &decs);
        let patch = build_patch(&arena, root);
        let Patch::Element { attrs, .. } = &patch[0] else {
            panic!("expected line div");
        };
        let class = attrs.iter().find(|(k, _)| k == "class").unwrap();
        assert_eq!(class.1, "cm-line md-h1");
    }

    #[test]
    fn newline_splits_into_two_lines() {
        let (arena, root) = build_tiles("ab\ncd", &[]);
        let patch = build_patch(&arena, root);
        assert_eq!(patch.len(), 2);
    }

    #[test]
    fn json_round_trip() {
        // Sanity that the serde derives produce something
        // legible. The JS patcher consumes this verbatim.
        let (arena, root) = build_tiles("hi", &[]);
        let patch = build_patch(&arena, root);
        let json = serde_json::to_string(&patch).unwrap();
        // Should mention both line and span structures.
        assert!(json.contains("\"tag\":\"div\""));
        assert!(json.contains("\"tag\":\"span\""));
        assert!(json.contains("\"text\":\"hi\""));
    }
}
