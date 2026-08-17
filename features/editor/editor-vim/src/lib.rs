//! Pure-Rust vim modal state machine. Sibling to `editor-state`;
//! takes an [`EditorState`] + a [`KeySpec`] and returns an
//! optional [`TransactionSpec`] to apply, mutating only the
//! [`VimState`] that the caller owns.
//!
//! No DOM, no Dioxus, no async. Drop-in wherever a key dispatcher
//! lives. Mirrors the feature scope of CM6's vim plugin
//! (`codemirror-vim/src/vim.js`) and cribs design from Zed
//! (`vim ref: zed/crates/vim/src/state.rs`) and Helix
//! (`vim ref: helix/helix-view/src/editor.rs`).
//!
//! # State shape
//!
//! A single [`VimState`] composes a small set of orthogonal
//! fields:
//!
//! - [`Mode`] — `Normal`, `Insert`, `Visual{Char,Line,Block}`,
//!   `Replace`, `Command`. Operator-pending is **not** a mode —
//!   it's `pending_operator: Option<Operator>` inside `Normal`,
//!   the same shape Zed uses.
//! - `pending_count: Option<usize>` — typed digits, multiplies
//!   the next motion/operator. `3d2w` → operator with count 3,
//!   motion with count 2.
//! - `pending_register: Option<RegisterKey>` — the next yank/
//!   delete/paste targets this register (`"ay`, `"ap`).
//! - `pending_motion_input: Option<MotionInput>` — `f`/`F`/`t`/
//!   `T`/`r` need a follow-up character; this holds the "next
//!   key is data" flag.
//! - `last_change: Option<Change>` — what `.` replays. Stored as
//!   a recorded sequence of intent (operator + motion, or insert
//!   text), not a raw key buffer.
//! - [`Registers`] — `HashMap<RegisterKey, String>` + an
//!   injected clipboard callback for `+`/`*`.
//!
//! # Key-press flow
//!
//! `handle_key` is the single entry point. It is a finite state
//! machine driven by `vim.mode` and the pending fields:
//!
//! 1. **Accumulate** digits → `pending_count`.
//! 2. **Accumulate** `"x` register prefix → `pending_register`.
//! 3. **Accumulate** operator (`d`/`c`/`y`) → `pending_operator`.
//! 4. **Consume** a motion or text object — combine count *
//!    operator (if any) to build a [`TransactionSpec`].
//! 5. Reset the pending fields and return.
//!
//! If the keystroke doesn't make sense in the current mode (e.g.
//! `h` in `Insert`), `handle_key` returns `None` and the caller
//! falls back to its non-vim behavior (typically: insert the
//! literal character via the normal text-input path).
//!
//! # Selection model
//!
//! - **Normal** mode is always a caret (`anchor == head`).
//! - **Visual** mode produces non-empty ranges anchored at the
//!   point `v`/`V`/`Ctrl-v` was pressed.
//! - After an operator runs from visual mode, the selection
//!   collapses to where the cursor lands (same as vim).
//! - Linewise visual is modeled by snapping `from`/`to` to line
//!   boundaries at apply time, not by storing the snapped range
//!   in the selection. This keeps the selection shape compatible
//!   with `editor-state` (which has no notion of linewise).
//! - Blockwise visual is v1 best-effort: we store anchor/head as
//!   normal and let operators reinterpret them. Real columnar
//!   editing (multi-cursor on a rectangle) needs a follow-up.
//!
//! # Registers
//!
//! [`Registers`] is a side table on [`VimState`]. Yank/delete
//! write to the unnamed register (`"`) and to the named one if
//! a register prefix was set. Paste reads in the reverse order.
//! System clipboard (`+`/`*`) is gated behind a `Clipboard`
//! callback that the host injects — `editor-state` has no DOM
//! dep and this crate honors that.
//!
//! # Caret discipline
//!
//! Normal-mode carets never sit on a `\n` or past EOF — motions
//! and delete commands clamp via [`motions::clamp_normal`], and
//! `<Esc>` out of insert steps one char left, matching vim.
//! `j`/`k` keep a sticky goal column (chars, UTF-8 safe) across
//! shorter lines.
//!
//! # What's stubbed
//!
//! - `Mode::Command` (`:`) — accepted, no-op body.
//! - `.` repeat covers operator+motion, operator+text-object,
//!   linewise (`dd`/`cc`) and operator+find (`df<c>`); insert-mode
//!   text replay is a follow-up.
//! - Marks, `/`/`?` search, macros (`q`/`@`), real visual-block
//!   (columnar) editing — none yet.

pub mod command_line;
pub mod macros;
pub mod motions;
pub mod operators;
pub mod registers;
pub mod state;
pub mod text_objects;

pub use command_line::{CmdKind, CommandLineState};
pub use motions::Motion;
pub use operators::Operator;
pub use registers::{Register, RegisterKey, Registers};
pub use state::{Mode, Search, VimState};
pub use text_objects::TextObject;

use editor_state::{EditorState, KeySpec, TransactionSpec};

/// Single entry point. Reads `key` against the pending state in
/// `vim`, possibly mutates it, and returns a transaction spec to
/// apply (or `None` if the key was consumed for accumulation or
/// is irrelevant in this mode).
pub fn handle_key(
    state: &EditorState,
    vim: &mut VimState,
    key: &KeySpec,
) -> Option<TransactionSpec> {
    state::dispatch(state, vim, key)
}
