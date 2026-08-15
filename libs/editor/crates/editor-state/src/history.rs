//! Undo/redo history — records every doc-changing transaction as
//! an *inverted* change set plus the selection it started from,
//! so undoing restores both the text and the caret.
//!
//! Mirrors `@codemirror/history` (now `historyField` in
//! `@codemirror/commands`). See
//! `~/Development/research/codemirror/commands/src/history.ts`.
//! Differences from CM6:
//!
//! - No wall-clock `newGroupDelay` — grouping is purely
//!   structural. Consecutive single-run typing edits (origin
//!   annotation `"input"` / `"before-input"`) whose regions touch
//!   coalesce into one undo group; a selection jump, a different
//!   origin, or a non-adjacent edit starts a new group.
//! - Instead of a general `ChangeSet.compose`, coalescing tracks
//!   the group's touched region in current-doc and original-doc
//!   coordinates and rebuilds the inverted change from the
//!   original doc (cheap — [`Doc`] is a reference-counted rope).
//!
//! Wiring contract: call [`History::record`] for **every**
//! transaction applied to the state (it skips what it must —
//! selection-only specs, and undo/redo replays tagged with
//! `user_event("undo"/"redo")` or `annotate("vim", "undo"/"redo")`).
//! [`History::undo`] / [`History::redo`] return a
//! [`TransactionSpec`] to apply, already tagged
//! `.user_event("undo")` / `.user_event("redo")` so feeding it
//! back through `record` is a no-op.

use crate::change::{Assoc, Change, Changes};
use crate::doc::Doc;
use crate::selection::Selection;
use crate::state::EditorState;
use crate::transaction::TransactionSpec;

/// Maximum number of undo groups kept. Oldest groups are dropped
/// once the cap is hit (CM6's `minDepth` is 100; we keep a bit
/// more since groups are cheap rope handles).
pub const DEFAULT_MAX_DEPTH: usize = 200;

/// While a group is still "open" for coalescing, we track the
/// edited region in both coordinate spaces plus the doc the group
/// started from, so each absorbed keystroke can rebuild the
/// group's single inverted change exactly.
///
/// Invariant: `cur_doc[..from_cur] == doc_before[..from_orig]`
/// and `cur_doc[to_cur..] == doc_before[to_orig..]` — everything
/// outside the tracked region is untouched by the group.
#[derive(Clone, Debug)]
struct Coalesce {
    /// Doc as it was before the first edit of the group.
    doc_before: Doc,
    /// Touched region in the *current* doc (after all edits so far).
    from_cur: usize,
    to_cur: usize,
    /// Corresponding region in `doc_before`.
    from_orig: usize,
    to_orig: usize,
}

/// One undo group: applying `inverted` to the current doc yields
/// the doc before the group, and `selection_before` is where the
/// caret was, so undo restores both.
#[derive(Clone, Debug)]
struct Group {
    /// Inverted changes — valid against the doc *after* the group.
    inverted: Changes,
    /// Selection before the first transaction of the group.
    selection_before: Selection,
    /// `Some` while the group can still absorb adjacent typing.
    /// Cleared ("sealed") by selection jumps, non-typing edits,
    /// and undo/redo round-trips.
    coalesce: Option<Coalesce>,
}

