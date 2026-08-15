//! [`VimState`] + the central dispatcher.
//!
//! vim ref: zed/crates/vim/src/state.rs:45 (`Mode`, `Operator`)
//! vim ref: codemirror-vim/src/vim.js (`vim_api` / commandDispatcher)

use editor_state::{Changes, EditorState, KeySpec, Range, Selection, TransactionSpec};

use crate::motions::{self, Motion, MotionKind};
use crate::operators::{self, Operator};
use crate::registers::{RegisterKey, Registers};
use crate::text_objects::{self, TextObject};

/// Current vim mode. Operator-pending is modeled as a flag
/// inside `Normal`, not its own variant — `pending_operator`
/// being `Some` is what makes us "operator-pending".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    VisualChar,
    VisualLine,
    VisualBlock,
    Replace,
    Command,
}

/// Sticky pending state between key presses. Each non-`None`
/// field represents a partially-typed command.
#[derive(Clone, Debug, Default)]
pub struct VimState {
    pub mode: Mode,
    pub pending_count: Option<usize>,
    pub pending_operator: Option<Operator>,
    pub pending_register: Option<RegisterKey>,
    /// Set when the previous key was one of `f F t T r` and the
    /// next key is a literal char, not a command.
    pub pending_motion_input: Option<MotionInput>,
    /// Set after `g` is pressed in normal mode — the next key
    /// finishes a `g`-prefixed command (`gg`, `gu`, `gU`, `g~`,
    /// `ge`).
    pub pending_g: bool,
    /// Set after `gu`/`gU`/`g~` — the next key is a motion and
    /// the resolved range gets case-changed. Holds the case op
    /// (`'u'`/`'U'`/`'~'`).
    pub pending_g_case: Option<char>,
    /// Most recent search (`*`/`#`/`/`/`?`). `n` / `N` repeat
    /// against this.
    pub last_search: Option<Search>,
    /// Live command-line buffer while `mode == Command` (`:`,
    /// `/`, `?`). The host renders this in its status strip.
    pub command_line: Option<crate::command_line::CommandLineState>,
    /// Most recent `f`/`F`/`t`/`T` target, for `;` / `,` repeat.
    pub last_find: Option<(MotionInput, char)>,
    /// Sticky column (in chars) for `j`/`k` runs — vim keeps the
    /// column you started from across shorter lines. Cleared by
    /// any non-`j`/`k` key.
    pub goal_col: Option<usize>,
    /// Anchor offset for visual mode. `None` outside visual.
    pub visual_anchor: Option<usize>,
    pub registers: Registers,
    pub last_change: Option<LastChange>,
}

/// A repeatable search target. `*`/`#` set `whole_word`; `/`/`?`
/// are plain substring searches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Search {
    pub pattern: String,
    pub forward: bool,
    pub whole_word: bool,
}

/// Which pending command is waiting on a character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionInput {
    FindForward,  // `f`
    FindBackward, // `F`
    TillForward,  // `t`
    TillBackward, // `T`
    Replace,      // `r`
    Register,     // `"` — the next char names a register
    /// Operator or visual text object (`diw`, `da"`, `viw`, …).
    /// The next char names the object.
    TextObject {
        around: bool,
    },
}

/// Recorded last change for `.` repeat. We store *intent* rather
/// than keystrokes: an operator + motion (or text object) + count,
/// or an insert payload.
///
/// vim ref: codemirror-vim/src/vim.js#L4500 (`vim.lastEditInputState`)
#[derive(Clone, Debug)]
pub enum LastChange {
    OperatorMotion {
        operator: Operator,
        motion: Motion,
        count: usize,
    },
    OperatorTextObject {
        operator: Operator,
        object: TextObject,
        around: bool,
        count: usize,
    },
    /// Doubled-op linewise change (`dd`, `cc`, `yy`, `>>`).
    OperatorLine { operator: Operator, count: usize },
    /// Operator + find motion (`df<c>`, `ct<c>`, …).
    OperatorFind {
        operator: Operator,
        input: MotionInput,
        ch: char,
        count: usize,
    },
    /// Insert-mode text that was committed by `<Esc>`. Replay
    /// inserts this at the caret. Not wired through `apply`
    /// yet — see `macros.rs`.
    Insert(String),
}

impl VimState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset all the transient pending fields. Call after a
    /// command commits or is aborted.
    pub fn clear_pending(&mut self) {
        self.pending_count = None;
        self.pending_operator = None;
        self.pending_register = None;
        self.pending_motion_input = None;
        self.pending_g = false;
        self.pending_g_case = None;
    }

    #[must_use]
    pub fn is_visual(&self) -> bool {
        matches!(
            self.mode,
            Mode::VisualChar | Mode::VisualLine | Mode::VisualBlock
        )
    }

    /// True when the host should let raw character keystrokes
    /// fall through to the contenteditable / text-input path.
    /// Used by `editor-view`'s onkeydown to gate
    /// `preventDefault` between non-Insert and Insert modes.
    #[must_use]
    pub fn is_inserting(&self) -> bool {
        matches!(self.mode, Mode::Insert | Mode::Replace)
    }

    /// Hook for the editor's history system. The dispatcher
    /// emits `request_undo` / `request_redo` via metadata on the
    /// returned [`TransactionSpec`]; the host resolves them
    /// against its history.
    #[must_use]
    pub fn request_undo(&self) -> Option<TransactionSpec> {
        Some(
            TransactionSpec::new()
                .annotate("vim", "undo")
                .user_event("undo"),
        )
    }

    #[must_use]
    pub fn request_redo(&self) -> Option<TransactionSpec> {
        Some(
            TransactionSpec::new()
                .annotate("vim", "redo")
                .user_event("redo"),
        )
    }
}

