//! Markdown live-preview decoration source.
//!
//! Scans the doc for `**…**` (bold), `*…*` (italic), and
//! `` `…` `` (inline code) spans and emits decorations:
//!
//! - The body of the span gets a `MarkDecoration` with the
//!   corresponding class (`md-bold`, `md-italic`, `md-code`).
//! - The opening + closing markers are `Replace`d **only when
//!   the primary cursor is outside the span**. While the cursor
//!   is on the span, markers stay visible so the user sees
//!   the raw markdown and can edit it directly.
//!
//! This is exactly Obsidian's "Live Preview" mode in spirit —
//! it's a renderer trick, not a document-model change.
//!
//! The parser is single-pass and intentionally tiny. Not a real
//! `CommonMark` implementation; just enough to demo the
//! decoration pipeline. A future commit can swap in a proper
//! markdown parser (pulldown-cmark or a port of CM6's
//! lang-markdown) without touching the decoration shape.

use crate::decoration::{DecoratedRange, Decoration};
use crate::selection::Range;
use crate::state::EditorState;

/// The full live-preview decoration source. Suitable to register
/// as `editor_view::DecorationSource`.
/// Trait the editor uses to resolve cross-file references —
/// `((uuid))`, `[[Page]]`, `![[Page#Heading]]`, etc. — without
/// pulling a vault implementation into `editor-state`. The
/// `vault` crate provides the canonical impl; tests / single-
/// file uses pass `None`.
pub trait VaultLookup {
    /// Find a block by full UUID across the vault. Returns the
    /// containing page's basename and a short preview.
    fn lookup_block(&self, uuid: &str) -> Option<VaultBlockHit>;
    /// Find a page by basename (case-insensitive). Returns a
    /// content preview suitable for an embed card.
    fn lookup_page(&self, name: &str) -> Option<VaultPageHit>;
    /// Find a section `Page#Heading`. Returns the body of the
    /// section (heading line + content until next same-or-
    /// higher heading), or None when the page or heading is
    /// missing.
    fn lookup_section(&self, page: &str, heading: &str) -> Option<String>;
    /// Song metadata when `name` resolves to a `type: song` note —
    /// `None` (default) renders the wikilink normally.
    fn lookup_song(&self, _name: &str) -> Option<VaultSongHit> {
        None
    }
    /// The target note's frontmatter `type:` ("song", "setlist",
    /// "contact", "event", …) — drives kind-specific wikilink rendering
    /// (setlist cards, contact chips). `None` (default) = plain link.
    fn lookup_note_kind(&self, _name: &str) -> Option<String> {
        None
    }
    /// Setlist metadata when `name` resolves to a `type: setlist` note.
    fn lookup_setlist(&self, _name: &str) -> Option<VaultSetlistHit> {
        None
    }
    /// Scripture reference resolution: when `target` parses as a verse
    /// reference (`John 3:16`, `John 3:16-20`, `Rom 5:8@ESV`) the host
    /// returns display info + (possibly still loading) verse text, and
    /// the link renders as a scripture chip / verse card instead of an
    /// unresolved wikilink. Only consulted when no page matches the
    /// target, so a real `John 3:16.md` note still wins. `None`
    /// (default) = plain link.
    fn lookup_scripture(&self, _target: &str) -> Option<VaultScriptureHit> {
        None
    }
    /// Find a block by Obsidian short-id `Page#^id`.
    fn lookup_block_short(&self, page: &str, short_id: &str) -> Option<String>;
}

#[derive(Clone, Debug)]
pub struct VaultBlockHit {
    pub page: String,
    pub preview: String,
}

#[derive(Clone, Debug)]
pub struct VaultPageHit {
    pub preview: String,
}

/// Setlist metadata for a wikilink that targets a `type: setlist` note —
/// drives the inline SETLIST CARD (a standalone `[[Setlist]]` line embeds
/// the set as a compact card).
#[derive(Clone, Debug, PartialEq)]
pub struct VaultSetlistHit {
    pub title: String,
    pub song_count: usize,
    pub total_seconds: f64,
    /// The set's songs, in order — the embed renders the full reference
    /// player (header + one row per song).
    pub songs: Vec<VaultSetlistSongRow>,
}

/// One song row inside a setlist embed.
#[derive(Clone, Debug, PartialEq)]
pub struct VaultSetlistSongRow {
    /// The wikilink target (note name) — drives navigation + play.
    pub link: String,
    pub artist: Option<String>,
    pub duration_sec: f64,
    pub stem_count: usize,
}

/// A wikilink target that parses as a scripture reference — drives the
/// inline SCRIPTURE CHIP (any `[[John 3:16]]` in running text) and the
/// VERSE CARD (a standalone `[[John 3:16]]` line embeds the verse text).
#[derive(Clone, Debug, PartialEq)]
pub struct VaultScriptureHit {
    /// Canonical display reference, e.g. `John 3:16–20`.
    pub display: String,
    /// OSIS id / range (`John.3.16`), the stable anchor key.
    pub osis: String,
    /// The verse text (range text joined), or `None` while the host is
    /// still fetching — the card shows a loading row, the chip renders
    /// resolved either way.
    pub text: Option<String>,
    /// Translation id the text came from (`WEB`, `ESV`).
    pub translation: String,
}

/// Song metadata for a wikilink that targets a `type: song` note —
/// drives the inline SONG STRIP widget (a standalone `[[Song]]` line
/// renders as a playable row instead of a plain link).
#[derive(Clone, Debug, PartialEq)]
pub struct VaultSongHit {
    pub title: String,
    pub artist: Option<String>,
    pub duration_sec: f64,
    pub stem_count: usize,
}

/// Host-supplied resolver for `kbd:@action` inline shortcuts — maps an
/// action id (numeric or named command, e.g. `40044` or
/// `_FTS_SESSION_TAKE_RANK_PLAYPOS_1`) to the key sequence currently
/// bound to it (`"<C-S-space>"`, `"n d"`). Mirrors [`VaultLookup`]:
/// `editor-state` stays app-agnostic, the host owns the keymap.
/// Unresolved refs render as a distinct "unbound" cap.
pub trait KbdLookup {
    fn keys_for_action(&self, action: &str) -> Option<String>;
}

/// Key-caps widget for a `kbd:` code span. `spec` is what follows the
/// prefix: a literal key sequence (`<C-s>`, `n d`) or `@action`.
/// Returns `None` when the spec is empty/unparseable so the caller
/// falls back to plain inline-code styling.
fn kbd_widget_html(spec: &str, kbd: Option<&dyn KbdLookup>) -> Option<String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let keys: String = if let Some(action) = spec.strip_prefix('@') {
        let action = action.trim();
        if action.is_empty() {
            return None;
        }
        match kbd.and_then(|k| k.keys_for_action(action)) {
            Some(keys) => keys,
            // Unresolved action ref: a distinct "unbound" cap showing
            // the action id, rather than breaking the note.
            None => {
                return Some(format!(
                    r#"<span class="md-kbd md-kbd-unbound" title="No key currently bound to this action"><kbd class="md-kbd-key">@{}</kbd></span>"#,
                    escape_html(action),
                ));
            }
        }
    } else {
        spec.to_string()
    };

    let chords: Vec<Vec<String>> = keys.split_whitespace().map(kbd_chord_labels).collect();
    if chords.is_empty() || chords.iter().any(Vec::is_empty) {
        return None;
    }
    let mut html = String::from(r#"<span class="md-kbd">"#);
    for (ci, chord) in chords.iter().enumerate() {
        if ci > 0 {
            html.push_str(r#"<span class="md-kbd-then">then</span>"#);
        }
        for (ki, key) in chord.iter().enumerate() {
            if ki > 0 {
                html.push_str(r#"<span class="md-kbd-plus">+</span>"#);
            }
            html.push_str(&format!(
                r#"<kbd class="md-kbd-key">{}</kbd>"#,
                escape_html(key)
            ));
        }
    }
    html.push_str("</span>");
    Some(html)
}

/// One chord token → display labels: `"<C-S-space>"` → `Ctrl Shift
/// Space`, `"r"` → `R`. `C`/`S`/`A` are Ctrl/Shift/Alt; `M` and `D`
/// both mean the platform Meta/Cmd key.
fn kbd_chord_labels(token: &str) -> Vec<String> {
    let inner = token
        .strip_prefix('<')
        .and_then(|t| t.strip_suffix('>'))
        .unwrap_or(token);

    let mut parts = Vec::new();
    let mut rest = inner;
    while let Some((m, tail)) = rest.split_once('-') {
        let label = match m {
            "C" => "Ctrl",
            "S" => "Shift",
            "A" => "Alt",
            "M" | "D" => "Meta",
            _ => break,
        };
        parts.push(label.to_string());
        rest = tail;
    }

    // Bare-modifier chords like `<C->` have no tail key.
    if rest.is_empty() {
        return parts;
    }

    let key = match rest {
        "space" => "Space".to_string(),
        "enter" | "return" => "Enter".to_string(),
        "esc" | "escape" => "Esc".to_string(),
        "tab" => "Tab".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" | "del" => "Delete".to_string(),
        "minus" => "-".to_string(),
        "plus" => "+".to_string(),
        "up" => "\u{2191}".to_string(),
        "down" => "\u{2193}".to_string(),
        "left" => "\u{2190}".to_string(),
        "right" => "\u{2192}".to_string(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.to_string(),
    };
    parts.push(key);
    parts
}

#[must_use]
pub fn live_preview(state: &EditorState) -> Vec<DecoratedRange> {
    live_preview_with_lookups(state, None, None)
}

pub fn live_preview_with(
    state: &EditorState,
    vault: Option<&dyn VaultLookup>,
) -> Vec<DecoratedRange> {
    live_preview_with_lookups(state, vault, None)
}

pub fn live_preview_with_lookups(
    state: &EditorState,
    vault: Option<&dyn VaultLookup>,
    kbd: Option<&dyn KbdLookup>,
) -> Vec<DecoratedRange> {
    // Per-pass compile budget for Typst — bounds the worst
    // case at a couple of cold compiles per render so a doc
    // full of fresh math doesn't block typing. See `typst`
    // submodule for the budget value and rationale.
    reset_compile_budget();
    reset_mermaid_budget();
    reset_keyflow_budget();
    reset_tabs_budget();
    reset_block_index();

    let text = state.doc.to_string();
    // In reading mode, swap the primary selection for one that
    // can't touch any byte range — `cursor_touches` then always
    // returns false, so every marker stays hidden. Same effect
    // as Obsidian's preview-only mode.
    let primary = if state.reading_mode {
        Range::caret(usize::MAX)
    } else {
        state.selection.primary()
    };
    let mut out = Vec::new();

    // Per-step timing for the perf trace. The cost in this fn is
    // dominated by `emit_fence_tokens` (tree-sitter) on docs
    // with code fences; the rest is O(doc-length) byte walking.
    let t_blocks = now_ms_native();
    let fenced_ranges = scan_blocks(&text, primary, &mut out);
    let blocks_ms = now_ms_native() - t_blocks;

    let t_inline = now_ms_native();
    let inline_decs_before = out.len();
    emit_status_pills(&text, primary, &mut out);
    emit_roster_rows(&text, primary, vault, &mut out);
    // Lazily computed on the first song strip (resolver scans are cheap
    // and cached, but most documents have no strips at all).
    let mut strip_runs: Option<std::collections::HashMap<usize, StripRunCtx>> = None;
    for span in find_spans(&text) {
        if in_fenced_code(&fenced_ranges, span.outer.start) {
            continue;
        }
        if !span.body.is_empty() {
            // Embed: `![[file.png|opts]]` etc. Render an `<img>` /
            // `<video>` / `<audio>` / `<iframe>` widget when the
            // caret is off the span. While the caret is on the
            // span, the inner Mark + visible source bytes win so
            // the user can edit. Matches Obsidian / Quartz
            // `ofm.ts:233-265`.
            // Math — `$x$` inline or `$$x$$` display. Source
            // stays visible when the caret's on the span (so
            // the user can edit), otherwise replaced with a
            // rendered Typst SVG widget.
            if span.class == "md-math-inline" || span.class == "md-math-block" {
                if !cursor_touches(primary, span.outer.clone()) {
                    let body = &text[span.body.clone()];
                    let kind = if span.class == "md-math-inline" {
                        TypstKind::MathInline
                    } else {
                        TypstKind::MathBlock
                    };
                    if let Some(svg) = render_typst(kind, body) {
                        out.push(Decoration::replace(span.outer.clone()));
                        // `data-focus-pos` lets the JS click
                        // handler route a click on the widget
                        // back to a caret inside the source
                        // span, so the user can edit math by
                        // tapping the rendered output.
                        let html = format!(
                            r#"<span class="{cls}" data-focus-pos="{pos}">{svg}</span>"#,
                            cls = if kind == TypstKind::MathInline {
                                "md-math-widget md-math-widget-inline"
                            } else {
                                "md-math-widget md-math-widget-block"
                            },
                            pos = span.body.start,
                        );
                        out.push(Decoration::widget(span.outer.start, html));
                        continue;
                    }
                }
                // Source visible (caret on, or compile failed).
                out.push(Decoration::mark(span.body.clone(), span.class));
                continue;
            }
            // Comments — `%%…%%` source is hidden entirely
            // (body + markers) when the caret is away. Only the
            // body stays visible while editing.
            if span.class == "md-comment" {
                if cursor_touches(primary, span.outer.clone()) {
                    out.push(Decoration::mark(span.body.clone(), "md-comment"));
                } else {
                    out.push(Decoration::replace(span.outer.clone()));
                }
                continue;
            }
            // `((uuid))` block reference — render as an atomic
            // chip showing the target block's first-line
            // content. UUID source is never visible (would
            // invite editing → broken refs). Always render the
            // widget; the chip itself is the only visible form.
            if span.class == "md-block-ref" {
                let uuid = &text[span.body.clone()];
                // Resolve in this order: intra-doc block index →
                // vault lookup → unresolved. The vault hit
                // brings its own preview (target page may live
                // anywhere); intra-doc hits read from this
                // doc's text directly.
                let (preview, source_page, is_resolved) =
                    if let Some(anchor) = block_anchor_for_uuid(uuid) {
                        (block_preview(&text, anchor), None, true)
                    } else if let Some(hit) = vault.and_then(|v| v.lookup_block(uuid)) {
                        (hit.preview, Some(hit.page), true)
                    } else {
                        (
                            format!("unresolved {}", &uuid[..8.min(uuid.len())]),
                            None,
                            false,
                        )
                    };
                let cls = if is_resolved {
                    "md-block-ref-chip"
                } else {
                    "md-block-ref-chip md-block-ref-unresolved"
                };
                let page_hint = source_page.map(|p| format!(" › {p}")).unwrap_or_default();
                let html = format!(
                    r#"<span class="{cls}" data-uuid="{uuid}" title="{full}">{glyph} {preview}{page}</span>"#,
                    glyph = "🔗",
                    full = escape_html(uuid),
                    preview = escape_html(&preview),
                    page = escape_html(&page_hint),
                );
                out.push(Decoration::replace(span.outer.clone()));
                out.push(Decoration::widget(span.outer.start, html));
                out.push(Decoration::atomic(span.outer.clone()));
                continue;
            }
            // `{{embed ((uuid))}}` — render the target block's
            // content inline in a bordered card. Same atomic +
            // hidden-source treatment as block refs.
            if span.class == "md-block-embed" {
                let uuid = &text[span.body.clone()];
                let (content, source_page, is_resolved) =
                    if let Some(anchor) = block_anchor_for_uuid(uuid) {
                        (block_preview(&text, anchor), None, true)
                    } else if let Some(hit) = vault.and_then(|v| v.lookup_block(uuid)) {
                        (hit.preview, Some(hit.page), true)
                    } else {
                        (
                            format!("unresolved {}", &uuid[..8.min(uuid.len())]),
                            None,
                            false,
                        )
                    };
                let cls = if is_resolved {
                    "md-block-embed-card"
                } else {
                    "md-block-embed-card md-block-ref-unresolved"
                };
                let page_chip = source_page
                    .map(|p| format!(
                        r#"<div class="md-embed-head">📄 <span class="md-embed-title">{title}</span></div>"#,
                        title = escape_html(&p),
                    ))
                    .unwrap_or_default();
                let html = format!(
                    r#"<div class="{cls}" data-uuid="{uuid}">{page_chip}{content}</div>"#,
                    uuid = escape_html(uuid),
                    content = escape_html(&content),
                );
                out.push(Decoration::replace(span.outer.clone()));
                out.push(Decoration::widget(span.outer.start, html));
                out.push(Decoration::atomic(span.outer.clone()));
                continue;
            }
            // Inline footnotes `^[body]` — Obsidian renders them
            // as an auto-numbered superscript reference; the body
            // is hidden until the user mouses over or clicks. We
            // don't auto-number yet (no footnote registry), so
            // collapse to a generic `[*]` marker when caret is
            // away. Source stays visible while editing.
            if span.class == "md-inline-footnote" {
                if cursor_touches(primary, span.outer.clone()) {
                    out.push(Decoration::mark(span.body.clone(), "md-inline-footnote"));
                } else {
                    out.push(Decoration::replace(span.outer.clone()));
                    out.push(Decoration::widget(
                        span.outer.start,
                        format!(
                            r#"<sup class="md-inline-footnote-marker" data-focus-pos="{}">[*]</sup>"#,
                            span.body.start,
                        ),
                    ));
                }
                continue;
            }
            if span.class == "md-embed" {
                let raw = &text[span.body.clone()];
                if !cursor_touches(primary, span.outer.clone()) {
                    if let Some(html) = embed_widget_html(raw, &text, vault) {
                        out.push(Decoration::replace(span.outer.clone()));
                        out.push(Decoration::widget(span.outer.start, html));
                        continue;
                    }
                }
                // Fallback (or caret on): style the body like a
                // wikilink so it's still recognizable as a link.
                out.push(Decoration::mark(span.body.clone(), "md-wikilink"));
                if !cursor_touches(primary, span.outer.clone()) {
                    if span.body.start > span.outer.start {
                        out.push(Decoration::replace(span.outer.start..span.body.start));
                    }
                    if span.outer.end > span.body.end {
                        out.push(Decoration::replace(span.body.end..span.outer.end));
                    }
                }
                continue;
            }
            // `kbd:` inline shortcuts — code spans with a `kbd:` prefix
            // render as key caps. Two forms: literal `kbd:<C-s>` (keys
            // as written) and action-ref `kbd:@40044` (whatever keys the
            // host's [`KbdLookup`] says are currently bound). Caret on
            // the span shows the raw source for editing, like links.
            if span.class == "md-code" {
                let body = &text[span.body.clone()];
                if let Some(spec) = body.strip_prefix("kbd:") {
                    if !cursor_touches(primary, span.outer.clone()) {
                        if let Some(html) = kbd_widget_html(spec, kbd) {
                            out.push(Decoration::replace(span.outer.clone()));
                            out.push(Decoration::widget(span.outer.start, html));
                            out.push(Decoration::atomic(span.outer.clone()));
                            continue;
                        }
                    }
                    // Caret inside (or empty spec): fall through to the
                    // normal inline-code styling with raw source.
                }
            }
            let href = match span.class {
                "md-link" => Some(text[span.body.end + 2..span.outer.end - 1].to_string()),
                "md-wikilink" => Some(text[span.body.clone()].to_string()),
                _ => None,
            };
            // For `[[target|display]]` only the display text is shown —
            // the `target|` prefix is hidden (like the brackets). The
            // display range is the body after the first `|`; without an
            // alias it's the whole body. `#Heading`-only links keep
            // their body verbatim (Obsidian shows "Page#Heading").
            let display = if span.class == "md-wikilink" {
                match text[span.body.clone()].find('|') {
                    Some(rel) => (span.body.start + rel + 1)..span.body.end,
                    None => span.body.clone(),
                }
            } else {
                span.body.clone()
            };
            // SONG STRIP: a wikilink ALONE on its line whose target is a
            // `type: song` note renders as a playable song row (title ·
            // artist · stems · duration, with a play control the host
            // wires via `data-href="song-play:<target>"`). Caret on the
            // line falls through to the normal editable link.
            if span.class == "md-wikilink" && !cursor_touches(primary, span.outer.clone()) {
                if let Some(h2) = href.as_deref() {
                    let page_part = h2.split(['#', '|']).next().unwrap_or(h2).trim();
                    let line_start = text[..span.outer.start].rfind('\n').map_or(0, |i| i + 1);
                    let line_end = text[span.outer.end..]
                        .find('\n')
                        .map_or(text.len(), |i| span.outer.end + i);
                    let standalone = text[line_start..span.outer.start].trim().is_empty()
                        && text[span.outer.end..line_end].trim().is_empty();
                    if standalone {
                        if let Some(setlist) = vault.and_then(|v| v.lookup_setlist(page_part)) {
                            out.push(Decoration::replace(span.outer.clone()));
                            out.push(Decoration::widget(
                                span.outer.start,
                                setlist_card_html(page_part, &setlist),
                            ));
                            out.push(Decoration::atomic(span.outer.clone()));
                            continue;
                        }
                        if let Some(song) = vault.and_then(|v| v.lookup_song(page_part)) {
                            let ctx = strip_runs
                                .get_or_insert_with(|| song_strip_runs(&text, vault))
                                .get(&line_start)
                                .copied()
                                .unwrap_or_default();
                            out.push(Decoration::replace(span.outer.clone()));
                            out.push(Decoration::widget(
                                span.outer.start,
                                song_strip_html(page_part, &song, ctx),
                            ));
                            out.push(Decoration::atomic(span.outer.clone()));
                            continue;
                        }
                        // VERSE CARD: a standalone scripture reference
                        // embeds the verse text. Real pages win (checked
                        // above via setlist/song; the general page check
                        // below keeps ordinary links untouched).
                        if vault.is_some_and(|v| v.lookup_page(page_part).is_none()) {
                            if let Some(sc) = vault.and_then(|v| v.lookup_scripture(page_part)) {
                                out.push(Decoration::replace(span.outer.clone()));
                                out.push(Decoration::widget(
                                    span.outer.start,
                                    scripture_card_html(page_part, &sc),
                                ));
                                out.push(Decoration::atomic(span.outer.clone()));
                                continue;
                            }
                        }
                    }
                }
            }
            if let Some(h) = href {
                // Wikilinks: consult the vault to decide
                // resolved (purple, default) vs unresolved
                // (red). Without a vault the link stays
                // unresolved — `#Heading` / `#^id` suffixes are
                // stripped before the page-name lookup so
                // `[[Page#Section]]` resolves when Page exists.
                let mut scripture_hit: Option<VaultScriptureHit> = None;
                let cls = if span.class == "md-wikilink" {
                    let page_part = h.split(['#', '|']).next().unwrap_or(&h).trim();
                    let resolved = vault.is_some_and(|v| v.lookup_page(page_part).is_some());
                    if resolved {
                        // Kind-specific styling: contact links render as
                        // person chips wherever they appear (inline too).
                        match vault.and_then(|v| v.lookup_note_kind(page_part)).as_deref() {
                            Some("contact") => "md-wikilink md-contact-chip",
                            _ => "md-wikilink",
                        }
                    } else if let Some(sc) = vault.and_then(|v| v.lookup_scripture(page_part)) {
                        // Scripture reference: resolved chip, verse text
                        // as hover tooltip once it lands.
                        scripture_hit = Some(sc);
                        "md-wikilink md-scripture-chip"
                    } else {
                        "md-wikilink md-wikilink-unresolved"
                    }
                } else {
                    span.class
                };
                let mut attrs = vec![("data-href".into(), h)];
                if let Some(text) = scripture_hit.and_then(|sc| {
                    sc.text
                        .map(|t| format!("{} ({})\n{}", sc.display, sc.translation, t))
                }) {
                    attrs.push(("title".into(), text));
                }
                out.push(Decoration::mark_with_attrs(display.clone(), cls, attrs));
                if !cursor_touches(primary, span.outer.clone()) {
                    // Caret elsewhere: treat the link as one
                    // atomic unit. Clicks anywhere inside snap
                    // to the nearer edge so the user never lands
                    // in the hidden marker bytes (`](url)` etc.).
                    out.push(Decoration::atomic(span.outer.clone()));
                }
            } else {
                out.push(Decoration::mark(span.body.clone(), span.class));
            }
        }
        if !cursor_touches(primary, span.outer.clone()) {
            // Hide the opening bracket(s) and, for an aliased wikilink,
            // the `target|` prefix up to the display text.
            let hide_left_to = if span.class == "md-wikilink" {
                match text[span.body.clone()].find('|') {
                    Some(rel) => span.body.start + rel + 1,
                    None => span.body.start,
                }
            } else {
                span.body.start
            };
            if hide_left_to > span.outer.start {
                out.push(Decoration::replace(span.outer.start..hide_left_to));
            }
            if span.outer.end > span.body.end {
                out.push(Decoration::replace(span.body.end..span.outer.end));
            }
        }
    }
    let inline_ms = now_ms_native() - t_inline;
    let inline_decs = out.len() - inline_decs_before;
    tracing::debug!(
        doc_len = text.len(),
        block_decs = inline_decs_before,
        inline_decs,
        fence_count = fenced_ranges.len(),
        blocks_ms = %format!("{:.2}", blocks_ms),
        inline_ms = %format!("{:.2}", inline_ms),
        "md.live_preview"
    );
    out
}

/// Wall-clock milliseconds. wasm-safe alias around the
/// view-layer `now_ms`; mirrored here so editor-state stays
/// free of dioxus deps.
fn now_ms_native() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        static START: OnceLock<std::time::Instant> = OnceLock::new();
        let s = START.get_or_init(std::time::Instant::now);
        s.elapsed().as_secs_f64() * 1000.0
    }
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now())
    }
}

