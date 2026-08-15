//! Obsidian `tasks` CLI compatibility — scan a vault for
//! `- [ ]`-style task list items.
//!
//! Native-only. Reads from an in-memory [`Vault`] snapshot; the
//! line numbers are reconstructed by walking the original raw
//! markdown of each page in source order so duplicate task lines
//! still get distinct line numbers.

#![cfg(not(target_arch = "wasm32"))]

use crate::vault::{Vault, VaultPage};

/// One task item discovered in a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskItem<'a> {
    /// Vault-relative page path (borrowed from the [`VaultPage`]).
    pub page: &'a str,
    /// 1-based line number in the page's raw markdown.
    pub line: usize,
    /// First char of the task marker. ' ' = open, 'x'/'X' = done,
    /// '/' = in-progress, plus Obsidian's other variants ('-', '?',
    /// '!', '*', etc.).
    pub marker: char,
    /// Task text — the block's content with the `[m] ` marker
    /// stripped if it was still present at the front.
    pub text: String,
}

impl TaskItem<'_> {
    /// Done state — Obsidian treats both `x` and `X` as completed.
    #[must_use]
    pub fn is_done(&self) -> bool {
        matches!(self.marker, 'x' | 'X')
    }

    /// Open state — only the literal unchecked marker (a single
    /// space) counts as open; any other glyph is some sentinel
    /// (in-progress, cancelled, question, etc.).
    #[must_use]
    pub fn is_open(&self) -> bool {
        matches!(self.marker, ' ')
    }
}

/// Scan every page in the vault for task list items.
#[must_use]
pub fn list_tasks(vault: &Vault) -> Vec<TaskItem<'_>> {
    let mut out = Vec::new();
    for page in &vault.pages {
        out.extend(page_tasks(page));
    }
    out
}

/// Tasks belonging to a single page.
#[must_use]
pub fn page_tasks(page: &VaultPage) -> Vec<TaskItem<'_>> {
    // Pre-scan the raw to get every (line_no, marker) for task
    // list items, in source order. We then zip this with the
    // parsed blocks that carry `list_task`.
    let mut task_lines: Vec<(usize, char)> = Vec::with_capacity(8);
    for (idx, line) in page.raw.split('\n').enumerate() {
        if let Some(marker) = scan_task_marker(line) {
            task_lines.push((idx + 1, marker));
        }
    }

    let mut cursor = 0usize;
    let mut tasks = Vec::new();
    for block in &page.parsed.blocks {
        let Some(raw_mark) = block.list_task.as_ref() else {
            continue;
        };
        let marker = raw_mark.chars().next().unwrap_or(' ');

        // Walk forward in the line index until we find a marker
        // that matches this block. If markers disagree (rare —
        // a hand-edited block list out of sync with raw), just
        // take the next available line and keep going.
        let (line_no, line_marker) = match task_lines.get(cursor) {
            Some(&(ln, m)) => {
                cursor += 1;
                (ln, m)
            }
            None => (0, marker),
        };

        // Prefer the marker that came out of the raw scan if it
        // looks valid; otherwise fall back to the block's.
        let final_marker = if line_marker == marker || line_marker != '\0' {
            line_marker
        } else {
            marker
        };

        tasks.push(TaskItem {
            page: page.rel_path.as_str(),
            line: line_no,
            marker: final_marker,
            text: strip_marker(&block.content),
        });
    }
    tasks
}

/// If `line` is a task list item (`- [m] ...` / `* [m] ...` /
/// `+ [m] ...` / `N. [m] ...` after leading whitespace), return
/// the marker char. Otherwise `None`.
fn scan_task_marker(line: &str) -> Option<char> {
    let trimmed = line.trim_start();
    let rest = if let Some(r) = trimmed.strip_prefix("- ") {
        r
    } else if let Some(r) = trimmed.strip_prefix("* ") {
        r
    } else if let Some(r) = trimmed.strip_prefix("+ ") {
        r
    } else {
        // Ordered list: digits then `. `.
        let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        let after = &trimmed[digits.len()..];
        after.strip_prefix(". ")?
    };
    // Now `rest` must start with `[m]` followed by space or EOL.
    let bytes = rest.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'[' || bytes[2] != b']' {
        return None;
    }
    // Trailing space (or end of line) — Obsidian/CommonMark accept
    // either, though "[ ]\n" without a space is degenerate.
    if bytes.len() > 3 && bytes[3] != b' ' && bytes[3] != b'\t' {
        return None;
    }
    Some(rest[1..2].chars().next().unwrap_or(' '))
}

/// Strip a leading `[m] ` from the block's content if the parser
/// kept it in. `parse_page` already excludes it for some block
/// kinds; this is defensive.
fn strip_marker(content: &str) -> String {
    let bytes = content.as_bytes();
    if bytes.len() >= 4 && bytes[0] == b'[' && bytes[2] == b']' && bytes[3] == b' ' {
        content[4..].to_string()
    } else {
        content.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn touch(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn list_tasks_finds_open_and_done() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("Todo.md"),
            "# Today\n\n- [ ] buy milk\n- [x] ship build\n- [/] write docs\n- not a task\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let tasks = list_tasks(&v);
        assert_eq!(tasks.len(), 3, "tasks: {tasks:?}");

        let open: Vec<_> = tasks.iter().filter(|t| t.is_open()).collect();
        let done: Vec<_> = tasks.iter().filter(|t| t.is_done()).collect();
        assert_eq!(open.len(), 1);
        assert_eq!(done.len(), 1);
        assert_eq!(open[0].text, "buy milk");
        assert_eq!(done[0].text, "ship build");

        let in_prog = tasks.iter().find(|t| t.marker == '/').unwrap();
        assert_eq!(in_prog.text, "write docs");
    }

    #[test]
    fn line_numbers_are_one_based_and_unique_for_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("Dup.md"),
            "intro\n- [ ] same\n- [ ] same\n- [ ] same\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let tasks = list_tasks(&v);
        assert_eq!(tasks.len(), 3);
        let lines: Vec<usize> = tasks.iter().map(|t| t.line).collect();
        assert_eq!(lines, vec![2, 3, 4]);
    }

    #[test]
    fn page_tasks_borrows_rel_path() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Sub/A.md"), "- [ ] one\n");
        let v = Vault::open(dir.path()).unwrap();
        let page = v.page("Sub/A.md").unwrap();
        let t = page_tasks(page);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].page, "Sub/A.md");
        assert_eq!(t[0].line, 1);
        assert_eq!(t[0].marker, ' ');
    }

    #[test]
    fn non_task_list_items_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("Plain.md"),
            "- regular bullet\n- another\n- [x] only this is a task\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let tasks = list_tasks(&v);
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].is_done());
    }
}
