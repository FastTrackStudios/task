//! Minimal markdown → rsx renderer for task `details` bodies.
//!
//! Why not reuse a heavier renderer: the vault editor renders
//! markdown through its block-editor state machine (overkill
//! for a read-only PRD body), and `chat-ui`'s renderer lives in
//! the upstream architect-ui repo, not this workspace. So we parse
//! with the workspace's existing `pulldown-cmark` dep and emit
//! rsx natively. Two deliberate properties:
//!
//! - **No raw HTML.** `Event::Html` / `Event::InlineHtml` are
//!   rendered as literal text (rsx escapes them), so a task body
//!   can't inject script into the app shell.
//! - **Testable intermediate.** [`parse_markdown`] produces a
//!   plain block tree ([`MdBlock`]) that unit tests assert on
//!   without a Dioxus runtime; the [`Markdown`] component is a
//!   dumb tree-walk over it.
//!
//! Coverage: headings, paragraphs, bold / italic / strike,
//! inline code, fenced + indented code blocks, ordered /
//! unordered / task lists (nested), blockquotes, links, rules.
//! Tables and footnotes are not enabled — they degrade to plain
//! text, which is fine for PRD bodies until someone needs them.

use dioxus::prelude::*;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

// ---------------------------------------------------------------------------
// Block tree
// ---------------------------------------------------------------------------

/// Inline span inside a paragraph / heading / list item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MdInline {
    Text(String),
    Code(String),
    Strong(Vec<MdInline>),
    Em(Vec<MdInline>),
    Strike(Vec<MdInline>),
    Link {
        href: String,
        children: Vec<MdInline>,
    },
    /// Hard line break (`two-space\n` or `\\\n`).
    Break,
}

/// One list item — `checked` is `Some(_)` for task-list items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdListItem {
    pub checked: Option<bool>,
    pub blocks: Vec<MdBlock>,
}

/// Block-level node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MdBlock {
    Heading {
        level: u8,
        inlines: Vec<MdInline>,
    },
    Paragraph(Vec<MdInline>),
    Code {
        lang: String,
        code: String,
    },
    Quote(Vec<MdBlock>),
    List {
        ordered: bool,
        items: Vec<MdListItem>,
    },
    Rule,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse a markdown string into a renderable block tree.