/// Dispatch a single key. See module docs in `lib.rs` for the
/// state machine outline.
pub(crate) fn dispatch(
    state: &EditorState,
    vim: &mut VimState,
    key: &KeySpec,
) -> Option<TransactionSpec> {
    // Pending-input first: `f<x>`, `r<x>`, `"<x>` consume the
    // next keystroke verbatim. Doing this *before* mode dispatch
    // keeps motion-with-arg logic centralized.
    if let Some(input) = vim.pending_motion_input.take() {
        return finish_pending_input(state, vim, key, input);
    }

    match vim.mode {
        Mode::Normal => dispatch_normal(state, vim, key),
        Mode::Insert => dispatch_insert(state, vim, key),
        Mode::VisualChar | Mode::VisualLine | Mode::VisualBlock => dispatch_visual(state, vim, key),
        Mode::Replace => dispatch_replace(state, vim, key),
        Mode::Command => crate::command_line::dispatch(state, vim, key),
    }
}

// --- Normal mode -----------------------------------------------

fn dispatch_normal(
    state: &EditorState,
    vim: &mut VimState,
    key: &KeySpec,
) -> Option<TransactionSpec> {
    // `<Esc>` always clears pending state.
    if key.key == "Escape" {
        vim.clear_pending();
        vim.goal_col = None;
        return None;
    }

    // No modifiers? Treat the key string as the raw vim "char".
    // We deliberately ignore `shift` for letter case — pressing
    // capital `A` arrives as key="A" without shift mattering for
    // dispatch (the lexical case carries the info).
    if key.ctrl || key.alt || key.meta {
        return dispatch_normal_modified(state, vim, key);
    }

    let ch = single_char(key)?;

    // `j`/`k` runs keep their goal column; anything else drops it.
    if !matches!(ch, 'j' | 'k') && !ch.is_ascii_digit() {
        vim.goal_col = None;
    }

    // Count accumulation: `0` only starts a count if there's
    // already one in flight — otherwise `0` is the line-start
    // motion. vim ref: codemirror-vim/src/vim.js (numberRegex)
    if ch.is_ascii_digit() && !(ch == '0' && vim.pending_count.is_none()) {
        let d = ch.to_digit(10).unwrap() as usize;
        vim.pending_count = Some(vim.pending_count.unwrap_or(0) * 10 + d);
        return None;
    }

    // Register prefix.
    if ch == '"' {
        vim.pending_motion_input = Some(MotionInput::Register);
        return None;
    }

    // Pending case op (`gu`/`gU`/`g~`): the next key is a
    // motion, and we re-case the resolved range. Doubled (`guu`/
    // `gUU`/`g~~`) acts on the current line — vim convention.
    if let Some(case_op) = vim.pending_g_case {
        let from = caret(state);
        if (case_op == 'u' && ch == 'u')
            || (case_op == 'U' && ch == 'U')
            || (case_op == '~' && ch == '~')
        {
            let lo = motions::line_start(state, from);
            let hi = motions::line_end(state, from);
            return Some(apply_case_change(state, vim, case_op, lo, hi));
        }
        if let Some(motion) = Motion::from_char(ch) {
            let count = vim.pending_count.take();
            let to = motion_target(state, motion, count);
            let (lo, hi) = ordered_inclusive(state, from, to, motion.kind());
            return Some(apply_case_change(state, vim, case_op, lo, hi));
        }
        vim.clear_pending();
        return None;
    }

    // `g`-prefix: the next key finishes a `g`-prefixed command.
    if vim.pending_g {
        vim.pending_g = false;
        return finish_g_command(state, vim, ch);
    }
    if ch == 'g' {
        vim.pending_g = true;
        return None;
    }

    // Operator-pending: if `pending_operator` is `Some`, this
    // keystroke must produce a motion or be the doubled-op
    // shorthand (`dd`/`cc`/`yy`).
    if let Some(op) = vim.pending_operator {
        return finish_operator(state, vim, op, ch);
    }

    // Try a single-char command (mode change, paste, undo, etc.)
    // before falling through to motion.
    if let Some(spec) = single_char_normal_command(state, vim, ch) {
        return Some(spec);
    }

    // Try an operator key.
    if let Some(op) = Operator::from_char(ch) {
        vim.pending_operator = Some(op);
        return None;
    }

    // Try a motion. Motions that need a char (`f F t T`) set the
    // pending-input flag and return.
    if let Some(motion) = Motion::from_char(ch) {
        if let Some(needed) = motion_needs_input(motion) {
            vim.pending_motion_input = Some(needed);
            return None;
        }
        let count = vim.pending_count.take();
        let new_pos = if matches!(motion, Motion::Up | Motion::Down) {
            let (p, col) = motions::vertical(
                state,
                caret(state),
                count.unwrap_or(1),
                motion == Motion::Down,
                vim.goal_col,
            );
            vim.goal_col = Some(col);
            p
        } else {
            motion_target(state, motion, count)
        };
        vim.clear_pending();
        let pos = motions::clamp_normal(state, new_pos);
        return Some(TransactionSpec::new().selection(Selection::caret(pos)));
    }

    None
}

fn dispatch_normal_modified(
    state: &EditorState,
    vim: &mut VimState,
    key: &KeySpec,
) -> Option<TransactionSpec> {
    // Ctrl-r = redo. Ctrl-v in normal = enter visual block.
    if key.ctrl && key.key == "r" {
        vim.clear_pending();
        return vim.request_redo();
    }
    if key.ctrl && key.key == "v" {
        vim.mode = Mode::VisualBlock;
        vim.visual_anchor = Some(caret(state));
        return None;
    }
    None
}

/// Resolve a motion's target offset, count-aware. `G` and `gg`
/// are line-addressed when a count is present (`5G` → line 5),
/// otherwise last / first line.
fn motion_target(state: &EditorState, motion: Motion, count: Option<usize>) -> usize {
    match motion {
        Motion::DocEnd => match count {
            Some(n) => motions::nth_line_first_nonblank(state, n.saturating_sub(1)),
            None => motions::last_line_first_nonblank(state),
        },
        Motion::DocStart => {
            motions::nth_line_first_nonblank(state, count.unwrap_or(1).saturating_sub(1))
        }
        _ => motions::apply(state, motion, count.unwrap_or(1)),
    }
}

/// Order `(from, to)` and widen for an inclusive motion (the
/// char at the range end is part of the operated text).
fn ordered_inclusive(
    state: &EditorState,
    from: usize,
    to: usize,
    kind: MotionKind,
) -> (usize, usize) {
    let (lo, hi) = (from.min(to), from.max(to));
    if kind == MotionKind::Inclusive {
        let s = state.doc.to_string();
        let bytes = s.as_bytes();
        if hi < bytes.len() && bytes[hi] != b'\n' {
            return (lo, motions::next_char_boundary(bytes, hi));
        }
    }
    (lo, hi)
}

