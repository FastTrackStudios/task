//! Transactions — the only way to mutate an [`EditorState`].
//!
//! A transaction describes *what* should change (text edits and
//! optionally a new selection) and is then applied to produce a
//! new state. Mirrors `@codemirror/state`'s `Transaction` /
//! `TransactionSpec`. See `~/Development/research/codemirror/state/src/transaction.ts`.

use crate::change::Changes;
use crate::selection::Selection;
use crate::state::EditorState;

#[derive(Clone, Debug, Default)]
pub struct TransactionSpec {
    pub changes: Changes,
    pub selection: Option<Selection>,
    pub user_event: Option<String>,
    pub annotations: Vec<(String, String)>,
    /// Replace `state.folds` wholesale. `None` falls through to
    /// "map existing folds through the change set".
    pub folds: Option<Vec<std::ops::Range<usize>>>,
    /// Set `state.reading_mode` to this value. `None` preserves.
    pub reading_mode: Option<bool>,
}

impl TransactionSpec {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn changes(mut self, changes: Changes) -> Self {
        self.changes = changes;
        self
    }

    #[must_use]
    pub fn selection(mut self, sel: Selection) -> Self {
        self.selection = Some(sel);
        self
    }

    #[must_use]
    pub fn user_event(mut self, name: impl Into<String>) -> Self {
        self.user_event = Some(name.into());
        self
    }

    #[must_use]
    pub fn annotate(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.annotations.push((key.into(), value.into()));
        self
    }

    #[must_use]
    pub fn folds(mut self, folds: Vec<std::ops::Range<usize>>) -> Self {
        self.folds = Some(folds);
        self
    }

    #[must_use]
    pub fn reading_mode(mut self, on: bool) -> Self {
        self.reading_mode = Some(on);
        self
    }
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub before: EditorState,
    pub spec: TransactionSpec,
}

impl Transaction {
    #[must_use]
    pub fn new(before: EditorState, spec: TransactionSpec) -> Self {
        Self { before, spec }
    }

    #[must_use]
    pub fn apply(&self) -> EditorState {
        let new_doc = self.spec.changes.apply(&self.before.doc);
        let new_selection = self
            .spec
            .selection
            .clone()
            .unwrap_or_else(|| self.before.selection.map(&self.spec.changes));
        let new_doc_len = new_doc.len();
        let new_folds = if let Some(folds) = self.spec.folds.clone() {
            folds
                .into_iter()
                .filter(|r| r.end <= new_doc_len && r.start < r.end)
                .collect()
        } else {
            self.before
                .folds
                .iter()
                .filter_map(|r| {
                    let from = self
                        .spec
                        .changes
                        .map_position(r.start, crate::change::Assoc::Before)
                        .min(new_doc_len);
                    let to = self
                        .spec
                        .changes
                        .map_position(r.end, crate::change::Assoc::After)
                        .min(new_doc_len);
                    if to > from { Some(from..to) } else { None }
                })
                .collect()
        };
        let new_reading_mode = self.spec.reading_mode.unwrap_or(self.before.reading_mode);
        EditorState {
            doc: new_doc,
            selection: new_selection,
            folds: new_folds,
            reading_mode: new_reading_mode,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;
    use crate::selection::{Range, Selection};

    fn st(text: &str, head: usize) -> EditorState {
        EditorState {
            doc: Doc::from_str(text),
            selection: Selection::caret(head),
            folds: Vec::new(),
            reading_mode: false,
        }
    }

    #[test]
    fn insert_advances_caret() {
        let state = st("hello", 5);
        let tr = Transaction::new(
            state,
            TransactionSpec::new().changes(Changes::insert(5, " world")),
        );
        let next = tr.apply();
        assert_eq!(next.doc.to_string(), "hello world");
        assert_eq!(next.selection.primary(), Range::caret(11));
    }

    #[test]
    fn explicit_selection_overrides_mapping() {
        let state = st("hello", 5);
        let tr = Transaction::new(
            state,
            TransactionSpec::new()
                .changes(Changes::insert(0, "X"))
                .selection(Selection::caret(0)),
        );
        let next = tr.apply();
        assert_eq!(next.doc.to_string(), "Xhello");
        assert_eq!(next.selection.primary(), Range::caret(0));
    }

    #[test]
    fn reading_mode_can_be_toggled() {
        let state = st("hi", 0);
        let next = state.update(TransactionSpec::new().reading_mode(true));
        assert!(next.reading_mode);
        assert!(!state.reading_mode);
    }
}