// YAML frontmatter lives in its own submodule — parser,
// serializer, and Properties widget renderer.
pub mod frontmatter;
use frontmatter::render_properties_html;
pub use frontmatter::{FrontMatter, PropValue, Property, parse_frontmatter, serialize_property};

mod typst;
use typst::{TypstKind, render_typst, reset_compile_budget};

mod mermaid;
use mermaid::{render_mermaid, reset_compile_budget as reset_mermaid_budget};

mod keyflow;
use keyflow::{render_keyflow, reset_compile_budget as reset_keyflow_budget};

mod tabs;
use tabs::{render_tabs, reset_render_budget as reset_tabs_budget};

pub(crate) fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn in_fenced_code(ranges: &[std::ops::Range<usize>], pos: usize) -> bool {
    ranges.iter().any(|r| pos >= r.start && pos < r.end)
}

/// Layout context for one song-strip line: joined to a strip directly
/// above/below (no blank line between) + alternating parity within its
/// run — lets adjacent strips render as one flush, striped list.
#[derive(Clone, Copy, Default)]
struct StripRunCtx {
    joined_above: bool,
    joined_below: bool,
    odd: bool,
    /// 1-based position within the run — the setlist order number shown
    /// where the play control appears on hover.
    index: usize,
}

/// Scan the document for standalone `[[Song]]` lines (resolver-confirmed
/// songs) and compute each one's run context, keyed by line start.
fn song_strip_runs(
    text: &str,
    vault: Option<&dyn VaultLookup>,
) -> std::collections::HashMap<usize, StripRunCtx> {
    let Some(vault) = vault else {
        return Default::default();
    };
    // Collect candidate lines: (line_start, line_end).
    let mut candidates: Vec<(usize, usize)> = Vec::new();
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let t = content.trim();
        if let Some(inner) = t.strip_prefix("[[").and_then(|r| r.strip_suffix("]]")) {
            let page = inner.split(['#', '|']).next().unwrap_or(inner).trim();
            if vault.lookup_song(page).is_some() {
                candidates.push((pos, pos + content.len()));
            }
        }
        pos += line.len();
    }
    // Group into runs: consecutive candidates whose lines are ADJACENT
    // (exactly one newline between them).
    let mut out = std::collections::HashMap::new();
    let mut i = 0;
    while i < candidates.len() {
        let mut j = i;
        while j + 1 < candidates.len() && candidates[j + 1].0 == candidates[j].1 + 1 {
            j += 1;
        }
        for (k, &(start, _)) in candidates[i..=j].iter().enumerate() {
            out.insert(
                start,
                StripRunCtx {
                    joined_above: k > 0,
                    joined_below: i + k < j,
                    odd: k % 2 == 1,
                    index: k + 1,
                },
            );
        }
        i = j + 1;
    }
    out
}

/// A small stroke-icon (Lucide-shaped, `currentColor`) for widget HTML —
/// inherits the role chip's color.
fn role_icon_svg(kind: &str) -> String {
    let body = match kind {
        "drum" => {
            r#"<path d="m2 2 8 8"/><path d="m22 2-8 8"/><ellipse cx="12" cy="9" rx="10" ry="5"/><path d="M7 13.4v7.9"/><path d="M12 14v8"/><path d="M17 13.4v7.9"/><path d="M2 9v8a10 5 0 0 0 20 0V9"/>"#
        }
        "guitar" => {
            r#"<circle cx="8" cy="16" r="5"/><path d="m11.8 12.2 7.2-7.2"/><path d="m18 3 3 3"/><path d="m19 4-2.5 2.5"/>"#
        }
        "keys" => {
            r#"<rect x="2" y="6" width="20" height="12" rx="1"/><path d="M7 6v7"/><path d="M12 6v7"/><path d="M17 6v7"/>"#
        }
        "mic" => {
            r#"<path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><line x1="12" x2="12" y1="19" y2="22"/>"#
        }
        "sliders" => {
            r#"<line x1="21" x2="14" y1="4" y2="4"/><line x1="10" x2="3" y1="4" y2="4"/><line x1="21" x2="12" y1="12" y2="12"/><line x1="8" x2="3" y1="12" y2="12"/><line x1="21" x2="16" y1="20" y2="20"/><line x1="12" x2="3" y1="20" y2="20"/><line x1="14" x2="14" y1="2" y2="6"/><line x1="8" x2="8" y1="10" y2="14"/><line x1="16" x2="16" y1="18" y2="22"/>"#
        }
        "bulb" => {
            r#"<path d="M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1.3.5 2.6 1.5 3.5.8.8 1.3 1.5 1.5 2.5"/><path d="M9 18h6"/><path d="M10 22h4"/>"#
        }
        "monitor" => {
            r#"<rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/>"#
        }
        "video" => {
            r#"<path d="m16 13 5.2 3.5a.5.5 0 0 0 .8-.4V7.9a.5.5 0 0 0-.8-.4L16 11"/><rect x="2" y="6" width="14" height="12" rx="2"/>"#
        }
        // "music" and anything unrecognized.
        _ => {
            r#"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"#
        }
    };
    format!(
        r#"<svg class="md-role-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
    )
}

/// The FTS instrument color scheme + an icon per role — reflected on the
/// roster's role chips (Drums red, Bass yellow, Electric blue, Acoustic
/// cyan, Keys green, Synth purple, vocals pink, tech slate…).
fn role_style(role: &str) -> (&'static str, &'static str) {
    let r = role.to_ascii_lowercase();
    if r.contains("drum") || r.contains("perc") {
        ("md-role--red", "drum")
    } else if r.contains("bass") {
        ("md-role--yellow", "guitar")
    } else if r.contains("electric") {
        ("md-role--blue", "guitar")
    } else if r.contains("acoustic") {
        ("md-role--cyan", "guitar")
    } else if r.contains("key") || r.contains("piano") {
        ("md-role--green", "keys")
    } else if r.contains("synth") || r.contains("organ") {
        ("md-role--purple", "keys")
    } else if r.contains("vocal") || r.contains("worship leader") || r.contains("singer") {
        ("md-role--pink", "mic")
    } else if r.contains("music director") {
        ("md-role--orange", "music")
    } else if r.contains("foh") || r.contains("audio") || r.contains("sound") {
        ("md-role--slate", "sliders")
    } else if r.contains("light") {
        ("md-role--amber", "bulb")
    } else if r.contains("graphic") || r.contains("lyric") || r.contains("screen") {
        ("md-role--slate", "monitor")
    } else if r.contains("production") || r.contains("director") {
        ("md-role--orange", "video")
    } else {
        ("md-role--slate", "music")
    }
}

/// Roster rows: `Role - [[Name]] (Status)[, [[Name]] (Status)…]` where
/// every target is a `type: contact` note renders as a TEAM row widget —
/// role chip + one CONTACT CARD per person: initials avatar with a
/// status ring + badge (green ✓ confirmed / amber ? pending / red ✕
/// declined) and the name. Caret on the line = raw editable text.
fn emit_roster_rows(
    text: &str,
    primary: Range,
    vault: Option<&dyn VaultLookup>,
    out: &mut Vec<DecoratedRange>,
) {
    let Some(vault) = vault else { return };
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let line_from = pos;
        pos += line.len();
        let line_to = line_from + content.len();
        let t = content.trim();
        let Some(dash) = t.find(" - [[") else {
            continue;
        };
        let role = t[..dash].trim();
        if role.is_empty() || role.starts_with('#') || role.starts_with('[') {
            continue;
        }
        // Parse the people list: repeated `[[Name]]` + optional `(status)`.
        let mut rest = &t[dash + 3..];
        let mut people: Vec<(String, &'static str, &'static str, &'static str)> = Vec::new();
        while let Some(open) = rest.find("[[") {
            let Some(close_rel) = rest[open..].find("]]") else {
                break;
            };
            let name = rest[open + 2..open + close_rel].trim();
            let name = name.split(['#', '|']).next().unwrap_or(name).trim();
            rest = &rest[open + close_rel + 2..];
            let (st_cls, badge, ring) = {
                let after = rest.trim_start().trim_start_matches(',').trim_start();
                if let Some(inner) = after
                    .strip_prefix('(')
                    .and_then(|r| r.split_once(')').map(|(a, _)| a))
                {
                    match inner.trim().to_ascii_lowercase().as_str() {
                        "confirmed" => ("md-av--confirmed", "✓", "confirmed"),
                        "declined" => ("md-av--declined", "✕", "declined"),
                        _ => ("md-av--pending", "?", "pending"),
                    }
                } else {
                    ("md-av--none", "", "")
                }
            };
            if vault.lookup_note_kind(name).as_deref() != Some("contact") {
                people.clear();
                break;
            }
            people.push((name.to_owned(), st_cls, badge, ring));
        }
        if people.is_empty() || cursor_touches(primary, line_from..line_to) {
            continue;
        }
        let cards: String = people
            .iter()
            .map(|(name, st, badge, ring)| {
                let initials: String = name
                    .split_whitespace()
                    .take(2)
                    .filter_map(|w| w.chars().next())
                    .collect::<String>()
                    .to_uppercase();
                let badge_html = if badge.is_empty() {
                    String::new()
                } else {
                    format!(r#"<span class="md-av-badge md-av-badge--{ring}">{badge}</span>"#)
                };
                format!(
                    r#"<span class="md-contact-card" data-href="{n}"><span class="md-avatar {st}">{initials}{badge_html}</span><span class="md-contact-name">{n}</span></span>"#,
                    n = html_escape(name),
                )
            })
            .collect();
        let (role_cls, icon_kind) = role_style(role);
        let icon = role_icon_svg(icon_kind);
        out.push(Decoration::replace(line_from..line_to));
        out.push(Decoration::widget(
            line_from,
            format!(
                r#"<span class="md-roster-row"><span class="md-roster-role {role_cls}">{icon}{role}</span><span class="md-roster-people">{cards}</span></span>"#,
                role = html_escape(role),
            ),
        ));
        out.push(Decoration::atomic(line_from..line_to));
    }
}

/// Assignment-status pills: a line ending in `(Confirmed)` / `(Pending)`
/// / `(Declined)` (the event-planner roster convention:
/// `Drums - [[Name]] (Pending)`) renders the token as a colored pill and
/// hides the parens. Caret on the line keeps the raw text editable.
fn emit_status_pills(text: &str, primary: Range, out: &mut Vec<DecoratedRange>) {
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let line_from = pos;
        pos += line.len();
        let trimmed_end = content.trim_end();
        let Some(open_rel) = trimmed_end.rfind('(') else {
            continue;
        };
        let Some(inner) = trimmed_end[open_rel..]
            .strip_prefix('(')
            .and_then(|r| r.strip_suffix(')'))
        else {
            continue;
        };
        let status = match inner.trim().to_ascii_lowercase().as_str() {
            "confirmed" => "md-status--confirmed",
            "pending" => "md-status--pending",
            "declined" => "md-status--declined",
            _ => continue,
        };
        let line_to = line_from + content.len();
        if cursor_touches(primary, line_from..line_to) {
            continue;
        }
        let open_abs = line_from + open_rel;
        let close_abs = line_from + trimmed_end.len() - 1;
        let word_from = open_abs + 1;
        let word_to = close_abs;
        out.push(Decoration::replace(open_abs..word_from));
        out.push(Decoration::mark(
            word_from..word_to,
            match status {
                "md-status--confirmed" => "md-status-pill md-status--confirmed",
                "md-status--pending" => "md-status-pill md-status--pending",
                _ => "md-status-pill md-status--declined",
            },
        ));
        out.push(Decoration::replace(word_to..close_abs + 1));
    }
}