fn single_char_normal_command(
    state: &EditorState,
    vim: &mut VimState,
    ch: char,
) -> Option<TransactionSpec> {
    match ch {
        'i' => {
            vim.mode = Mode::Insert;
            vim.clear_pending();
            Some(TransactionSpec::new())
        }
        'I' => {
            // First non-blank on current line, then insert.
            let pos = motions::line_first_nonblank(state, caret(state));
            vim.mode = Mode::Insert;
            vim.clear_pending();
            Some(TransactionSpec::new().selection(Selection::caret(pos)))
        }
        'a' => {
            // Insert after the caret's char — never past the
            // line's `\n`.
            let s = state.doc.to_string();
            let pos = motions::next_char_boundary(s.as_bytes(), caret(state))
                .min(motions::line_end(state, caret(state)));
            vim.mode = Mode::Insert;
            vim.clear_pending();
            Some(TransactionSpec::new().selection(Selection::caret(pos)))
        }
        'A' => {
            let pos = motions::line_end(state, caret(state));
            vim.mode = Mode::Insert;
            vim.clear_pending();
            Some(TransactionSpec::new().selection(Selection::caret(pos)))
        }
        'o' => {
            let end = motions::line_end(state, caret(state));
            vim.mode = Mode::Insert;
            vim.clear_pending();
            Some(
                TransactionSpec::new()
                    .changes(Changes::insert(end, "\n"))
                    .selection(Selection::caret(end + 1)),
            )
        }
        'O' => {
            let start = motions::line_start(state, caret(state));
            vim.mode = Mode::Insert;
            vim.clear_pending();
            Some(
                TransactionSpec::new()
                    .changes(Changes::insert(start, "\n"))
                    .selection(Selection::caret(start)),
            )
        }
        's' => {
            // Substitute `count` chars (line-bounded), enter insert.
            let count = vim.pending_count.take().unwrap_or(1);
            let from = caret(state);
            let to = chars_ahead_in_line(state, from, count);
            vim.mode = Mode::Insert;
            if to > from {
                let text = state.doc.slice(from..to);
                vim.registers.write_unnamed(&text);
            }
            vim.clear_pending();
            Some(
                TransactionSpec::new()
                    .changes(Changes::delete(from..to))
                    .selection(Selection::caret(from)),
            )
        }
        'S' => {
            // Linewise change (`cc`): count lines, keep the
            // trailing newline + indent.
            let count = vim.pending_count.take().unwrap_or(1);
            let from = motions::line_start(state, caret(state));
            let to = (motions::line_end_n(state, caret(state), count) + 1).min(state.doc.len());
            Some(operators::apply_linewise(
                state,
                vim,
                Operator::Change,
                from,
                to,
            ))
        }
        'r' => {
            vim.pending_motion_input = Some(MotionInput::Replace);
            None
        }
        'R' => {
            vim.mode = Mode::Replace;
            vim.clear_pending();
            Some(TransactionSpec::new())
        }
        'v' => {
            vim.mode = Mode::VisualChar;
            vim.visual_anchor = Some(caret(state));
            vim.clear_pending();
            None
        }
        'V' => {
            vim.mode = Mode::VisualLine;
            vim.visual_anchor = Some(caret(state));
            vim.clear_pending();
            None
        }
        ':' => {
            vim.mode = Mode::Command;
            vim.command_line = Some(crate::command_line::CommandLineState::new(
                crate::command_line::CmdKind::Ex,
            ));
            vim.clear_pending();
            None
        }
        // `/` search. NOTE: hosts that wire a slash-command
        // palette intercept `/` before vim sees it (Obsidian UX)
        // — `?` searches backward and is always available.
        '/' => {
            vim.mode = Mode::Command;
            vim.command_line = Some(crate::command_line::CommandLineState::new(
                crate::command_line::CmdKind::SearchForward,
            ));
            vim.clear_pending();
            None
        }
        '?' => {
            vim.mode = Mode::Command;
            vim.command_line = Some(crate::command_line::CommandLineState::new(
                crate::command_line::CmdKind::SearchBackward,
            ));
            vim.clear_pending();
            None
        }
        'u' => {
            vim.clear_pending();
            vim.request_undo()
        }
        'p' => Some(paste(state, vim, /*before=*/ false)),
        'P' => Some(paste(state, vim, /*before=*/ true)),
        'x' => {
            // Delete `count` chars under/after caret — never the
            // newline (vim `x` doesn't join lines).
            let count = vim.pending_count.take().unwrap_or(1);
            let from = caret(state);
            let to = chars_ahead_in_line(state, from, count);
            vim.clear_pending();
            if from == to {
                return None;
            }
            let text = state.doc.slice(from..to);
            vim.registers.write_unnamed(&text);
            vim.last_change = Some(LastChange::OperatorMotion {
                operator: Operator::Delete,
                motion: Motion::Right,
                count,
            });
            let caret_after = caret_after_line_delete(state, from, to);
            Some(
                TransactionSpec::new()
                    .changes(Changes::delete(from..to))
                    .selection(Selection::caret(caret_after)),
            )
        }
        ';' => repeat_find(state, vim, /*swap=*/ false),
        ',' => repeat_find(state, vim, /*swap=*/ true),
        '*' => search_word_under_caret(state, vim, /*forward=*/ true),
        '#' => search_word_under_caret(state, vim, /*forward=*/ false),
        'n' => search_repeat(state, vim, /*reverse=*/ false),
        'N' => search_repeat(state, vim, /*reverse=*/ true),
        'J' => {
            let count = vim.pending_count.take().unwrap_or(2);
            Some(join_lines(state, vim, caret(state), count))
        }
        'C' => {
            // Change to end of line. `c$` shorthand.
            let count = vim.pending_count.take().unwrap_or(1);
            let from = caret(state);
            let to = motions::line_end_n(state, from, count);
            vim.last_change = Some(LastChange::OperatorMotion {
                operator: Operator::Change,
                motion: Motion::LineEnd,
                count,
            });
            Some(operators::apply_range(
                state,
                vim,
                Operator::Change,
                from,
                to,
            ))
        }
        'D' => {
            // Delete to end of line. `d$` shorthand.
            let count = vim.pending_count.take().unwrap_or(1);
            let from = caret(state);
            let to = motions::line_end_n(state, from, count);
            vim.last_change = Some(LastChange::OperatorMotion {
                operator: Operator::Delete,
                motion: Motion::LineEnd,
                count,
            });
            Some(operators::apply_range(
                state,
                vim,
                Operator::Delete,
                from,
                to,
            ))
        }
        'Y' => {
            // Yank the line (`yy`). Neovim default; classic vim
            // ships `Y` = `y$` but Neovim flipped it years ago.
            let count = vim.pending_count.take().unwrap_or(1);
            let from = motions::line_start(state, caret(state));
            let line_end = motions::line_end_n(state, caret(state), count);
            let to = (line_end + 1).min(state.doc.len());
            Some(operators::apply_linewise(
                state,
                vim,
                Operator::Yank,
                from,
                to,
            ))
        }
        '~' => Some(toggle_case_char(state, vim)),
        '.' => crate::macros::replay_last(state, vim),
        _ => None,
    }
}

