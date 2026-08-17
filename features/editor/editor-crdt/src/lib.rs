//! Adapter between the editor's byte-offset [`Changes`] and the
//! unicode-scalar-indexed [`TextOp`] stream that architect's crdt
//! lib applies to a `LoroText` container.
//!
//! A CRDT host wires this to the editor's `on_transaction` sink:
//!
//! ```text
//! on_transaction event            remote loro update
//!   │ (skip if event.is_remote())   │
//!   ▼                               ▼
//! changes_to_text_ops(           remote_text_to_changes(
//!   event.doc_before.rope(),       state.doc.rope(),
//!   &event.changes)                &loro_text.to_string())
//!   │                               │
//!   ▼                               ▼
//! crdt::codec::apply_text_ops    state.set(cur.update(spec
//!   (echo-guarded: the editor      .changes(changes)
//!    originated this edit)         .user_event("remote")))
//! ```
//!
//! The `user_event("remote")` tag is the echo-guard convention: a
//! host applying remote ops tags the transaction so that, even if an
//! event for it ever surfaces, `event.is_remote()` filters it before
//! it can loop back into the CRDT.
//!
//! ## Offset spaces
//!
//! The editor addresses the document in **UTF-8 bytes** ([`Changes`]
//! offsets index `EditorState::doc`). Loro's `LoroText` insert/delete
//! API addresses **unicode scalar values** (`char`s). [`ropey`]'s
//! `byte_to_char` does the translation against the *pre-change*
//! document, which is exactly what `TransactionEvent::doc_before`
//! carries.
//!
//! ## Why `TextOp` is mirrored here instead of imported
//!
//! The canonical type is `crdt::codec::TextOp` in the architect repo
//! (`architect/libs/crdt/src/codec.rs`). This workspace builds
//! standalone — sibling repos are reached via codeberg git deps, not
//! `path` (root Cargo.toml documents the same choice for keyflow) —
//! and even a feature-gated or dev-only path dependency is resolved
//! by cargo when the path is missing, which would break every clone
//! without `../architect`. The mirror is kept field-for-field
//! identical; converting at the host is a two-arm `match`:
//!
//! ```ignore
//! fn to_crdt(op: editor_crdt::TextOp) -> crdt::codec::TextOp {
//!     match op {
//!         editor_crdt::TextOp::Insert { pos, text } => crdt::codec::TextOp::Insert { pos, text },
//!         editor_crdt::TextOp::Delete { pos, len } => crdt::codec::TextOp::Delete { pos, len },
//!     }
//! }
//! ```
//!
//! The semantics this adapter targets (and that the tests simulate):
//! `apply_text_ops` runs each op **against the post-state of the
//! previous ops**, positions in scalar units — the editor's
//! stream-of-keystrokes model.

use editor_state::{Change, Changes};
use ropey::Rope;

/// Edit op against a `LoroText` child container. Mirror of
/// `crdt::codec::TextOp` (architect repo,
/// `architect/libs/crdt/src/codec.rs`) — keep the two definitions
/// in lockstep. Positions are unicode scalar units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextOp {
    Insert { pos: u32, text: String },
    Delete { pos: u32, len: u32 },
}

/// Translate an applied editor change set into the `TextOp` stream
/// that reproduces it on a `LoroText` holding the same pre-change
/// content.
///
/// `doc_before` is the document the byte offsets in `changes` are
/// valid against (a host wired to the editor's `on_transaction` sink
/// passes `event.doc_before.rope()`).
///
/// Ops are emitted in **reverse document order** (highest offset
/// first). `apply_text_ops` applies sequentially against the
/// evolving post-state; editing from the back means earlier ops
/// never shift the scalar positions of the (lower-offset) ops still
/// to come, so every emitted position can be computed against the
/// single pre-change snapshot. Within one replacement the delete is
/// emitted before the insert at the same position — also
/// post-state-consistent.
#[must_use]
pub fn changes_to_text_ops(doc_before: &Rope, changes: &Changes) -> Vec<TextOp> {
    let mut ops = Vec::new();
    let collected: Vec<&Change> = changes.iter().collect();
    for c in collected.into_iter().rev() {
        let pos = doc_before.byte_to_char(c.from) as u32;
        let end = doc_before.byte_to_char(c.to) as u32;
        if end > pos {
            ops.push(TextOp::Delete {
                pos,
                len: end - pos,
            });
        }
        if !c.inserted.is_empty() {
            ops.push(TextOp::Insert {
                pos,
                text: c.inserted.clone(),
            });
        }
        // from == to with nothing inserted (the no-op replace shape
        // diff_text produces for identical strings) emits nothing.
    }
    ops
}