/// The inline setlist-card widget for a standalone `[[Setlist]]` wikilink
/// — a compact embed: art tile · title · song count · duration. Clicking
/// navigates to the setlist note (`data-href`).
fn setlist_card_html(target: &str, setlist: &VaultSetlistHit) -> String {
    let safe = html_escape(target);
    let title = html_escape(&setlist.title);
    let n = setlist.song_count;
    let rows: String = setlist
        .songs
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let link = html_escape(&row.link);
            let (disp_title, disp_artist) = split_title_artist(&row.link, row.artist.as_deref());
            let title = html_escape(&disp_title);
            let artist = disp_artist
                .map(|a| format!(r#"<span class="md-song-strip-artist">{}</span>"#, html_escape(&a)))
                .unwrap_or_default();
            let initial = html_escape(
                &disp_title.chars().next().unwrap_or('♪').to_uppercase().to_string(),
            );
            let mut cls = String::from("md-song-strip");
            if i > 0 {
                cls.push_str(" md-song-strip--ja");
            }
            if i + 1 < setlist.songs.len() {
                cls.push_str(" md-song-strip--jb");
            }
            if i % 2 == 1 {
                cls.push_str(" md-song-strip--alt");
            }
            format!(
                r#"<span class="{cls}" data-href="song-play:{link}"><span class="md-song-strip-num" data-href="song-play:{link}"><span class="md-ss-idx">{idx}</span><svg class="md-ss-play" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg></span><span class="md-ss-art"><span class="md-ss-art-i">{initial}</span><span class="md-ss-eq"><i></i><i></i><i></i><i></i></span></span><span class="md-ss-titles"><span class="md-song-strip-title">{title}</span>{artist}</span><span class="md-ss-open" data-href="{link}" title="Open song"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M7 17 17 7"/><path d="M9 7h8v8"/></svg></span><span class="md-ss-more" data-href="song-more:{link}"><svg viewBox="0 0 24 24" fill="currentColor"><circle cx="5" cy="12" r="1.9"/><circle cx="12" cy="12" r="1.9"/><circle cx="19" cy="12" r="1.9"/></svg></span></span>"#,
                idx = i + 1,
            )
        })
        .collect();
    format!(
        r#"<span class="md-setlist-embed"><span class="md-setlist-card" data-href="{safe}"><span class="md-setlist-card-art">🎵</span><span class="md-setlist-card-titles"><span class="md-setlist-card-title">{title}</span><span class="md-setlist-card-sub">Setlist · {n} songs</span></span><span class="md-setlist-card-open">Open ›</span></span>{rows}</span>"#
    )
}

/// Split a `"Title - Artist"` display string into (title, artist). Falls back
/// to `fallback` for the artist when there's no ` - ` separator, so the strip
/// shows a clean title with the artist as a subtitle rather than repeating
/// `"Song - Artist"` on the title line.
fn split_title_artist(raw: &str, fallback: Option<&str>) -> (String, Option<String>) {
    if let Some((t, a)) = raw.split_once(" - ") {
        let a = a.trim();
        (t.trim().to_string(), (!a.is_empty()).then(|| a.to_string()))
    } else {
        (raw.trim().to_string(), fallback.map(|s| s.to_string()))
    }
}

/// The inline song-strip widget for a standalone `[[Song]]` wikilink.
/// The whole strip navigates (`data-href` = the link target); the play
/// control carries `data-href="song-play:<target>"` — the host's
/// `on_link_click` intercepts the scheme and drives playback.
fn song_strip_html(target: &str, song: &VaultSongHit, ctx: StripRunCtx) -> String {
    let safe = html_escape(target);
    let (disp_title, disp_artist) = split_title_artist(&song.title, song.artist.as_deref());
    let title = html_escape(&disp_title);
    let artist = disp_artist
        .map(|a| {
            format!(
                r#"<span class="md-song-strip-artist">{}</span>"#,
                html_escape(&a)
            )
        })
        .unwrap_or_default();
    let mut cls = String::from("md-song-strip");
    if ctx.joined_above {
        cls.push_str(" md-song-strip--ja");
    }
    if ctx.joined_below {
        cls.push_str(" md-song-strip--jb");
    }
    if ctx.odd {
        cls.push_str(" md-song-strip--alt");
    }
    let idx = ctx.index.max(1);
    let initial = html_escape(
        &disp_title
            .chars()
            .next()
            .unwrap_or('♪')
            .to_uppercase()
            .to_string(),
    );
    format!(
        r#"<span class="{cls}" data-href="song-play:{safe}"><span class="md-song-strip-num" data-href="song-play:{safe}"><span class="md-ss-idx">{idx}</span><svg class="md-ss-play" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg></span><span class="md-ss-art"><span class="md-ss-art-i">{initial}</span><span class="md-ss-eq"><i></i><i></i><i></i><i></i></span></span><span class="md-ss-titles"><span class="md-song-strip-title">{title}</span>{artist}</span><span class="md-ss-open" data-href="{safe}" title="Open song"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M7 17 17 7"/><path d="M9 7h8v8"/></svg></span><span class="md-ss-more" data-href="song-more:{safe}"><svg viewBox="0 0 24 24" fill="currentColor"><circle cx="5" cy="12" r="1.9"/><circle cx="12" cy="12" r="1.9"/><circle cx="19" cy="12" r="1.9"/></svg></span></span>"#
    )
}

/// The inline verse-card widget for a standalone `[[John 3:16]]`
/// wikilink: the verse text as a block quote with the reference +
/// translation as the caption. The card carries
/// `data-href="scripture-open:<target>"` — the host routes it to the
/// scripture reader anchored at the verse.
fn scripture_card_html(target: &str, sc: &VaultScriptureHit) -> String {
    let safe = html_escape(target);
    let display = html_escape(&sc.display);
    let tx = html_escape(&sc.translation);
    let body = match &sc.text {
        Some(t) => html_escape(t),
        None => "Loading…".to_string(),
    };
    format!(
        r#"<span class="md-scripture-card" data-href="scripture-open:{safe}"><span class="md-scripture-card-text">{body}</span><span class="md-scripture-card-ref"><span class="md-scripture-card-display">{display}</span><span class="md-scripture-card-tx">{tx}</span><span class="md-scripture-card-open">Study ›</span></span></span>"#
    )
}

/// Render the HTML for an `![[file|opts]]` embed when the
/// target's extension maps to a media kind we know. Returns
/// `None` for embeds we don't yet support (notes, unknown
/// formats) — the caller then falls back to a wikilink-style
/// mark so the source stays visible. Quartz reference: same
/// dispatch in `ofm.ts:233-265`.
fn embed_widget_html(raw: &str, doc: &str, vault: Option<&dyn VaultLookup>) -> Option<String> {
    let (target, opts) = match raw.split_once('|') {
        Some((t, o)) => (t.trim(), Some(o.trim())),
        None => (raw.trim(), None),
    };
    let ext = target.rsplit_once('.').map(|x| x.1.to_ascii_lowercase());
    let ext = ext.as_deref().unwrap_or("");
    let safe_target = html_escape(target);
    let style = opts.and_then(parse_size_opts).unwrap_or_default();
    // 1. Media extensions first (image / video / audio / pdf).
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "avif" | "bmp" => {
            return Some(format!(
                r#"<img class="md-embed-image" src="{safe_target}" alt="{safe_target}"{style}>"#
            ));
        }
        "mp4" | "webm" | "mov" | "ogv" => {
            return Some(format!(
                r#"<video class="md-embed-video" src="{safe_target}" controls{style}></video>"#
            ));
        }
        "mp3" | "wav" | "ogg" | "flac" | "m4a" => {
            return Some(format!(
                r#"<audio class="md-embed-audio" src="{safe_target}" controls></audio>"#
            ));
        }
        "pdf" => {
            return Some(format!(
                r#"<iframe class="md-embed-pdf" src="{safe_target}"{style}></iframe>"#
            ));
        }
        _ => {}
    }
    // 2. Note-style embeds. Split into page + fragment parts.
    //    Shapes:
    //      `![[Page]]`            — whole-page embed
    //      `![[Page#Heading]]`    — section embed
    //      `![[Page#^short-id]]`  — block embed (Obsidian short id)
    //      `![[#Heading]]`        — section in current doc
    //      `![[#^short-id]]`      — block in current doc
    let (page_part, frag_part) = match target.split_once('#') {
        Some((p, f)) => (p.trim(), Some(f.trim())),
        None => (target.trim(), None),
    };
    let is_intra_doc = page_part.is_empty();
    let safe_page = if page_part.is_empty() {
        "this page".to_string()
    } else {
        html_escape(page_part)
    };
    // Section / short-id fragment.
    if let Some(frag) = frag_part {
        if let Some(short_id) = frag.strip_prefix('^') {
            // Block embed via short id. Intra-doc resolution
            // first; cross-doc through the vault.
            let resolved = if is_intra_doc {
                resolve_block_short_id(doc, short_id)
            } else {
                vault.and_then(|v| v.lookup_block_short(page_part, short_id))
            };
            return Some(render_embed_card_short(
                "🔗",
                &safe_page,
                &html_escape(frag),
                resolved.as_deref(),
            ));
        }
        // Section embed. Intra-doc walks this file's headings;
        // cross-doc asks the vault.
        let resolved = if is_intra_doc {
            resolve_heading_section(doc, frag)
        } else {
            vault.and_then(|v| v.lookup_section(page_part, frag))
        };
        return Some(render_embed_card_section(
            "📄",
            &safe_page,
            &html_escape(frag),
            resolved.as_deref(),
        ));
    }
    // 3. Whole-page embed. Cross-doc resolution via vault;
    //    intra-doc has no meaningful behavior (a page embedding
    //    itself), so falls back to placeholder.
    let resolved = if is_intra_doc {
        None
    } else {
        vault
            .and_then(|v| v.lookup_page(page_part))
            .map(|h| h.preview)
    };
    Some(render_embed_card_page(
        "📄",
        &safe_page,
        resolved.as_deref(),
    ))
}

/// Walk the doc for a heading whose text matches `name` (case-
/// insensitive trim). Returns the heading body content (up to
/// the next same-or-higher heading) when found.
fn resolve_heading_section(doc: &str, name: &str) -> Option<String> {
    let needle = name.trim().to_lowercase();
    let lines: Vec<(usize, usize)> = line_ranges(doc);
    let mut start_idx: Option<(usize, usize)> = None; // (line_index, level)
    for (i, (lf, lt)) in lines.iter().enumerate() {
        let line = &doc[*lf..*lt];
        if let Some((level, marker_end)) = parse_heading(line) {
            let title = line[marker_end..].trim();
            if title.to_lowercase() == needle {
                start_idx = Some((i, level));
                break;
            }
        }
    }
    let (start_i, start_level) = start_idx?;
    // Collect content lines until the next heading of level
    // <= start_level.
    let mut body = String::new();
    for (lf, lt) in lines.iter().skip(start_i + 1) {
        let line = &doc[*lf..*lt];
        if let Some((level, _)) = parse_heading(line) {
            if level <= start_level {
                break;
            }
        }
        body.push_str(line);
        body.push('\n');
    }
    Some(body.trim().to_string())
}

/// Find an Obsidian short block-id `^id` in the doc and return
/// the containing paragraph's text.
fn resolve_block_short_id(doc: &str, short_id: &str) -> Option<String> {
    let needle = format!("^{short_id}");
    let pos = doc.find(&needle)?;
    let line_start = doc[..pos].rfind('\n').map_or(0, |n| n + 1);
    let line_end = doc[pos..].find('\n').map_or(doc.len(), |n| pos + n);
    let line = &doc[line_start..line_end];
    // Strip the trailing `^id` so the embed shows the body text.
    Some(line[..line.len() - needle.len()].trim_end().to_string())
}

fn render_embed_card_page(icon: &str, page: &str, resolved: Option<&str>) -> String {
    let body = match resolved {
        Some(s) => html_escape(s),
        None => {
            r#"<span class="md-embed-placeholder">multi-file lookup pending</span>"#.to_string()
        }
    };
    format!(
        r#"<div class="md-embed-card md-embed-page"><div class="md-embed-head">{icon} <span class="md-embed-title">{page}</span></div><div class="md-embed-body">{body}</div></div>"#
    )
}

fn render_embed_card_section(
    icon: &str,
    page: &str,
    heading: &str,
    resolved: Option<&str>,
) -> String {
    let body = match resolved {
        Some(s) => html_escape(s),
        None => {
            r#"<span class="md-embed-placeholder">multi-file lookup pending</span>"#.to_string()
        }
    };
    format!(
        r#"<div class="md-embed-card md-embed-section"><div class="md-embed-head">{icon} <span class="md-embed-title">{page}</span> <span class="md-embed-sep">›</span> <span class="md-embed-frag">{heading}</span></div><div class="md-embed-body">{body}</div></div>"#
    )
}