/// Byte offset `count` chars ahead of `from`, clamped to the end
/// of the line (never past the `\n`).
fn chars_ahead_in_line(state: &EditorState, from: usize, count: usize) -> usize {
    let hi = motions::line_end(state, from);
    let s = state.doc.to_string();
    let bytes = s.as_bytes();
    let mut p = from;
    for _ in 0..count.max(1) {
        if p >= hi {
            break;
        }
        p = motions::next_char_boundary(bytes, p).min(hi);
    }
    p
}

/// Where the caret lands after deleting `[from, to)` within a
/// line: stays at `from`, unless that now sits past the line's
/// last char — then it clamps back one char (vim `x` at EOL).
fn caret_after_line_delete(state: &EditorState, from: usize, to: usize) -> usize {
    let s = state.doc.to_string();
    let bytes = s.as_bytes();
    let at_eol_after = to >= bytes.len() || bytes[to] == b'\n';
    let ls = motions::line_start(state, from);
    if at_eol_after && from > ls {
        motions::prev_char_boundary(bytes, from).max(ls)
    } else {
        from
    }
}

/// Resolve a `g`-prefixed command. The first `g` has already
/// been consumed, and `ch` is whatever the user pressed next.
/// Works in operator-pending too (`dgg`, `dG` comes via `G`).
fn finish_g_command(state: &EditorState, vim: &mut VimState, ch: char) -> Option<TransactionSpec> {
    match ch {
        'g' => {
            let count = vim.pending_count.take();
            if let Some(op) = vim.pending_operator.take() {
                // `dgg` — linewise from the current line to line
                // `count` (default: first).
                let target = motion_target(state, Motion::DocStart, count);
                return Some(operator_linewise_between(
                    state,
                    vim,
                    op,
                    caret(state),
                    target,
                ));
            }
            let pos = motion_target(state, Motion::DocStart, count);
            vim.clear_pending();
            Some(TransactionSpec::new().selection(Selection::caret(pos)))
        }
        'e' => {
            // `ge` — end of previous word (inclusive motion).
            let count = vim.pending_count.take();
            if let Some(op) = vim.pending_operator.take() {
                let from = caret(state);
                let to = motion_target(state, Motion::EndPrevWord, count);
                let (lo, hi) = ordered_inclusive(state, from, to, MotionKind::Inclusive);
                vim.last_change = Some(LastChange::OperatorMotion {
                    operator: op,
                    motion: Motion::EndPrevWord,
                    count: count.unwrap_or(1),
                });
                return Some(operators::apply_range(state, vim, op, lo, hi));
            }
            let pos = motion_target(state, Motion::EndPrevWord, count);
            vim.clear_pending();
            Some(TransactionSpec::new().selection(Selection::caret(pos)))
        }
        'u' | 'U' | '~' => {
            // `gu<motion>` / `gU<motion>` / `g~<motion>` —
            // change case over a motion. Park the case-op tag in
            // `pending_g_case`; the next key is read as a motion
            // and the resolved range gets re-cased.
            vim.pending_g_case = Some(ch);
            None
        }
        'v' => {
            // `gv` needs a stored last-visual range — not kept
            // yet. No-op.
            vim.clear_pending();
            None
        }
        _ => {
            vim.clear_pending();
            None
        }
    }
}

/// Apply a pending case change (`gu`/`gU`/`g~`) over a resolved
/// `[from, to)` range. Returns the transaction spec.
fn apply_case_change(
    state: &EditorState,
    vim: &mut VimState,
    op: char,
    from: usize,
    to: usize,
) -> TransactionSpec {
    let (lo, hi) = (from.min(to), from.max(to));
    let text = state.doc.slice(lo..hi);
    let new_text: String = text
        .chars()
        .map(|c| match op {
            'u' => c.to_lowercase().next().unwrap_or(c),
            'U' => c.to_uppercase().next().unwrap_or(c),
            '~' => {
                if c.is_uppercase() {
                    c.to_lowercase().next().unwrap_or(c)
                } else if c.is_lowercase() {
                    c.to_uppercase().next().unwrap_or(c)
                } else {
                    c
                }
            }
            _ => c,
        })
        .collect();
    vim.clear_pending();
    vim.mode = Mode::Normal;
    vim.visual_anchor = None;
    TransactionSpec::new()
        .changes(Changes::replace(lo..hi, new_text))
        .selection(Selection::caret(lo))
}

/// Linewise operator over the lines spanned by `[a, b]`
/// (either order). Used by `dj`, `dk`, `dG`, `dgg`.
fn operator_linewise_between(
    state: &EditorState,
    vim: &mut VimState,
    op: Operator,
    a: usize,
    b: usize,
) -> TransactionSpec {
    let (lo_pos, hi_pos) = (a.min(b), a.max(b));
    let lo = motions::line_start(state, lo_pos);
    let hi = (motions::line_end(state, hi_pos) + 1).min(state.doc.len());
    operators::apply_linewise(state, vim, op, lo, hi)
}

