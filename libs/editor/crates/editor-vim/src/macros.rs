//! `.` repeat + (eventually) `q`/`@` macros. v1 handles
//! operator-motion, operator-text-object, linewise (`dd`/`cc`)
//! and operator-find (`df<c>`) replays; insert-text replay is a
//! follow-up.
//!
//! vim ref: codemirror-vim/src/vim.js (`vim.lastEditInputState`)
//! vim ref: zed/crates/vim/src/normal/repeat.rs

use editor_state::{EditorState, TransactionSpec};

use crate::motions::{self, MotionKind};
use crate::operators;
use crate::state::{LastChange, VimState};
use crate::text_objects;

pub(crate) fn replay_last(state: &EditorState, vim: &mut VimState) -> Option<TransactionSpec> {
    let last = vim.last_change.clone()?;
    let caret = state.selection.primary().head;
    match last {
        LastChange::OperatorMotion {
            operator,
            motion,
            count,
        } => {
            let to = motions::apply(state, motion, count);
            match motion.kind() {
                MotionKind::Linewise => {
                    let (a, b) = (caret.min(to), caret.max(to));
                    let lo = motions::line_start(state, a);
                    let hi = (motions::line_end(state, b) + 1).min(state.doc.len());
                    Some(operators::apply_linewise(state, vim, operator, lo, hi))
                }
                kind => {
                    let (lo, mut hi) = (caret.min(to), caret.max(to));
                    if kind == MotionKind::Inclusive {
                        let s = state.doc.to_string();
                        let bytes = s.as_bytes();
                        if hi < bytes.len() && bytes[hi] != b'\n' {
                            hi = motions::next_char_boundary(bytes, hi);
                        }
                    }
                    Some(operators::apply_range(state, vim, operator, lo, hi))
                }
            }
        }
        LastChange::OperatorTextObject {
            operator,
            object,
            around,
            ..
        } => {
            let r = text_objects::apply(state, object, around, caret);
            Some(operators::apply_range(state, vim, operator, r.start, r.end))
        }
        LastChange::OperatorLine { operator, count } => {
            let from = motions::line_start(state, caret);
            let to = (motions::line_end_n(state, caret, count) + 1).min(state.doc.len());
            Some(operators::apply_linewise(state, vim, operator, from, to))
        }
        LastChange::OperatorFind {
            operator,
            input,
            ch,
            count,
        } => {
            let target = motions::find_char(state, caret, ch, input, count)?;
            let s = state.doc.to_string();
            let bytes = s.as_bytes();
            let (lo, hi) = if caret <= target {
                (caret, motions::next_char_boundary(bytes, target))
            } else {
                (target, caret)
            };
            Some(operators::apply_range(state, vim, operator, lo, hi))
        }
        LastChange::Insert(_) => None, // v1: TODO
    }
}
