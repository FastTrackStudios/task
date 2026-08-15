//! Transaction sink — the host-facing event emitted for every
//! transaction the editor applies *itself*.
//!
//! The `<Editor>` component owns a `Signal<EditorState>` it mutates
//! from many internal paths: the JS bridge (typing, IME, property
//! widgets, task toggles), the keymap, vim, and the slash palette.
//! [`apply_tx`] is the single choke point all of those route through.
//! When the host sets the optional `on_transaction` prop, every
//! applied transaction is mirrored out as a [`TransactionEvent`].
//!
//! Designed for a CRDT / persistence host:
//!
//! - `changes` carries the byte-offset edit set — feed it (plus
//!   `doc_before` for byte→scalar translation) to an adapter such as
//!   `editor-crdt::changes_to_text_ops` to produce CRDT text ops.
//! - `user_event` carries the spec's tag. Convention: a host applying
//!   *remote* ops tags its spec with `.user_event("remote")` so any
//!   event that does leak back can be echo-guarded via
//!   [`TransactionEvent::is_remote`]. (Host-driven `state.set` calls
//!   bypass `apply_tx` entirely and never emit — the tag is a second
//!   line of defense.)
//! - selection-only transactions (caret moves) emit too, with an
//!   empty `changes` — useful for presence cursors; dirty-tracking
//!   hosts filter with [`TransactionEvent::is_edit`].

use dioxus::prelude::*;
use editor_state::{Changes, Doc, EditorState, TransactionSpec};

/// One applied editor transaction, mirrored to the host.
///
/// `Doc` is a rope wrapper whose clone is O(1), so carrying both
/// sides of the edit is cheap. `doc_before` is the document the
/// `changes` offsets are valid against; `doc_after` is the result.
#[derive(Clone, Debug)]
pub struct TransactionEvent {
    /// The applied edit set — byte offsets into `doc_before`.
    /// Empty for selection-only transactions.
    pub changes: Changes,
    /// The spec's `user_event` tag, if any. `Some("remote")` marks a
    /// host-applied remote transaction (echo-guard convention).
    pub user_event: Option<String>,
    /// The spec's annotations (e.g. `("origin", "input")`).
    pub annotations: Vec<(String, String)>,
    /// Document snapshot the changes apply to (pre-transaction).
    pub doc_before: Doc,
    /// Document snapshot after the transaction.
    pub doc_after: Doc,
}

impl TransactionEvent {
    /// `true` when this transaction was tagged as a remote
    /// (host-applied) edit via `.user_event("remote")` — a CRDT host
    /// must not echo these back into its document.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        self.user_event.as_deref() == Some("remote")
    }

    /// `true` when the transaction edited the document (as opposed
    /// to a selection-only / mode-only transaction).
    #[must_use]
    pub fn is_edit(&self) -> bool {
        !self.changes.is_empty()
    }

    /// The `origin` annotation, when present (`"input"`, `"slash"`,
    /// `"task-toggle"`, …).
    #[must_use]
    pub fn origin(&self) -> Option<&str> {
        self.annotations
            .iter()
            .find(|(k, _)| k == "origin")
            .map(|(_, v)| v.as_str())
    }
}

/// The single choke point for every *internal* state mutation.
///
/// Applies `spec` to `cur`, writes the result into the state signal,
/// and — when the host registered an `on_transaction` sink — emits a
/// [`TransactionEvent`] describing what happened.
///
/// Host-driven mutations (the host calling `state.set(...)` from
/// outside the editor, e.g. to apply a remote CRDT op or load a file)
/// do NOT go through here and therefore never emit: the sink only
/// reports edits the editor itself originated.
pub(crate) fn apply_tx(
    mut state: Signal<EditorState>,
    cur: &EditorState,
    spec: TransactionSpec,
    sink: Option<Callback<TransactionEvent>>,
) {
    let mut spec = spec;
    // History integration. The `<Editor>` component provides a
    // `Signal<History>` via context; every edit is recorded, and
    // the empty `"undo"`/`"redo"`-tagged specs (emitted by vim's
    // `u`/`Ctrl-r` or a host keymap) are swapped for the real
    // inverse transaction from the history. Missing context (unit
    // tests, headless callers) degrades to no history.
    if let Some(mut hist) = try_consume_context::<Signal<editor_state::History>>() {
        match spec.user_event.as_deref() {
            Some("undo") if spec.changes.is_empty() => {
                let Some(inverse) = hist.write().undo(cur) else {
                    return;
                };
                spec = inverse;
            }
            Some("redo") if spec.changes.is_empty() => {
                let Some(inverse) = hist.write().redo(cur) else {
                    return;
                };
                spec = inverse;
            }
            _ => {}
        }
        // `record` self-ignores undo/redo-tagged specs, so feeding
        // the swapped spec back through is safe.
        hist.write().record(cur, &spec);
    }
    let Some(cb) = sink else {
        // No sink registered — plain apply, no event bookkeeping.
        state.set(cur.update(spec));
        return;
    };
    let next = cur.update(spec.clone());
    let event = TransactionEvent {
        changes: spec.changes,
        user_event: spec.user_event,
        annotations: spec.annotations,
        doc_before: cur.doc.clone(),
        doc_after: next.doc.clone(),
    };
    state.set(next);
    cb.call(event);
}

/// Apply a transaction to `state` and notify `sink` — the public entry
/// point for edits originating **outside** the `<Editor>` subtree (e.g.
/// a properties panel in a side sidebar that mutates the same document).
///
/// Reads the current state, then runs the same [`apply_tx`] path the
/// in-editor bridge uses, so the edit reaches the CRDT host through
/// `sink` (the editor's `on_transaction`) identically. History
/// integration only engages when a `Signal<History>` context is in
/// scope (it is inside `<Editor>`, not in a sibling sidebar), so
/// out-of-tree edits simply aren't added to the editor's undo stack —
/// they degrade gracefully, exactly as headless callers do.
pub fn dispatch_spec(
    state: Signal<EditorState>,
    spec: TransactionSpec,
    sink: Option<Callback<TransactionEvent>>,
) {
    let cur = state.read().clone();
    apply_tx(state, &cur, spec, sink);
}