fn finish_operator(
    state: &EditorState,
    vim: &mut VimState,
    op: Operator,
    ch: char,
) -> Option<TransactionSpec> {
    // Doubled-op shorthand: `dd`, `cc`, `yy` act on the current
    // line (linewise, count extends downward).
    if Operator::from_char(ch) == Some(op) {
        let count = vim.pending_count.take().unwrap_or(1);
        let from = motions::line_start(state, caret(state));
        let to = motions::line_end_n(state, caret(state), count);
        let to_inclusive = (to + 1).min(state.doc.len());
        vim.last_change = Some(LastChange::OperatorLine {
            operator: op,
            count,
        });
        return Some(operators::apply_linewise(
            state,
            vim,
            op,
            from,
            to_inclusive,
        ));
    }

    // Text object: `iw`, `aw`, `i"`, ...
    if ch == 'i' || ch == 'a' {
        vim.pending_motion_input = Some(MotionInput::TextObject { around: ch == 'a' });
        return None;
    }

    // `g`-prefixed motion (`dgg`, `dge`).
    if ch == 'g' {
        vim.pending_g = true;
        return None;
    }

    // Otherwise we expect a motion.
    if let Some(motion) = Motion::from_char(ch) {
        if let Some(needed) = motion_needs_input(motion) {
            // Stash op + register on vim, mark waiting for char.
            // Stays pending: vim.pending_operator already set.
            vim.pending_motion_input = Some(needed);
            return None;
        }
        // `cw` acts like `ce` when the caret is on a non-blank —
        // vim's most famous special case (`:help cw`).
        let motion = if op == Operator::Change && caret_on_nonblank(state) {
            match motion {
                Motion::WordForward => Motion::WordEnd,
                Motion::WORDForward => Motion::WORDEnd,
                m => m,
            }
        } else {
            motion
        };
        let count = vim.pending_count.take();
        let from = caret(state);
        let to = motion_target(state, motion, count);
        vim.last_change = Some(LastChange::OperatorMotion {
            operator: op,
            motion,
            count: count.unwrap_or(1),
        });
        vim.pending_operator = None;
        let spec = match motion.kind() {
            MotionKind::Linewise => operator_linewise_between(state, vim, op, from, to),
            kind => {
                let (lo, hi) = ordered_inclusive(state, from, to, kind);
                operators::apply_range(state, vim, op, lo, hi)
            }
        };
        return Some(spec);
    }

    // Unknown key aborts operator-pending (vim beeps).
    vim.clear_pending();
    None
}

fn caret_on_nonblank(state: &EditorState) -> bool {
    let s = state.doc.to_string();
    let bytes = s.as_bytes();
    let p = caret(state);
    p < bytes.len() && !bytes[p].is_ascii_whitespace()
}

/// Apply an `f`/`F`/`t`/`T` jump — shared by the pending-input
/// path and `;` / `,` repeat. Honors a pending operator.
fn apply_find(
    state: &EditorState,
    vim: &mut VimState,
    input: MotionInput,
    ch: char,
    count: usize,
) -> Option<TransactionSpec> {
    let target = motions::find_char(state, caret(state), ch, input, count);
    let Some(target) = target else {
        vim.clear_pending();
        return None;
    };
    if let Some(op) = vim.pending_operator {
        let from = caret(state);
        let s = state.doc.to_string();
        let bytes = s.as_bytes();
        // Forward find/till is inclusive of the target char;
        // backward is exclusive of the caret's char.
        let (lo, hi) = if from <= target {
            (from, motions::next_char_boundary(bytes, target))
        } else {
            (target, from)
        };
        vim.last_change = Some(LastChange::OperatorFind {
            operator: op,
            input,
            ch,
            count,
        });
        let spec = operators::apply_range(state, vim, op, lo, hi);
        return Some(spec);
    }
    vim.clear_pending();
    if vim.is_visual() {
        let anchor = vim.visual_anchor.unwrap_or(caret(state));
        return Some(
            TransactionSpec::new().selection(Selection::single(Range::new(anchor, target))),
        );
    }
    Some(TransactionSpec::new().selection(Selection::caret(target)))
}

/// `;` / `,` — repeat the last `f`/`F`/`t`/`T`, optionally
/// direction-swapped.
fn repeat_find(state: &EditorState, vim: &mut VimState, swap: bool) -> Option<TransactionSpec> {
    let (input, ch) = vim.last_find?;
    let input = if swap {
        match input {
            MotionInput::FindForward => MotionInput::FindBackward,
            MotionInput::FindBackward => MotionInput::FindForward,
            MotionInput::TillForward => MotionInput::TillBackward,
            MotionInput::TillBackward => MotionInput::TillForward,
            other => other,
        }
    } else {
        input
    };
    let count = vim.pending_count.take().unwrap_or(1);
    apply_find(state, vim, input, ch, count)
}