#[must_use]
pub fn parse_markdown(src: &str) -> Vec<MdBlock> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);

    // Container stacks. `blocks` grows a level per blockquote /
    // list item; `inlines` a level per paragraph / heading /
    // emphasis span. Tight list items emit bare `Text` without
    // a `Paragraph` wrapper, so text falling outside any inline
    // frame opens an *implicit* paragraph that's flushed at the
    // next block boundary.
    let mut blocks: Vec<Vec<MdBlock>> = vec![Vec::new()];
    let mut inlines: Vec<Vec<MdInline>> = Vec::new();
    let mut implicit_para = false;
    let mut lists: Vec<(bool, Vec<MdListItem>)> = Vec::new();
    let mut item_checked: Vec<Option<bool>> = Vec::new();
    let mut code: Option<(String, String)> = None;
    // `Tag::Link`'s dest is only on the Start event but the
    // children arrive before End — stack in case of nesting
    // (links can't nest, but a link inside an image alt can).
    let mut link_dests: Vec<String> = Vec::new();

    // Close an implicit (tight list) paragraph if one is open.
    macro_rules! flush_implicit {
        () => {
            if implicit_para {
                implicit_para = false;
                if let Some(v) = inlines.pop() {
                    if !v.is_empty() {
                        if let Some(parent) = blocks.last_mut() {
                            parent.push(MdBlock::Paragraph(v));
                        }
                    }
                }
            }
        };
    }
    // Current inline buffer, opening an implicit paragraph when
    // text arrives outside any explicit inline container.
    macro_rules! cur_inlines {
        () => {{
            if inlines.is_empty() {
                inlines.push(Vec::new());
                implicit_para = true;
            }
            inlines.last_mut().expect("non-empty inline stack")
        }};
    }

    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    flush_implicit!();
                    inlines.push(Vec::new());
                }
                Tag::Heading { .. } => {
                    flush_implicit!();
                    inlines.push(Vec::new());
                }
                Tag::BlockQuote => {
                    flush_implicit!();
                    blocks.push(Vec::new());
                }
                Tag::CodeBlock(kind) => {
                    flush_implicit!();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    flush_implicit!();
                    lists.push((start.is_some(), Vec::new()));
                }
                Tag::Item => {
                    flush_implicit!();
                    blocks.push(Vec::new());
                    item_checked.push(None);
                }
                Tag::Emphasis | Tag::Strong | Tag::Strikethrough => {
                    // Ensure a parent inline frame exists (tight
                    // list items emit spans without Paragraph).
                    let _ = cur_inlines!();
                    inlines.push(Vec::new());
                }
                Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                    let _ = cur_inlines!();
                    link_dests.push(dest_url.to_string());
                    inlines.push(Vec::new());
                }
                // Tables / footnotes / metadata aren't enabled;
                // their inner Text events land in an implicit
                // paragraph, which is an acceptable degrade.
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if let (Some(v), Some(parent)) = (inlines.pop(), blocks.last_mut()) {
                        parent.push(MdBlock::Paragraph(v));
                    }
                }
                TagEnd::Heading(level) => {
                    if let (Some(v), Some(parent)) = (inlines.pop(), blocks.last_mut()) {
                        parent.push(MdBlock::Heading {
                            level: level as u8,
                            inlines: v,
                        });
                    }
                }
                TagEnd::BlockQuote => {
                    flush_implicit!();
                    if let Some(inner) = blocks.pop() {
                        if let Some(parent) = blocks.last_mut() {
                            parent.push(MdBlock::Quote(inner));
                        }
                    }
                }
                TagEnd::CodeBlock => {
                    if let (Some((lang, body)), Some(parent)) = (code.take(), blocks.last_mut()) {
                        parent.push(MdBlock::Code { lang, code: body });
                    }
                }
                TagEnd::Item => {
                    flush_implicit!();
                    if let Some(inner) = blocks.pop() {
                        if let Some((_, items)) = lists.last_mut() {
                            items.push(MdListItem {
                                checked: item_checked.pop().flatten(),
                                blocks: inner,
                            });
                        }
                    }
                }
                TagEnd::List(_) => {
                    if let (Some((ordered, items)), Some(parent)) = (lists.pop(), blocks.last_mut())
                    {
                        parent.push(MdBlock::List { ordered, items });
                    }
                }
                TagEnd::Emphasis => {
                    if let Some(v) = inlines.pop() {
                        cur_inlines!().push(MdInline::Em(v));
                    }
                }
                TagEnd::Strong => {
                    if let Some(v) = inlines.pop() {
                        cur_inlines!().push(MdInline::Strong(v));
                    }
                }
                TagEnd::Strikethrough => {
                    if let Some(v) = inlines.pop() {
                        cur_inlines!().push(MdInline::Strike(v));
                    }
                }
                TagEnd::Link | TagEnd::Image => {
                    if let Some(v) = inlines.pop() {
                        let href = link_dests.pop().unwrap_or_default();
                        cur_inlines!().push(MdInline::Link { href, children: v });
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some((_, body)) = code.as_mut() {
                    body.push_str(&t);
                } else {
                    cur_inlines!().push(MdInline::Text(t.to_string()));
                }
            }
            Event::Code(t) => cur_inlines!().push(MdInline::Code(t.to_string())),
            // Raw HTML stays inert: rendered as literal text.
            Event::Html(t) | Event::InlineHtml(t) => {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    cur_inlines!().push(MdInline::Text(trimmed.to_string()));
                }
            }
            Event::SoftBreak => cur_inlines!().push(MdInline::Text(" ".to_string())),
            Event::HardBreak => cur_inlines!().push(MdInline::Break),
            Event::Rule => {
                flush_implicit!();
                if let Some(parent) = blocks.last_mut() {
                    parent.push(MdBlock::Rule);
                }
            }
            Event::TaskListMarker(done) => {
                if let Some(slot) = item_checked.last_mut() {
                    *slot = Some(done);
                }
            }
            Event::FootnoteReference(_) => {}
        }
    }

    // Trailing implicit paragraph (degenerate inputs).
    if implicit_para {
        if let Some(v) = inlines.pop() {
            if !v.is_empty() {
                if let Some(parent) = blocks.last_mut() {
                    parent.push(MdBlock::Paragraph(v));
                }
            }
        }
    }

    blocks.pop().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render markdown `source` as styled rsx.
