//! `:` ex commands and `/`/`?` search — vim's command-line mode.
//!
//! Keystrokes buffer into [`CommandLineState`] (the host renders
//! it in its status strip); `<Enter>` executes, `<Esc>` cancels,
//! `<Backspace>` on an empty buffer exits.
//!
//! Ex commands understood:
//!
//! - `:w` / `:write` — emits a changeless spec tagged
//!   `user_event("save")`; the HOST persists (the editor has no
//!   filesystem). Same for `:q` (`"quit"`) and `:wq`/`:x`
//!   (`"save-quit"`).
//! - `:N` / `:$` — go to line N / last line.
//! - `:[range]s/pat/rep/[g]` — literal (non-regex) substitute.
//!   Range: none = current line, `%` = whole doc, `N,M` = lines
//!   N..=M (1-based). Flag `g` replaces every occurrence per
//!   line instead of the first.
//! - `:noh` — clears search state (no highlight layer yet, but
//!   `n`/`N` stop repeating).
//!
//! `/pat` / `?pat` set [`crate::state::Search`] (substring, not
//! whole-word) and jump; `n`/`N` repeat.
//!
//! vim ref: codemirror-vim/src/vim.js (`exCommandDispatcher`)

use editor_state::{Changes, EditorState, KeySpec, Selection, TransactionSpec};

use crate::motions;
use crate::state::{Mode, Search, VimState, single_char};

/// What kind of command line is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdKind {
    Ex,             // `:`
    SearchForward,  // `/`
    SearchBackward, // `?`
}

impl CmdKind {
    /// The prompt char the host shows before the buffer.
    #[must_use]
    pub fn prompt(self) -> char {
        match self {
            Self::Ex => ':',
            Self::SearchForward => '/',
            Self::SearchBackward => '?',
        }
    }
}

/// Live command-line buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandLineState {
    pub kind: CmdKind,
    pub buffer: String,
}

impl CommandLineState {
    #[must_use]
    pub fn new(kind: CmdKind) -> Self {
        Self {
            kind,
            buffer: String::new(),
        }
    }
}

pub(crate) fn dispatch(
    state: &EditorState,
    vim: &mut VimState,
    key: &KeySpec,
) -> Option<TransactionSpec> {
    if key.key == "Escape" {
        vim.mode = Mode::Normal;
        vim.command_line = None;
        vim.clear_pending();
        return None;
    }
    let Some(cl) = vim.command_line.as_mut() else {
        // Stale Command mode without a buffer — recover.
        vim.mode = Mode::Normal;
        return None;
    };
    match key.key.as_str() {
        "Enter" => {
            let cl = vim.command_line.take().unwrap();
            vim.mode = Mode::Normal;
            vim.clear_pending();
            execute(state, vim, &cl)
        }
        "Backspace" => {
            if cl.buffer.pop().is_none() {
                vim.mode = Mode::Normal;
                vim.command_line = None;
            }
            None
        }
        _ => {
            if let Some(ch) = single_char(key) {
                if !key.ctrl && !key.alt && !key.meta {
                    cl.buffer.push(ch);
                }
            }
            None
        }
    }
}

fn execute(
    state: &EditorState,
    vim: &mut VimState,
    cl: &CommandLineState,
) -> Option<TransactionSpec> {
    match cl.kind {
        CmdKind::Ex => execute_ex(state, vim, cl.buffer.trim()),
        CmdKind::SearchForward | CmdKind::SearchBackward => {
            let forward = cl.kind == CmdKind::SearchForward;
            if cl.buffer.is_empty() {
                // Bare `/` + Enter repeats the last search, vim-style.
                return crate::state::search_repeat(state, vim, false);
            }
            vim.last_search = Some(Search {
                pattern: cl.buffer.clone(),
                forward,
                whole_word: false,
            });
            let doc = state.doc.to_string();
            let from = state.selection.primary().head;
            Some(crate::state::jump_to(
                &doc, &cl.buffer, from, forward, /*whole_word=*/ false,
            ))
        }
    }
}