fn finish_pending_input(
    state: &EditorState,
    vim: &mut VimState,
    key: &KeySpec,
    input: MotionInput,
) -> Option<TransactionSpec> {
    if key.key == "Escape" {
        vim.clear_pending();
        return None;
    }
    let ch = single_char(key)?;
    match input {
        MotionInput::Register => {
            vim.pending_register = RegisterKey::from_char(ch);
            None
        }
        MotionInput::Replace => {
            // `r<c>` with count: replace `count` chars with `c`.
            // vim fails (no-op) when the line has fewer chars.
            let count = vim.pending_count.take().unwrap_or(1);
            let from = caret(state);
            let to = chars_ahead_in_line(state, from, count);
            let s = state.doc.to_string();
            let replaced = s[from..to].chars().count();
            vim.clear_pending();
            if replaced < count {
                return None;
            }
            let new_text: String = ch.to_string().repeat(count);
            let caret_pos = from + new_text.len() - ch.len_utf8();
            Some(
                TransactionSpec::new()
                    .changes(Changes::replace(from..to, new_text))
                    .selection(Selection::caret(caret_pos)),
            )
        }
        MotionInput::TextObject { around } => {
            let obj = TextObject::from_char(ch)?;
            let count = vim.pending_count.take().unwrap_or(1);
            let range = text_objects::apply(state, obj, around, caret(state));
            if let Some(op) = vim.pending_operator {
                vim.last_change = Some(LastChange::OperatorTextObject {
                    operator: op,
                    object: obj,
                    around,
                    count,
                });
                let spec = operators::apply_range(state, vim, op, range.start, range.end);
                return Some(spec);
            }
            if vim.is_visual() && range.end > range.start {
                // `viw` — expand the selection to the object. The
                // head sits ON the object's last char (visual is
                // head-inclusive at operate time).
                let s = state.doc.to_string();
                let head = motions::prev_char_boundary(s.as_bytes(), range.end).max(range.start);
                vim.visual_anchor = Some(range.start);
                vim.clear_pending();
                return Some(
                    TransactionSpec::new()
                        .selection(Selection::single(Range::new(range.start, head))),
                );
            }
            vim.clear_pending();
            None
        }
        MotionInput::FindForward
        | MotionInput::FindBackward
        | MotionInput::TillForward
        | MotionInput::TillBackward => {
            let count = vim.pending_count.take().unwrap_or(1);
            vim.last_find = Some((input, ch));
            apply_find(state, vim, input, ch, count)
        }
    }
}

fn motion_needs_input(motion: Motion) -> Option<MotionInput> {
    match motion {
        Motion::FindForward => Some(MotionInput::FindForward),
        Motion::FindBackward => Some(MotionInput::FindBackward),
        Motion::TillForward => Some(MotionInput::TillForward),
        Motion::TillBackward => Some(MotionInput::TillBackward),
        _ => None,
    }
}

// --- Insert mode -----------------------------------------------

fn dispatch_insert(
    state: &EditorState,
    vim: &mut VimState,
    key: &KeySpec,
) -> Option<TransactionSpec> {
    if key.key == "Escape" {
        vim.mode = Mode::Normal;
        vim.clear_pending();
        // vim steps the caret one char left on leaving insert
        // (unless already at line start).
        let pos = caret(state);
        let ls = motions::line_start(state, pos);
        let s = state.doc.to_string();
        let new_pos = if pos > ls {
            motions::prev_char_boundary(s.as_bytes(), pos).max(ls)
        } else {
            pos
        };
        return Some(TransactionSpec::new().selection(Selection::caret(new_pos)));
    }
    // All other keys fall through to the host's text-input path.
    None
}

// --- Replace mode ----------------------------------------------

fn dispatch_replace(
    state: &EditorState,
    vim: &mut VimState,
    key: &KeySpec,
) -> Option<TransactionSpec> {
    if key.key == "Escape" {
        vim.mode = Mode::Normal;
        let pos = caret(state);
        let ls = motions::line_start(state, pos);
        let s = state.doc.to_string();
        let new_pos = if pos > ls {
            motions::prev_char_boundary(s.as_bytes(), pos).max(ls)
        } else {
            pos
        };
        return Some(TransactionSpec::new().selection(Selection::caret(new_pos)));
    }
    let ch = single_char(key)?;
    let from = caret(state);
    // At end of line, `R` inserts instead of overwriting the
    // newline (vim semantics).
    let hi = motions::line_end(state, from);
    let s = state.doc.to_string();
    let to = if from < hi {
        motions::next_char_boundary(s.as_bytes(), from).min(hi)
    } else {
        from
    };
    Some(
        TransactionSpec::new()
            .changes(Changes::replace(from..to, ch.to_string()))
            .selection(Selection::caret(from + ch.len_utf8())),
    )
}

// --- Visual mode -----------------------------------------------

/// Resolve the visual selection into an operable byte range.
/// Char-visual is inclusive of the char under the head (and the
/// anchor, whichever is later); line-visual snaps to whole lines
/// including the trailing newline.
fn visual_range(state: &EditorState, vim: &VimState, force_linewise: bool) -> (usize, usize, bool) {
    let r = state.selection.primary();
    let (lo, to) = (r.from(), r.to());
    let linewise = force_linewise || vim.mode == Mode::VisualLine;
    if linewise {
        let start = motions::line_start(state, lo);
        let end = (motions::line_end(state, to) + 1).min(state.doc.len());
        (start, end, true)
    } else {
        let s = state.doc.to_string();
        let bytes = s.as_bytes();
        let hi = if to < bytes.len() && bytes[to] != b'\n' {
            motions::next_char_boundary(bytes, to)
        } else {
            to
        };
        (lo, hi.max(lo), false)
    }
}

fn visual_operator(
    state: &EditorState,
    vim: &mut VimState,
    op: Operator,
    force_linewise: bool,
) -> TransactionSpec {
    let (lo, hi, linewise) = visual_range(state, vim, force_linewise);
    vim.mode = Mode::Normal;
    vim.visual_anchor = None;
    if linewise {
        operators::apply_linewise(state, vim, op, lo, hi)
    } else {
        operators::apply_range(state, vim, op, lo, hi)
    }
}