fn render_embed_card_short(icon: &str, page: &str, short: &str, resolved: Option<&str>) -> String {
    let body = match resolved {
        Some(s) => html_escape(s),
        None => {
            r#"<span class="md-embed-placeholder">multi-file lookup pending</span>"#.to_string()
        }
    };
    format!(
        r#"<div class="md-embed-card md-embed-block"><div class="md-embed-head">{icon} <span class="md-embed-title">{page}</span> <span class="md-embed-sep">›</span> <span class="md-embed-frag">{short}</span></div><div class="md-embed-body">{body}</div></div>"#
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Parse Obsidian's `|WxH`, `|W`, or `|HxW` opts on an embed.
/// Returns an inline `style` snippet to drop into the widget.
fn parse_size_opts(opts: &str) -> Option<String> {
    let (w, h) = match opts.split_once('x') {
        Some((w, h)) => (w.parse::<u32>().ok(), h.parse::<u32>().ok()),
        None => (opts.parse::<u32>().ok(), None),
    };
    let mut style = String::new();
    if let Some(w) = w {
        style.push_str(&format!(" style=\"width:{w}px"));
        if let Some(h) = h {
            style.push_str(&format!(";height:{h}px"));
        }
        style.push('"');
    }
    if style.is_empty() {
        return None;
    }
    Some(style)
}

struct Span {
    /// Includes the opening + closing markers.
    outer: std::ops::Range<usize>,
    /// Just the inner content.
    body: std::ops::Range<usize>,
    /// CSS class to apply to the body. Static for now; later
    /// callers may want to inject their own class names.
    class: &'static str,
}

/// Did the primary selection touch any byte in `range`? A caret
/// *adjacent* to the span counts as touching — so cursors at
/// either edge keep the markers visible (matches Obsidian).
fn cursor_touches(primary: Range, range: std::ops::Range<usize>) -> bool {
    let (sel_from, sel_to) = (primary.from(), primary.to());
    sel_to >= range.start && sel_from <= range.end
}

/// Block-level scanner. Walks the doc line by line, recognizing
/// headings, blockquotes, lists, task lists, HRs and fenced code
/// blocks. Pushes the right `Decoration`s onto `out` and returns
/// the byte ranges occupied by fenced-code *content* so the
/// caller can skip inline parsing inside them.
fn scan_blocks(
    text: &str,
    primary: Range,
    out: &mut Vec<DecoratedRange>,
) -> Vec<std::ops::Range<usize>> {
    // `type: setlist` notes render their FIRST `# ` heading as the
    // setlist header widget (art tile · title · SETLIST · play) — the
    // note's own title IS the player header. Editable when the caret is
    // on the line; plain text in raw views (it is only a decoration).
    let doc_is_setlist = frontmatter_declares_setlist(text);
    // `type: event` notes: the first H1 renders as the EVENT header
    // (title · date · recurrence) — weekly events are distinguished by
    // their date, so it leads.
    let doc_is_event = frontmatter_scalar(text, "type").as_deref() == Some("event");
    let event_date = frontmatter_scalar(text, "date").unwrap_or_default();
    let mut setlist_h1_done = false;

    let mut fenced_ranges = Vec::new();
    // ── YAML frontmatter ───────────────────────────────────
    //
    // Obsidian renders `---\n…\n---` at the top of a note as a
    // "Properties" panel. Only the very start of the doc
    // counts — `---` mid-doc is a horizontal rule.
    //
    // When caret is outside the block, replace the source with
    // the rendered properties widget. When caret is inside,
    // leave the raw YAML visible (so the user can edit), and
    // still register the range so the inline scanner doesn't
    // interpret `key: value` colons as anything markdown.
    // Frontmatter widget is always shown once the block parses
    // — including when the caret is inside the YAML — so vim
    // row-navigation has something to look at. The active row
    // (the one containing the caret) is flagged in the HTML so
    // CSS can highlight it. Only the `---` delimiter lines stay
    // raw when caret is on them, in case the user wants to
    // collapse the block by deleting them.
    let frontmatter_range = parse_frontmatter(text).map(|fm| fm.outer.clone());
    if let Some(fm) = parse_frontmatter(text) {
        fenced_ranges.push(fm.outer.clone());
        let caret = primary.head;
        let active_idx = fm
            .props
            .iter()
            .position(|p| caret >= p.range.start && caret < p.range.end);
        let html = render_properties_html(&fm.props, active_idx);
        out.push(Decoration::replace(fm.outer.clone()));
        out.push(Decoration::widget(fm.outer.start, html));
    }

    // Fence-tracking state:
    //   - `Some((open_line_end, marker_char, marker_len))` while
    //     we're inside a fence; `open_line_end` is the byte AFTER
    //     the opening fence's `\n` (or end of doc if the fence is
    //     the last thing).
    // (open_pos, fence_char, fence_len, is_keyflow). The last flag marks
    // `kf`/`kf+` fences so every line sheds the grey code-block frame —
    // the engraved chart (and its own source block) stands full width,
    // not boxed like code. `kf-src` is NOT flagged: it stays a code block.
    let mut fence: Option<(usize, u8, usize, bool)> = None;
    // Callout-tracking state: while we're inside a `> [!type]…`
    // block, every subsequent `>`-prefixed line inherits the
    // callout's class so CSS can group them visually.
    // Callout stack: one entry per nesting depth. `> [!note]\n>
    // > [!warning]` pushes "note" then "warning"; a line with
    // fewer `>` markers pops back. Non-blockquote lines drain
    // the whole stack. Indexed by depth - 1 (depth 1 → [0]).
    let mut callout_stack: Vec<&'static str> = Vec::new();

    // Setext-style heading detection. For each window of two
    // consecutive lines, if the first is content and the second
    // is `===` or `---` (any length), the pair becomes an H1 or
    // H2. Stash the underline-line offsets so the main loop can
    // skip them, and stash the content-line offsets so we know
    // which level to emit.
    //
    // `---` is also a HR — disambiguation: setext wins when the
    // line above is non-blank AND not a block-opening marker
    // (heading, list, blockquote, fence, frontmatter). HR wins
    // otherwise.
    let all_lines = line_ranges(text);
    let mut setext_content_level: std::collections::HashMap<usize, u8> =
        std::collections::HashMap::new();
    let mut setext_underline: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for win in all_lines.windows(2) {
        let (lf, lt) = win[0];
        let (ulf, ult) = win[1];
        let above = &text[lf..lt];
        let under = text[ulf..ult].trim_end();
        if above.trim().is_empty() || under.is_empty() {
            continue;
        }
        let setext_ok_above = !above.starts_with('#')
            && !above.starts_with('>')
            && !above.starts_with('-')
            && !above.starts_with('*')
            && !above.starts_with('+')
            && !above.starts_with("```")
            && !above.starts_with("~~~")
            && !above.starts_with("---")
            && !above.starts_with("===");
        if !setext_ok_above {
            continue;
        }
        if under.chars().all(|c| c == '=') {
            setext_content_level.insert(lf, 1);
            setext_underline.insert(ulf);
        } else if under.chars().all(|c| c == '-') {
            setext_content_level.insert(lf, 2);
            setext_underline.insert(ulf);
        }
    }

    for (line_from, line_to) in line_ranges(text) {
        // Setext underline — emit a styling class on the line so
        // it's visually paired with the heading above, then move
        // on. Without this skip, the `---` underline would be
        // matched as an HR by the block below.
        if setext_underline.contains(&line_from) {
            out.push(Decoration::line(line_from, "md-setext-underline"));
            continue;
        }
        // Setext content line — render with the heading class.
        // The underline isn't part of the heading text so we
        // don't bring it into the Replace; the active-state
        // tooling still treats the body as plain inline.
        if let Some(&level) = setext_content_level.get(&line_from) {
            let class = HEADING_CLASS[(level as usize) - 1];
            out.push(Decoration::line(line_from, class));
            continue;
        }
        let line = &text[line_from..line_to];

        // `id:: <uuid>` block-id property line (Logseq form).
        // Whole line is hidden from the rendered view; we never
        // want the user to accidentally edit a UUID (it'd break
        // every ref). Atomic so arrow-keys / Backspace treat
        // the hidden range as a single unit.
        if let Some(uuid_range) = parse_block_id_line(line, line_from) {
            // Replace the whole line content + its trailing
            // newline so neighbouring lines collapse together.
            let end = if line_to < text.len() {
                line_to + 1
            } else {
                line_to
            };
            out.push(Decoration::replace(line_from..end));
            out.push(Decoration::atomic(line_from..end));
            // Index this block id against the byte offset of the
            // line above (its block content). Stashed for
            // cross-line resolution + the `🔗` widget.
            register_block_id(
                &text[uuid_range.clone()],
                find_block_anchor(text, line_from),
            );
            continue;
        }

        // Lines inside the frontmatter range are handled above
        // (Replace + widget or delimiter marks) — skip block
        // parsing so the `---` opener isn't misread as a HR
        // and `key: value` lines aren't matched against block
        // patterns.
        if let Some(r) = &frontmatter_range {
            if line_from < r.end {
                continue;
            }
        }

        // ── Table (GFM pipe table) ─────────────────────────
        //
        // First-line check: `| header | … |` followed by a
        // separator row `|---|---|`. The separator's column
        // count must match the header. When we recognize a
        // table, jump the outer scan past its last row and emit
        // a single rendered `<table>` widget covering the whole
        // range. Quartz: `ofm.ts:123-126` via `remark-gfm`.
        let table_match = if !line.trim().is_empty()
            && fence.is_none()
            && callout_stack.is_empty()
            && is_table_header(line)
        {
            try_parse_table(text, line_from, line_to)
        } else {
            None
        };
        if let Some(rows) = table_match {
            let table_end = rows.last().map_or(line_to, |r| r.1);
            // Header / separator + body cells.
            let cells = collect_table_cells(text, &rows);
            let html = render_table_html(&cells);
            // When caret is anywhere in the table, leave the
            // source visible (Obsidian behavior — typing in
            // tables works against the source). Otherwise
            // replace + widget.
            if !cursor_touches(primary, line_from..table_end) {
                out.push(Decoration::replace(line_from..table_end));
                out.push(Decoration::widget(line_from, html));
            }
            // Skip the outer loop forward to the last table
            // line so we don't re-process its rows.
            // The outer iterator drives off `line_ranges`, so
            // we can't actually skip lines from in here. Mark
            // the table range as a "fenced-like" zone so the
            // inline scanner doesn't reparse cell contents as
            // bold/italic at odd positions — and have the next
            // iterations see them as inside-fence.
            fenced_ranges.push(line_from..table_end);
        }

        // ── Inside a fence ─────────────────────────────────
        if let Some((_, mc, mlen, is_kf)) = fence {
            if is_closing_fence(line, mc, mlen) {
                out.push(Decoration::line(line_from, "md-code-block"));
                if is_kf {
                    out.push(Decoration::line(line_from, "md-keyflow-bare"));
                }
                // Caret on the closing fence: source stays
                // visible so the user can edit the `\`\`\``.
                // Off: hidden via Replace so the line just shows
                // the code-block background.
                if !cursor_touches(primary, line_from..line_to) {
                    out.push(Decoration::replace(line_from..line_to));
                }
                if let Some((open_end, _, _, _)) = fence.take() {
                    fenced_ranges.push(open_end..line_to);
                }
                continue;
            }
            out.push(Decoration::line(line_from, "md-code-block"));
            if is_kf {
                out.push(Decoration::line(line_from, "md-keyflow-bare"));
            }
            continue;
        }

        // ── Starting a fence ───────────────────────────────
        let trimmed = line.trim_start();
        if let Some((mc, mlen, info_start)) = opens_fence(trimmed) {
            let info_peek = trimmed[info_start..].trim();
            let is_kf_fence = info_peek.eq_ignore_ascii_case("kf")
                || info_peek.eq_ignore_ascii_case("kf+")
                || info_peek.eq_ignore_ascii_case("kf-");
            out.push(Decoration::line(line_from, "md-code-block"));
            if is_kf_fence {
                out.push(Decoration::line(line_from, "md-keyflow-bare"));
            }
            let caret_on_opener = cursor_touches(primary, line_from..line_to);
            // Caret on opener: leave the `\`\`\`lang` source
            // visible so it's editable. Off: hide source +
            // overlay the lang/copy widget.
            if !caret_on_opener {
                out.push(Decoration::replace(line_from..line_to));
            }
            let info = trimmed[info_start..].trim();
            let content_start = if line_to < text.len() {
                line_to + 1
            } else {
                line_to
            };
            fence = Some((line_from, mc, mlen, is_kf_fence));
            // The lang+copy header overlays the opener line for
            // ordinary code fences. Skip it for fences we
            // render as a widget (typst, mermaid) — the
            // rendered SVG already speaks for itself, and the
            // floating header would be a leftover when the user
            // moves the caret onto the fence to edit source.
            // Keyflow fences build their own header (tag + copy) inside
            // the widget, so skip the absolute-positioned code header
            // that ordinary fences overlay.
            let is_rendered_fence = info.eq_ignore_ascii_case("typst")
                || info.eq_ignore_ascii_case("mermaid")
                || info.eq_ignore_ascii_case("kf")
                || info.eq_ignore_ascii_case("kf+")
                || info.eq_ignore_ascii_case("kf-")
                || info.eq_ignore_ascii_case("tabs");
            if !caret_on_opener && !is_rendered_fence {
                let body_end_estimate = find_fence_close(text, content_start, mc, mlen);
                let header_html = format!(
                    r#"<span class="md-code-header"><span class="md-code-lang">{lang}</span><button class="md-code-copy" data-copy-from="{from}" data-copy-to="{to}" title="Copy">⧉</button></span>"#,
                    lang = if info.is_empty() { "plain" } else { info },
                    from = content_start,
                    to = body_end_estimate,
                );
                out.push(Decoration::widget(line_from, header_html));
            }
            if let Some(lang) = editor_syntax::Lang::from_fence_tag(info) {
                emit_fence_tokens(text, content_start, mc, mlen, lang, out);
            }
            // ```typst — render the body as a Typst document
            // and emit a single SVG widget on the closing fence
            // line so the rendered output sits below the source
            // code. Skip when the caret is anywhere inside the
            // fence range (so the user sees the raw source while
            // editing).
            if info.eq_ignore_ascii_case("typst")
                || info.eq_ignore_ascii_case("mermaid")
                || info.eq_ignore_ascii_case("kf")
                || info.eq_ignore_ascii_case("kf+")
                || info.eq_ignore_ascii_case("kf-")
                || info.eq_ignore_ascii_case("tabs")
            {
                let body_end = find_fence_close(text, content_start, mc, mlen);
                let body = &text[content_start..body_end];
                // Extend the replace range to cover the closing
                // ``` line so the rendered output stands alone
                // when caret is away.
                let bytes = text.as_bytes();
                let mut close_end = body_end;
                while close_end < bytes.len() && bytes[close_end] != b'\n' {
                    close_end += 1;
                }
                let fence_range = line_from..close_end;
                if !cursor_touches(primary, fence_range.clone()) && !body.trim().is_empty() {
                    let is_mermaid = info.eq_ignore_ascii_case("mermaid");
                    // The keyflow fence family — the AUTHOR picks per
                    // snippet what shows, portable in the markdown:
                    //   ```kf   → engraved chart only (default)
                    //   ```kf+  → chart AND keyflow-highlighted source
                    //   ```kf-  → highlighted source only, no chart
                    // All three shed the code frame and carry a header
                    // (the `kf` tag + a copy button, top-right). kf/kf+
                    // also get a `</>` hover toggle to flip the source.
                    let kf_kind = if info.eq_ignore_ascii_case("kf") {
                        Some((false, true)) // (show_source, has_chart)
                    } else if info.eq_ignore_ascii_case("kf+") {
                        Some((true, true))
                    } else if info.eq_ignore_ascii_case("kf-") {
                        Some((true, false))
                    } else {
                        None
                    };
                    if let Some((show_source, has_chart)) = kf_kind {
                        // Chart (kf/kf+) needs a successful engrave;
                        // source-only (kf-) always renders.
                        let svg = if has_chart {
                            render_keyflow(body)
                        } else {
                            None
                        };
                        if has_chart && svg.is_none() {
                            // Bad chart source — leave the raw fence
                            // (falls through to the code path below).
                        } else {
                            let show = if show_source {
                                " md-keyflow-show-source"
                            } else {
                                ""
                            };
                            let only = if has_chart {
                                ""
                            } else {
                                " md-keyflow-source-only"
                            };
                            // Header (fence tag + copy, plus the source
                            // toggle when a chart is present). It lives
                            // INSIDE the display block — the chart's
                            // top-right corner (or the source block's,
                            // when there's no chart) — anchored there,
                            // not overlaid on the widget. Copy grabs the
                            // raw body.
                            let toggle = if has_chart {
                                r#"<button type="button" class="md-keyflow-toggle" title="Show source">&lt;/&gt;</button>"#
                            } else {
                                ""
                            };
                            let header = format!(
                                r#"<div class="md-keyflow-header"><span class="md-keyflow-lang">{tag}</span><button class="md-code-copy" data-copy-from="{content_start}" data-copy-to="{body_end}" title="Copy">⧉</button>{toggle}</div>"#,
                                tag = escape_html(info),
                            );
                            let highlighted = editor_keyflow::highlight_html(body);
                            let html = if let Some(svg) = svg {
                                // Chart present: header anchors to the
                                // chart's top-right; the source block (if
                                // shown) stacks above it.
                                format!(
                                    r#"<div class="md-keyflow-widget{show}{only}" data-focus-pos="{content_start}"><div class="md-keyflow-sourcebox"><pre class="md-keyflow-source"><code class="kf-code">{highlighted}</code></pre></div><div class="md-keyflow-render">{header}{svg}</div></div>"#,
                                )
                            } else {
                                // Source only: header anchors to the
                                // source block's top-right.
                                format!(
                                    r#"<div class="md-keyflow-widget{show}{only}" data-focus-pos="{content_start}"><div class="md-keyflow-sourcebox">{header}<pre class="md-keyflow-source"><code class="kf-code">{highlighted}</code></pre></div></div>"#,
                                )
                            };
                            out.push(Decoration::replace(fence_range.clone()));
                            out.push(Decoration::widget(fence_range.start, html));
                        }
                    } else if info.eq_ignore_ascii_case("tabs") {
                        // ```tabs — split the body into `=== Tab`
                        // panels and render one self-contained,
                        // CSS-only tab widget (hidden radios +
                        // `<label>` strip + `:checked ~ .panel`
                        // rules — no JS, since the widget is a
                        // static injected HTML string). The scope
                        // hash folds in `content_start` so two
                        // blocks never share a radio group.
                        if let Some(inner) = render_tabs(body, content_start) {
                            let html = format!(
                                r#"<div class="md-tabs-widget" data-focus-pos="{content_start}">{inner}</div>"#,
                            );
                            out.push(Decoration::replace(fence_range.clone()));
                            out.push(Decoration::widget(fence_range.start, html));
                        }
                    } else {
                        let svg = if is_mermaid {
                            render_mermaid(body)
                        } else {
                            render_typst(TypstKind::Block, body)
                        };
                        if let Some(svg) = svg {
                            let class = if is_mermaid {
                                "md-mermaid-widget"
                            } else {
                                "md-typst-widget"
                            };
                            let html = format!(
                                r#"<div class="{class}" data-focus-pos="{content_start}">{svg}</div>"#,
                            );
                            out.push(Decoration::replace(fence_range.clone()));
                            out.push(Decoration::widget(fence_range.start, html));
                        }
                    }
                }
            }
            continue;
        }

        // ── Headings ───────────────────────────────────────
        if let Some((level, marker_end)) = parse_heading(line) {
            let abs_marker_end = line_from + marker_end;
            if doc_is_setlist
                && level == 1
                && !setlist_h1_done
                && !cursor_touches(primary, line_from..line_to)
            {
                setlist_h1_done = true;
                let title = html_escape(line[marker_end..].trim());
                out.push(Decoration::replace(line_from..line_to));
                out.push(Decoration::widget(
                    line_from,
                    format!(
                        r#"<span class="md-setlist-header"><span class="md-setlist-art">🎵</span><span class="md-setlist-titles"><span class="md-setlist-title">{title}</span><span class="md-setlist-kind">Setlist</span></span><span class="md-setlist-playbtn" data-href="setlist-play:">▶</span><span class="md-setlist-openbtn" data-href="setlist-open:">Open</span></span>"#
                    ),
                ));
                out.push(Decoration::atomic(line_from..line_to));
                continue;
            }
            if doc_is_event
                && level == 1
                && !setlist_h1_done
                && !cursor_touches(primary, line_from..line_to)
            {
                setlist_h1_done = true;
                let title = html_escape(line[marker_end..].trim());
                let date = html_escape(&event_date);
                let date_html = if date.is_empty() {
                    String::new()
                } else {
                    format!(r#"<span class="md-event-date">{date}</span>"#)
                };
                out.push(Decoration::replace(line_from..line_to));
                out.push(Decoration::widget(
                    line_from,
                    format!(
                        r#"<span class="md-setlist-header md-event-header"><span class="md-setlist-art">📅</span><span class="md-setlist-titles"><span class="md-setlist-title">{title}</span><span class="md-setlist-kind">Event{date_sep}{date_html}</span></span></span>"#,
                        date_sep = if date.is_empty() { "" } else { " · " },
                    ),
                ));
                out.push(Decoration::atomic(line_from..line_to));
                continue;
            }
            if (doc_is_setlist || doc_is_event) && level == 1 && !setlist_h1_done {
                // Caret on the title: keep it editable, but mark it
                // consumed so a SECOND h1 renders normally.
                setlist_h1_done = true;
            }
            let class = HEADING_CLASS[level - 1];
            out.push(Decoration::line(line_from, class));
            // Marker stays visible (muted) any time the caret
            // is anywhere on the line — typing inside the
            // heading body shouldn't make the marker disappear.
            if cursor_touches(primary, line_from..line_to) {
                out.push(Decoration::mark(
                    line_from..abs_marker_end,
                    "md-heading-marker",
                ));
            } else {
                out.push(Decoration::replace(line_from..abs_marker_end));
            }
            continue;
        }

        // ── HR ─────────────────────────────────────────────
        if is_hr(line) {
            // Two states:
            //   `md-hr-active` while the caret is on the line —
            //     just dim the `---` text, leave it on one row
            //     so the user can edit it.
            //   `md-hr` otherwise — hide the text bytes and let
            //     CSS render a single-row horizontal rule via
            //     the line's own border-top.
            if cursor_touches(primary, line_from..line_to) {
                out.push(Decoration::line(line_from, "md-hr-active"));
            } else {
                out.push(Decoration::line(line_from, "md-hr"));
                out.push(Decoration::replace(line_from..line_to));
            }
            continue;
        }

        // ── Task list ──────────────────────────────────────
        if let Some((prefix_end, checked)) = parse_task_marker(line) {
            let abs_prefix_end = line_from + prefix_end;
            out.push(Decoration::line(line_from, "md-task"));
            // The checkbox widget is always emitted so it stays
            // clickable regardless of caret position. The source
            // bytes are hidden via Replace only when the caret is
            // off the line — when the user is editing the line
            // we keep them visible so they can mutate the marker
            // directly and clicks/motions land on real text.
            let html = if checked {
                format!(
                    r#"<span class="md-task-checkbox checked" data-task-pos="{line_from}">✓</span>"#
                )
            } else {
                format!(r#"<span class="md-task-checkbox" data-task-pos="{line_from}"></span>"#)
            };
            // Two states: source-visible when the caret is on
            // the line (so `- [ ]` is editable), checkbox-widget
            // otherwise. Rendering both at once gave you the
            // checkbox + the literal source overlapping.
            if cursor_touches(primary, line_from..line_to) {
                out.push(Decoration::mark(
                    line_from..abs_prefix_end,
                    "md-task-marker-active",
                ));
            } else {
                out.push(Decoration::replace(line_from..abs_prefix_end));
                out.push(Decoration::widget(line_from, html));
            }
            continue;
        }

        // ── Blockquote / Callout (with nesting) ────────────
        if let Some((depth, marker_end)) = parse_blockquote_depth(line) {
            let abs_marker_end = line_from + marker_end;
            // Lines with fewer `>` markers close any deeper
            // callouts. e.g. on `> > body` after `> [!a]\n> >
            // [!b]\n> >body` we'd be at depth 2 still — only a
            // depth-1 or 0 line pops back.
            while callout_stack.len() > depth {
                callout_stack.pop();
            }
            let after_marker = &line[marker_end..];
            // Callout header at the deepest open level.
            if let Some((kind, header_end_off)) = parse_callout_header(after_marker) {
                // Extend the stack with synthetic ancestors if
                // the user opens a depth-3 callout without
                // having opened a depth-2 first. Real docs
                // almost never hit this; the fallback keeps
                // indexing safe.
                while callout_stack.len() < depth - 1 {
                    callout_stack.push("note");
                }
                if callout_stack.len() == depth - 1 {
                    callout_stack.push(kind);
                } else if depth >= 1 {
                    callout_stack[depth - 1] = kind;
                }
                let line_class = callout_class(kind, true);
                out.push(Decoration::line(line_from, line_class));
                if depth > 1 {
                    out.push(Decoration::line(line_from, callout_depth_class(depth)));
                }
                // Hide the `> > [!type] Title` markers when
                // caret is off the line — the line class draws
                // the icon / title styling instead. The marker
                // span covers all `>` chars + their spaces.
                let abs_header_end = abs_marker_end + header_end_off;
                if !cursor_touches(primary, line_from..line_to) {
                    out.push(Decoration::mark(
                        line_from..abs_marker_end,
                        "md-quote-marker",
                    ));
                    out.push(Decoration::replace(abs_marker_end..abs_header_end));
                }
                continue;
            }
            // Plain blockquote or callout body — pick the kind
            // of the deepest currently-open callout (if any).
            let line_class = match callout_stack.last().copied() {
                Some(kind) => callout_class(kind, false),
                None => "md-blockquote",
            };
            out.push(Decoration::line(line_from, line_class));
            if depth > 1 {
                out.push(Decoration::line(line_from, callout_depth_class(depth)));
            }
            if !cursor_touches(primary, line_from..line_to) {
                out.push(Decoration::mark(
                    line_from..abs_marker_end,
                    "md-quote-marker",
                ));
            }
            continue;
        }
        // A line without `>` drains the whole nesting stack.
        callout_stack.clear();

        // ── List (unordered or ordered) ────────────────────
        if let Some(marker_end) = parse_list_marker(line) {
            let abs_marker_end = line_from + marker_end;
            out.push(Decoration::line(line_from, "md-list-item"));
            // Caret on the line: keep the raw `- ` / `1. ` source
            // visible (muted) so clicks land on real text and vim
            // motions don't fall through the Replace into the
            // line-tile end fallback. Off the line: hide source
            // and render the bullet/number widget.
            if cursor_touches(primary, line_from..line_to) {
                out.push(Decoration::mark(
                    line_from..abs_marker_end,
                    "md-list-marker-active",
                ));
            } else {
                let kind_byte = line.trim_start().as_bytes()[0];
                let widget_html = if kind_byte.is_ascii_digit() {
                    let leading = line.len() - line.trim_start().len();
                    let num_end = marker_end - 2 - leading;
                    let num = &line.trim_start()[..num_end];
                    format!(r#"<span class="md-list-marker">{num}.&nbsp;</span>"#)
                } else {
                    r#"<span class="md-list-marker">-&nbsp;</span>"#.into()
                };
                out.push(Decoration::replace(line_from..abs_marker_end));
                out.push(Decoration::widget(line_from, widget_html));
            }
            continue;
        }
    }

    // EOF with unclosed fence — close it implicitly at doc end so
    // the inline parser still skips that range.
    if let Some((open_end, _, _, _)) = fence {
        fenced_ranges.push(open_end..text.len());
    }

    fenced_ranges
}

/// A frontmatter scalar (`key: value`) from the document's YAML fence.
fn frontmatter_scalar(text: &str, key: &str) -> Option<String> {
    let rest = text.strip_prefix("---")?;
    let (front, _) = rest.split_once("\n---")?;
    front.lines().find_map(|l| {
        l.trim_start()
            .strip_prefix(key)
            .and_then(|r| r.strip_prefix(':'))
            .map(|v| v.trim().trim_matches(['"', '\'']).trim().to_owned())
    })
}

/// Does the document's YAML frontmatter declare `type: setlist`?
fn frontmatter_declares_setlist(text: &str) -> bool {
    frontmatter_scalar(text, "type").as_deref() == Some("setlist")
}

const HEADING_CLASS: [&str; 6] = ["md-h1", "md-h2", "md-h3", "md-h4", "md-h5", "md-h6"];

/// Match a callout header `[!type] [title]` after the `> ` of a
/// blockquote line. Returns the canonical callout kind (lower-
/// cased + alias-resolved) and the byte offset within `after`
/// where the `[!type]` syntax ends (excluding any title). The
/// type list mirrors Obsidian / Quartz: `ofm.ts:63-91`.
fn parse_callout_header(after: &str) -> Option<(&'static str, usize)> {
    let b = after.as_bytes();
    if !b.starts_with(b"[!") {
        return None;
    }
    let close = b.iter().skip(2).position(|&c| c == b']')?;
    let raw = &after[2..2 + close];
    // Strip the optional collapse suffix the user can add via
    // `+`/`-` on the closing bracket — `[!note]+` / `[!note]-`.
    let kind = canonical_callout_kind(raw)?;
    let mut end = 2 + close + 1;
    if matches!(b.get(end), Some(b'+' | b'-')) {
        end += 1;
    }
    // Consume the space that typically follows.
    if b.get(end) == Some(&b' ') {
        end += 1;
    }
    Some((kind, end))
}

fn canonical_callout_kind(raw: &str) -> Option<&'static str> {
    Some(match raw.trim().to_ascii_lowercase().as_str() {
        "note" => "note",
        "abstract" | "summary" | "tldr" => "abstract",
        "info" => "info",
        "todo" => "todo",
        "tip" | "hint" | "important" => "tip",
        "success" | "check" | "done" => "success",
        "question" | "help" | "faq" => "question",
        "warning" | "attention" | "caution" => "warning",
        "failure" | "missing" | "fail" => "failure",
        "danger" | "error" => "danger",
        "bug" => "bug",
        "example" => "example",
        "quote" | "cite" => "quote",
        _ => return None,
    })
}

/// Cheap "is this even a candidate?" check: a non-trivial pipe
/// table header must start (after optional spaces) with `|` and
/// contain at least one other `|`.
fn is_table_header(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('|') && t[1..].contains('|')
}

/// Walk forward from the line that looks like a table header.
/// Returns the byte ranges of all table rows (header + sep +
/// body) when valid, or `None` if the structure doesn't hold.
fn try_parse_table(
    text: &str,
    header_from: usize,
    header_to: usize,
) -> Option<Vec<(usize, usize)>> {
    let bytes = text.as_bytes();
    // Find the separator line directly after the header.
    let sep_from = if header_to < bytes.len() && bytes[header_to] == b'\n' {
        header_to + 1
    } else {
        return None;
    };
    let mut sep_end = sep_from;
    while sep_end < bytes.len() && bytes[sep_end] != b'\n' {
        sep_end += 1;
    }
    let sep_line = &text[sep_from..sep_end];
    let header_cells = split_pipe_cells(&text[header_from..header_to]);
    let sep_cells = split_pipe_cells(sep_line);
    if header_cells.len() != sep_cells.len() || header_cells.is_empty() {
        return None;
    }
    for cell in &sep_cells {
        let c = cell.trim();
        if c.is_empty() {
            return None;
        }
        if !c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')) {
            return None;
        }
    }
    let mut rows = vec![(header_from, header_to), (sep_from, sep_end)];
    let mut i = if sep_end < bytes.len() {
        sep_end + 1
    } else {
        sep_end
    };
    while i < bytes.len() {
        let row_from = i;
        let mut row_end = row_from;
        while row_end < bytes.len() && bytes[row_end] != b'\n' {
            row_end += 1;
        }
        let row_line = &text[row_from..row_end];
        if row_line.trim().is_empty() || !row_line.trim_start().starts_with('|') {
            break;
        }
        rows.push((row_from, row_end));
        i = if row_end < bytes.len() {
            row_end + 1
        } else {
            row_end
        };
    }
    Some(rows)
}

fn split_pipe_cells(line: &str) -> Vec<&str> {
    let mut t = line.trim();
    if let Some(stripped) = t.strip_prefix('|') {
        t = stripped;
    }
    if let Some(stripped) = t.strip_suffix('|') {
        t = stripped;
    }
    t.split('|').map(str::trim).collect()
}

fn collect_table_cells(text: &str, rows: &[(usize, usize)]) -> Vec<Vec<String>> {
    rows.iter()
        .enumerate()
        .filter(|(idx, _)| *idx != 1) // drop the separator row
        .map(|(_, (f, t))| {
            split_pipe_cells(&text[*f..*t])
                .into_iter()
                .map(std::string::ToString::to_string)
                .collect()
        })
        .collect()
}

fn render_table_html(cells: &[Vec<String>]) -> String {
    if cells.is_empty() {
        return String::new();
    }
    let mut s = String::from(r#"<table class="md-table">"#);
    let mut iter = cells.iter();
    if let Some(header) = iter.next() {
        s.push_str("<thead><tr>");
        for c in header {
            s.push_str(r"<th>");
            s.push_str(&html_escape(c));
            s.push_str("</th>");
        }
        s.push_str("</tr></thead>");
    }
    s.push_str("<tbody>");
    for row in iter {
        s.push_str("<tr>");
        for c in row {
            s.push_str(r"<td>");
            s.push_str(&html_escape(c));
            s.push_str("</td>");
        }
        s.push_str("</tr>");
    }
    s.push_str("</tbody></table>");
    s
}

/// Composite class for nested callouts so CSS can indent the
/// deeper levels. Depth `1` is the unnested base (no class
/// emitted by the caller); `2`+ each get a level-specific
/// class. Caps at 4 — anything deeper falls back to level 4
/// styling, which is fine for the rare deep-nesting edge case.
fn callout_depth_class(depth: usize) -> &'static str {
    match depth {
        2 => "md-callout-nested-2",
        3 => "md-callout-nested-3",
        _ => "md-callout-nested-4",
    }
}

fn callout_class(kind: &str, is_header: bool) -> &'static str {
    // The decoration::Line variant takes a `String` so we have
    // to return a `&'static str` selected from a fixed table.
    // 13 kinds × 2 (header/body) — 26 entries; one match.
    match (kind, is_header) {
        ("note", true) => "md-callout md-callout-note md-callout-header",
        ("note", false) => "md-callout md-callout-note",
        ("abstract", true) => "md-callout md-callout-abstract md-callout-header",
        ("abstract", false) => "md-callout md-callout-abstract",
        ("info", true) => "md-callout md-callout-info md-callout-header",
        ("info", false) => "md-callout md-callout-info",
        ("todo", true) => "md-callout md-callout-todo md-callout-header",
        ("todo", false) => "md-callout md-callout-todo",
        ("tip", true) => "md-callout md-callout-tip md-callout-header",
        ("tip", false) => "md-callout md-callout-tip",
        ("success", true) => "md-callout md-callout-success md-callout-header",
        ("success", false) => "md-callout md-callout-success",
        ("question", true) => "md-callout md-callout-question md-callout-header",
        ("question", false) => "md-callout md-callout-question",
        ("warning", true) => "md-callout md-callout-warning md-callout-header",
        ("warning", false) => "md-callout md-callout-warning",
        ("failure", true) => "md-callout md-callout-failure md-callout-header",
        ("failure", false) => "md-callout md-callout-failure",
        ("danger", true) => "md-callout md-callout-danger md-callout-header",
        ("danger", false) => "md-callout md-callout-danger",
        ("bug", true) => "md-callout md-callout-bug md-callout-header",
        ("bug", false) => "md-callout md-callout-bug",
        ("example", true) => "md-callout md-callout-example md-callout-header",
        ("example", false) => "md-callout md-callout-example",
        ("quote", true) => "md-callout md-callout-quote md-callout-header",
        ("quote", false) => "md-callout md-callout-quote",
        _ => "md-blockquote",
    }
}

/// Iterate `(line_from, line_to)` byte ranges, exclusive of the
/// trailing `\n`. The last line (no trailing `\n`) is included.
fn line_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            out.push((start, i));
            start = i + 1;
        }
    }
    if start <= bytes.len() {
        out.push((start, bytes.len()));
    }
    out
}