/// Undo/redo stacks. Owned by whoever drives the editor (the view
/// layer) — the state itself stays a pure value.
#[derive(Clone, Debug)]
pub struct History {
    /// Undo stack — most recent group last.
    done: Vec<Group>,
    /// Redo stack — populated by `undo`, cleared by any new edit.
    undone: Vec<Group>,
    max_depth: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_depth(DEFAULT_MAX_DEPTH)
    }

    /// History with a custom group cap (mainly for tests).
    #[must_use]
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self {
            done: Vec::new(),
            undone: Vec::new(),
            max_depth: max_depth.max(1),
        }
    }

    /// `true` when there is something to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    /// `true` when there is something to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// Observe a transaction about to be applied to `before`.
    /// Call this for every transaction the editor applies:
    ///
    /// - Undo/redo replays (`user_event` `"undo"`/`"redo"`, or a
    ///   `("vim", "undo"/"redo")` annotation) are ignored — they
    ///   are managed by [`Self::undo`] / [`Self::redo`] directly.
    /// - Selection-only transactions are not recorded, but they
    ///   *seal* the top group so a caret jump ends a typing burst.
    /// - Everything else (including `"remote"` edits) records an
    ///   undo group and clears the redo stack.
    pub fn record(&mut self, before: &EditorState, spec: &TransactionSpec) {
        if is_undo_redo(spec) {
            return;
        }
        if spec.changes.is_empty() {
            // Selection-only movement: a caret jump means the next
            // keystroke should start a fresh undo group.
            if spec.selection.is_some() {
                if let Some(top) = self.done.last_mut() {
                    top.coalesce = None;
                }
            }
            return;
        }

        // A real edit invalidates any redo future.
        self.undone.clear();

        if is_typing(spec) && self.try_coalesce(spec) {
            return;
        }

        // Any edit that opens a new group seals the previous one —
        // its [`Coalesce`] offsets describe an older doc and must
        // never absorb typing across an intervening edit.
        if let Some(top) = self.done.last_mut() {
            top.coalesce = None;
        }
        let coalesce = if is_typing(spec) {
            single_change(&spec.changes).map(|c| Coalesce {
                doc_before: before.doc.clone(),
                from_cur: c.from,
                to_cur: c.from + c.inserted.len(),
                from_orig: c.from,
                to_orig: c.to,
            })
        } else {
            None
        };
        self.done.push(Group {
            inverted: spec.changes.invert(&before.doc),
            selection_before: before.selection.clone(),
            coalesce,
        });
        if self.done.len() > self.max_depth {
            let overflow = self.done.len() - self.max_depth;
            self.done.drain(..overflow);
        }
    }

    /// Pop the most recent undo group and return the transaction
    /// spec that reverts it (changes + the selection the group
    /// started from), tagged `.user_event("undo")`. The reverted
    /// group moves to the redo stack. `state` must be the state
    /// the spec will be applied to — i.e. the state produced by
    /// the last recorded transaction.
    pub fn undo(&mut self, state: &EditorState) -> Option<TransactionSpec> {
        let group = self.done.pop()?;
        // The redo entry inverts the undo itself: applying
        // `group.inverted` to `state.doc` is the undo, so its
        // inverse (against the current doc) re-does the edit. The
        // pre-undo selection is what redo should restore.
        self.undone.push(Group {
            inverted: group.inverted.invert(&state.doc),
            selection_before: state.selection.clone(),
            coalesce: None,
        });
        Some(
            TransactionSpec::new()
                .changes(group.inverted)
                .selection(group.selection_before)
                .user_event("undo"),
        )
    }

    /// Pop the most recent redo group and return the spec that
    /// re-applies it, tagged `.user_event("redo")`. The group
    /// moves back to the undo stack (sealed — redone typing does
    /// not resume coalescing).
    pub fn redo(&mut self, state: &EditorState) -> Option<TransactionSpec> {
        let group = self.undone.pop()?;
        self.done.push(Group {
            inverted: group.inverted.invert(&state.doc),
            selection_before: state.selection.clone(),
            coalesce: None,
        });
        Some(
            TransactionSpec::new()
                .changes(group.inverted)
                .selection(group.selection_before)
                .user_event("redo"),
        )
    }

    /// Try to absorb a typing edit into the top group. Returns
    /// `true` on success. Mirrors CM6's `isAdjacent` merge: the
    /// new (single) change must touch the group's tracked region.
    fn try_coalesce(&mut self, spec: &TransactionSpec) -> bool {
        let Some(top) = self.done.last_mut() else {
            return false;
        };
        let Some(co) = top.coalesce.as_mut() else {
            return false;
        };
        let Some(c) = single_change(&spec.changes) else {
            return false;
        };
        // Adjacency: the edit overlaps or touches the region the
        // group already owns. Anything else is a caret jump.
        if c.from > co.to_cur || c.to < co.from_cur {
            return false;
        }

        // Grow the original-doc region by however far the edit
        // reaches *outside* the current region — those bytes are
        // untouched, so they line up 1:1 with `doc_before` (see
        // the invariant on [`Coalesce`]).
        co.from_orig -= co.from_cur.saturating_sub(c.from);
        co.to_orig += c.to.saturating_sub(co.to_cur);
        // Map the current-doc region through the new change and
        // union it with the change's own footprint.
        co.from_cur = spec
            .changes
            .map_position(co.from_cur, Assoc::Before)
            .min(c.from);
        co.to_cur = spec
            .changes
            .map_position(co.to_cur, Assoc::After)
            .max(c.from + c.inserted.len());

        // Rebuild the group's single inverted change from scratch:
        // restore the original bytes over the current region.
        top.inverted = Changes::replace(
            co.from_cur..co.to_cur,
            co.doc_before.slice(co.from_orig..co.to_orig),
        );
        true
    }
}

