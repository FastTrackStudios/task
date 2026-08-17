//! Diagnostics — the consumer side of `textDocument/publishDiagnostics`.
//!
//! The server pushes diagnostics with UTF-16 line/character ranges
//! and (when it supports `versionSupport`) the document version they
//! were computed against. This module turns them into plain
//! byte-range data the host can hand to the view, and guards against
//! the classic staleness race: the user types, we send `didChange`
//! v5, and a beat later diagnostics for v4 arrive — positioned
//! against text that no longer exists. Those are dropped (see
//! [`DiagnosticsStore::apply`]); versionless publishes are accepted
//! on faith, as the spec allows.
//!
//! Between an edit and the next (fresh) publish, stored diagnostics
//! are kept visually anchored by mapping their byte ranges through
//! the local change set ([`DiagnosticsStore::map_through`]) — the
//! same position-mapping machinery decorations use, mirroring how
//! CM6's lint package maps its ranges forward per transaction.

use std::collections::HashMap;

use editor_state::change::Assoc;
use editor_state::{Changes, DecoratedRange, Decoration, Doc};
use lsp_types::{NumberOrString, Uri};

use crate::pos::range_to_byte_range;

/// A raw `publishDiagnostics` payload, positions still in LSP
/// line/character form. The client surfaces this (rather than
/// resolved byte offsets) because resolution needs the *current*
/// document, which only the host holds.
#[derive(Clone, Debug)]
pub struct PublishedDiagnostics {
    pub uri: Uri,
    /// The document version the server computed against, when the
    /// server supports `versionSupport`.
    pub version: Option<i32>,
    pub diagnostics: Vec<lsp_types::Diagnostic>,
}

/// Severity, ordered most→least severe. Plain enum so hosts don't
/// need lsp-types to consume diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    fn from_lsp(sev: Option<lsp_types::DiagnosticSeverity>) -> Self {
        match sev {
            Some(lsp_types::DiagnosticSeverity::WARNING) => Self::Warning,
            Some(lsp_types::DiagnosticSeverity::INFORMATION) => Self::Information,
            Some(lsp_types::DiagnosticSeverity::HINT) => Self::Hint,
            // Spec: clients should treat missing severity as Error.
            _ => Self::Error,
        }
    }

    /// CSS class the view layer styles (squiggle color etc.).
    #[must_use]
    pub fn css_class(self) -> &'static str {
        match self {
            Self::Error => "cm-lsp-error",
            Self::Warning => "cm-lsp-warning",
            Self::Information => "cm-lsp-info",
            Self::Hint => "cm-lsp-hint",
        }
    }
}

/// One resolved diagnostic: a byte range into the current document
/// plus display data. Pure data — no lsp-types in the host's face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Byte range into the document the diagnostic was resolved
    /// against. `from == to` never occurs — zero-width server ranges
    /// are widened to the following character (or the preceding one
    /// at doc end) so a squiggle has something to sit under, matching
    /// what CM6's lint package renders for empty ranges.
    pub from: usize,
    pub to: usize,
    pub severity: Severity,
    pub message: String,
    /// Producer name (`"rustc"`, `"rust-analyzer"`, …).
    pub source: Option<String>,
    /// Diagnostic code (`"E0308"`, `"unused_variables"`, …).
    pub code: Option<String>,
}

/// Resolve one raw LSP diagnostic against the current document.
#[must_use]
pub fn resolve(doc: &Doc, raw: &lsp_types::Diagnostic) -> Diagnostic {
    let range = range_to_byte_range(doc, raw.range);
    let (mut from, mut to) = (range.start, range.end);
    if from == to {
        // Widen zero-width ranges to one char so they're visible.
        if to < doc.len() {
            to = next_char_boundary(doc, to);
        } else if from > 0 {
            from = prev_char_boundary(doc, from);
        }
    }
    Diagnostic {
        from,
        to,
        severity: Severity::from_lsp(raw.severity),
        message: raw.message.clone(),
        source: raw.source.clone(),
        code: raw.code.as_ref().map(|c| match c {
            NumberOrString::Number(n) => n.to_string(),
            NumberOrString::String(s) => s.clone(),
        }),
    }
}

/// Byte offset of the char boundary after `at` (which must be on a
/// boundary, as `position_to_byte` guarantees).
fn next_char_boundary(doc: &Doc, at: usize) -> usize {
    let rope = doc.rope();
    rope.char_to_byte(rope.byte_to_char(at) + 1)
}

/// Byte offset of the char boundary before `at`.
fn prev_char_boundary(doc: &Doc, at: usize) -> usize {
    let rope = doc.rope();
    rope.char_to_byte(rope.byte_to_char(at) - 1)
}

