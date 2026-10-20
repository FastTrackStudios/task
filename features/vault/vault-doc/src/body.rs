//! Text edits on a document's `body`, and the one display convention a
//! body editor needs.
//!
//! - [`diff_ops`] / [`apply_ops_clamped`] — the minimal single-region
//!   edit between two texts, as Loro text ops (Unicode scalar positions),
//!   applied clamped so ops computed against a slightly older text still
//!   land and never panic.
//! - [`shown`] / [`shown_edit_ops`] — a task file sets its description
//!   off from the frontmatter with one blank line, which the task parser
//!   takes back off; so the body *document* starts with `\n` and the
//!   description *editor* must not show it. These map between the two,
//!   and bring the separator along when a first word goes into an empty
//!   body. A note body has no such convention and edits the body as is.

use crdt::codec::TextOp;

/// What a description editor shows for a task-style body: the body
/// without its one leading separator newline.
#[must_use]
pub fn shown(body: &str) -> &str {
    body.strip_prefix('\n').unwrap_or(body)
}

/// Ops on the body document that make it [`shown`] as `new_shown`,
/// keeping (or adding, for a first word) the separator newline.
#[must_use]
pub fn shown_edit_ops(body: &str, new_shown: &str) -> Vec<TextOp> {
    let separated = body.starts_with('\n');
    let old_shown = shown(body);
    if old_shown == new_shown {
        return Vec::new();
    }
    let mut out = Vec::new();
    let base: u32 = if separated {
        1
    } else if !new_shown.is_empty() && old_shown.is_empty() {
        out.push(TextOp::Insert {
            pos: 0,
            text: "\n".to_owned(),
        });
        1
    } else {
        0
    };
    for op in diff_ops(old_shown, new_shown) {
        out.push(match op {
            TextOp::Insert { pos, text } => TextOp::Insert {
                pos: pos.saturating_add(base),
                text,
            },
            TextOp::Delete { pos, len } => TextOp::Delete {
                pos: pos.saturating_add(base),
                len,
            },
        });
    }
    out
}

/// Minimal insert/delete pair from `old` to `new`: common prefix and
/// common suffix stripped, over Unicode scalars. Delete first, then the
/// insert at the same position.
#[must_use]
pub fn diff_ops(old: &str, new: &str) -> Vec<TextOp> {
    if old == new {
        return Vec::new();
    }
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let mut prefix = 0;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < a.len() - prefix
        && suffix < b.len() - prefix
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let del = a.len() - prefix - suffix;
    let ins: String = b[prefix..b.len() - suffix].iter().collect();
    let mut ops = Vec::with_capacity(2);
    if del > 0 {
        ops.push(TextOp::Delete {
            pos: to_u32(prefix),
            len: to_u32(del),
        });
    }
    if !ins.is_empty() {
        ops.push(TextOp::Insert {
            pos: to_u32(prefix),
            text: ins,
        });
    }
    ops
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Apply ops to a `LoroText`, clamping each position to the current
/// length.
pub fn apply_ops_clamped(text: &crdt::loro::LoroText, ops: &[TextOp]) {
    for op in ops {
        let len = text.len_unicode();
        let result = match op {
            TextOp::Insert { pos, text: t } => text.insert((*pos as usize).min(len), t),
            TextOp::Delete { pos, len: n } => {
                let from = (*pos as usize).min(len);
                let n = (*n as usize).min(len - from);
                if n == 0 {
                    continue;
                }
                text.delete(from, n)
            }
        };
        if let Err(e) = result {
            tracing::warn!("vault-doc: text op failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, ops: &[TextOp]) -> String {
        let doc = crdt::loro::LoroDoc::new();
        let t = doc.get_text("t");
        t.insert(0, text).unwrap();
        apply_ops_clamped(&t, ops);
        t.to_string()
    }

    #[test]
    fn a_task_body_hides_and_keeps_its_separator() {
        let body = "\nFirst line\n";
        assert_eq!(shown(body), "First line\n");
        let out = apply(body, &shown_edit_ops(body, "First line\nSecond\n"));
        assert_eq!(out, "\nFirst line\nSecond\n");
        assert_eq!(apply(body, &shown_edit_ops(body, "")), "\n");
    }

    #[test]
    fn a_first_word_in_an_empty_body_brings_the_separator() {
        assert_eq!(apply("", &shown_edit_ops("", "Notes")), "\nNotes");
        // A body written without one (hand-edited file) is left as it is.
        assert_eq!(apply("abc", &shown_edit_ops("abc", "abXc")), "abXc");
    }

    #[test]
    fn diffs_count_scalars() {
        assert_eq!(apply("ä🦀b", &diff_ops("ä🦀b", "ä🦀Xb")), "ä🦀Xb");
        assert_eq!(
            apply("hello world", &diff_ops("hello world", "hello")),
            "hello"
        );
    }

    #[test]
    fn clamped_ops_tolerate_a_shorter_text() {
        let ops = [
            TextOp::Insert {
                pos: 99,
                text: "!".into(),
            },
            TextOp::Delete { pos: 99, len: 5 },
        ];
        assert_eq!(apply("ab", &ops), "ab!");
    }
}