fn execute_ex(state: &EditorState, vim: &mut VimState, cmd: &str) -> Option<TransactionSpec> {
    if cmd.is_empty() {
        return None;
    }
    // `:N` — go to line N (1-based). `:$` — last line.
    if cmd.chars().all(|c| c.is_ascii_digit()) {
        let n: usize = cmd.parse().ok()?;
        let pos = motions::nth_line_first_nonblank(state, n.saturating_sub(1));
        return Some(TransactionSpec::new().selection(Selection::caret(pos)));
    }
    if cmd == "$" {
        let pos = motions::last_line_first_nonblank(state);
        return Some(TransactionSpec::new().selection(Selection::caret(pos)));
    }
    match cmd {
        "w" | "write" => {
            return Some(
                TransactionSpec::new()
                    .user_event("save")
                    .annotate("vim", "ex:w"),
            );
        }
        "q" | "q!" | "quit" => {
            return Some(
                TransactionSpec::new()
                    .user_event("quit")
                    .annotate("vim", "ex:q"),
            );
        }
        "wq" | "x" => {
            return Some(
                TransactionSpec::new()
                    .user_event("save-quit")
                    .annotate("vim", "ex:wq"),
            );
        }
        "noh" | "nohl" | "nohlsearch" => {
            vim.last_search = None;
            return None;
        }
        _ => {}
    }
    substitute_command(state, cmd)
}

/// Parse and run `[range]s/pat/rep/[flags]`. Literal patterns —
/// no regex in v1.
fn substitute_command(state: &EditorState, cmd: &str) -> Option<TransactionSpec> {
    let doc = state.doc.to_string();
    let caret = state.selection.primary().head;

    // Range prefix.
    let (range, rest) = if let Some(rest) = cmd.strip_prefix('%') {
        (SubstRange::WholeDoc, rest)
    } else if cmd.starts_with(|c: char| c.is_ascii_digit()) {
        // `N,Ms/…`
        let comma = cmd.find(',')?;
        let s_pos = cmd[comma..].find('s')? + comma;
        let lo: usize = cmd[..comma].parse().ok()?;
        let hi: usize = cmd[comma + 1..s_pos].parse().ok()?;
        (SubstRange::Lines(lo, hi), &cmd[s_pos..])
    } else {
        (SubstRange::CurrentLine, cmd)
    };

    let rest = rest.strip_prefix('s')?;
    let sep = rest.chars().next()?;
    if sep.is_ascii_alphanumeric() {
        return None;
    }
    let mut parts = rest[sep.len_utf8()..].splitn(3, sep);
    let pat = parts.next()?;
    let rep = parts.next().unwrap_or("");
    let flags = parts.next().unwrap_or("");
    if pat.is_empty() {
        return None;
    }
    let global = flags.contains('g');

    // Resolve the byte range of the affected lines.
    let (lo, hi) = match range {
        SubstRange::CurrentLine => (
            motions::line_start(state, caret),
            motions::line_end(state, caret),
        ),
        SubstRange::WholeDoc => (0, doc.len()),
        SubstRange::Lines(a, b) => {
            let lo_line = motions::nth_line_first_nonblank(state, a.saturating_sub(1));
            let hi_line = motions::nth_line_first_nonblank(state, b.saturating_sub(1));
            (
                motions::line_start(state, lo_line),
                motions::line_end(state, hi_line),
            )
        }
    };

    // Walk line by line so non-`g` replaces only the first hit
    // per line (vim semantics).
    let mut changes = Vec::new();
    let mut line_start = lo;
    let mut last_change_at = None;
    while line_start < hi {
        let line_end = doc[line_start..hi]
            .find('\n')
            .map_or(hi, |i| line_start + i);
        let line = &doc[line_start..line_end];
        for (off, _) in line.match_indices(pat) {
            let at = line_start + off;
            changes.push(editor_state::Change::replace(at..at + pat.len(), rep));
            last_change_at = Some(at);
            if !global {
                break;
            }
        }
        line_start = line_end + 1;
    }
    if changes.is_empty() {
        return None;
    }
    // Overlap guard: `g` on patterns like `aa` in `aaa` can't
    // overlap because match_indices is non-overlapping already.
    let caret_to = last_change_at.unwrap_or(caret);
    Some(
        TransactionSpec::new()
            .changes(Changes::from_sorted(changes))
            .selection(Selection::caret(caret_to))
            .annotate("vim", "ex:s"),
    )
}

enum SubstRange {
    CurrentLine,
    WholeDoc,
    Lines(usize, usize),
}