/// Map resolved diagnostics to view decorations: a [`Decoration::mark`]
/// per diagnostic with the severity class plus a shared
/// `cm-lsp-diagnostic` class, the message carried in a
/// `data-lsp-message` attribute for tooltip/hover use.
#[must_use]
pub fn to_decorations(diagnostics: &[Diagnostic]) -> Vec<DecoratedRange> {
    diagnostics
        .iter()
        .map(|d| {
            Decoration::mark_with_attrs(
                d.from..d.to,
                format!("cm-lsp-diagnostic {}", d.severity.css_class()),
                vec![("data-lsp-message".to_owned(), d.message.clone())],
            )
        })
        .collect()
}

/// Per-document diagnostic state: the latest accepted publish, keyed
/// by URI, with version-based staleness filtering. The host owns one
/// of these next to its editor state and reads
/// [`get`](DiagnosticsStore::get) when building decorations.
#[derive(Debug, Default)]
pub struct DiagnosticsStore {
    docs: HashMap<String, DocDiagnostics>,
}

#[derive(Debug)]
struct DocDiagnostics {
    /// Version of the last accepted publish (`None` = versionless).
    version: Option<i32>,
    items: Vec<Diagnostic>,
}

impl DiagnosticsStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept or reject a publish. Returns `true` when the store was
    /// updated (fresh publish), `false` when the publish was stale
    /// and dropped.
    ///
    /// - `current_version` is the version the *client* last sent via
    ///   `didChange` (see [`crate::client::LspClient::version_of`]).
    ///   A publish versioned older than it was computed against text
    ///   the user has since edited — dropped.
    /// - A publish versioned older than the last accepted publish is
    ///   likewise dropped (out-of-order delivery).
    /// - Versionless publishes are always accepted; the spec doesn't
    ///   require servers to implement `versionSupport`.
    ///
    /// `doc` must be the host's *current* document for the URI — it's
    /// what LSP ranges are resolved against, which is sound precisely
    /// because stale versions were filtered out.
    pub fn apply(
        &mut self,
        published: &PublishedDiagnostics,
        current_version: Option<i32>,
        doc: &Doc,
    ) -> bool {
        let key = published.uri.to_string();
        if let Some(v) = published.version {
            if let Some(cur) = current_version {
                if v < cur {
                    return false; // computed against text we've since edited
                }
            }
            if let Some(prev) = self.docs.get(&key).and_then(|d| d.version) {
                if v < prev {
                    return false; // out-of-order publish
                }
            }
        }
        let items = published
            .diagnostics
            .iter()
            .map(|d| resolve(doc, d))
            .collect();
        self.docs.insert(
            key,
            DocDiagnostics {
                version: published.version,
                items,
            },
        );
        true
    }

    /// The current diagnostics for a document (empty when none).
    #[must_use]
    pub fn get(&self, uri: &Uri) -> &[Diagnostic] {
        self.docs
            .get(&uri.to_string())
            .map_or(&[], |d| d.items.as_slice())
    }

    /// Shift stored byte ranges through a local edit so squiggles
    /// stay visually anchored until the server's next publish lands.
    /// Bias is the *opposite* of default mark inclusivity: text
    /// typed at either edge of a squiggle stays outside it (the
    /// flagged token itself didn't grow), so `from` binds after and
    /// `to` binds before. Ranges fully swallowed by a deletion are
    /// dropped. Call this from the same place the host sends
    /// `didChange`.
    pub fn map_through(&mut self, uri: &Uri, changes: &Changes) {
        let Some(entry) = self.docs.get_mut(&uri.to_string()) else {
            return;
        };
        entry.items.retain_mut(|d| {
            d.from = changes.map_position(d.from, Assoc::After);
            d.to = changes.map_position(d.to, Assoc::Before);
            d.to > d.from
        });
    }

    /// Forget a document (pair with `didClose`).
    pub fn remove(&mut self, uri: &Uri) {
        self.docs.remove(&uri.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};
    use std::str::FromStr;

    fn uri() -> Uri {
        Uri::from_str("file:///tmp/test.rs").unwrap()
    }

    fn raw(start: (u32, u32), end: (u32, u32), message: &str) -> lsp_types::Diagnostic {
        lsp_types::Diagnostic {
            range: Range {
                start: Position {
                    line: start.0,
                    character: start.1,
                },
                end: Position {
                    line: end.0,
                    character: end.1,
                },
            },
            message: message.to_owned(),
            ..Default::default()
        }
    }

    fn published(
        version: Option<i32>,
        diagnostics: Vec<lsp_types::Diagnostic>,
    ) -> PublishedDiagnostics {
        PublishedDiagnostics {
            uri: uri(),
            version,
            diagnostics,
        }
    }

    #[test]
    fn resolve_maps_utf16_range_to_bytes() {
        // Doc with an emoji before the flagged word: "b" spans UTF-16
        // characters 3..4 but bytes 5..6.
        let doc = Doc::from_str("a😀bc");
        let d = resolve(&doc, &raw((0, 3), (0, 4), "bad"));
        assert_eq!((d.from, d.to), (5, 6));
        assert_eq!(d.severity, Severity::Error); // missing severity → Error
        assert_eq!(d.message, "bad");
    }

    #[test]
    fn resolve_multiline_range() {
        let doc = Doc::from_str("fn main() {\n    let x = ;\n}\n");
        let d = resolve(&doc, &raw((1, 4), (1, 9), "syntax"));
        assert_eq!(doc.slice(d.from..d.to), "let x");
    }

    #[test]
    fn resolve_widens_zero_width_range() {
        let doc = Doc::from_str("abc");
        let d = resolve(&doc, &raw((0, 1), (0, 1), "here"));
        assert_eq!((d.from, d.to), (1, 2));
        // Zero-width over a multi-byte char widens by the whole char.
        let doc = Doc::from_str("😀x");
        let d = resolve(&doc, &raw((0, 0), (0, 0), "here"));
        assert_eq!((d.from, d.to), (0, 4));
        // At doc end it widens backwards.
        let doc = Doc::from_str("ab");
        let d = resolve(&doc, &raw((0, 2), (0, 2), "eof"));
        assert_eq!((d.from, d.to), (1, 2));
    }

    #[test]
    fn store_accepts_fresh_and_versionless() {
        let doc = Doc::from_str("hello");
        let mut store = DiagnosticsStore::new();
        assert!(store.apply(
            &published(Some(3), vec![raw((0, 0), (0, 5), "x")]),
            Some(3),
            &doc
        ));
        assert_eq!(store.get(&uri()).len(), 1);
        // Versionless always accepted.
        assert!(store.apply(&published(None, vec![]), Some(9), &doc));
        assert!(store.get(&uri()).is_empty());
    }

    #[test]
    fn store_drops_stale_versions() {
        let doc = Doc::from_str("hello");
        let mut store = DiagnosticsStore::new();
        assert!(store.apply(
            &published(Some(5), vec![raw((0, 0), (0, 5), "v5")]),
            Some(5),
            &doc
        ));
        // Older than the client's current didChange version.
        assert!(!store.apply(&published(Some(4), vec![]), Some(5), &doc));
        // Older than the last accepted publish, even with no newer
        // client version to compare against.
        assert!(!store.apply(&published(Some(4), vec![]), None, &doc));
        assert_eq!(store.get(&uri())[0].message, "v5");
    }

    #[test]
    fn store_map_through_shifts_ranges() {
        let doc = Doc::from_str("let x = 1;");
        let mut store = DiagnosticsStore::new();
        store.apply(
            &published(Some(1), vec![raw((0, 4), (0, 5), "unused")]),
            Some(1),
            &doc,
        );
        assert_eq!((store.get(&uri())[0].from, store.get(&uri())[0].to), (4, 5));
        // Insert "mut " at offset 4 — the squiggle slides right.
        store.map_through(&uri(), &Changes::insert(4, "mut "));
        assert_eq!((store.get(&uri())[0].from, store.get(&uri())[0].to), (8, 9));
        // Delete the whole flagged range — the diagnostic drops.
        store.map_through(&uri(), &Changes::delete(7..10));
        assert!(store.get(&uri()).is_empty());
    }

    #[test]
    fn store_remove_forgets_document() {
        let doc = Doc::from_str("x");
        let mut store = DiagnosticsStore::new();
        store.apply(&published(None, vec![raw((0, 0), (0, 1), "m")]), None, &doc);
        store.remove(&uri());
        assert!(store.get(&uri()).is_empty());
    }

    #[test]
    fn decorations_carry_severity_class_and_message() {
        let diags = vec![
            Diagnostic {
                from: 0,
                to: 3,
                severity: Severity::Error,
                message: "boom".into(),
                source: None,
                code: None,
            },
            Diagnostic {
                from: 5,
                to: 8,
                severity: Severity::Hint,
                message: "meh".into(),
                source: None,
                code: None,
            },
        ];
        let decos = to_decorations(&diags);
        assert_eq!(decos.len(), 2);
        assert_eq!(decos[0].byte_range(), 0..3);
        let editor_state::DecorationKind::Mark { class, attrs } = &decos[0].kind else {
            panic!("expected mark");
        };
        assert_eq!(class, "cm-lsp-diagnostic cm-lsp-error");
        assert_eq!(attrs[0], ("data-lsp-message".to_owned(), "boom".to_owned()));
        let editor_state::DecorationKind::Mark { class, .. } = &decos[1].kind else {
            panic!("expected mark");
        };
        assert_eq!(class, "cm-lsp-diagnostic cm-lsp-hint");
    }

    #[test]
    fn severity_from_lsp_and_ordering() {
        use lsp_types::DiagnosticSeverity as S;
        assert_eq!(Severity::from_lsp(Some(S::ERROR)), Severity::Error);
        assert_eq!(Severity::from_lsp(Some(S::WARNING)), Severity::Warning);
        assert_eq!(
            Severity::from_lsp(Some(S::INFORMATION)),
            Severity::Information
        );
        assert_eq!(Severity::from_lsp(Some(S::HINT)), Severity::Hint);
        assert!(Severity::Error < Severity::Warning);
    }
}