fn parse_heading(line: &str) -> Option<(usize, usize)> {
    let b = line.as_bytes();
    let mut level = 0;
    while level < 6 && b.get(level) == Some(&b'#') {
        level += 1;
    }
    if level == 0 {
        return None;
    }
    // Must be followed by a space (`# foo`) — otherwise it's a
    // tag (`#foo`) or just hashes.
    if b.get(level) != Some(&b' ') {
        return None;
    }
    Some((level, level + 1))
}

/// Count the depth of a nested blockquote (number of `>`
/// markers at the start of the line) and the byte offset where
/// the content body begins. Spaces between successive `>` are
/// tolerated — `> > [!note]` is the canonical Obsidian form.
/// Returns `None` when the line doesn't start with `>`.
fn parse_blockquote_depth(line: &str) -> Option<(usize, usize)> {
    let b = line.as_bytes();
    if b.first() != Some(&b'>') {
        return None;
    }
    let mut i = 0;
    let mut depth = 0;
    while i < b.len() && b[i] == b'>' {
        depth += 1;
        i += 1;
        // Eat the optional separator space — either between
        // successive `>` markers or before the body.
        if i < b.len() && b[i] == b' ' {
            i += 1;
        }
    }
    Some((depth, i))
}

fn is_hr(line: &str) -> bool {
    let t = line.trim();
    if t.len() < 3 {
        return false;
    }
    let c = t.as_bytes()[0];
    if c != b'-' && c != b'*' && c != b'_' {
        return false;
    }
    t.bytes().all(|x| x == c)
}

fn parse_task_marker(line: &str) -> Option<(usize, bool)> {
    let b = line.as_bytes();
    // `- [ ]` / `- [x]` / `- [/]` / `- [>]` / `* [ ]` ... — must
    // be at least 5 bytes for the `- [X]` form.
    if b.len() < 5 {
        return None;
    }
    let bullet = b[0];
    if (bullet != b'-' && bullet != b'*' && bullet != b'+') || b[1] != b' ' || b[2] != b'[' {
        return None;
    }
    let inner = b[3];
    if b[4] != b']' {
        return None;
    }
    // Accept any single printable ASCII as a status — Obsidian's
    // Tasks plugin convention uses `x`, ` `, `/` (in-progress),
    // `>` (forwarded), `<` (scheduled), `-` (cancelled), `?`
    // (question), `!` (important). Anything else still parses
    // (treated as unchecked) so we don't break funky user
    // schemes; styling can specialize via data attrs later.
    let is_valid_status = inner == b' '
        || inner.is_ascii_alphanumeric()
        || matches!(inner, b'/' | b'>' | b'<' | b'-' | b'?' | b'!' | b'.' | b'*');
    if !is_valid_status {
        return None;
    }
    let checked = matches!(inner, b'x' | b'X');
    let end = if b.get(5) == Some(&b' ') { 6 } else { 5 };
    Some((end, checked))
}

fn parse_list_marker(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    let leading = b.iter().take_while(|&&c| c == b' ').count();
    let after = &b[leading..];
    // Unordered: `- ` / `* ` / `+ `.
    if let Some(&c) = after.first() {
        if (c == b'-' || c == b'*' || c == b'+') && after.get(1) == Some(&b' ') {
            return Some(leading + 2);
        }
    }
    // Ordered: `1. ` / `12. `.
    let digit_count = after.iter().take_while(|&&x| x.is_ascii_digit()).count();
    if digit_count > 0
        && after.get(digit_count) == Some(&b'.')
        && after.get(digit_count + 1) == Some(&b' ')
    {
        return Some(leading + digit_count + 2);
    }
    None
}

/// Does this trimmed line *open* a fence? Returns
/// `(marker_char, marker_len, info_string_start_offset)`.
fn opens_fence(trimmed: &str) -> Option<(u8, usize, usize)> {
    let b = trimmed.as_bytes();
    if b.len() < 3 {
        return None;
    }
    let c = b[0];
    if c != b'`' && c != b'~' {
        return None;
    }
    let run = b.iter().take_while(|&&x| x == c).count();
    if run < 3 {
        return None;
    }
    Some((c, run, run))
}

/// Find the byte offset of the closing fence's `\n` (or doc end)
/// when walking forward from `content_start`. Used by both the
/// syntax-highlighter and the lang/copy header widget.
fn find_fence_close(text: &str, content_start: usize, marker_char: u8, marker_len: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = content_start;
    while i < bytes.len() {
        let line_from = i;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        let line = &text[line_from..i];
        if is_closing_fence(line, marker_char, marker_len) {
            return line_from;
        }
        if i < bytes.len() {
            i += 1;
        }
    }
    text.len()
}

/// Slice the fenced-body bytes between `content_start` and the
/// matching closing fence (or doc end), run the syntax highlighter,
/// and emit one `Mark` decoration per token.
///
/// Memoized by `(lang, body)` — typing OUTSIDE a fence shouldn't
/// re-parse the fence with tree-sitter on every keystroke. The
/// cache is bounded so it doesn't grow forever; entries evict
/// LRU-style when the bound is hit.
fn emit_fence_tokens(
    text: &str,
    content_start: usize,
    marker_char: u8,
    marker_len: usize,
    lang: editor_syntax::Lang,
    out: &mut Vec<DecoratedRange>,
) {
    let t_find = now_ms_native();
    let end = find_fence_close(text, content_start, marker_char, marker_len);
    let find_ms = now_ms_native() - t_find;
    if end <= content_start {
        return;
    }
    let body = &text[content_start..end];
    let t_tok = now_ms_native();
    let cached = with_fence_cache(|cache| cache.get(lang, body));
    let was_cached = cached.is_some();
    let tokens = if let Some(toks) = cached {
        toks
    } else {
        let toks = editor_syntax::highlight(lang, body);
        with_fence_cache(|cache| cache.put(lang, body.to_string(), toks.clone()));
        toks
    };
    let tok_ms = now_ms_native() - t_tok;
    tracing::trace!(
        body_len = body.len(),
        token_count = tokens.len(),
        cache_hit = was_cached,
        find_ms = %format!("{:.2}", find_ms),
        tokenize_ms = %format!("{:.2}", tok_ms),
        "md.fence_tokens"
    );
    for tok in tokens {
        let abs_from = content_start + tok.start;
        let abs_to = content_start + tok.end;
        let class = format!("md-tok-{}", tok.tag);
        out.push(Decoration::mark(abs_from..abs_to, class));
    }
}

/// Bounded LRU-ish cache of `(lang, body) -> tokens`. Sized for
/// the common case of a handful of fences in a doc. Tree-sitter
/// parses are the per-keystroke cost we want to avoid.
struct FenceCache {
    entries: Vec<(editor_syntax::Lang, String, Vec<editor_syntax::Token>)>,
    cap: usize,
}

impl FenceCache {
    fn new(cap: usize) -> Self {
        Self {
            entries: Vec::with_capacity(cap),
            cap,
        }
    }
    fn get(&mut self, lang: editor_syntax::Lang, body: &str) -> Option<Vec<editor_syntax::Token>> {
        let idx = self
            .entries
            .iter()
            .position(|(l, b, _)| *l == lang && b == body)?;
        // Move to back so this entry is "freshest".
        let hit = self.entries.remove(idx);
        let toks = hit.2.clone();
        self.entries.push(hit);
        Some(toks)
    }
    fn put(&mut self, lang: editor_syntax::Lang, body: String, toks: Vec<editor_syntax::Token>) {
        if self.entries.len() >= self.cap {
            self.entries.remove(0);
        }
        self.entries.push((lang, body, toks));
    }
}

fn with_fence_cache<R>(f: impl FnOnce(&mut FenceCache) -> R) -> R {
    thread_local! {
        static CACHE: std::cell::RefCell<FenceCache> =
            std::cell::RefCell::new(FenceCache::new(16));
    }
    CACHE.with(|c| f(&mut c.borrow_mut()))
}

fn is_closing_fence(line: &str, marker_char: u8, marker_len: usize) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < marker_len {
        return false;
    }
    let b = trimmed.as_bytes();
    let run = b.iter().take_while(|&&x| x == marker_char).count();
    run >= marker_len && b[run..].iter().all(|&x| x == b' ')
}