fn dispatch_visual(
    state: &EditorState,
    vim: &mut VimState,
    key: &KeySpec,
) -> Option<TransactionSpec> {
    if key.key == "Escape" {
        vim.mode = Mode::Normal;
        let pos = motions::clamp_normal(state, caret(state));
        vim.visual_anchor = None;
        vim.clear_pending();
        return Some(TransactionSpec::new().selection(Selection::caret(pos)));
    }
    if key.ctrl || key.alt || key.meta {
        return None;
    }
    let ch = single_char(key)?;

    if !matches!(ch, 'j' | 'k') && !ch.is_ascii_digit() {
        vim.goal_col = None;
    }

    // Counts work in visual too (`v3w`, `V2j`).
    if ch.is_ascii_digit() && !(ch == '0' && vim.pending_count.is_none()) {
        let d = ch.to_digit(10).unwrap() as usize;
        vim.pending_count = Some(vim.pending_count.unwrap_or(0) * 10 + d);
        return None;
    }

    if ch == '"' {
        vim.pending_motion_input = Some(MotionInput::Register);
        return None;
    }

    // Operator on the current visual range.
    if let Some(op) = Operator::from_char(ch) {
        return Some(visual_operator(state, vim, op, false));
    }

    match ch {
        // `x`/`s` in visual are delete/change on the selection.
        'x' => return Some(visual_operator(state, vim, Operator::Delete, false)),
        's' => return Some(visual_operator(state, vim, Operator::Change, false)),
        // `X`/`S`/`R` are the linewise variants.
        'X' => return Some(visual_operator(state, vim, Operator::Delete, true)),
        'S' | 'R' => return Some(visual_operator(state, vim, Operator::Change, true)),
        // Case ops act on the selection and exit visual.
        '~' | 'u' | 'U' => {
            let (lo, hi, _) = visual_range(state, vim, false);
            let case_op = if ch == '~' { '~' } else { ch };
            return Some(apply_case_change(state, vim, case_op, lo, hi));
        }
        // Paste over the selection; the replaced text lands in
        // the unnamed register (vim semantics).
        'p' | 'P' => {
            let (lo, hi, _) = visual_range(state, vim, false);
            let reg = vim.pending_register.take();
            let entry = vim.registers.read_full(reg)?;
            let old = state.doc.slice(lo..hi);
            vim.registers.write_unnamed(&old);
            vim.mode = Mode::Normal;
            vim.visual_anchor = None;
            vim.clear_pending();
            let text = entry.text.clone();
            let caret_pos = if text.is_empty() {
                lo
            } else {
                let last_len = text.chars().last().map_or(1, char::len_utf8);
                lo + text.len() - last_len
            };
            return Some(
                TransactionSpec::new()
                    .changes(Changes::replace(lo..hi, text))
                    .selection(Selection::caret(caret_pos)),
            );
        }
        // Swap the two ends of the selection.
        'o' => {
            let r = state.selection.primary();
            vim.visual_anchor = Some(r.head);
            return Some(
                TransactionSpec::new().selection(Selection::single(Range::new(r.head, r.anchor))),
            );
        }
        // Mode toggles.
        'v' => {
            if vim.mode == Mode::VisualChar {
                vim.mode = Mode::Normal;
                let pos = motions::clamp_normal(state, caret(state));
                vim.visual_anchor = None;
                return Some(TransactionSpec::new().selection(Selection::caret(pos)));
            }
            vim.mode = Mode::VisualChar;
            return None;
        }
        'V' => {
            if vim.mode == Mode::VisualLine {
                vim.mode = Mode::Normal;
                let pos = motions::clamp_normal(state, caret(state));
                vim.visual_anchor = None;
                return Some(TransactionSpec::new().selection(Selection::caret(pos)));
            }
            vim.mode = Mode::VisualLine;
            return None;
        }
        // Text objects expand the selection (`viw`, `va"`).
        'i' | 'a' => {
            vim.pending_motion_input = Some(MotionInput::TextObject { around: ch == 'a' });
            return None;
        }
        'J' => {
            let (lo, hi, _) = visual_range(state, vim, false);
            vim.mode = Mode::Normal;
            vim.visual_anchor = None;
            // Join every line the selection spans.
            let s = state.doc.to_string();
            let joins = s[lo..hi.min(s.len())]
                .bytes()
                .filter(|&b| b == b'\n')
                .count()
                .max(1);
            return Some(join_lines(state, vim, lo, joins + 1));
        }
        _ => {}
    }

    // Motion in visual: extend `head` to motion target, keep anchor.
    if let Some(motion) = Motion::from_char(ch) {
        if let Some(needed) = motion_needs_input(motion) {
            vim.pending_motion_input = Some(needed);
            return None;
        }
        let count = vim.pending_count.take();
        let new_head = if matches!(motion, Motion::Up | Motion::Down) {
            let (p, col) = motions::vertical(
                state,
                caret(state),
                count.unwrap_or(1),
                motion == Motion::Down,
                vim.goal_col,
            );
            vim.goal_col = Some(col);
            p
        } else {
            motion_target(state, motion, count)
        };
        let anchor = vim.visual_anchor.unwrap_or(caret(state));
        return Some(
            TransactionSpec::new().selection(Selection::single(Range::new(anchor, new_head))),
        );
    }
    None
}

// --- helpers ----------------------------------------------------

fn caret(state: &EditorState) -> usize {
    state.selection.primary().head
}

pub(crate) fn single_char(key: &KeySpec) -> Option<char> {
    let mut chars = key.key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(c)
}

fn paste(state: &EditorState, vim: &mut VimState, before: bool) -> TransactionSpec {
    let key = vim.pending_register.take();
    let count = vim.pending_count.take().unwrap_or(1);
    vim.clear_pending();
    let Some(entry) = vim.registers.read_full(key) else {
        return TransactionSpec::new();
    };
    if entry.text.is_empty() {
        return TransactionSpec::new();
    }
    let text = entry.text.repeat(count);
    let s = state.doc.to_string();
    let bytes = s.as_bytes();
    if entry.linewise {
        if before {
            // `P` — insert above the current line; caret at the
            // first non-blank of the pasted text.
            let start = motions::line_start(state, caret(state));
            let nb = text.len() - text.trim_start_matches([' ', '\t']).len();
            return TransactionSpec::new()
                .changes(Changes::insert(start, text))
                .selection(Selection::caret(start + nb));
        }
        let end = motions::line_end(state, caret(state));
        if end == state.doc.len() {
            // Pasting below the last line of a doc without a
            // trailing newline: prepend the `\n` instead.
            let payload = format!("\n{}", text.trim_end_matches('\n'));
            let nb = payload[1..].len() - payload[1..].trim_start_matches([' ', '\t']).len();
            let new_caret = end + 1 + nb;
            return TransactionSpec::new()
                .changes(Changes::insert(end, payload))
                .selection(Selection::caret(new_caret));
        }
        let insert_at = end + 1;
        let nb = text.len() - text.trim_start_matches([' ', '\t']).len();
        TransactionSpec::new()
            .changes(Changes::insert(insert_at, text))
            .selection(Selection::caret(insert_at + nb))
    } else {
        let p = if before {
            caret(state)
        } else {
            motions::next_char_boundary(bytes, caret(state))
                .min(motions::line_end(state, caret(state)))
        };
        let last_len = text.chars().last().map_or(1, char::len_utf8);
        let new_caret = p + text.len() - last_len;
        TransactionSpec::new()
            .changes(Changes::insert(p, text))
            .selection(Selection::caret(new_caret))
    }
}

