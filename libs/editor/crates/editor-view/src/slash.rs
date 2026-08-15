//! Slash-command palette. Trigger on `/`, fuzzy filter the
//! catalog, dispatch the chosen command as a `TransactionSpec`.
//!
//! Ported from `~/Development/Task/crates/editor/src/handler/commands.rs`
//! and adapted for our editor's `EditorState` / `Changes` API
//! instead of Task's block-based document model. The
//! `CommandKind` enum mirrors the Task code closely; the
//! catalog is markdown-flavored (callouts, code fences, math
//! blocks, headings, etc.).
//!
//! Architecture follows `CodeMirror`'s `@codemirror/autocomplete`:
//! a `SlashState` signal holds the open menu + query; each
//! state update re-runs `detect_slash` against the current
//! line; selection nav fires `move_selection`; pick fires
//! `run_command` and clears the state.

use dioxus::prelude::*;
use editor_state::{Changes, EditorState, Selection, TransactionSpec};

/// Slash-command popup. Reads the open state from the
/// `slash` signal threaded down from the host (typically the
/// playground or whichever shell embeds the editor). Renders
/// rows grouped by `group`; clicks pick a command. Keyboard
/// nav lives in `Editor`'s `onkeydown` rather than here so it
/// works without the menu element being focused.
//
// Dioxus 0.7 flags `key:` on non-first nodes in a block as
// deprecated; the slash-row rsx! emits an optional header
// followed by the row div, which trips the lint. Suppressed
// at the component level; refactoring to a single keyed
// outer element would lose the leading group divider.
#[allow(deprecated)]
#[component]
pub fn SlashMenu(
    state: Signal<EditorState>,
    slash: Signal<Option<SlashState>>,
    /// Optional transaction sink — pass the same callback given to
    /// `Editor`'s `on_transaction` so command picks made by *mouse*
    /// (clicking a row) report through the host's sink exactly like
    /// keyboard picks (which route via the editor's keydown handler).
    #[props(default)]
    on_transaction: Option<Callback<crate::TransactionEvent>>,
) -> Element {
    let snapshot = slash.read().clone();
    let Some(current) = snapshot else {
        return rsx! { Fragment {} };
    };
    // Anchor the menu just under the current caret on every
    // render. Lets the menu track the user's typing position
    // instead of docking at the bottom of the editor frame.
    use_effect(|| {
        let script = r"(()=>{
            const menu = document.querySelector('.slash-menu');
            if (!menu) return;
            const sel = window.getSelection && window.getSelection();
            if (!sel || sel.rangeCount === 0) return;
            const r = sel.getRangeAt(0);
            const rects = r.getClientRects();
            const rect = rects.length
                ? rects[rects.length - 1]
                : r.getBoundingClientRect();
            if (!rect) return;
            const w = menu.offsetWidth;
            const h = menu.offsetHeight;
            const vw = window.innerWidth;
            const vh = window.innerHeight;
            let top = rect.bottom + 6;
            if (top + h > vh - 8) top = rect.top - h - 6;
            let left = rect.left;
            if (left + w > vw - 8) left = vw - w - 8;
            if (left < 8) left = 8;
            menu.style.top = top + 'px';
            menu.style.left = left + 'px';
        })();";
        let _ = document::eval(script);
    });
    let hits = filter_commands(&current.query);
    if hits.is_empty() {
        return rsx! {
            div { class: "slash-menu",
                div { class: "slash-empty", "No commands match." }
            }
        };
    }
    let selected = current.selected.min(hits.len().saturating_sub(1));
    let mut last_group: Option<&str> = None;
    let mut row_idx: usize = 0;
    rsx! {
        div { class: "slash-menu",
            for entry in hits.iter().cloned() {
                {
                    let show_header = last_group != Some(entry.group);
                    last_group = Some(entry.group);
                    let is_selected = row_idx == selected;
                    let idx_for_click = row_idx;
                    let entry_for_click = entry.clone();
                    let state_for_click = state;
                    let mut slash_for_click = slash;
                    let sink_for_click = on_transaction;
                    let current_for_click = current.clone();
                    row_idx += 1;
                    rsx! {
                        {
                            if show_header {
                                rsx! { div { class: "slash-group", "{entry.group}" } }
                            } else { rsx! {} }
                        }
                        div {
                            key: "{idx_for_click}",
                            class: if is_selected { "slash-row selected" } else { "slash-row" },
                            // Mousedown.preventDefault keeps the
                            // editor's caret from blurring as the
                            // click lands, so the next render keeps
                            // selection state coherent.
                            onmousedown: move |e: Event<MouseData>| e.prevent_default(),
                            onclick: move |_| {
                                let cur = state_for_click.read().clone();
                                let end = current_for_click.slash_start + 1 + current_for_click.query.len();
                                if let Some(spec) = run_command(
                                    &cur,
                                    current_for_click.slash_start..end,
                                    entry_for_click.kind,
                                ) {
                                    crate::event::apply_tx(state_for_click, &cur, spec, sink_for_click);
                                }
                                slash_for_click.set(None);
                            },
                            div { class: "slash-row-icon", "{entry.icon}" }
                            div { class: "slash-row-body",
                                div { class: "slash-row-label", "{entry.label}" }
                                if !entry.desc.is_empty() {
                                    div { class: "slash-row-desc", "{entry.desc}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One menu entry. Title shows in the row, group is the header
/// above it ("Heading", "Format", "Callout", …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandEntry {
    pub label: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    pub kind: CommandKind,
    /// Single-glyph icon shown left of the label — `LSP-completion`
    /// vibe so the user can scan-by-kind. Falls back to a
    /// blank space when empty so all rows align.
    pub icon: &'static str,
}

/// What the command does. Each variant carries the data the
/// runner needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    /// Splice a literal snippet at the slash range. `caret_back`
    /// is how many bytes to move the caret back from the end of
    /// the insert (e.g. `("[[]]", 2)` lands the caret between
    /// the brackets).
    InsertSnippet(&'static str, usize),
    /// Replace the slash range with a block-shaped snippet that
    /// starts on its own line. If the line containing the slash
    /// has other content, the runner inserts a newline first.
    /// Same `caret_back` semantics as `InsertSnippet`.
    InsertBlockSnippet(&'static str, usize),
    /// Set the heading level of the current line (1-6, or 0 to
    /// strip). Strips any existing `#…#` prefix first.
    SetHeading(u8),
    /// Promote the current line to a list item of the given
    /// kind. Replaces any existing list marker.
    SetList(ListKind),
    /// Toggle the current line's task checkbox (`[ ]` ↔ `[x]`).
    ToggleTask,
    /// Add a UUID block id to the block at the caret (Logseq
    /// style) so it can be referenced via `((uuid))`.
    AddBlockId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    Unordered,
    Ordered,
    Task,
}

/// Open-state of the slash menu. `None` when the menu's closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashState {
    /// Byte offset of the `/` that triggered the menu.
    pub slash_start: usize,
    /// Body typed after the slash (does NOT include the `/`).
    pub query: String,
    /// Currently highlighted row.
    pub selected: usize,
}

/// Scan back from the caret for a `/` that hasn't been closed
/// by whitespace, returning the trigger position + the query
/// typed after it. Mirrors `~/Development/Task/.../editable.rs::detect_slash`
/// but operates on the slice from the start of the current line
/// up to the caret, so a `/` deep in the doc doesn't keep the
/// menu open across line breaks.
#[must_use]
pub fn detect_slash(doc: &str, caret: usize) -> Option<(usize, String)> {
    let caret = caret.min(doc.len());
    let line_start = doc[..caret].rfind('\n').map_or(0, |n| n + 1);
    let segment = &doc[line_start..caret];
    let bytes = segment.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        let c = bytes[i - 1];
        if c == b'/' {
            // Reject URL-shaped patterns: `://` (after a scheme
            // like `https:`) and `//` (already inside a URL).
            // Everything else triggers — typing `/` after
            // ordinary text in the middle of a paragraph should
            // open the menu, the way Notion / most modern
            // editors do it.
            if i >= 2 && (bytes[i - 2] == b':' || bytes[i - 2] == b'/') {
                return None;
            }
            let query = segment[i..].to_string();
            return Some((line_start + i - 1, query));
        }
        if (c as char).is_whitespace() {
            return None;
        }
        i -= 1;
    }
    None
}

/// Case-insensitive substring filter over label / group / desc.
#[must_use]
pub fn filter_commands(query: &str) -> Vec<CommandEntry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return all_commands();
    }
    all_commands()
        .into_iter()
        .filter(|c| {
            c.label.to_lowercase().contains(&q)
                || c.group.to_lowercase().contains(&q)
                || c.desc.to_lowercase().contains(&q)
        })
        .collect()
}

/// Resolve a picked command into a `TransactionSpec`. Removes
/// the `/query` first, then either splices a snippet or runs a
/// line-level transform (heading / list / task).
#[must_use]
pub fn run_command(
    state: &EditorState,
    slash_range: std::ops::Range<usize>,
    cmd: CommandKind,
) -> Option<TransactionSpec> {
    let doc = state.doc.to_string();
    if slash_range.end > doc.len() || slash_range.start > slash_range.end {
        return None;
    }
    match cmd {
        CommandKind::InsertSnippet(text, caret_back) => {
            let new_caret = slash_range.start + text.len() - caret_back;
            Some(
                TransactionSpec::new()
                    .changes(Changes::replace(slash_range, text))
                    .selection(Selection::caret(new_caret))
                    .annotate("origin", "slash"),
            )
        }
        CommandKind::InsertBlockSnippet(text, caret_back) => {
            // Snap to the start of the current line. If there's
            // other text before the slash on this line, drop the
            // whole block on a fresh line below.
            let line_start = doc[..slash_range.start].rfind('\n').map_or(0, |n| n + 1);
            let prefix_text = &doc[line_start..slash_range.start];
            let line_has_content = !prefix_text.trim().is_empty();
            let snippet = if line_has_content {
                format!("\n{text}")
            } else {
                text.to_string()
            };
            // Build the final doc directly: strip the `/query`,
            // then insert the block snippet at the line-aware
            // anchor.
            let before = &doc[..slash_range.start];
            let after = &doc[slash_range.end..];
            let stripped = format!("{before}{after}");
            let anchor = if line_has_content {
                slash_range.start
            } else {
                line_start
            };
            let head = &stripped[..anchor];
            let tail = &stripped[anchor..];
            let final_doc = format!("{head}{snippet}{tail}");
            let new_caret = anchor + snippet.len() - caret_back;
            Some(
                TransactionSpec::new()
                    .changes(Changes::replace(0..doc.len(), final_doc))
                    .selection(Selection::caret(new_caret))
                    .annotate("origin", "slash"),
            )
        }
        CommandKind::SetHeading(level) => {
            // Replace the whole line containing the slash range
            // with a heading-prefixed version of its non-slash
            // content. Doing this as one atomic replace avoids
            // the bug where stripping the slash separately and
            // then calling `set_heading` on a synthetic doc
            // produces offsets that don't map back to the real
            // doc.
            let (line_start, line_end) = line_bounds(&doc, &slash_range);
            let body = line_without_slash(&doc, line_start, line_end, &slash_range);
            let body = strip_heading(&body);
            let new_line = if level == 0 {
                body.to_string()
            } else {
                let prefix = "#".repeat(level as usize);
                format!("{prefix} {body}")
            };
            let caret = line_start + new_line.len();
            Some(
                TransactionSpec::new()
                    .changes(Changes::replace(line_start..line_end, new_line))
                    .selection(Selection::caret(caret))
                    .annotate("origin", "slash"),
            )
        }
        CommandKind::SetList(kind) => {
            let target = match kind {
                ListKind::Unordered => "- ",
                ListKind::Ordered => "1. ",
                ListKind::Task => "- [ ] ",
            };
            let (line_start, line_end) = line_bounds(&doc, &slash_range);
            let body = line_without_slash(&doc, line_start, line_end, &slash_range);
            let body = strip_list_marker(&body);
            let new_line = format!("{target}{body}");
            let caret = line_start + new_line.len();
            Some(
                TransactionSpec::new()
                    .changes(Changes::replace(line_start..line_end, new_line))
                    .selection(Selection::caret(caret))
                    .annotate("origin", "slash"),
            )
        }
        CommandKind::ToggleTask => {
            let (line_start, line_end) = line_bounds(&doc, &slash_range);
            let body = line_without_slash(&doc, line_start, line_end, &slash_range);
            // If the line already starts with `- [ ]`/`- [x]`,
            // flip it; otherwise promote to task.
            let b = body.as_bytes();
            let is_task = b.len() >= 5
                && matches!(b[0], b'-' | b'*' | b'+')
                && b[1] == b' '
                && b[2] == b'['
                && b[4] == b']';
            let new_line = if is_task {
                let inner = b[3];
                let new_inner = if inner == b' ' { 'x' } else { ' ' };
                let mut bs = b.to_vec();
                bs[3] = new_inner as u8;
                String::from_utf8(bs).unwrap_or(body.clone())
            } else {
                format!("- [ ] {body}")
            };
            let caret = line_start + new_line.len();
            Some(
                TransactionSpec::new()
                    .changes(Changes::replace(line_start..line_end, new_line))
                    .selection(Selection::caret(caret))
                    .annotate("origin", "slash"),
            )
        }
        CommandKind::AddBlockId => {
            // Strip the slash first by reconstructing the line,
            // then run `add_block_id` on the result.
            let (line_start, line_end) = line_bounds(&doc, &slash_range);
            let body = line_without_slash(&doc, line_start, line_end, &slash_range);
            // Build a synthetic state with the slash removed
            // and run the helper. Inline transcription avoids a
            // direct dep on the helper's `(spec, ref_str)`
            // shape — we just compute the final line.
            if body.trim().is_empty() {
                return None;
            }
            let uuid = uuid_v4_string();
            let new_text = format!("{body}\nid:: {uuid}");
            let caret = line_start + body.len();
            Some(
                TransactionSpec::new()
                    .changes(Changes::replace(line_start..line_end, new_text))
                    .selection(Selection::caret(caret))
                    .annotate("origin", "slash-block-id"),
            )
        }
    }
}

fn uuid_v4_string() -> String {
    // Re-export point — keep `uuid` as an editor-state dep only.
    uuid::Uuid::now_v7().to_string()
}

/// Byte-range of the (single) line containing the slash. The
/// slash range is guaranteed to live on one line because the
/// parser closes on newlines.
fn line_bounds(doc: &str, slash_range: &std::ops::Range<usize>) -> (usize, usize) {
    let line_start = doc[..slash_range.start].rfind('\n').map_or(0, |n| n + 1);
    let line_end = doc[slash_range.end..]
        .find('\n')
        .map_or(doc.len(), |n| slash_range.end + n);
    (line_start, line_end)
}

/// Reconstruct the line text with the `/query` removed.
fn line_without_slash(
    doc: &str,
    line_start: usize,
    line_end: usize,
    slash_range: &std::ops::Range<usize>,
) -> String {
    let mut out = String::with_capacity(line_end - line_start);
    out.push_str(&doc[line_start..slash_range.start]);
    out.push_str(&doc[slash_range.end..line_end]);
    out
}

/// Strip a leading `#…# ` heading marker (1-6 hashes + space).
fn strip_heading(line: &str) -> &str {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line.as_bytes().get(hashes).copied() == Some(b' ') {
        &line[hashes + 1..]
    } else {
        line
    }
}

fn strip_list_marker(line: &str) -> &str {
    let b = line.as_bytes();
    // task: `- [X] `
    if b.len() >= 6
        && matches!(b[0], b'-' | b'*' | b'+')
        && b[1] == b' '
        && b[2] == b'['
        && b[4] == b']'
        && b[5] == b' '
    {
        return &line[6..];
    }
    // ordered: `N. `
    if b.len() >= 3 && b[0].is_ascii_digit() && b[1] == b'.' && b[2] == b' ' {
        return &line[3..];
    }
    // unordered: `- `
    if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+') && b[1] == b' ' {
        return &line[2..];
    }
    line
}

/// The full catalog. Lives as a function (not const) so the
/// `&'static str` text gets the right ownership semantics
/// through the filter pipeline.
#[must_use]
pub fn all_commands() -> Vec<CommandEntry> {
    let mut out = Vec::new();

    // ── Headings ───────────────────────────────────────────
    for (level, label) in [
        (1u8, "Heading 1"),
        (2, "Heading 2"),
        (3, "Heading 3"),
        (4, "Heading 4"),
        (5, "Heading 5"),
        (6, "Heading 6"),
    ] {
        out.push(CommandEntry {
            label,
            group: "Heading",
            desc: "",
            kind: CommandKind::SetHeading(level),
            icon: "H",
        });
    }

    // ── Lists ──────────────────────────────────────────────
    out.extend([
        CommandEntry {
            label: "Bulleted list",
            group: "Structure",
            desc: "- item",
            kind: CommandKind::SetList(ListKind::Unordered),
            icon: "•",
        },
        CommandEntry {
            label: "Numbered list",
            group: "Structure",
            desc: "1. item",
            kind: CommandKind::SetList(ListKind::Ordered),
            icon: "1.",
        },
        CommandEntry {
            label: "Task",
            group: "Structure",
            desc: "- [ ] task",
            kind: CommandKind::SetList(ListKind::Task),
            icon: "☐",
        },
        CommandEntry {
            label: "Toggle task",
            group: "Structure",
            desc: "Mark current line as / un-task",
            kind: CommandKind::ToggleTask,
            icon: "☑",
        },
        CommandEntry {
            label: "Quote",
            group: "Structure",
            desc: "> blockquote",
            kind: CommandKind::InsertSnippet("> ", 0),
            icon: "❝",
        },
        CommandEntry {
            label: "Horizontal rule",
            group: "Structure",
            desc: "---",
            kind: CommandKind::InsertBlockSnippet("---\n", 0),
            icon: "—",
        },
        CommandEntry {
            label: "Table",
            group: "Structure",
            desc: "GFM pipe table skeleton",
            kind: CommandKind::InsertBlockSnippet(
                "| col1 | col2 |\n| ---- | ---- |\n|      |      |\n",
                21,
            ),
            icon: "▦",
        },
    ]);

    // ── Code & math ────────────────────────────────────────
    out.extend([
        CommandEntry {
            label: "Code block",
            group: "Code",
            desc: "```lang \\n … \\n```",
            kind: CommandKind::InsertBlockSnippet("```\n\n```\n", 5),
            icon: "{}",
        },
        CommandEntry {
            label: "Rust code block",
            group: "Code",
            desc: "```rust",
            kind: CommandKind::InsertBlockSnippet("```rust\n\n```\n", 5),
            icon: "🦀",
        },
        CommandEntry {
            label: "TypeScript code block",
            group: "Code",
            desc: "```ts",
            kind: CommandKind::InsertBlockSnippet("```ts\n\n```\n", 5),
            icon: "TS",
        },
        CommandEntry {
            label: "Typst block",
            group: "Code",
            desc: "Compiled Typst — math, diagrams, layout",
            kind: CommandKind::InsertBlockSnippet("```typst\n\n```\n", 5),
            icon: "𝒯",
        },
        CommandEntry {
            label: "Mermaid diagram",
            group: "Code",
            desc: "```mermaid",
            kind: CommandKind::InsertBlockSnippet("```mermaid\n\n```\n", 5),
            icon: "⤳",
        },
        CommandEntry {
            label: "Tabs",
            group: "Code",
            desc: "Switchable tab panels",
            // Caret lands after the first `=== Tab 1\n` so the
            // user types straight into the opening panel.
            kind: CommandKind::InsertBlockSnippet("```tabs\n=== Tab 1\n\n=== Tab 2\n\n```\n", 16),
            icon: "⑃",
        },
        CommandEntry {
            label: "Inline math",
            group: "Math",
            desc: "$x$",
            kind: CommandKind::InsertSnippet("$$", 1),
            icon: "∑",
        },
        CommandEntry {
            label: "Math block",
            group: "Math",
            desc: "$$\\n…\\n$$",
            kind: CommandKind::InsertBlockSnippet("$$\n\n$$\n", 4),
            icon: "∫",
        },
        CommandEntry {
            label: "Keyboard shortcut",
            group: "Code",
            desc: "`kbd:<C-s>` — rendered as key caps",
            kind: CommandKind::InsertSnippet("`kbd:`", 1),
            icon: "⌘",
        },
        CommandEntry {
            label: "Shortcut for action",
            group: "Code",
            desc: "`kbd:@action` — the keys currently bound to an action id",
            kind: CommandKind::InsertSnippet("`kbd:@`", 1),
            icon: "⌘",
        },
    ]);

    // ── Callouts ────────────────────────────────────────────
    // All 13 canonical Obsidian types. Snippet always has a
    // trailing space + newline + body so `caret_back = 3` lands
    // the caret on the title line (right after `> [!type] `),
    // ready for the user to type the title. Next Enter takes
    // them to the body.
    // No `Todo` callout — that's what the `- [ ]` task list
    // is for. Picking `/task` is the right path for actionable
    // items; the callout was redundant with it.
    for (kind, label, icon) in [
        ("note", "Note", "📝"),
        ("abstract", "Abstract", "📄"),
        ("info", "Info", "ⓘ"),
        ("tip", "Tip", "💡"),
        ("success", "Success", "✅"),
        ("question", "Question", "❓"),
        ("warning", "Warning", "⚠"),
        ("failure", "Failure", "❌"),
        ("danger", "Danger", "⚡"),
        ("bug", "Bug", "🐞"),
        ("example", "Example", "🧪"),
        ("quote", "Quote", "❞"),
    ] {
        // Just the header line with a trailing space — caret
        // ends right after `> [!type] ` ready for the title.
        // User hits Enter to drop into the body; the existing
        // `enter_continue_list` command picks up the
        // blockquote prefix and inserts `> ` on the next line.
        let snippet: &'static str = match kind {
            "note" => "> [!note] ",
            "abstract" => "> [!abstract] ",
            "info" => "> [!info] ",
            "tip" => "> [!tip] ",
            "success" => "> [!success] ",
            "question" => "> [!question] ",
            "warning" => "> [!warning] ",
            "failure" => "> [!failure] ",
            "danger" => "> [!danger] ",
            "bug" => "> [!bug] ",
            "example" => "> [!example] ",
            "quote" => "> [!quote] ",
            _ => unreachable!(),
        };
        out.push(CommandEntry {
            label,
            group: "Callout",
            desc: "",
            kind: CommandKind::InsertBlockSnippet(snippet, 0),
            icon,
        });
    }

    // ── Block IDs / refs (Logseq-style) ─────────────────────
    // `Mod-Shift-K` is the keymap binding; the slash entry is
    // a discoverability path.
    out.push(CommandEntry {
        label: "Block id",
        group: "Block",
        desc: "Give this block an id so it can be referenced",
        kind: CommandKind::AddBlockId,
        icon: "🔗",
    });

    // ── Embeds & links ─────────────────────────────────────
    out.extend([
        CommandEntry {
            label: "Link",
            group: "Link",
            desc: "[text](url)",
            kind: CommandKind::InsertSnippet("[]()", 3),
            icon: "🔗",
        },
        CommandEntry {
            label: "Wikilink",
            group: "Link",
            desc: "[[Page]]",
            kind: CommandKind::InsertSnippet("[[]]", 2),
            icon: "[[",
        },
        CommandEntry {
            label: "Embed",
            group: "Link",
            desc: "![[file]] — image / audio / video / pdf",
            kind: CommandKind::InsertSnippet("![[]]", 2),
            icon: "🖼",
        },
        CommandEntry {
            label: "Footnote ref",
            group: "Link",
            desc: "[^id]",
            kind: CommandKind::InsertSnippet("[^]", 1),
            icon: "ⁿ",
        },
        CommandEntry {
            label: "Inline footnote",
            group: "Link",
            desc: "^[note]",
            kind: CommandKind::InsertSnippet("^[]", 1),
            icon: "ⁿ",
        },
    ]);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_slash_at_start_of_doc() {
        assert_eq!(detect_slash("/cal", 4), Some((0, "cal".to_string())));
    }

    #[test]
    fn detect_slash_after_whitespace() {
        assert_eq!(detect_slash("hi /code", 8), Some((3, "code".to_string())));
    }

    #[test]
    fn detect_slash_ignores_url_form() {
        // `://` after a scheme — common URL pattern, must not
        // open the menu when the cursor is right after the
        // second `/`.
        assert_eq!(detect_slash("https://", 8), None);
        // Inside a URL path, e.g. `https://foo/bar` — the
        // second `/` is preceded by another `/` already in
        // the URL.
        assert_eq!(detect_slash("foo//", 5), None);
    }

    #[test]
    fn detect_slash_triggers_after_ordinary_text() {
        // Typing `/` at the end of normal prose should open
        // the menu — matches Notion / most modern editors. The
        // previous Logseq-strict behavior wouldn't fire here.
        assert_eq!(detect_slash("hello/", 6), Some((5, String::new())));
        assert_eq!(detect_slash("hello/cal", 9), Some((5, "cal".to_string())));
    }

    #[test]
    fn detect_slash_closes_on_space() {
        assert_eq!(detect_slash("/foo bar", 8), None);
    }

    #[test]
    fn detect_slash_scoped_to_current_line() {
        // A slash on a previous line shouldn't keep the menu
        // open across newlines.
        assert_eq!(detect_slash("/old\nnew here", 13), None);
    }

    #[test]
    fn filter_matches_group() {
        let hits = filter_commands("call");
        // After the label rename, callouts are matched by group
        // (`"Callout"`) rather than by the label itself.
        assert!(hits.iter().any(|c| c.group == "Callout"));
    }
}