/// `true` for transactions that *are* an undo/redo replay and so
/// must not be re-recorded as regular edits.
fn is_undo_redo(spec: &TransactionSpec) -> bool {
    if matches!(spec.user_event.as_deref(), Some("undo" | "redo")) {
        return true;
    }
    spec.annotations
        .iter()
        .any(|(k, v)| k == "vim" && (v == "undo" || v == "redo"))
}

/// `true` for edits that came from typing — the only origins that
/// coalesce. Matches the `origin` annotation the view layer sets
/// on `beforeinput`/`input`-driven transactions.
fn is_typing(spec: &TransactionSpec) -> bool {
    spec.annotations
        .iter()
        .any(|(k, v)| k == "origin" && (v == "input" || v == "before-input"))
}

/// The change set's single change, if it has exactly one.
fn single_change(changes: &Changes) -> Option<Change> {
    let mut it = changes.iter();
    let first = it.next()?.clone();
    if it.next().is_some() { None } else { Some(first) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::Range;

    fn st(text: &str, head: usize) -> EditorState {
        EditorState {
            doc: Doc::from_str(text),
            selection: Selection::caret(head),
            folds: Vec::new(),
            reading_mode: false,
        }
    }

    /// Record + apply in one step, like the view layer would.
    fn edit(hist: &mut History, state: &EditorState, spec: TransactionSpec) -> EditorState {
        hist.record(state, &spec);
        state.update(spec)
    }

    /// Type `text` at `at` as an `origin: "input"` transaction.
    fn type_at(hist: &mut History, state: &EditorState, at: usize, text: &str) -> EditorState {
        edit(
            hist,
            state,
            TransactionSpec::new()
                .changes(Changes::insert(at, text))
                .annotate("origin", "input"),
        )
    }

    fn apply_undo(hist: &mut History, state: &EditorState) -> EditorState {
        let spec = hist.undo(state).expect("undo available");
        hist.record(state, &spec); // replay must be a no-op
        state.update(spec)
    }

    fn apply_redo(hist: &mut History, state: &EditorState) -> EditorState {
        let spec = hist.redo(state).expect("redo available");
        hist.record(state, &spec);
        state.update(spec)
    }

    #[test]
    fn undo_insert_restores_text_and_caret() {
        let mut h = History::new();
        let s0 = st("hello", 5);
        let s1 = edit(
            &mut h,
            &s0,
            TransactionSpec::new().changes(Changes::insert(5, " world")),
        );
        assert_eq!(s1.doc.to_string(), "hello world");
        let s2 = apply_undo(&mut h, &s1);
        assert_eq!(s2.doc.to_string(), "hello");
        assert_eq!(s2.selection.primary(), Range::caret(5));
        assert!(!h.can_undo());
    }

    #[test]
    fn undo_delete_restores_text_and_caret() {
        let mut h = History::new();
        let s0 = st("hello world", 11);
        let s1 = edit(
            &mut h,
            &s0,
            TransactionSpec::new()
                .changes(Changes::delete(5..11))
                .selection(Selection::caret(5)),
        );
        assert_eq!(s1.doc.to_string(), "hello");
        let s2 = apply_undo(&mut h, &s1);
        assert_eq!(s2.doc.to_string(), "hello world");
        assert_eq!(s2.selection.primary(), Range::caret(11));
    }

    #[test]
    fn undo_replace_restores_text() {
        let mut h = History::new();
        let s0 = st("hello world", 6);
        let s1 = edit(
            &mut h,
            &s0,
            TransactionSpec::new().changes(Changes::replace(6..11, "rust")),
        );
        assert_eq!(s1.doc.to_string(), "hello rust");
        let s2 = apply_undo(&mut h, &s1);
        assert_eq!(s2.doc.to_string(), "hello world");
    }

    #[test]
    fn redo_reapplies_and_restores_pre_undo_selection() {
        let mut h = History::new();
        let s0 = st("hello", 5);
        let s1 = edit(
            &mut h,
            &s0,
            TransactionSpec::new().changes(Changes::insert(5, "!")),
        );
        assert_eq!(s1.selection.primary(), Range::caret(6));
        let s2 = apply_undo(&mut h, &s1);
        assert_eq!(s2.doc.to_string(), "hello");
        let s3 = apply_redo(&mut h, &s2);
        assert_eq!(s3.doc.to_string(), "hello!");
        assert_eq!(s3.selection.primary(), Range::caret(6));
        // And undo works again after redo.
        let s4 = apply_undo(&mut h, &s3);
        assert_eq!(s4.doc.to_string(), "hello");
    }

    #[test]
    fn typing_burst_coalesces_into_one_group() {
        let mut h = History::new();
        let s0 = st("", 0);
        let s1 = type_at(&mut h, &s0, 0, "h");
        let s2 = type_at(&mut h, &s1, 1, "i");
        let s3 = type_at(&mut h, &s2, 2, "!");
        assert_eq!(s3.doc.to_string(), "hi!");
        let s4 = apply_undo(&mut h, &s3);
        assert_eq!(s4.doc.to_string(), "");
        assert_eq!(s4.selection.primary(), Range::caret(0));
        assert!(!h.can_undo());
        // Redo brings the whole burst back at once.
        let s5 = apply_redo(&mut h, &s4);
        assert_eq!(s5.doc.to_string(), "hi!");
    }

    #[test]
    fn backspace_coalesces_with_typing() {
        let mut h = History::new();
        let s0 = st("", 0);
        let s1 = type_at(&mut h, &s0, 0, "ab");
        // Backspace the "b" — origin "before-input", adjacent.
        let s2 = edit(
            &mut h,
            &s1,
            TransactionSpec::new()
                .changes(Changes::delete(1..2))
                .annotate("origin", "before-input"),
        );
        assert_eq!(s2.doc.to_string(), "a");
        let s3 = apply_undo(&mut h, &s2);
        assert_eq!(s3.doc.to_string(), "");
        assert!(!h.can_undo());
    }

    #[test]
    fn non_adjacent_typing_starts_new_group() {
        let mut h = History::new();
        let s0 = st("ab", 0);
        let s1 = type_at(&mut h, &s0, 0, "X"); // "Xab"
        let s2 = type_at(&mut h, &s1, 3, "Y"); // "XabY" — not touching 0..1
        assert_eq!(s2.doc.to_string(), "XabY");
        let s3 = apply_undo(&mut h, &s2);
        assert_eq!(s3.doc.to_string(), "Xab");
        let s4 = apply_undo(&mut h, &s3);
        assert_eq!(s4.doc.to_string(), "ab");
    }

    #[test]
    fn selection_jump_seals_the_group() {
        let mut h = History::new();
        let s0 = st("abc", 3);
        let s1 = type_at(&mut h, &s0, 3, "d"); // "abcd"
        // Caret jump (selection-only transaction) — not recorded,
        // but ends the typing burst.
        let jump = TransactionSpec::new().selection(Selection::caret(0));
        h.record(&s1, &jump);
        let s2 = s1.update(jump);
        // Jump back and type at the same spot: adjacency would
        // have coalesced this without the seal.
        let back = TransactionSpec::new().selection(Selection::caret(4));
        h.record(&s2, &back);
        let s3 = s2.update(back);
        let s4 = type_at(&mut h, &s3, 4, "e"); // "abcde"
        let s5 = apply_undo(&mut h, &s4);
        assert_eq!(s5.doc.to_string(), "abcd");
        let s6 = apply_undo(&mut h, &s5);
        assert_eq!(s6.doc.to_string(), "abc");
    }

    #[test]
    fn different_origin_starts_new_group() {
        let mut h = History::new();
        let s0 = st("", 0);
        let s1 = type_at(&mut h, &s0, 0, "ab");
        // A completion insert right at the caret — adjacent, but
        // not a typing origin, so it gets its own group.
        let s2 = edit(
            &mut h,
            &s1,
            TransactionSpec::new()
                .changes(Changes::insert(2, "cdef"))
                .annotate("origin", "completion"),
        );
        assert_eq!(s2.doc.to_string(), "abcdef");
        let s3 = apply_undo(&mut h, &s2);
        assert_eq!(s3.doc.to_string(), "ab");
        // ...and typing after it doesn't merge backwards either.
        let s4 = type_at(&mut h, &s3, 2, "x");
        let s5 = apply_undo(&mut h, &s4);
        assert_eq!(s5.doc.to_string(), "ab");
        let s6 = apply_undo(&mut h, &s5);
        assert_eq!(s6.doc.to_string(), "");
    }

    #[test]
    fn new_edit_clears_redo_stack() {
        let mut h = History::new();
        let s0 = st("a", 1);
        let s1 = edit(
            &mut h,
            &s0,
            TransactionSpec::new().changes(Changes::insert(1, "b")),
        );
        let s2 = apply_undo(&mut h, &s1);
        assert!(h.can_redo());
        let _s3 = edit(
            &mut h,
            &s2,
            TransactionSpec::new().changes(Changes::insert(1, "c")),
        );
        assert!(!h.can_redo());
        assert!(h.redo(&_s3).is_none());
    }

    #[test]
    fn multi_change_set_undoes_atomically() {
        let mut h = History::new();
        let s0 = st("hello world", 0);
        let s1 = edit(
            &mut h,
            &s0,
            TransactionSpec::new().changes(Changes::from_sorted(vec![
                Change::replace(0..5, "HELLO"),
                Change::replace(6..11, "WORLD"),
            ])),
        );
        assert_eq!(s1.doc.to_string(), "HELLO WORLD");
        let s2 = apply_undo(&mut h, &s1);
        assert_eq!(s2.doc.to_string(), "hello world");
        let s3 = apply_redo(&mut h, &s2);
        assert_eq!(s3.doc.to_string(), "HELLO WORLD");
    }

    #[test]
    fn utf8_typing_burst_round_trips() {
        let mut h = History::new();
        let s0 = st("naïve — héllo", 0);
        // Type multibyte text at the front, one "keystroke" at a
        // time ("é" is 2 bytes, "日" is 3).
        let s1 = type_at(&mut h, &s0, 0, "é");
        let s2 = type_at(&mut h, &s1, 2, "日");
        assert_eq!(s2.doc.to_string(), "é日naïve — héllo");
        let s3 = apply_undo(&mut h, &s2);
        assert_eq!(s3.doc.to_string(), "naïve — héllo");
        let s4 = apply_redo(&mut h, &s3);
        assert_eq!(s4.doc.to_string(), "é日naïve — héllo");
    }

    #[test]
    fn depth_cap_drops_oldest_groups() {
        let mut h = History::with_max_depth(3);
        let mut s = st("", 0);
        // Five separate (non-typing) edits → five groups, capped at 3.
        for i in 0..5 {
            s = edit(
                &mut h,
                &s,
                TransactionSpec::new().changes(Changes::insert(s.doc.len(), format!("{i}"))),
            );
        }
        assert_eq!(s.doc.to_string(), "01234");
        for expect in ["0123", "012", "01"] {
            s = apply_undo(&mut h, &s);
            assert_eq!(s.doc.to_string(), expect);
        }
        assert!(!h.can_undo());
        assert!(h.undo(&s).is_none());
    }

    #[test]
    fn undo_redo_replays_are_not_recorded() {
        let mut h = History::new();
        let s0 = st("x", 1);
        // Specs tagged as undo/redo — vim's u / Ctrl-r style —
        // must never create undo groups.
        h.record(
            &s0,
            &TransactionSpec::new()
                .changes(Changes::insert(1, "y"))
                .annotate("vim", "undo")
                .user_event("undo"),
        );
        h.record(
            &s0,
            &TransactionSpec::new()
                .changes(Changes::insert(1, "y"))
                .user_event("redo"),
        );
        assert!(!h.can_undo());
    }

    #[test]
    fn remote_edits_are_recorded_normally() {
        let mut h = History::new();
        let s0 = st("local", 0);
        let s1 = edit(
            &mut h,
            &s0,
            TransactionSpec::new()
                .changes(Changes::insert(5, " remote"))
                .user_event("remote"),
        );
        assert_eq!(s1.doc.to_string(), "local remote");
        let s2 = apply_undo(&mut h, &s1);
        assert_eq!(s2.doc.to_string(), "local");
    }

    #[test]
    fn typing_over_a_selection_coalesces_replacement() {
        let mut h = History::new();
        let s0 = st("hello world", 0);
        // Select "hello" and type over it, then keep typing.
        let s1 = edit(
            &mut h,
            &s0,
            TransactionSpec::new()
                .changes(Changes::replace(0..5, "H"))
                .annotate("origin", "before-input"),
        );
        assert_eq!(s1.doc.to_string(), "H world");
        let s2 = type_at(&mut h, &s1, 1, "i");
        assert_eq!(s2.doc.to_string(), "Hi world");
        let s3 = apply_undo(&mut h, &s2);
        assert_eq!(s3.doc.to_string(), "hello world");
        assert!(!h.can_undo());
    }
}