/// `*` / `#` — find the word under the caret, then jump to its
/// next/previous whole-word occurrence. Stores the word + the
/// initial direction in `vim.last_search` so `n` / `N` can
/// repeat it without re-reading the doc.
fn search_word_under_caret(
    state: &EditorState,
    vim: &mut VimState,
    forward: bool,
) -> Option<TransactionSpec> {
    let doc = state.doc.to_string();
    let bytes = doc.as_bytes();
    let pos = caret(state);
    // Identify the word containing the caret (or the next word
    // forward, if the caret is on whitespace).
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = pos;
    while start > 0 && start <= bytes.len() && is_word(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = start;
    while end < bytes.len() && is_word(bytes[end]) {
        end += 1;
    }
    if start == end {
        // No word at the caret — try the next word forward.
        let mut p = pos;
        while p < bytes.len() && !is_word(bytes[p]) {
            p += 1;
        }
        if p >= bytes.len() {
            vim.clear_pending();
            return None;
        }
        start = p;
        end = p;
        while end < bytes.len() && is_word(bytes[end]) {
            end += 1;
        }
    }
    let word = doc[start..end].to_string();
    vim.last_search = Some(Search {
        pattern: word.clone(),
        forward,
        whole_word: true,
    });
    vim.clear_pending();
    Some(jump_to(
        &doc, &word, pos, forward, /*whole_word=*/ true,
    ))
}

/// `n` / `N` — repeat the last `*`/`#` search. `reverse=true`
/// flips the stored direction (that's `N`).
pub(crate) fn search_repeat(
    state: &EditorState,
    vim: &mut VimState,
    reverse: bool,
) -> Option<TransactionSpec> {
    let search = vim.last_search.clone()?;
    let effective = search.forward ^ reverse;
    let doc = state.doc.to_string();
    vim.clear_pending();
    Some(jump_to(
        &doc,
        &search.pattern,
        caret(state),
        effective,
        search.whole_word,
    ))
}

/// Walk the doc for the next/previous occurrence of `word`
/// relative to `from`, wrapping at the doc bounds. `whole_word`
/// requires word-boundary neighbors (`*`/`#`); plain search
/// (`/`/`?`) matches substrings. Empty `word` is a no-op.
pub(crate) fn jump_to(
    doc: &str,
    word: &str,
    from: usize,
    forward: bool,
    whole_word: bool,
) -> TransactionSpec {
    if word.is_empty() {
        return TransactionSpec::new().selection(Selection::caret(from));
    }
    let bytes = doc.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let whole_match_at = |i: usize| -> bool {
        if i + word.len() > bytes.len() || !doc.is_char_boundary(i) {
            return false;
        }
        if &doc[i..i + word.len()] != word {
            return false;
        }
        if !whole_word {
            return true;
        }
        let left_ok = i == 0 || !is_word(bytes[i - 1]);
        let right_ok = i + word.len() == bytes.len() || !is_word(bytes[i + word.len()]);
        left_ok && right_ok
    };
    if forward {
        let mut i = from + 1;
        while i < bytes.len() {
            if whole_match_at(i) {
                return TransactionSpec::new().selection(Selection::caret(i));
            }
            i += 1;
        }
        // Wrap.
        let mut i = 0;
        while i <= from {
            if whole_match_at(i) {
                return TransactionSpec::new().selection(Selection::caret(i));
            }
            i += 1;
        }
    } else {
        let mut i = from.saturating_sub(1);
        loop {
            if whole_match_at(i) {
                return TransactionSpec::new().selection(Selection::caret(i));
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
        // Wrap.
        let mut i = bytes.len().saturating_sub(1);
        while i > from {
            if whole_match_at(i) {
                return TransactionSpec::new().selection(Selection::caret(i));
            }
            i -= 1;
        }
    }
    TransactionSpec::new().selection(Selection::caret(from))
}

/// Join `count` lines starting at the line containing `at` —
/// vim `J`: each `\n` (+ following indent) becomes one space.
/// `count` counts *lines*, so `J` == `2J` == one join.
fn join_lines(state: &EditorState, vim: &mut VimState, at: usize, count: usize) -> TransactionSpec {
    vim.clear_pending();
    let joins = count.max(2) - 1;
    let doc_str = state.doc.to_string();
    let bytes = doc_str.as_bytes();
    let mut changes = Vec::new();
    let mut p = at;
    let mut first_join = None;
    for _ in 0..joins {
        let line_end = motions::line_end(state, p);
        if line_end >= bytes.len() {
            break;
        }
        let mut end = line_end + 1;
        while end < bytes.len() && (bytes[end] == b' ' || bytes[end] == b'\t') {
            end += 1;
        }
        changes.push(editor_state::Change::replace(line_end..end, " "));
        first_join.get_or_insert(line_end);
        p = end;
    }
    let Some(first) = first_join else {
        return TransactionSpec::new();
    };
    TransactionSpec::new()
        .changes(Changes::from_sorted(changes))
        .selection(Selection::caret(first))
}

fn toggle_case_char(state: &EditorState, vim: &mut VimState) -> TransactionSpec {
    // `~` with count toggles `count` chars, line-bounded, and
    // advances the caret past the last one (clamped in line).
    let count = vim.pending_count.take().unwrap_or(1);
    vim.clear_pending();
    let from = caret(state);
    let to = chars_ahead_in_line(state, from, count);
    if from == to {
        return TransactionSpec::new();
    }
    let ch = state.doc.slice(from..to);
    let flipped: String = ch
        .chars()
        .map(|c| {
            if c.is_uppercase() {
                c.to_lowercase().next().unwrap_or(c)
            } else if c.is_lowercase() {
                c.to_uppercase().next().unwrap_or(c)
            } else {
                c
            }
        })
        .collect();
    let caret_after = motions::clamp_normal(state, to);
    TransactionSpec::new()
        .changes(Changes::replace(from..to, flipped))
        .selection(Selection::caret(caret_after))
}