/// Diff a remote document state against the editor's current one and
/// return the byte-offset [`Changes`] that brings the editor up to
/// date — feed it into a transaction tagged `.user_event("remote")`.
///
/// Mirrors the editor's `diff_text` semantics: trim the common
/// prefix and suffix (byte-wise, then walked back to UTF-8 char
/// boundaries) and replace the middle. O(n); minimal for typing
/// flows, a single replace for arbitrary remote edits — not optimal,
/// always correct. Unlike `diff_text` it returns [`Changes::empty`]
/// when nothing changed, so hosts can apply unconditionally without
/// triggering no-op transactions.
#[must_use]
pub fn remote_text_to_changes(old: &Rope, new: &str) -> Changes {
    let old = old.to_string();
    if old == new {
        return Changes::empty();
    }
    let ob = old.as_bytes();
    let nb = new.as_bytes();
    let mut start = 0;
    while start < ob.len() && start < nb.len() && ob[start] == nb[start] {
        start += 1;
    }
    let mut o_end = ob.len();
    let mut n_end = nb.len();
    while o_end > start && n_end > start && ob[o_end - 1] == nb[n_end - 1] {
        o_end -= 1;
        n_end -= 1;
    }
    // Walk back to UTF-8 boundaries in case the binary trim landed
    // in the middle of a multi-byte sequence.
    while start > 0 && !old.is_char_boundary(start) {
        start -= 1;
    }
    while o_end < ob.len() && !old.is_char_boundary(o_end) {
        o_end += 1;
    }
    while n_end < nb.len() && !new.is_char_boundary(n_end) {
        n_end += 1;
    }
    Changes::replace(start..o_end, &new[start..n_end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_state::Doc;

    /// Reference simulation of `crdt::codec::apply_text_ops`: each
    /// op runs against the post-state of the previous ops, positions
    /// in unicode scalar units (`LoroText` insert/delete semantics).
    fn apply_ops_scalar(text: &str, ops: &[TextOp]) -> String {
        let mut chars: Vec<char> = text.chars().collect();
        for op in ops {
            match op {
                TextOp::Insert { pos, text } => {
                    let at = *pos as usize;
                    assert!(at <= chars.len(), "insert past end");
                    chars.splice(at..at, text.chars());
                }
                TextOp::Delete { pos, len } => {
                    let from = *pos as usize;
                    let to = from + *len as usize;
                    assert!(to <= chars.len(), "delete past end");
                    chars.splice(from..to, std::iter::empty());
                }
            }
        }
        chars.into_iter().collect()
    }

    /// Apply `changes` both ways — through the editor's own
    /// `Changes::apply` and through the scalar-op simulation — and
    /// assert they converge.
    fn roundtrip(text: &str, changes: &Changes) -> String {
        let doc = Doc::from_str(text);
        let expected = changes.apply(&doc).to_string();
        let ops = changes_to_text_ops(doc.rope(), changes);
        let via_ops = apply_ops_scalar(text, &ops);
        assert_eq!(via_ops, expected, "ops diverged for {text:?} {changes:?}");
        expected
    }

    #[test]
    fn single_insert() {
        assert_eq!(
            roundtrip("hello", &Changes::insert(5, " world")),
            "hello world"
        );
    }

    #[test]
    fn insert_at_start_and_empty_doc() {
        assert_eq!(roundtrip("", &Changes::insert(0, "x")), "x");
        assert_eq!(roundtrip("bc", &Changes::insert(0, "a")), "abc");
    }

    #[test]
    fn single_delete() {
        assert_eq!(roundtrip("hello world", &Changes::delete(5..11)), "hello");
    }

    #[test]
    fn delete_entire_doc() {
        assert_eq!(roundtrip("gone", &Changes::delete(0..4)), "");
    }

    #[test]
    fn replace_middle() {
        assert_eq!(
            roundtrip("hello world", &Changes::replace(6..11, "rust")),
            "hello rust"
        );
    }

    #[test]
    fn noop_replace_emits_no_ops() {
        // `diff_text` produces `replace(n..n, "")` for identical
        // strings — must translate to zero ops.
        let doc = Doc::from_str("same");
        let changes = Changes::replace(4..4, "");
        assert!(changes_to_text_ops(doc.rope(), &changes).is_empty());
    }

    #[test]
    fn multi_change_set_applies_in_reverse() {
        let changes = Changes::from_sorted(vec![
            Change::replace(0..5, "HELLO"),
            Change::insert(6, "big "),
            Change::replace(6..11, "WORLD"),
        ]);
        assert_eq!(roundtrip("hello world", &changes), "HELLO big WORLD");
        // And the emission order really is reverse (highest offset
        // first) — the post-state contract.
        let doc = Doc::from_str("hello world");
        let ops = changes_to_text_ops(doc.rope(), &changes);
        let first_pos = match &ops[0] {
            TextOp::Insert { pos, .. } | TextOp::Delete { pos, .. } => *pos,
        };
        let last_pos = match ops.last().unwrap() {
            TextOp::Insert { pos, .. } | TextOp::Delete { pos, .. } => *pos,
        };
        assert!(first_pos >= last_pos);
    }

    #[test]
    fn unicode_emoji_offsets_translate() {
        // "a🦀b" — bytes: a=0, 🦀=1..5, b=5. Scalars: a=0, 🦀=1, b=2.
        let text = "a🦀b";
        let changes = Changes::insert(5, "X"); // byte 5 == before 'b'
        let doc = Doc::from_str(text);
        let ops = changes_to_text_ops(doc.rope(), &changes);
        assert_eq!(
            ops,
            vec![TextOp::Insert {
                pos: 2,
                text: "X".into()
            }]
        );
        assert_eq!(roundtrip(text, &changes), "a🦀Xb");
    }

    #[test]
    fn unicode_cjk_delete() {
        // "日本語テスト" — 6 scalars, 18 bytes. Delete 日本 (bytes 0..6).
        let text = "日本語テスト";
        let changes = Changes::delete(0..6);
        let doc = Doc::from_str(text);
        let ops = changes_to_text_ops(doc.rope(), &changes);
        assert_eq!(ops, vec![TextOp::Delete { pos: 0, len: 2 }]);
        assert_eq!(roundtrip(text, &changes), "語テスト");
    }

    #[test]
    fn unicode_multi_change_mixed_planes() {
        // ASCII + combining-free accents + emoji + CJK in one doc,
        // three changes in one set.
        let text = "héllo 🦀 世界 end";
        let h_end = "héllo".len(); // é is 2 bytes
        let crab_start = text.find('🦀').unwrap();
        let world_start = text.find('世').unwrap();
        let changes = Changes::from_sorted(vec![
            Change::replace(0..h_end, "HI"),
            Change::delete(crab_start..crab_start + 4),
            Change::insert(world_start, "中"),
        ]);
        roundtrip(text, &changes);
    }

    #[test]
    fn edge_offsets_at_doc_bounds() {
        let text = "🦀";
        // Insert at the very end (byte 4 == scalar 1).
        assert_eq!(roundtrip(text, &Changes::insert(4, "!")), "🦀!");
        // Replace the whole emoji.
        assert_eq!(roundtrip(text, &Changes::replace(0..4, "ok")), "ok");
    }

    #[test]
    fn replacement_text_keeps_unicode() {
        let changes = Changes::replace(0..3, "🦀🦀");
        assert_eq!(roundtrip("abc", &changes), "🦀🦀");
    }

    // ── remote_text_to_changes ────────────────────────────────────

    fn remote_roundtrip(old: &str, new: &str) {
        let doc = Doc::from_str(old);
        let changes = remote_text_to_changes(doc.rope(), new);
        assert_eq!(
            changes.apply(&doc).to_string(),
            new,
            "remote diff diverged for {old:?} -> {new:?}"
        );
    }

    #[test]
    fn remote_identical_is_empty() {
        let doc = Doc::from_str("same");
        assert!(remote_text_to_changes(doc.rope(), "same").is_empty());
    }

    #[test]
    fn remote_append_insert_delete_replace() {
        remote_roundtrip("hello", "helloa");
        remote_roundtrip("hello world", "hello big world");
        remote_roundtrip("helloa", "hello");
        remote_roundtrip("hello world", "hello RUST");
        remote_roundtrip("", "fresh");
        remote_roundtrip("gone", "");
    }

    #[test]
    fn remote_unicode_boundary_trim() {
        // Suffix/prefix trim must not split multi-byte sequences:
        // 🦀 (F0 9F A6 80) vs 🦁 (F0 9F A6 81) share 3 leading bytes.
        remote_roundtrip("a🦀b", "a🦁b");
        remote_roundtrip("日本語", "日本誤");
        remote_roundtrip("héllo", "hello");
    }

    #[test]
    fn remote_overlapping_repeats() {
        remote_roundtrip("aaa", "aa");
        remote_roundtrip("aa", "aaa");
        remote_roundtrip("abab", "ab");
    }

    #[test]
    fn remote_diff_feeds_text_ops() {
        // Full cycle: remote diff → Changes → TextOps → scalar apply.
        let old = "shared 🦀 doc";
        let new = "shared 🦀 document";
        let doc = Doc::from_str(old);
        let changes = remote_text_to_changes(doc.rope(), new);
        let ops = changes_to_text_ops(doc.rope(), &changes);
        assert_eq!(apply_ops_scalar(old, &ops), new);
    }
}