#[component]
pub fn Markdown(source: String, #[props(default)] class: String) -> Element {
    let blocks = parse_markdown(&source);
    rsx! {
        div { class: "flex flex-col gap-2 text-sm text-foreground {class}",
            {render_blocks(&blocks)}
        }
    }
}

fn render_blocks(blocks: &[MdBlock]) -> Element {
    rsx! {
        for (i , b) in blocks.iter().enumerate() {
            {render_block(i, b)}
        }
    }
}

fn render_block(key: usize, block: &MdBlock) -> Element {
    match block {
        MdBlock::Heading { level, inlines } => {
            let cls = match level {
                1 => "text-lg font-semibold text-foreground mt-2",
                2 => "text-base font-semibold text-foreground mt-2",
                3 => "text-sm font-semibold text-foreground mt-1",
                _ => "text-sm font-medium text-muted-foreground uppercase tracking-wide mt-1",
            };
            rsx! {
                div { key: "{key}", class: "{cls}", {render_inlines(inlines)} }
            }
        }
        MdBlock::Paragraph(inlines) => rsx! {
            p { key: "{key}", class: "leading-relaxed", {render_inlines(inlines)} }
        },
        MdBlock::Code { lang, code } => rsx! {
            pre {
                key: "{key}",
                class: "rounded-md border border-border bg-muted/30 p-2 text-xs overflow-x-auto",
                "data-lang": "{lang}",
                code { "{code}" }
            }
        },
        MdBlock::Quote(inner) => rsx! {
            blockquote {
                key: "{key}",
                class: "border-l-2 border-border pl-3 text-muted-foreground flex flex-col gap-2",
                {render_blocks(inner)}
            }
        },
        MdBlock::List { ordered, items } => {
            let tag_cls = if *ordered {
                "flex flex-col gap-1 pl-5 list-decimal"
            } else {
                "flex flex-col gap-1 pl-5 list-disc"
            };
            let inner = rsx! {
                for (i , item) in items.iter().enumerate() {
                    li {
                        key: "{i}",
                        class: if item.checked.is_some() { "list-none -ml-5 flex items-start gap-1.5" } else { "" },
                        if let Some(done) = item.checked {
                            span {
                                class: if done { "mt-0.5 flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-sm border border-emerald-500 bg-emerald-500/80 text-[9px] text-white" } else { "mt-0.5 flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-sm border border-border" },
                                if done { "✓" }
                            }
                        }
                        div { class: "flex-1 min-w-0 flex flex-col gap-1",
                            {render_blocks(&item.blocks)}
                        }
                    }
                }
            };
            if *ordered {
                rsx! {
                    ol { key: "{key}", class: "{tag_cls}", {inner} }
                }
            } else {
                rsx! {
                    ul { key: "{key}", class: "{tag_cls}", {inner} }
                }
            }
        }
        MdBlock::Rule => rsx! {
            hr { key: "{key}", class: "border-border" }
        },
    }
}

fn render_inlines(inlines: &[MdInline]) -> Element {
    rsx! {
        for (i , inline) in inlines.iter().enumerate() {
            {render_inline(i, inline)}
        }
    }
}