/// Single-pass scanner. Walks bytes, recognizing the inline
/// markdown flavors supported by live-preview. Doesn't cross
/// newlines (a stray `*` on one line shouldn't pair with one on
/// the next). Top-level only — no nesting yet (e.g. `**a~~b~~c**`
/// gets the bold but not the strike inside).
fn find_spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\n' {
            i += 1;
            continue;
        }
        // ***bold-italic***  — must precede `**` so the triple
        // marker isn't consumed as nested bold + italic.
        if i + 6 <= b.len() && &b[i..i + 3] == b"***" {
            if let Some(end) = find_close(b, i + 3, b"***") {
                out.push(Span {
                    outer: i..end + 3,
                    body: i + 3..end,
                    class: "md-bold-italic",
                });
                i = end + 3;
                continue;
            }
        }
        // **bold**
        if i + 4 <= b.len() && &b[i..i + 2] == b"**" {
            if let Some(end) = find_close(b, i + 2, b"**") {
                out.push(Span {
                    outer: i..end + 2,
                    body: i + 2..end,
                    class: "md-bold",
                });
                i = end + 2;
                continue;
            }
        }
        // ~~strikethrough~~
        if i + 4 <= b.len() && &b[i..i + 2] == b"~~" {
            if let Some(end) = find_close(b, i + 2, b"~~") {
                out.push(Span {
                    outer: i..end + 2,
                    body: i + 2..end,
                    class: "md-strike",
                });
                i = end + 2;
                continue;
            }
        }
        // ==highlight==
        if i + 4 <= b.len() && &b[i..i + 2] == b"==" {
            if let Some(end) = find_close(b, i + 2, b"==") {
                out.push(Span {
                    outer: i..end + 2,
                    body: i + 2..end,
                    class: "md-highlight",
                });
                i = end + 2;
                continue;
            }
        }
        // %% obsidian comment %% — body hidden when caret away,
        // styled subtly when revealed. Quartz: `ofm.ts:132`.
        if i + 4 <= b.len() && &b[i..i + 2] == b"%%" {
            if let Some(end) = find_close(b, i + 2, b"%%") {
                out.push(Span {
                    outer: i..end + 2,
                    body: i + 2..end,
                    class: "md-comment",
                });
                i = end + 2;
                continue;
            }
        }
        // $$display math$$ — must precede the `$inline$` arm so
        // the doubled marker doesn't get consumed as two empty
        // inline maths. Body is the Typst math source.
        if i + 4 <= b.len() && &b[i..i + 2] == b"$$" {
            if let Some(end) = find_close(b, i + 2, b"$$") {
                out.push(Span {
                    outer: i..end + 2,
                    body: i + 2..end,
                    class: "md-math-block",
                });
                i = end + 2;
                continue;
            }
        }
        // $inline math$ — single-dollar pair. Skip if the body
        // would be empty (`$$` already handled above) or starts
        // with whitespace (avoids matching prose like
        // "Cost is $5 not $10").
        if b[i] == b'$' && i + 2 < b.len() && b[i + 1] != b' ' && b[i + 1] != b'$' {
            if let Some(end) = find_close(b, i + 1, b"$") {
                if end > i + 1 && b[end - 1] != b' ' {
                    out.push(Span {
                        outer: i..end + 1,
                        body: i + 1..end,
                        class: "md-math-inline",
                    });
                    i = end + 1;
                    continue;
                }
            }
        }
        // `inline code`
        if b[i] == b'`' {
            if let Some(end) = find_close(b, i + 1, b"`") {
                out.push(Span {
                    outer: i..end + 1,
                    body: i + 1..end,
                    class: "md-code",
                });
                i = end + 1;
                continue;
            }
        }
        // *italic* — must not be `**` (handled above) and must
        // contain at least one char.
        if b[i] == b'*' {
            if let Some(end) = find_close(b, i + 1, b"*") {
                if end > i + 1 && b[end + 1..].first() != Some(&b'*') {
                    out.push(Span {
                        outer: i..end + 1,
                        body: i + 1..end,
                        class: "md-italic",
                    });
                    i = end + 1;
                    continue;
                }
            }
        }
        // ![[embed]]  — image/audio/video/pdf embed by file
        // extension on the target. Recognized before the plain
        // `[[wikilink]]` arm. Quartz: `ofm.ts:233-265`.
        if i + 5 <= b.len() && &b[i..i + 3] == b"![[" {
            if let Some(end) = find_close(b, i + 3, b"]]") {
                out.push(Span {
                    outer: i..end + 2,
                    body: i + 3..end,
                    class: "md-embed",
                });
                i = end + 2;
                continue;
            }
        }
        // [[wikilink]]  — keep before `[link]` so the `[[`
        // isn't misread as the start of a regular link.
        if i + 4 <= b.len() && &b[i..i + 2] == b"[[" {
            if let Some(end) = find_close(b, i + 2, b"]]") {
                out.push(Span {
                    outer: i..end + 2,
                    body: i + 2..end,
                    class: "md-wikilink",
                });
                i = end + 2;
                continue;
            }
        }
        // [^footnote-ref]
        if i + 4 <= b.len() && &b[i..i + 2] == b"[^" {
            if let Some(end) = find_close(b, i + 2, b"]") {
                out.push(Span {
                    outer: i..end + 1,
                    body: i + 2..end,
                    class: "md-footnote",
                });
                i = end + 1;
                continue;
            }
        }
        // ^[inline footnote body] — Obsidian extension. Body is
        // styled like a footnote reference but inline (the text
        // is the footnote content, not a refnum). Must match
        // BEFORE the `^block-id` arm, which would otherwise eat
        // the leading `^`.
        if b[i] == b'^' && b.get(i + 1) == Some(&b'[') {
            if let Some(end) = find_close(b, i + 2, b"]") {
                out.push(Span {
                    outer: i..end + 1,
                    body: i + 2..end,
                    class: "md-inline-footnote",
                });
                i = end + 1;
                continue;
            }
        }
        // ^block-id — an Obsidian block reference target,
        // emitted at the end of a paragraph / list-item. We
        // recognize it only when followed by EOL (or end of
        // doc), so a stray `^` inside a sentence isn't styled.
        // Boundary check on the left mirrors `tag_boundary_before`.
        if b[i] == b'^'
            && i + 1 < b.len()
            && (b[i + 1].is_ascii_alphanumeric() || b[i + 1] == b'-' || b[i + 1] == b'_')
            && (i == 0 || matches!(b[i - 1], b' ' | b'\t' | b'\n'))
        {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-' || b[j] == b'_') {
                j += 1;
            }
            if j == b.len() || b[j] == b'\n' {
                out.push(Span {
                    outer: i..j,
                    body: i..j,
                    class: "md-block-id",
                });
                i = j;
                continue;
            }
        }
        // `{{embed ((uuid))}}` — block embed (Logseq form).
        // Must match before `((uuid))` so the outer `(` of the
        // embed's payload isn't consumed by the bare-ref arm.
        if i + 13 <= b.len() && &b[i..i + 9] == b"{{embed (" {
            // Look for `))}}` closing.
            let payload_start = i + 9; // after `{{embed (`
            if b.get(payload_start) == Some(&b'(') {
                let uuid_start = payload_start + 1;
                if let Some(uuid_len) = peek_uuid(&b[uuid_start..]) {
                    let uuid_end = uuid_start + uuid_len;
                    if b.get(uuid_end..uuid_end + 4) == Some(b"))}}") {
                        out.push(Span {
                            outer: i..uuid_end + 4,
                            body: uuid_start..uuid_end,
                            class: "md-block-embed",
                        });
                        i = uuid_end + 4;
                        continue;
                    }
                }
            }
        }
        // `((uuid))` — Logseq block reference. The body is the
        // 36-char UUID itself; outer adds the `(( ))` markers.
        if i + 40 <= b.len() && &b[i..i + 2] == b"((" {
            let uuid_start = i + 2;
            if let Some(uuid_len) = peek_uuid(&b[uuid_start..]) {
                let uuid_end = uuid_start + uuid_len;
                if b.get(uuid_end..uuid_end + 2) == Some(b"))") {
                    out.push(Span {
                        outer: i..uuid_end + 2,
                        body: uuid_start..uuid_end,
                        class: "md-block-ref",
                    });
                    i = uuid_end + 2;
                    continue;
                }
            }
        }
        // <https://…> autolink (also matches mailto-shaped
        // `<foo@bar.baz>`). The body becomes the URL itself; the
        // angle brackets are styling-only.
        if b[i] == b'<' {
            if let Some(end) = find_close(b, i + 1, b">") {
                let body = &text[i + 1..end];
                let is_url = body.starts_with("http://")
                    || body.starts_with("https://")
                    || body.starts_with("mailto:")
                    || (body.contains('@') && !body.contains(' ') && body.contains('.'));
                if is_url {
                    out.push(Span {
                        outer: i..end + 1,
                        body: i + 1..end,
                        class: "md-autolink",
                    });
                    i = end + 1;
                    continue;
                }
            }
        }
        // [text](url) — find `]` then verify `(...)` follows.
        if b[i] == b'[' {
            if let Some(close_text) = find_close(b, i + 1, b"]") {
                if b.get(close_text + 1) == Some(&b'(') {
                    if let Some(close_paren) = find_close(b, close_text + 2, b")") {
                        out.push(Span {
                            outer: i..close_paren + 1,
                            body: i + 1..close_text,
                            class: "md-link",
                        });
                        i = close_paren + 1;
                        continue;
                    }
                }
            }
        }
        // #tag  — `#` at doc start or after non-word char,
        // followed by tag chars (alnum / `-` / `_` / `/`). The
        // body equals the outer (no markers to hide) so the
        // mark just colors the whole `#tag` string.
        if b[i] == b'#' && tag_boundary_before(b, i) {
            let start = i;
            let mut j = i + 1;
            while j < b.len() && is_tag_char(b[j]) {
                j += 1;
            }
            // Need at least one tag char after `#`.
            if j > i + 1 {
                out.push(Span {
                    outer: start..j,
                    body: start..j,
                    class: "md-tag",
                });
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn tag_boundary_before(b: &[u8], i: usize) -> bool {
    if i == 0 {
        // A `#` at the very start of the doc is a heading if
        // followed by a space; otherwise treat it as a tag.
        return b.get(1) != Some(&b' ');
    }
    let prev = b[i - 1];
    // `#` immediately after a newline followed by a space is a
    // heading marker, not a tag.
    if prev == b'\n' && b.get(i + 1) == Some(&b' ') {
        return false;
    }
    !prev.is_ascii_alphanumeric() && prev != b'_' && prev != b'/'
}

fn is_tag_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'/'
}

/// Find the next occurrence of `needle` in `b` starting at
/// `from`, returning the start byte offset. Stops at newlines
/// (a span can't cross a line boundary).
/// Parse a `id:: <uuid>` block-id property line. Returns the
/// byte range of the UUID within the doc (absolute) when the
/// line matches, else `None`. Leading whitespace is tolerated
/// so an indented bullet's block-id is recognized too.
fn parse_block_id_line(line: &str, line_from: usize) -> Option<std::ops::Range<usize>> {
    let trimmed_start = line.len() - line.trim_start().len();
    let rest = &line[trimmed_start..];
    let prefix = "id:: ";
    let rest = rest.strip_prefix(prefix)?;
    let rest_off = trimmed_start + prefix.len();
    let bytes = rest.as_bytes();
    let uuid_len = peek_uuid(bytes)?;
    // Allow trailing whitespace but nothing else after the UUID.
    if rest.len() > uuid_len && !rest[uuid_len..].trim().is_empty() {
        return None;
    }
    Some(line_from + rest_off..line_from + rest_off + uuid_len)
}

/// Walk back from a line offset to the start of the block the
/// `id::` line belongs to. For a paragraph or list item, that's
/// the line directly above (or the start of the nearest non-
/// empty block above). For v1 we just return the previous
/// non-empty line's start.
fn find_block_anchor(text: &str, id_line_from: usize) -> usize {
    if id_line_from == 0 {
        return 0;
    }
    let prefix = &text[..id_line_from];
    // Walk back over any blank lines (shouldn't be common — the
    // `id::` line should be flush against the block).
    let mut end = id_line_from;
    while end > 0 {
        let prev_nl = prefix[..end - 1].rfind('\n').map_or(0, |n| n + 1);
        let line = &text[prev_nl..end - 1];
        if !line.trim().is_empty() {
            return prev_nl;
        }
        end = prev_nl;
    }
    0
}

// Per-`live_preview`-pass registry of UUIDs in the current
// doc. Refreshed on each pass via `reset_block_index`. Used by
// the `((uuid))` chip renderer to look up the target block's
// first-line content and by the `🔗` indicator to know which
// blocks have ids.
thread_local! {
    static BLOCK_INDEX: std::cell::RefCell<std::collections::HashMap<String, usize>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

pub(crate) fn reset_block_index() {
    BLOCK_INDEX.with(|m| m.borrow_mut().clear());
}

pub(crate) fn register_block_id(uuid: &str, block_anchor: usize) {
    BLOCK_INDEX.with(|m| {
        m.borrow_mut().insert(uuid.to_string(), block_anchor);
    });
}

/// First ~40 chars of the block at `anchor`, stripped of
/// markdown markers for chip display. Stops at the first
/// newline.
pub(crate) fn block_preview(text: &str, anchor: usize) -> String {
    let line_end = text[anchor..].find('\n').map_or(text.len(), |n| anchor + n);
    let line = &text[anchor..line_end];
    let cleaned = line.trim_start_matches(|c: char| {
        c == '#'
            || c == '>'
            || c == '-'
            || c == '*'
            || c == '+'
            || c == ' '
            || c == '\t'
            || c == '['
    });
    let cleaned = cleaned.trim_end();
    let max = 40;
    if cleaned.chars().count() > max {
        let truncated: String = cleaned.chars().take(max).collect();
        format!("{truncated}…")
    } else {
        cleaned.to_string()
    }
}

/// Look up a block's anchor offset by UUID. Returns `None` when
/// the UUID isn't in the current doc — multi-file resolution
/// is a later slice.
pub(crate) fn block_anchor_for_uuid(uuid: &str) -> Option<usize> {
    BLOCK_INDEX.with(|m| m.borrow().get(uuid).copied())
}

/// Peek a UUID v4 string at the start of `bytes` and return its
/// length (always 36) if matched, else `None`. Accepted form is
/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` — hex digits in the
/// 8-4-4-4-12 layout, hyphens at positions 8/13/18/23.
pub(crate) fn peek_uuid(bytes: &[u8]) -> Option<usize> {
    const POSITIONS: [(usize, bool); 36] = {
        // (index, is_hyphen)
        let mut arr = [(0usize, false); 36];
        let mut i = 0;
        while i < 36 {
            arr[i] = (i, matches!(i, 8 | 13 | 18 | 23));
            i += 1;
        }
        arr
    };
    if bytes.len() < 36 {
        return None;
    }
    for (idx, is_hyphen) in POSITIONS {
        let c = bytes[idx];
        if is_hyphen {
            if c != b'-' {
                return None;
            }
        } else if !c.is_ascii_hexdigit() {
            return None;
        }
    }
    Some(36)
}

fn find_close(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let mut i = from;
    while i + needle.len() <= b.len() {
        if b[i] == b'\n' {
            return None;
        }
        if &b[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;
    use crate::selection::Selection;

    fn state(text: &str, caret: usize) -> EditorState {
        EditorState {
            doc: Doc::from_str(text),
            selection: Selection::caret(caret),
            folds: Vec::new(),
            reading_mode: false,
        }
    }

    #[test]
    fn bold_with_caret_outside_hides_markers() {
        // "**hi**" at offset 0..6. Body is 2..4 ("hi").
        // Caret at 7 (past the span) — markers should be hidden.
        let s = state("**hi** there", 7);
        let decs = live_preview(&s);
        // Expect: mark(2..4 bold), replace(0..2), replace(4..6).
        assert!(decs.iter().any(|d| d.from == 0 && d.to == 2));
        assert!(decs.iter().any(|d| d.from == 4 && d.to == 6));
        assert!(decs.iter().any(|d| d.from == 2 && d.to == 4));
    }

    #[test]
    fn bold_with_caret_inside_keeps_markers() {
        // Caret at 3 — inside "hi". Markers should NOT be hidden.
        let s = state("**hi** there", 3);
        let decs = live_preview(&s);
        let replace_count = decs
            .iter()
            .filter(|d| matches!(d.kind, crate::decoration::DecorationKind::Replace))
            .count();
        assert_eq!(replace_count, 0, "caret inside span should keep markers");
        // But the body mark is still there.
        assert!(decs.iter().any(|d| d.from == 2 && d.to == 4));
    }

    #[test]
    fn caret_adjacent_to_span_counts_as_touching() {
        // Caret right after the closing `**` — adjacent.
        let s = state("**hi**", 6);
        let decs = live_preview(&s);
        let replace_count = decs
            .iter()
            .filter(|d| matches!(d.kind, crate::decoration::DecorationKind::Replace))
            .count();
        assert_eq!(replace_count, 0);
    }

    #[test]
    fn italic_recognized() {
        let s = state("hello *world*", 0);
        let decs = live_preview(&s);
        assert!(decs.iter().any(|d| matches!(
            &d.kind,
            crate::decoration::DecorationKind::Mark { class, .. } if class == "md-italic"
        )));
    }

    #[test]
    fn inline_code_recognized() {
        let s = state("see `let x = 1`", 0);
        let decs = live_preview(&s);
        assert!(decs.iter().any(|d| matches!(
            &d.kind,
            crate::decoration::DecorationKind::Mark { class, .. } if class == "md-code"
        )));
    }

    #[test]
    fn span_does_not_cross_newline() {
        let s = state("**a\nb**", 0);
        let decs = live_preview(&s);
        // No span — the opening `**` doesn't pair across the \n.
        let marks: Vec<_> = decs
            .iter()
            .filter(|d| matches!(d.kind, crate::decoration::DecorationKind::Mark { .. }))
            .collect();
        assert!(marks.is_empty());
    }

    fn mark_classes(decs: &[DecoratedRange]) -> Vec<&str> {
        decs.iter()
            .filter_map(|d| match &d.kind {
                crate::decoration::DecorationKind::Mark { class, .. } => Some(class.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn bold_italic_triple_recognized() {
        let s = state("***hi***", 0);
        assert!(mark_classes(&live_preview(&s)).contains(&"md-bold-italic"));
    }

    #[test]
    fn strikethrough_recognized() {
        let s = state("~~gone~~", 0);
        assert!(mark_classes(&live_preview(&s)).contains(&"md-strike"));
    }

    #[test]
    fn highlight_recognized() {
        let s = state("==pop==", 0);
        assert!(mark_classes(&live_preview(&s)).contains(&"md-highlight"));
    }

    #[test]
    fn link_recognized() {
        let s = state("[text](https://x)", 0);
        let decs = live_preview(&s);
        assert!(mark_classes(&decs).contains(&"md-link"));
        // Body is just "text" (offsets 1..5).
        assert!(decs.iter().any(|d| d.from == 1 && d.to == 5));
    }

    #[test]
    fn wikilink_recognized() {
        let s = state("[[Page Name]]", 0);
        let decs = live_preview(&s);
        // Class is space-separated `md-wikilink
        // md-wikilink-unresolved` until a vault layer resolves
        // the target, so check the prefix rather than equality.
        assert!(
            mark_classes(&decs)
                .iter()
                .any(|c| c.starts_with("md-wikilink"))
        );
        assert!(decs.iter().any(|d| d.from == 2 && d.to == 11));
    }

    #[test]
    fn wikilink_alias_shows_only_display_text() {
        // `[[structure|Structure]]` — the caret away, only "Structure"
        // is marked; the `[[structure|` prefix and `]]` are replaced.
        let s = state("[[structure|Structure]]", 30);
        let decs = live_preview(&s);
        // The display range is the alias part: byte 12 ("Structure")
        // .. 21. The link mark covers exactly that, not the target.
        let disp_start = "[[structure|".len();
        let disp_end = "[[structure|Structure".len();
        assert!(
            decs.iter().any(|d| d.from == disp_start
                && d.to == disp_end
                && matches!(&d.kind, crate::decoration::DecorationKind::Mark { class, .. }
                    if class.starts_with("md-wikilink"))),
            "only the display text is marked as the link"
        );
        // `[[structure|` (0..12) is replaced (hidden) along with `]]`.
        assert!(
            decs.iter().any(|d| d.from == 0
                && d.to == disp_start
                && matches!(d.kind, crate::decoration::DecorationKind::Replace)),
            "the target|alias prefix is hidden"
        );
    }

    #[test]
    fn footnote_recognized() {
        let s = state("see[^1] here", 0);
        let decs = live_preview(&s);
        assert!(mark_classes(&decs).contains(&"md-footnote"));
    }

    #[test]
    fn tag_recognized() {
        let s = state("a #todo b", 0);
        assert!(mark_classes(&live_preview(&s)).contains(&"md-tag"));
    }

    #[test]
    fn tag_requires_word_boundary() {
        let s = state("foo#bar", 0);
        let decs = live_preview(&s);
        assert!(!mark_classes(&decs).contains(&"md-tag"));
    }

    fn has_line_class(decs: &[DecoratedRange], pos: usize, class: &str) -> bool {
        decs.iter().any(|d| {
            d.from == pos
                && d.to == pos
                && matches!(&d.kind,
                    crate::decoration::DecorationKind::Line { class: c } if c == class)
        })
    }

    #[test]
    fn heading_emits_line_class_and_hides_marker() {
        let s = state("# Title", 100);
        let decs = live_preview(&s);
        assert!(has_line_class(&decs, 0, "md-h1"));
        // Marker `# ` (2 bytes) replaced when caret elsewhere.
        assert!(decs.iter().any(|d| {
            d.from == 0 && d.to == 2 && matches!(d.kind, crate::decoration::DecorationKind::Replace)
        }));
    }

    #[test]
    fn heading_levels_2_through_6() {
        for level in 2..=6 {
            let prefix = "#".repeat(level);
            let s = state(&format!("{prefix} h"), 100);
            let class = format!("md-h{level}");
            assert!(has_line_class(&live_preview(&s), 0, &class));
        }
    }

    #[test]
    fn heading_with_caret_shows_marker() {
        let s = state("# Title", 0);
        let decs = live_preview(&s);
        // Line class still applied …
        assert!(has_line_class(&decs, 0, "md-h1"));
        // … but no Replace on the `# `.
        let replace_on_marker = decs.iter().any(|d| {
            d.from == 0 && d.to == 2 && matches!(d.kind, crate::decoration::DecorationKind::Replace)
        });
        assert!(!replace_on_marker);
    }

    #[test]
    fn blockquote_recognized() {
        let s = state("> quoted", 100);
        let decs = live_preview(&s);
        assert!(has_line_class(&decs, 0, "md-blockquote"));
    }

    #[test]
    fn table_recognized() {
        let s = state("| A | B |\n|---|---|\n| 1 | 2 |", 100);
        let decs = live_preview(&s);
        let widget = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-table"))
        });
        assert!(widget);
    }

    #[test]
    fn table_with_caret_inside_keeps_source_visible() {
        // Caret at byte 5 ("| A | B" inside header) — table
        // recognized but no Replace, source stays editable.
        let s = state("| A | B |\n|---|---|\n| 1 | 2 |", 5);
        let decs = live_preview(&s);
        let has_replace = decs
            .iter()
            .any(|d| matches!(d.kind, crate::decoration::DecorationKind::Replace));
        assert!(!has_replace);
    }

    #[test]
    fn table_requires_separator_row() {
        // No separator → not a table.
        let s = state("| A | B |\n| 1 | 2 |", 100);
        let decs = live_preview(&s);
        let widget = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-table"))
        });
        assert!(!widget);
    }

    #[test]
    fn inline_footnote_recognized() {
        // Caret away from the span: source replaced + marker
        // widget shown.
        let s = state("see ^[a side note] here", 0);
        let decs = live_preview(&s);
        let has_marker = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-inline-footnote-marker"))
        });
        assert!(has_marker);
    }

    #[test]
    fn inline_footnote_source_visible_when_caret_on() {
        // Caret inside the body: Mark, no Replace.
        let s = state("see ^[a side note] here", 8);
        let decs = live_preview(&s);
        assert!(mark_classes(&decs).contains(&"md-inline-footnote"));
        let has_replace = decs.iter().any(|d| {
            d.from == 4
                && d.to == 18
                && matches!(d.kind, crate::decoration::DecorationKind::Replace)
        });
        assert!(!has_replace);
    }

    #[test]
    fn embed_page_renders_card() {
        let s = state("![[OtherPage]]", 100);
        let decs = live_preview(&s);
        let has_card = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-embed-page"))
        });
        assert!(has_card);
    }

    #[test]
    fn embed_section_with_intra_doc_resolves() {
        // `![[#Section]]` looks up the heading in the current
        // doc.
        let src = "Before\n\n## Section\nbody line\nmore body\n\n## Next\n\n![[#Section]]";
        let s = state(src, 100);
        let decs = live_preview(&s);
        let card_html = decs
            .iter()
            .find_map(|d| match &d.kind {
                crate::decoration::DecorationKind::Widget { html }
                    if html.contains("md-embed-section") =>
                {
                    Some(html.clone())
                }
                _ => None,
            })
            .expect("section card");
        assert!(card_html.contains("body line"));
        assert!(!card_html.contains("md-embed-placeholder"));
    }

    #[test]
    fn embed_section_unresolved_shows_placeholder() {
        // Cross-doc section reference — no multi-file lookup
        // yet, so renders the placeholder.
        let s = state("![[OtherPage#Section]]", 100);
        let decs = live_preview(&s);
        let card_html = decs
            .iter()
            .find_map(|d| match &d.kind {
                crate::decoration::DecorationKind::Widget { html }
                    if html.contains("md-embed-section") =>
                {
                    Some(html.clone())
                }
                _ => None,
            })
            .expect("section card");
        assert!(card_html.contains("md-embed-placeholder"));
    }

    #[test]
    fn embed_block_via_short_id_resolves_intra_doc() {
        let src = "Paragraph body ^anchor-here\n\n![[#^anchor-here]]";
        let s = state(src, 100);
        let decs = live_preview(&s);
        let card_html = decs
            .iter()
            .find_map(|d| match &d.kind {
                crate::decoration::DecorationKind::Widget { html }
                    if html.contains("md-embed-block") =>
                {
                    Some(html.clone())
                }
                _ => None,
            })
            .expect("block card");
        assert!(card_html.contains("Paragraph body"));
    }

    #[test]
    fn block_id_property_line_replaced() {
        // A line that's just `id:: <uuid>` is hidden via a
        // Replace covering the whole line.
        let uuid = "5f9c1234-abcd-4ef0-8123-fedcba012345";
        let src = format!("paragraph content\nid:: {uuid}\nnext line");
        let s = state(&src, 0);
        let decs = live_preview(&s);
        let id_line_start = src.find("id::").unwrap();
        let id_line_end = src[id_line_start..]
            .find('\n')
            .map_or(src.len(), |n| id_line_start + n + 1);
        let has_replace = decs.iter().any(|d| {
            d.from == id_line_start
                && d.to == id_line_end
                && matches!(d.kind, crate::decoration::DecorationKind::Replace)
        });
        assert!(has_replace);
    }

    #[test]
    fn block_ref_rendered_as_chip_widget() {
        let uuid = "5f9c1234-abcd-4ef0-8123-fedcba012345";
        let src = format!("see (({uuid})) for details");
        let s = state(&src, 0);
        let decs = live_preview(&s);
        let has_chip = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-block-ref-chip"))
        });
        assert!(has_chip);
    }

    #[test]
    fn block_embed_rendered_as_card() {
        let uuid = "5f9c1234-abcd-4ef0-8123-fedcba012345";
        let src = format!("{{{{embed (({uuid}))}}}}\n");
        let s = state(&src, 0);
        let decs = live_preview(&s);
        let has_card = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-block-embed-card"))
        });
        assert!(has_card);
    }

    /// Stub vault for cross-doc resolution tests.
    #[allow(clippy::struct_field_names)]
    #[derive(Default)]
    struct FakeVault {
        block_hits: std::collections::HashMap<String, super::VaultBlockHit>,
        page_hits: std::collections::HashMap<String, super::VaultPageHit>,
        section_hits: std::collections::HashMap<(String, String), String>,
        scripture_hits: std::collections::HashMap<String, super::VaultScriptureHit>,
    }
    impl super::VaultLookup for FakeVault {
        fn lookup_block(&self, u: &str) -> Option<super::VaultBlockHit> {
            self.block_hits.get(u).cloned()
        }
        fn lookup_page(&self, n: &str) -> Option<super::VaultPageHit> {
            self.page_hits.get(n).cloned()
        }
        fn lookup_section(&self, p: &str, h: &str) -> Option<String> {
            self.section_hits.get(&(p.into(), h.into())).cloned()
        }
        fn lookup_block_short(&self, _p: &str, _id: &str) -> Option<String> {
            None
        }
        fn lookup_scripture(&self, t: &str) -> Option<super::VaultScriptureHit> {
            self.scripture_hits.get(t).cloned()
        }
    }

    #[test]
    fn block_ref_resolves_across_pages_via_vault() {
        let uuid = "11111111-1111-4111-8111-111111111111";
        let s = state(&format!("see (({uuid})) for context"), 0);
        let mut block_hits = std::collections::HashMap::new();
        block_hits.insert(
            uuid.to_string(),
            super::VaultBlockHit {
                page: "OtherPage".into(),
                preview: "Target block content".into(),
            },
        );
        let vault = FakeVault {
            block_hits,
            page_hits: Default::default(),
            section_hits: Default::default(),
            ..Default::default()
        };
        let decs = super::live_preview_with(&s, Some(&vault));
        let chip_html = decs
            .iter()
            .find_map(|d| match &d.kind {
                crate::decoration::DecorationKind::Widget { html }
                    if html.contains("md-block-ref-chip") =>
                {
                    Some(html.clone())
                }
                _ => None,
            })
            .expect("chip widget");
        assert!(chip_html.contains("Target block content"));
        assert!(chip_html.contains("OtherPage"));
        assert!(!chip_html.contains("md-block-ref-unresolved"));
    }

    fn scripture_vault(target: &str, text: Option<&str>) -> FakeVault {
        let mut scripture_hits = std::collections::HashMap::new();
        scripture_hits.insert(
            target.to_string(),
            super::VaultScriptureHit {
                display: "John 3:16".into(),
                osis: "John.3.16".into(),
                text: text.map(str::to_string),
                translation: "WEB".into(),
            },
        );
        FakeVault {
            scripture_hits,
            ..Default::default()
        }
    }

    #[test]
    fn inline_scripture_link_renders_chip() {
        let s = state("see [[John 3:16]] here", 0);
        let vault = scripture_vault("John 3:16", Some("For God so loved the world…"));
        let decs = super::live_preview_with(&s, Some(&vault));
        let chip = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Mark { class, .. }
                if class == "md-wikilink md-scripture-chip")
        });
        assert!(chip, "decs = {decs:?}");
    }

    #[test]
    fn standalone_scripture_link_renders_verse_card() {
        // Caret far from the line so the widget fires.
        let s = state("intro\n\n[[John 3:16]]\n\ntail", 0);
        let vault = scripture_vault("John 3:16", Some("For God so loved the world…"));
        let decs = super::live_preview_with(&s, Some(&vault));
        let card = decs
            .iter()
            .find_map(|d| match &d.kind {
                crate::decoration::DecorationKind::Widget { html }
                    if html.contains("md-scripture-card") =>
                {
                    Some(html.clone())
                }
                _ => None,
            })
            .expect("verse card widget");
        assert!(card.contains("For God so loved the world…"));
        assert!(card.contains("scripture-open:John 3:16"));
        assert!(card.contains("WEB"));
    }

    #[test]
    fn wikilink_resolved_class_when_vault_finds_page() {
        let s = state("see [[OtherPage]]", 0);
        let mut page_hits = std::collections::HashMap::new();
        page_hits.insert(
            "OtherPage".into(),
            super::VaultPageHit {
                preview: "Body".into(),
            },
        );
        let vault = FakeVault {
            block_hits: Default::default(),
            page_hits,
            section_hits: Default::default(),
            ..Default::default()
        };
        let decs = super::live_preview_with(&s, Some(&vault));
        // The wikilink's mark class should NOT carry the
        // unresolved suffix when the vault confirms existence.
        let has_resolved = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Mark { class, .. }
                if class == "md-wikilink")
        });
        assert!(has_resolved, "decs = {decs:?}");
    }

    #[test]
    fn cross_page_section_embed_resolves_via_vault() {
        // Caret well past the embed so the widget actually
        // fires (caret on the span keeps the source visible).
        let s = state("![[Project README#Goals]]\n\nbody", 50);
        let mut section_hits = std::collections::HashMap::new();
        section_hits.insert(
            ("Project README".into(), "Goals".into()),
            "Make notes good.\nShip quickly.".into(),
        );
        let vault = FakeVault {
            block_hits: Default::default(),
            page_hits: Default::default(),
            section_hits,
            ..Default::default()
        };
        let decs = super::live_preview_with(&s, Some(&vault));
        let card = decs
            .iter()
            .find_map(|d| match &d.kind {
                crate::decoration::DecorationKind::Widget { html }
                    if html.contains("md-embed-section") =>
                {
                    Some(html.clone())
                }
                _ => None,
            })
            .expect("section card");
        assert!(card.contains("Make notes good."));
        assert!(!card.contains("md-embed-placeholder"));
    }

    #[test]
    fn block_ref_resolves_when_target_block_has_id_above() {
        // When the doc contains a block with an `id::` line,
        // the `((uuid))` chip should render the target's
        // first-line content (not "unresolved").
        let uuid = "5f9c1234-abcd-4ef0-8123-fedcba012345";
        let src =
            format!("First block content here\nid:: {uuid}\n\nA later paragraph with (({uuid})).");
        let s = state(&src, 0);
        let decs = live_preview(&s);
        let chip_html = decs
            .iter()
            .find_map(|d| match &d.kind {
                crate::decoration::DecorationKind::Widget { html }
                    if html.contains("md-block-ref-chip") =>
                {
                    Some(html.clone())
                }
                _ => None,
            })
            .expect("chip widget");
        assert!(
            chip_html.contains("First block content here"),
            "expected chip to preview target, got: {chip_html}"
        );
        assert!(!chip_html.contains("md-block-ref-unresolved"));
    }

    #[test]
    fn block_id_recognized_at_eol_only() {
        // `^id` at end of line is a block ref.
        let s = state("paragraph ^block-1\nnext line", 0);
        let decs = live_preview(&s);
        assert!(mark_classes(&decs).contains(&"md-block-id"));
        // Mid-line `^` shouldn't trigger.
        let s = state("x^y not a block id", 0);
        let decs = live_preview(&s);
        assert!(!mark_classes(&decs).contains(&"md-block-id"));
    }

    #[test]
    fn autolink_recognized() {
        let s = state("see <https://anthropic.com> for more", 0);
        let decs = live_preview(&s);
        assert!(mark_classes(&decs).contains(&"md-autolink"));
    }

    #[test]
    fn autolink_email_recognized() {
        let s = state("mail <a@b.co>", 0);
        let decs = live_preview(&s);
        assert!(mark_classes(&decs).contains(&"md-autolink"));
    }

    #[test]
    fn setext_h1_recognized() {
        let s = state("Big Title\n=========\nbody", 0);
        let decs = live_preview(&s);
        let has_h1 = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Line { class }
                if class == "md-h1")
        });
        assert!(has_h1);
    }

    #[test]
    fn setext_h2_recognized() {
        let s = state("Subtitle\n--------\nbody", 0);
        let decs = live_preview(&s);
        let has_h2 = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Line { class }
                if class == "md-h2")
        });
        assert!(has_h2);
    }

    #[test]
    fn custom_task_status_recognized() {
        // `- [/]` in-progress, `- [>]` forwarded — still parses
        // as a task line, just with a non-canonical status char.
        let s = state("- [/] working on it\n- [>] later", 0);
        let decs = live_preview(&s);
        let has_task_line = decs
            .iter()
            .filter(|d| {
                matches!(&d.kind,
            crate::decoration::DecorationKind::Line { class }
                if class == "md-task")
            })
            .count();
        assert!(has_task_line >= 2);
    }

    #[test]
    fn frontmatter_parsed() {
        let src =
            "---\ntitle: Hello\ntags: [a, b]\npublished: true\naliases:\n  - x\n  - y\n---\n# body";
        let fm = super::parse_frontmatter(src).expect("fm found");
        assert_eq!(fm.props.len(), 4);
        assert_eq!(fm.props[0].key, "title");
        assert!(matches!(&fm.props[0].value, super::PropValue::Text(s) if s == "Hello"));
        assert!(
            matches!(&fm.props[1].value, super::PropValue::List(v) if v == &vec!["a".to_string(), "b".to_string()])
        );
        assert!(matches!(&fm.props[2].value, super::PropValue::Bool(true)));
        assert!(matches!(&fm.props[3].value, super::PropValue::List(v) if v.len() == 2));
    }

    #[test]
    fn frontmatter_property_ranges_are_atomic() {
        let src = "---\ntitle: x\ntags:\n  - a\n  - b\nactive: true\n---\n";
        let fm = super::parse_frontmatter(src).unwrap();
        // `title` should span only its one line.
        let title = &fm.props[0];
        assert_eq!(&src[title.range.clone()], "title: x\n");
        // `tags` should span the key line + both list items.
        let tags = &fm.props[1];
        assert_eq!(&src[tags.range.clone()], "tags:\n  - a\n  - b\n");
        // `active` is a scalar bool.
        let active = &fm.props[2];
        assert_eq!(&src[active.range.clone()], "active: true\n");
        assert!(matches!(active.value, super::PropValue::Bool(true)));
    }

    #[test]
    fn serialize_property_round_trips() {
        let s = super::serialize_property(
            "tags",
            &super::PropValue::List(vec!["a".into(), "b: c".into()]),
        );
        // The second item must be quoted because it contains a
        // colon; otherwise the parser would split it as a map.
        assert!(s.contains("\"b: c\""));
        assert!(s.starts_with("tags:\n"));
    }

    #[test]
    fn multiline_scalar_round_trips() {
        // `|` block scalars carry newlines verbatim.
        let src = "---\ndescription: |\n  first line\n  second line\n  third\nactive: true\n---\n";
        let fm = super::parse_frontmatter(src).unwrap();
        let desc = &fm.props[0];
        assert_eq!(desc.key, "description");
        if let super::PropValue::Text(t) = &desc.value {
            assert_eq!(t, "first line\nsecond line\nthird");
        } else {
            panic!("expected multiline text, got {:?}", desc.value);
        }
        // Serialize back out — must produce a `|` block, not a
        // collapsed single-line.
        let s = super::serialize_property("description", &desc.value);
        assert!(s.starts_with("description: |\n"));
        assert!(s.contains("  first line\n"));
        // Range covers the block + the closing indent line.
        assert_eq!(
            &src[desc.range.clone()],
            "description: |\n  first line\n  second line\n  third\n"
        );
    }

    #[test]
    fn frontmatter_only_at_doc_start() {
        // `---` mid-doc is a horizontal rule, not frontmatter.
        let src = "# heading\n\n---\nfoo: bar\n---\n";
        assert!(super::parse_frontmatter(src).is_none());
    }

    #[test]
    fn frontmatter_emits_widget_when_caret_away() {
        let s = state("---\ntitle: x\n---\n# body", 20);
        let decs = live_preview(&s);
        let has_widget = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-properties"))
        });
        assert!(has_widget);
    }

    #[test]
    fn kf_fence_engraves_on_all_targets() {
        // The exact snippet the keyflow guide's chords chapter ships.
        // editor-keyflow wraps engraver's CPU-only svg tier, so this
        // path runs on wasm32 too (the old native-only gate is gone).
        let s = state("```kf\nCmaj7 | F#m7b5 | Bbmaj9 | G7b9\n```\n\ntail", 44);
        let decs = live_preview(&s);
        let widget = decs.iter().find_map(|d| match &d.kind {
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-keyflow-widget") =>
            {
                Some(html.clone())
            }
            _ => None,
        });
        let html = widget.expect("kf fence should engrave a chart widget");
        assert!(
            html.contains("<svg"),
            "widget should embed the engraved SVG"
        );
        // Rendered-only default: the source ships hidden behind the
        // `</>` toggle (CSS hides .md-keyflow-source until
        // md-keyflow-show-source is on the widget).
        assert!(
            html.contains("md-keyflow-toggle"),
            "widget should carry the source toggle button"
        );
        assert!(
            html.contains("md-keyflow-source"),
            "source column still ships in the widget for the toggle"
        );
        assert!(
            !html.contains("md-keyflow-show-source"),
            "```kf defaults to the engraved chart only"
        );
        // Every fence line sheds the grey code-block frame (bare) so the
        // chart renders full width, not boxed like code.
        assert!(has_line_class(&decs, 0, "md-keyflow-bare"), "opener bare");
        let close_at = "```kf\nCmaj7 | F#m7b5 | Bbmaj9 | G7b9\n".len();
        assert!(
            has_line_class(&decs, close_at, "md-keyflow-bare"),
            "closer bare"
        );
    }

    #[test]
    fn kf_dash_fence_is_highlighted_source_only() {
        // ```kf- — highlighted source, NO chart, always shown.
        let s = state("```kf-\nCmaj7 | Dm7\n```\n\ntail", 40);
        let decs = live_preview(&s);
        let widget = decs.iter().find_map(|d| match &d.kind {
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-keyflow-widget") =>
            {
                Some(html.clone())
            }
            _ => None,
        });
        let html = widget.expect("kf- should widgetize a source block");
        assert!(
            html.contains("md-keyflow-source-only"),
            "kf- is source-only"
        );
        assert!(
            html.contains("class=\"kf-root\""),
            "kf- source is keyflow-highlighted"
        );
        assert!(!html.contains("<svg"), "kf- has NO chart");
        assert!(
            !html.contains("md-keyflow-toggle"),
            "kf- has no source toggle"
        );
        // Header with the tag + copy button.
        assert!(html.contains("md-keyflow-header"), "kf- carries a header");
        assert!(
            html.contains("md-code-copy"),
            "kf- header has a copy button"
        );
        // Sheds the code frame like the other keyflow fences.
        assert!(has_line_class(&decs, 0, "md-keyflow-bare"), "kf- is bare");
    }

    #[test]
    fn kf_plus_fence_shows_source_and_chart() {
        // ```kf+ — the author opts into source + chart together; the
        // widget ships with the show-source class already on.
        let s = state("```kf+\nCmaj7 | F#m7b5\n```\n\ntail", 30);
        let decs = live_preview(&s);
        let widget = decs.iter().find_map(|d| match &d.kind {
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-keyflow-widget") =>
            {
                Some(html.clone())
            }
            _ => None,
        });
        let html = widget.expect("kf+ fence should engrave a chart widget");
        assert!(
            html.contains("md-keyflow-show-source"),
            "kf+ starts with source visible"
        );
        assert!(html.contains("<svg"), "kf+ still embeds the engraved SVG");
        // The source block is keyflow-highlighted (not plain text) and
        // wrapped for the stacked layout — never the old flex split.
        assert!(
            html.contains("class=\"kf-root\""),
            "source is kf-highlighted"
        );
        assert!(html.contains("md-keyflow-source"), "source block present");
        assert!(!html.contains("md-keyflow-split"), "no side-by-side split");
        // Source comes BEFORE the rendered chart in the DOM (stacked).
        let src_at = html.find("md-keyflow-source").unwrap();
        let render_at = html.find("md-keyflow-render").unwrap();
        assert!(src_at < render_at, "source stacks above the chart");
    }

    #[test]
    fn kbd_literal_renders_key_caps() {
        // Caret away: the `kbd:` code span becomes a key-caps widget.
        let s = state("press `kbd:<C-S-space>` now", 0);
        let decs = live_preview(&s);
        let widget = decs.iter().find_map(|d| match &d.kind {
            crate::decoration::DecorationKind::Widget { html } if html.contains("md-kbd") => {
                Some(html.clone())
            }
            _ => None,
        });
        let html = widget.expect("kbd widget emitted");
        for cap in ["Ctrl", "Shift", "Space"] {
            assert!(html.contains(cap), "missing cap {cap} in {html}");
        }
        // Sequences render a "then" separator.
        let s = state("do `kbd:g g` twice", 0);
        let decs = live_preview(&s);
        assert!(decs.iter().any(|d| matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-kbd-then"))));
    }

    #[test]
    fn kbd_caret_inside_shows_source() {
        // Caret inside the span: raw source stays editable (plain
        // inline-code styling, no widget).
        let src = "press `kbd:<C-s>` now";
        let caret = src.find("C-s").unwrap();
        let s = state(src, caret);
        let decs = live_preview(&s);
        assert!(!decs.iter().any(|d| matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-kbd"))));
    }

    #[test]
    fn kbd_action_ref_resolves_through_lookup() {
        struct FakeKbd;
        impl KbdLookup for FakeKbd {
            fn keys_for_action(&self, action: &str) -> Option<String> {
                (action == "40044").then(|| "<space>".to_string())
            }
        }
        let s = state("press `kbd:@40044` to play, `kbd:@99999` is unbound", 0);
        let decs = live_preview_with_lookups(&s, None, Some(&FakeKbd));
        let widgets: Vec<String> = decs
            .iter()
            .filter_map(|d| match &d.kind {
                crate::decoration::DecorationKind::Widget { html } if html.contains("md-kbd") => {
                    Some(html.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(widgets.len(), 2, "both refs should render widgets");
        assert!(
            widgets.iter().any(|h| h.contains("Space")),
            "resolved ref shows keys"
        );
        assert!(
            widgets
                .iter()
                .any(|h| h.contains("md-kbd-unbound") && h.contains("@99999")),
            "unresolved ref renders the unbound cap"
        );
    }

    #[test]
    fn inline_math_recognized() {
        // Caret away: source replaced + math widget emitted.
        let s = state("Cost is $x^2$ today", 0);
        let decs = live_preview(&s);
        let has_widget = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-math-widget"))
        });
        assert!(has_widget);
    }

    #[test]
    fn block_math_recognized() {
        // `mc^2` would fail to compile in Typst (`mc` reads as
        // an unknown identifier); use `m c^2` so the smoke test
        // exercises a real render.
        let s = state("Before\n$$E = m c^2$$\nAfter", 0);
        let decs = live_preview(&s);
        let has_widget = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-math-widget-block"))
        });
        assert!(has_widget);
    }

    #[test]
    fn math_with_caret_inside_shows_source() {
        // Caret inside the body: no Replace, source visible
        // as `md-math-inline` mark so the user can edit.
        let s = state("Cost $x^2$ here", 7);
        let decs = live_preview(&s);
        let has_replace = decs.iter().any(|d| {
            d.from == 5
                && d.to == 10
                && matches!(d.kind, crate::decoration::DecorationKind::Replace)
        });
        assert!(!has_replace);
    }

    #[test]
    fn mermaid_fence_recognized() {
        // Caret past the closing fence so cursor_touches is
        // false and the widget actually fires.
        let src = "```mermaid\nflowchart TD\n  A --> B\n```\nx";
        let s = state(src, src.len() - 1);
        let decs = live_preview(&s);
        let has_widget = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-mermaid-widget"))
        });
        assert!(has_widget);
    }

    #[test]
    fn typst_fence_recognized() {
        // Caret past the closing fence so cursor_touches is
        // false and the widget actually fires.
        let src = "```typst\n= Section\n```\nx";
        let s = state(src, src.len() - 1);
        let decs = live_preview(&s);
        let has_widget = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-typst-widget"))
        });
        assert!(has_widget);
    }

    #[test]
    fn comment_recognized() {
        // Caret away from the `%%…%%` span: whole range hidden.
        let s = state("a %% hidden %% b", 0);
        let decs = live_preview(&s);
        let has_replace = decs.iter().any(|d| {
            d.from == 2
                && d.to == 14
                && matches!(d.kind, crate::decoration::DecorationKind::Replace)
        });
        assert!(has_replace);
    }

    #[test]
    fn comment_revealed_when_caret_inside() {
        // Caret inside the comment: body styled as `md-comment`.
        let s = state("a %% hidden %% b", 6);
        let decs = live_preview(&s);
        let has_mark = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Mark { class, .. } if class == "md-comment")
        });
        assert!(has_mark);
    }

    #[test]
    fn image_embed_recognized() {
        let s = state("![[pic.png]]", 100);
        let decs = live_preview(&s);
        let has_widget = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-embed-image"))
        });
        assert!(has_widget);
    }

    #[test]
    fn image_embed_with_size_opts() {
        let s = state("![[pic.png|320x200]]", 100);
        let decs = live_preview(&s);
        let widget = decs.iter().find_map(|d| match &d.kind {
            crate::decoration::DecorationKind::Widget { html } => Some(html),
            _ => None,
        });
        let html = widget.expect("widget");
        assert!(html.contains("width:320px"));
        assert!(html.contains("height:200px"));
    }

    #[test]
    fn video_embed_recognized() {
        let s = state("![[clip.mp4]]", 100);
        let decs = live_preview(&s);
        let has_video = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-embed-video"))
        });
        assert!(has_video);
    }

    #[test]
    fn unknown_extension_falls_back_to_wikilink() {
        // .md isn't a media kind — should NOT emit an embed widget.
        let s = state("![[other.md]]", 100);
        let decs = live_preview(&s);
        let has_widget = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html }
                if html.starts_with("<img") || html.starts_with("<video"))
        });
        assert!(!has_widget);
    }

    #[test]
    fn callout_note_emits_md_callout_class() {
        let s = state("> [!note] Title", 100);
        let decs = live_preview(&s);
        let has_callout = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Line { class }
                if class.contains("md-callout-note"))
        });
        assert!(has_callout);
    }

    #[test]
    fn nested_callout_emits_depth_class() {
        let src = "> [!note] outer\n> > [!warning] inner\n> > inner body\n";
        let s = state(src, 100);
        let decs = live_preview(&s);
        // Inner header line gets both `md-callout-warning` and
        // a depth-2 class.
        let inner_line_from = src.find("> > [!warning]").unwrap();
        let has_warning = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Line { class }
                if class.contains("md-callout-warning"))
                && d.from == inner_line_from
        });
        let has_depth = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Line { class }
                if class == "md-callout-nested-2")
                && d.from == inner_line_from
        });
        assert!(has_warning, "expected inner line to be warning-classed");
        assert!(has_depth, "expected inner line to carry depth-2 class");
    }

    #[test]
    fn nested_callout_body_inherits_inner_kind() {
        let src = "> [!note] outer\n> > [!warning] inner header\n> > body\n";
        let s = state(src, 100);
        let decs = live_preview(&s);
        let body_from = src.find("> > body").unwrap();
        let body_is_warning = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Line { class }
                if class.contains("md-callout-warning"))
                && d.from == body_from
        });
        assert!(body_is_warning);
    }

    #[test]
    fn dedent_closes_inner_callout() {
        // After `> > [!warning] inner`, a `> ` line at depth 1
        // should fall back to the OUTER callout kind, not the
        // inner one.
        let src = "> [!note] outer\n> > [!warning] inner\n> back to outer\n";
        let s = state(src, 100);
        let decs = live_preview(&s);
        let back_from = src.find("> back to outer").unwrap();
        let back_is_note = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Line { class }
                if class.contains("md-callout-note"))
                && d.from == back_from
        });
        assert!(back_is_note);
    }

    #[test]
    fn callout_warning_alias_resolves() {
        let s = state("> [!caution] Hey", 100);
        let decs = live_preview(&s);
        let has_warning = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Line { class }
                if class.contains("md-callout-warning"))
        });
        assert!(has_warning);
    }

    #[test]
    fn callout_body_lines_inherit_kind() {
        let s = state("> [!note] T\n> body line", 100);
        let decs = live_preview(&s);
        // Both lines should have a `md-callout-note` class.
        let count = decs
            .iter()
            .filter(|d| {
                matches!(&d.kind,
            crate::decoration::DecorationKind::Line { class }
                if class.contains("md-callout-note"))
            })
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn non_blockquote_line_closes_callout() {
        let s = state("> [!note] T\n> body\nafter", 100);
        let decs = live_preview(&s);
        // Line at pos 21 ("after") should NOT have callout class.
        let after_class = decs.iter().find_map(|d| match &d.kind {
            crate::decoration::DecorationKind::Line { class } if d.from == 21 => Some(class),
            _ => None,
        });
        // Either no Line at "after" (it's a plain line) or one
        // without the callout class.
        assert!(after_class.is_none_or(|c| !c.contains("md-callout")));
    }

    #[test]
    fn hr_recognized() {
        let s = state("---", 100);
        let decs = live_preview(&s);
        assert!(has_line_class(&decs, 0, "md-hr"));
    }

    #[test]
    fn hr_active_class_when_caret_on_line() {
        let s = state("---", 1);
        let decs = live_preview(&s);
        assert!(has_line_class(&decs, 0, "md-hr-active"));
        // And the `---` source isn't replaced — user can edit.
        let has_replace = decs.iter().any(|d| {
            d.from == 0 && d.to == 3 && matches!(d.kind, crate::decoration::DecorationKind::Replace)
        });
        assert!(!has_replace);
    }

    #[test]
    fn list_bullet_recognized() {
        let s = state("- item", 100);
        let decs = live_preview(&s);
        assert!(has_line_class(&decs, 0, "md-list-item"));
    }

    #[test]
    fn list_ordered_recognized() {
        let s = state("1. first", 100);
        let decs = live_preview(&s);
        assert!(has_line_class(&decs, 0, "md-list-item"));
    }

    #[test]
    fn list_with_caret_on_line_keeps_source_visible() {
        // Caret on the bullet line — no Replace, no widget; the
        // `- ` source stays editable. Same pattern as headings.
        let s = state("- item", 3);
        let decs = live_preview(&s);
        let has_replace = decs.iter().any(|d| {
            d.from == 0 && d.to == 2 && matches!(d.kind, crate::decoration::DecorationKind::Replace)
        });
        assert!(
            !has_replace,
            "marker source must stay visible while caret is on the line"
        );
        let has_widget = decs
            .iter()
            .any(|d| matches!(&d.kind, crate::decoration::DecorationKind::Widget { .. }));
        assert!(!has_widget, "no bullet widget while caret is on the line");
    }

    #[test]
    fn ordered_list_with_caret_on_line_keeps_source_visible() {
        let s = state("1. foo", 3);
        let decs = live_preview(&s);
        let has_replace = decs.iter().any(|d| {
            d.from == 0 && d.to == 3 && matches!(d.kind, crate::decoration::DecorationKind::Replace)
        });
        assert!(!has_replace);
    }

    #[test]
    fn task_with_caret_on_line_keeps_source_visible() {
        // Caret on the line: source bytes stay editable (no
        // Replace AND no widget — both at once would overlap).
        let s = state("- [ ] todo", 4);
        let decs = live_preview(&s);
        let has_replace = decs
            .iter()
            .any(|d| matches!(d.kind, crate::decoration::DecorationKind::Replace));
        assert!(!has_replace);
        let has_widget = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Widget { html } if html.contains("md-task-checkbox"))
        });
        assert!(!has_widget);
    }

    #[test]
    fn task_unchecked_recognized() {
        let s = state("- [ ] todo", 100);
        let decs = live_preview(&s);
        assert!(has_line_class(&decs, 0, "md-task"));
        // Widget emitted for the checkbox.
        let widget = decs.iter().any(|d| {
            matches!(&d.kind, crate::decoration::DecorationKind::Widget { html }
                if html.contains("md-task-checkbox") && !html.contains("checked"))
        });
        assert!(widget);
    }

    #[test]
    fn task_checked_recognized() {
        let s = state("- [x] done", 100);
        let decs = live_preview(&s);
        let widget = decs.iter().any(|d| {
            matches!(&d.kind, crate::decoration::DecorationKind::Widget { html }
                if html.contains("checked"))
        });
        assert!(widget);
    }

    #[test]
    fn code_fence_with_lang_emits_syntax_tokens() {
        let s = state("```rust\nfn main() {}\n```", 999);
        let decs = live_preview(&s);
        let has_token = decs.iter().any(|d| {
            matches!(&d.kind,
            crate::decoration::DecorationKind::Mark { class, .. }
                if class.starts_with("md-tok-"))
        });
        assert!(has_token, "expected at least one md-tok-* mark");
    }

    #[test]
    fn code_fence_spans_multiple_lines() {
        let s = state("```rust\nfn main() {}\n```", 999);
        let decs = live_preview(&s);
        // Every line gets md-code-block.
        assert!(has_line_class(&decs, 0, "md-code-block")); // open
        assert!(has_line_class(&decs, 8, "md-code-block")); // body
        // Closing fence at byte 21 (`...{}\n` ends at 20).
        let close_line_start = "```rust\nfn main() {}\n".len();
        assert!(has_line_class(&decs, close_line_start, "md-code-block"));
    }

    #[test]
    fn inline_inside_fence_is_skipped() {
        let s = state("```\n**bold**\n```", 999);
        let decs = live_preview(&s);
        // No bold mark should exist for the `**bold**` inside fence.
        let has_bold = decs.iter().any(|d| {
            matches!(&d.kind, crate::decoration::DecorationKind::Mark { class, .. }
                if class == "md-bold")
        });
        assert!(!has_bold);
    }

    #[test]
    fn tag_at_line_start_when_no_heading() {
        let s = state("#foo bar", 100);
        assert!(mark_classes(&live_preview(&s)).contains(&"md-tag"));
    }

    #[test]
    fn tag_has_no_hidden_markers() {
        let s = state("#todo", 100);
        let decs = live_preview(&s);
        let has_replace = decs
            .iter()
            .any(|d| matches!(d.kind, crate::decoration::DecorationKind::Replace));
        assert!(!has_replace, "tag should have no markers to hide");
    }
}