fn render_inline(key: usize, inline: &MdInline) -> Element {
    match inline {
        MdInline::Text(t) => rsx! {
            span { key: "{key}", "{t}" }
        },
        MdInline::Code(t) => rsx! {
            code {
                key: "{key}",
                class: "rounded bg-muted/50 px-1 py-0.5 text-[0.85em] font-mono",
                "{t}"
            }
        },
        MdInline::Strong(children) => rsx! {
            strong { key: "{key}", class: "font-semibold", {render_inlines(children)} }
        },
        MdInline::Em(children) => rsx! {
            em { key: "{key}", {render_inlines(children)} }
        },
        MdInline::Strike(children) => rsx! {
            del { key: "{key}", class: "line-through text-muted-foreground",
                {render_inlines(children)}
            }
        },
        MdInline::Link { href, children } => rsx! {
            a {
                key: "{key}",
                class: "text-primary underline underline-offset-2 hover:opacity-80",
                href: "{href}",
                target: "_blank",
                rel: "noopener noreferrer",
                {render_inlines(children)}
            }
        },
        MdInline::Break => rsx! {
            br { key: "{key}" }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_and_paragraph() {
        let blocks = parse_markdown("# Title\n\nBody text.");
        assert_eq!(
            blocks,
            vec![
                MdBlock::Heading {
                    level: 1,
                    inlines: vec![MdInline::Text("Title".into())],
                },
                MdBlock::Paragraph(vec![MdInline::Text("Body text.".into())]),
            ]
        );
    }

    #[test]
    fn emphasis_nesting() {
        let blocks = parse_markdown("some **bold _both_** text");
        assert_eq!(
            blocks,
            vec![MdBlock::Paragraph(vec![
                MdInline::Text("some ".into()),
                MdInline::Strong(vec![
                    MdInline::Text("bold ".into()),
                    MdInline::Em(vec![MdInline::Text("both".into())]),
                ]),
                MdInline::Text(" text".into()),
            ])]
        );
    }

    #[test]
    fn tight_task_list() {
        let blocks = parse_markdown("- [x] done thing\n- [ ] open thing\n- plain");
        let MdBlock::List { ordered, items } = &blocks[0] else {
            panic!("expected list, got {blocks:?}");
        };
        assert!(!ordered);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].checked, Some(true));
        assert_eq!(items[1].checked, Some(false));
        assert_eq!(items[2].checked, None);
        // Tight items: the bare Text lands in an implicit paragraph.
        assert_eq!(
            items[0].blocks,
            vec![MdBlock::Paragraph(vec![MdInline::Text(
                "done thing".into()
            )])]
        );
    }

    #[test]
    fn fenced_code_block_keeps_lang() {
        let blocks = parse_markdown("```rust\nfn main() {}\n```");
        assert_eq!(
            blocks,
            vec![MdBlock::Code {
                lang: "rust".into(),
                code: "fn main() {}\n".into(),
            }]
        );
    }

    #[test]
    fn blockquote_and_rule() {
        let blocks = parse_markdown("> quoted\n\n---");
        assert_eq!(
            blocks,
            vec![
                MdBlock::Quote(vec![MdBlock::Paragraph(vec![MdInline::Text(
                    "quoted".into()
                )])]),
                MdBlock::Rule,
            ]
        );
    }

    #[test]
    fn links_resolve_dest() {
        let blocks = parse_markdown("see [the spec](https://example.com/spec)");
        assert_eq!(
            blocks,
            vec![MdBlock::Paragraph(vec![
                MdInline::Text("see ".into()),
                MdInline::Link {
                    href: "https://example.com/spec".into(),
                    children: vec![MdInline::Text("the spec".into())],
                },
            ])]
        );
    }

    #[test]
    fn raw_html_is_inert_text() {
        let blocks = parse_markdown("hello <script>alert(1)</script> world");
        // Whatever the exact split, no block may carry anything
        // other than Text/plain inlines — i.e. the script tag
        // survives only as literal text.
        let MdBlock::Paragraph(inlines) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(
            inlines
                .iter()
                .all(|i| matches!(i, MdInline::Text(_) | MdInline::Code(_)))
        );
        let joined: String = inlines
            .iter()
            .map(|i| match i {
                MdInline::Text(t) | MdInline::Code(t) => t.as_str(),
                _ => "",
            })
            .collect();
        assert!(joined.contains("<script>"), "got: {joined}");
    }

    #[test]
    fn nested_list_in_item() {
        let blocks = parse_markdown("- outer\n  - inner");
        let MdBlock::List { items, .. } = &blocks[0] else {
            panic!("expected list");
        };
        assert_eq!(items.len(), 1);
        // The outer item holds the implicit paragraph + nested list.
        assert!(matches!(items[0].blocks[0], MdBlock::Paragraph(_)));
        assert!(matches!(items[0].blocks[1], MdBlock::List { .. }));
    }
}
