//! Keyflow language support for the Editor.
//!
//! Two pure functions bridge keyflow's text tooling into the editor's
//! decoration model so a `.kf` document edited in the [`editor`] view gets the
//! same syntax colors and live-diagnostic squiggles the standalone keyflow
//! editor shows:
//!
//! - [`keyflow_decorations`] is a [`DecorationSource`]-shaped
//!   `fn(&EditorState) -> Vec<DecoratedRange>`. It runs keyflow's line
//!   highlighter over every line and `ide::analyze` over the whole document,
//!   emitting `Mark` decorations carrying the keyflow CSS class for each token
//!   and a wavy-underline class for each diagnostic.
//! - [`highlight_css`] returns the stylesheet that colors those classes — the
//!   keyflow theme's `.kf-*` rules plus the `.kf-diag-*` squiggle rules.
//!
//! No Dioxus dependency: callers wire the function into `<Editor decorations>`
//! and inject the CSS however they mount styles.
//!
//! [`DecorationSource`]: https://docs.rs/editor-view (editor_view::DecorationSource)

use std::sync::atomic::{AtomicBool, Ordering};

use editor_state::{DecoratedRange, Decoration, EditorState, HoverTooltip};
use keyflow_text::highlighting::{Highlighter, Renderer, Theme};
use keyflow_text::ide::{self, Severity};

/// Re-export so callers can pick a palette for [`highlight_css`] without taking
/// a direct `keyflow-text` dependency:
/// `editor_keyflow_lang::HighlightTheme::default_dark()`.
pub use keyflow_text::highlighting::Theme as HighlightTheme;

/// Build the keyflow decoration set for the current document.
///
/// Wrap with `editor_view::DecorationSource::ptr(keyflow_decorations)` and
/// pass it straight into the `<Editor>` `decorations` prop.
///
/// Highlight spans come from [`Highlighter::highlight_line`], which works per
/// line with line-relative byte offsets; we add each line's start offset to
/// get document-absolute ranges. Diagnostics come from [`ide::analyze`], whose
/// ranges are already absolute. The returned vec is sorted by start offset.
#[must_use]
pub fn keyflow_decorations(state: &EditorState) -> Vec<DecoratedRange> {
    let text = state.doc.to_string();
    let len = text.len();

    // The whole output is a pure function of (text, phase, overlay toggle):
    // syntax highlighting depends on the text, and the analysis-derived
    // diagnostics + overlays depend on the text and the overlay flag. So a
    // single-entry memo keyed on those three turns the common "nothing
    // changed but the view re-rendered" pass — every caret move, every
    // selection update — into a clone instead of a fresh whole-document
    // `parse_chart`. Editing busts the key (text changed); the debounced
    // `Full` pass recomputes once, then caret motion rides the cache.
    let full = matches!(editor_state::deco_phase(), editor_state::DecoPhase::Full);
    let overlays = overlays_enabled();
    if let Some(cached) = DECO_MEMO.with_borrow(|m| {
        m.as_ref()
            .filter(|c| c.full == full && c.overlays == overlays && c.text == text)
            .map(|c| c.out.clone())
    }) {
        return cached;
    }

    let mut out: Vec<DecoratedRange> = Vec::new();

    // Syntax highlighting — one pass per line, offsets shifted to absolute.
    // `split_inclusive` keeps the trailing '\n' so `line_start` advances by the
    // full line length; we strip it (and a stray '\r') before highlighting.
    let mut line_start = 0usize;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        for span in Highlighter::highlight_line(content) {
            let start = line_start + span.span.start;
            let end = line_start + span.span.end();
            if start < end && end <= len {
                out.push(Decoration::mark(start..end, span.kind.css_class()));
            }
        }
        line_start += line.len();
    }

    // Whole-document analysis (diagnostic squiggles + resolved-chord /
    // section overlays) is the expensive part of this pass. We still skip
    // re-running it while the user types (`Structural`), but instead of
    // *dropping* the badges until the debounced `Full` pass catches up —
    // which made them flash off and back on every keystroke — we re-emit
    // the LAST analysis's decorations, mapped through the text edits since.
    // The badges stay on screen and slide with the text; their content
    // updates in place once the `Full` pass recomputes (the patcher only
    // rewrites a widget's inner HTML when it actually changes, so there's
    // no flicker). Syntax highlighting above is cheap and always fresh.
    let analyze_decos: Vec<DecoratedRange> = if full {
        let computed = analyze_decorations(&text, len, overlays);
        ANALYZE_CACHE.with_borrow_mut(|c| {
            *c = Some(AnalyzeCache {
                text: text.clone(),
                decos: computed.clone(),
            });
        });
        computed
    } else {
        ANALYZE_CACHE.with_borrow(|c| {
            let Some(cache) = c.as_ref() else {
                return Vec::new();
            };
            if cache.text == text {
                return cache.decos.clone();
            }
            // Map the cached decorations from the text they were computed
            // against onto the current text, then clamp anything the edit
            // pushed out of range.
            let changes = diff_to_changes(&cache.text, &text);
            editor_state::DecoRangeSet::new(cache.decos.clone())
                .map_through(&changes)
                .as_slice()
                .iter()
                .filter(|d| d.to <= len && d.from <= len)
                .cloned()
                .collect()
        })
    };
    out.extend(analyze_decos);

    out.sort_by_key(|d| d.from);
    DECO_MEMO.with_borrow_mut(|m| {
        *m = Some(DecoMemo {
            text,
            full,
            overlays,
            out: out.clone(),
        });
    });
    out
}

/// Run `ide::analyze` and turn it into the diagnostic + overlay
/// decorations. Split out of [`keyflow_decorations`] so the `Full` pass
/// can compute-and-cache it while the `Structural` pass replays the
/// cached set mapped through edits.
fn analyze_decorations(text: &str, len: usize, overlays: bool) -> Vec<DecoratedRange> {
    let analysis = ide::analyze(text);
    let mut out = Vec::new();

    // Live diagnostics — wavy underline + the message as a `title` tooltip.
    for d in &analysis.diagnostics {
        let start = d.range.start;
        let end = d.range.end();
        if start < end && end <= len {
            out.push(Decoration::mark_with_attrs(
                start..end,
                diagnostic_class(d.severity),
                vec![("title".to_string(), d.message.clone())],
            ));
        }
    }

    // Resolved-chord type overlays — IDE-style inline inlay badges. For each
    // chord written in a key-relative system (Nashville number / Roman) that
    // resolves against the key, drop a dim badge right after the symbol
    // showing the absolute letter-name chord (e.g. `5` → `G`, `V7` → `G7`).
    //
    // The key is resolved PER CHORD via `key_at_position` — keyflow key
    // changes can happen in any measure (written on a section header like
    // `CH {…} #G`), so a single chart-level key would mis-resolve every chord
    // after a change. Gated by `SHOW_OVERLAYS`. `ChordInstance::source_span`
    // is document-absolute, so the badges land right after each symbol.
    if overlays {
        for section in &analysis.chart.sections {
            // Section-name badge — the written header is a terse, dynamic
            // marker (`VS`, `CH`) whose resolved name carries a number and
            // split letter assigned across the whole chart (`Verse 1a`).
            // Implicit sections (no header) have no span, so no badge.
            if let Some(span) = section.source_span {
                let label = section.section.display_name();
                let at = span.end().min(len);
                out.push(Decoration::widget(
                    at,
                    format!(
                        "<span class=\"kf-inlay kf-section-inlay\">{}</span>",
                        escape_html(&label)
                    ),
                ));
            }
            for measure in section.measures() {
                for ci in &measure.chords {
                    let Some(span) = ci.source_span else { continue };
                    let Some(key) = analysis.chart.key_at_position(&ci.position) else {
                        continue;
                    };
                    if let Some(label) = ci.resolved_symbol(key) {
                        let at = span.end().min(len);
                        out.push(Decoration::widget(
                            at,
                            format!("<span class=\"kf-inlay\">{}</span>", escape_html(&label)),
                        ));
                    }
                }
            }
        }
    }
    out
}

/// Minimal single-region diff of `old` → `new` as a one-`Change` set:
/// trim the common prefix and suffix (on char boundaries), the rest is
/// one replacement. Used to map the cached analysis decorations forward
/// through a burst of edits while analysis is debounced. A multi-region
/// burst collapses to one wide replacement — overlays in that span just
/// re-settle on the next `Full` pass.
fn diff_to_changes(old: &str, new: &str) -> editor_state::Changes {
    if old == new {
        return editor_state::Changes::empty();
    }
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut p = 0;
    let max_p = ob.len().min(nb.len());
    while p < max_p && ob[p] == nb[p] {
        p += 1;
    }
    while p > 0 && !(old.is_char_boundary(p) && new.is_char_boundary(p)) {
        p -= 1;
    }
    let mut s = 0;
    let max_s = (ob.len() - p).min(nb.len() - p);
    while s < max_s && ob[ob.len() - 1 - s] == nb[nb.len() - 1 - s] {
        s += 1;
    }
    while s > 0 && !(old.is_char_boundary(old.len() - s) && new.is_char_boundary(new.len() - s)) {
        s -= 1;
    }
    let from = p;
    let old_to = old.len() - s;
    let inserted = &new[p..new.len() - s];
    editor_state::Changes::single(editor_state::Change::replace(from..old_to, inserted))
}

/// Single-entry memo for [`keyflow_decorations`] — see the cache note
/// there. Thread-local because the editor runs the decoration source on
/// one (UI) thread; no synchronization needed.
struct DecoMemo {
    text: String,
    full: bool,
    overlays: bool,
    out: Vec<DecoratedRange>,
}

/// Last `Full`-pass analysis decorations + the text they were computed
/// against. The `Structural` pass replays these (mapped through edits) so
/// badges persist while analysis is debounced.
struct AnalyzeCache {
    text: String,
    decos: Vec<DecoratedRange>,
}

thread_local! {
    static DECO_MEMO: std::cell::RefCell<Option<DecoMemo>> = const { std::cell::RefCell::new(None) };
    static ANALYZE_CACHE: std::cell::RefCell<Option<AnalyzeCache>> = const { std::cell::RefCell::new(None) };
}

/// Process-global toggle for the resolved-chord overlays. Default on.
static SHOW_OVERLAYS: AtomicBool = AtomicBool::new(true);

/// Whether resolved-chord type overlays are currently shown.
#[must_use]
pub fn overlays_enabled() -> bool {
    SHOW_OVERLAYS.load(Ordering::Relaxed)
}

/// Turn resolved-chord type overlays on or off. The app should bump a signal
/// after calling this so the editor recomputes decorations.
pub fn set_overlays_enabled(on: bool) {
    SHOW_OVERLAYS.store(on, Ordering::Relaxed);
}

/// Flip the resolved-chord overlay toggle, returning the new state.
pub fn toggle_overlays() -> bool {
    let next = !overlays_enabled();
    set_overlays_enabled(next);
    next
}

/// Hover source for the keyflow editor — a [`HoverSource`]-shaped
/// `fn(&EditorState, usize) -> Option<HoverTooltip>`. Pass it straight into the
/// `<Editor>` `hover` prop.
///
/// Combines, for the byte offset under the pointer:
/// - any diagnostic whose range covers it (rendered first, colored by
///   severity), so hovering a squiggle shows the keyflow error, and
/// - keyflow's [`ide::hover`] token info (chord / scale degree / command /
///   melody-variable), rendered from its markdown.
///
/// [`HoverSource`]: editor_state::HoverSource
#[must_use]
pub fn keyflow_hover(state: &EditorState, pos: usize) -> Option<HoverTooltip> {
    let text = state.doc.to_string();
    let pos = pos.min(text.len());
    let analysis = ide::analyze(&text);

    let mut html = String::new();
    let mut from = pos;
    let mut to = pos;

    let mut covered = false;
    for d in &analysis.diagnostics {
        let (s, e) = (d.range.start, d.range.end());
        if pos >= s && pos <= e {
            html.push_str(&format!(
                "<div class=\"kf-hover-diag kf-hover-{}\"><strong>{}</strong> {}</div>",
                severity_slug(d.severity),
                severity_label(d.severity),
                escape_html(&d.message),
            ));
            if !covered {
                from = s;
                to = e;
                covered = true;
            }
        }
    }

    if let Some(info) = ide::hover(&text, pos, &analysis.chart) {
        html.push_str(&format!(
            "<div class=\"kf-hover-info\">{}</div>",
            md_to_html(&info.markdown)
        ));
        if !covered {
            from = info.range.start;
            to = info.range.end();
        }
    }

    if html.is_empty() {
        return None;
    }
    Some(HoverTooltip { html, from, to })
}

/// CSS for the classes [`keyflow_decorations`] emits: the keyflow theme's
/// per-token `.kf-*` color rules (from [`Renderer::generate_css`]) plus the
/// `.kf-diag-*` wavy-underline rules for diagnostics.
///
/// The squiggle colors reference `architect-ui` theme custom properties with literal
/// fallbacks, so they track light/dark themes when those tokens are defined and
/// still render standalone when they aren't.
#[must_use]
pub fn highlight_css(theme: &Theme) -> String {
    let mut css = Renderer::generate_css(theme);
    css.push_str(DIAGNOSTIC_CSS);
    css.push_str(OVERLAY_CSS);
    css
}

/// Wavy-underline class for a diagnostic severity. Two classes per range: the
/// shared `kf-diag` (offset/skip-ink) plus the severity color class.
const fn diagnostic_class(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "kf-diag kf-diag-error",
        Severity::Warning => "kf-diag kf-diag-warning",
        Severity::Info => "kf-diag kf-diag-info",
        Severity::Hint => "kf-diag kf-diag-hint",
    }
}

const fn severity_slug(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

const fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "Error",
        Severity::Warning => "Warning",
        Severity::Info => "Info",
        Severity::Hint => "Hint",
    }
}

/// Escape the five HTML-significant characters for safe injection into the
/// widget / tooltip `dangerous_inner_html`.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Tiny markdown → HTML for the short snippets `ide::hover` emits: fenced code
/// blocks, inline `` `code` ``, `**bold**`, and newlines. Input is escaped
/// first, so the only HTML produced is from these recognized tokens.
fn md_to_html(md: &str) -> String {
    let mut out = String::new();
    // Split on fenced code blocks first; even segments are prose, odd are code.
    for (i, seg) in md.split("```").enumerate() {
        if i % 2 == 1 {
            out.push_str("<pre class=\"kf-hover-code\">");
            out.push_str(&escape_html(seg.trim_matches('\n')));
            out.push_str("</pre>");
        } else {
            out.push_str(&inline_md(seg));
        }
    }
    out
}

/// Inline markdown for a prose segment: `**bold**`, `` `code` ``, and newlines.
fn inline_md(seg: &str) -> String {
    let esc = escape_html(seg);
    // `**bold**` → <strong>
    let mut s = String::with_capacity(esc.len());
    let mut rest = esc.as_str();
    let mut bold_open = false;
    while let Some(idx) = rest.find("**") {
        s.push_str(&rest[..idx]);
        s.push_str(if bold_open { "</strong>" } else { "<strong>" });
        bold_open = !bold_open;
        rest = &rest[idx + 2..];
    }
    s.push_str(rest);
    if bold_open {
        s.push_str("</strong>");
    }
    // `` `code` `` → <code>
    let mut t = String::with_capacity(s.len());
    let mut rest = s.as_str();
    let mut code_open = false;
    while let Some(idx) = rest.find('`') {
        t.push_str(&rest[..idx]);
        t.push_str(if code_open { "</code>" } else { "<code>" });
        code_open = !code_open;
        rest = &rest[idx + 1..];
    }
    t.push_str(rest);
    if code_open {
        t.push_str("</code>");
    }
    t.replace('\n', "<br>")
}

const DIAGNOSTIC_CSS: &str = "\
.kf-diag { text-decoration-skip-ink: none; text-underline-offset: 3px; }
.kf-diag-error { text-decoration: underline wavy var(--destructive, #e5484d); }
.kf-diag-warning { text-decoration: underline wavy var(--warning, #f5a623); }
.kf-diag-info { text-decoration: underline wavy var(--info, #4a9eff); }
.kf-diag-hint { text-decoration: underline wavy var(--muted-foreground, #888888); }
";

/// Styles for the resolved-chord inlay badges and the keyflow hover content.
/// `.editor-hover-tooltip` (the floating container) is styled by the editor
/// view itself; these rules cover the keyflow-specific bits inside it plus the
/// inline `.kf-inlay` badge.
const OVERLAY_CSS: &str = "\
.kf-inlay {
  display: inline-block;
  margin: 0 0.15em 0 0.3em;
  padding: 0 0.35em;
  border-radius: 0.35em;
  font-size: 0.82em;
  line-height: 1.45;
  vertical-align: baseline;
  color: var(--muted-foreground, #9a9aa2);
  background: color-mix(in srgb, var(--muted-foreground, #9a9aa2) 14%, transparent);
  user-select: none;
}
.kf-section-inlay {
  font-weight: 600;
  text-transform: none;
  color: var(--accent-foreground, #7aa2f7);
  background: color-mix(in srgb, var(--accent-foreground, #7aa2f7) 16%, transparent);
}
.kf-hover-info { white-space: normal; }
.kf-hover-info code { font-family: ui-monospace, Menlo, monospace; }
.kf-hover-code {
  margin: 0.35em 0 0;
  padding: 0.35em 0.5em;
  border-radius: 0.3em;
  background: color-mix(in srgb, var(--muted-foreground, #9a9aa2) 12%, transparent);
  font-family: ui-monospace, Menlo, monospace;
  white-space: pre-wrap;
}
.kf-hover-diag { margin-bottom: 0.35em; }
.kf-hover-error strong { color: var(--destructive, #e5484d); }
.kf-hover-warning strong { color: var(--warning, #f5a623); }
.kf-hover-info strong, .kf-hover-hint strong { color: var(--info, #4a9eff); }
";

#[cfg(test)]
mod tests {
    use super::*;
    use editor_state::EditorState;

    #[test]
    fn empty_doc_has_no_decorations() {
        let state = EditorState::new(String::new());
        assert!(keyflow_decorations(&state).is_empty());
    }

    #[test]
    fn highlights_a_chord_line() {
        let state = EditorState::new("Cmaj7 | G7".to_string());
        let decs = keyflow_decorations(&state);
        // At least the two chord roots should be marked.
        assert!(!decs.is_empty());
        // Sorted by start offset.
        assert!(decs.windows(2).all(|w| w[0].from <= w[1].from));
        // Every range is within bounds.
        let len = state.doc.to_string().len();
        assert!(decs.iter().all(|d| d.to <= len && d.from < d.to));
    }

    #[test]
    fn absolute_offsets_across_lines() {
        // A token on the second line must be marked at a doc-absolute offset
        // (past the first line + newline), never at a line-relative one.
        let state = EditorState::new("Cmaj7\nDm7".to_string());
        let decs = keyflow_decorations(&state);
        assert!(decs.iter().any(|d| d.from >= 6), "second-line token should be at absolute offset >= 6");
    }

    #[test]
    fn css_includes_diagnostic_and_overlay_rules() {
        let css = highlight_css(&Theme::default_dark());
        assert!(css.contains(".kf-diag-error"));
        assert!(css.contains(".kf-inlay"));
    }

    #[test]
    fn escape_html_handles_specials() {
        assert_eq!(escape_html("<a>&\"'"), "&lt;a&gt;&amp;&quot;&#39;");
    }

    #[test]
    fn md_to_html_basic() {
        let h = md_to_html("**bold** and `code`\nline2");
        assert!(h.contains("<strong>bold</strong>"));
        assert!(h.contains("<code>code</code>"));
        assert!(h.contains("<br>"));
    }

    #[test]
    fn md_to_html_fenced_code() {
        let h = md_to_html("text\n```\nC | G\n```");
        assert!(h.contains("<pre class=\"kf-hover-code\">C | G</pre>"));
    }

    #[test]
    fn hover_returns_content_on_a_chord() {
        let state = EditorState::new("Cmaj7 | G7".to_string());
        let h = keyflow_hover(&state, 1).expect("hover over a chord token");
        assert!(!h.html.is_empty());
        assert!(h.from <= 1 && 1 <= h.to);
    }

    #[test]
    fn hover_none_on_blank() {
        let state = EditorState::new("   ".to_string());
        assert!(keyflow_hover(&state, 1).is_none());
    }

    fn widget_count(decs: &[DecoratedRange]) -> usize {
        decs.iter()
            .filter(|d| matches!(d.kind, editor_state::DecorationKind::Widget { .. }))
            .count()
    }

    #[test]
    fn diff_to_changes_single_insert() {
        // Insert "2" at the end → one replacement of the empty tail.
        let cs = diff_to_changes("5 | 1", "5 | 12");
        let v: Vec<_> = cs.iter().cloned().collect();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].from, 5);
        assert_eq!(v[0].to, 5);
        assert_eq!(v[0].inserted, "2");
    }

    #[test]
    fn diff_to_changes_mid_edit_trims_prefix_and_suffix() {
        // "5 | 1" → "5 X | 1": common prefix "5 ", common suffix "| 1",
        // so the change is inserting "X " at offset 2.
        let cs = diff_to_changes("5 | 1", "5 X | 1");
        let v: Vec<_> = cs.iter().cloned().collect();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].from, 2);
        assert_eq!(v[0].to, 2);
        assert_eq!(v[0].inserted, "X ");
    }

    #[test]
    fn diff_to_changes_identity_is_empty() {
        assert!(diff_to_changes("abc", "abc").is_empty());
    }

    // One self-contained test owns the process-global overlay toggle so parallel
    // tests don't race on it.
    #[test]
    fn overlays_resolve_number_system_and_toggle() {
        // `#C` sets the key; `5` and `1` are scale-degree chords that resolve to
        // G and C in C major.
        let state = EditorState::new("4/4 120bpm #C\n\n5 | 1\n".to_string());

        set_overlays_enabled(true);
        let on = keyflow_decorations(&state);
        let widgets: Vec<_> = on
            .iter()
            .filter_map(|d| match &d.kind {
                editor_state::DecorationKind::Widget { html } => Some(html.clone()),
                _ => None,
            })
            .collect();
        assert!(
            !widgets.is_empty(),
            "expected resolved-chord overlays for a #C number-system chart, got none"
        );
        // The `5` should resolve to a G-rooted badge.
        assert!(
            widgets.iter().any(|h| h.contains("kf-inlay") && h.contains('G')),
            "expected a 'G' inlay badge for degree 5 in C; widgets = {widgets:?}"
        );
        // Doc-absolute placement: the badge sits right after the source `5`
        // (offset 15 in this chart), NOT at a line-relative offset near 0.
        let five_at = "4/4 120bpm #C\n\n5 | 1\n".find('5').unwrap();
        assert!(
            on.iter().any(|d| {
                matches!(d.kind, editor_state::DecorationKind::Widget { .. }) && d.from >= five_at
            }),
            "overlay widgets must use doc-absolute offsets (≥{five_at}); got {:?}",
            on.iter()
                .filter(|d| matches!(d.kind, editor_state::DecorationKind::Widget { .. }))
                .map(|d| d.from)
                .collect::<Vec<_>>()
        );

        set_overlays_enabled(false);
        assert_eq!(
            widget_count(&keyflow_decorations(&state)),
            0,
            "overlays disabled should emit no widget decorations"
        );

        // Mid-chart key change: `#G` switches the key inline, so chords before
        // it resolve in C (1→C, 5→G) and after it in G (1→G, 5→D). The 'C' and
        // 'D' badges can BOTH appear only when the key is resolved per chord
        // position (key_at_position) — a single chart-level key can't.
        set_overlays_enabled(true);
        let kc = EditorState::new("120bpm 4/4 #C\n\nvs\n1 5 #G 1 5\n".to_string());
        let badges: Vec<String> = keyflow_decorations(&kc)
            .iter()
            .filter_map(|d| match &d.kind {
                editor_state::DecorationKind::Widget { html } => Some(html.clone()),
                _ => None,
            })
            .collect();
        assert!(
            badges.iter().any(|h| h.contains("kf-inlay\">C")),
            "expected a C-rooted badge (degree 1 in C); got {badges:?}"
        );
        assert!(
            badges.iter().any(|h| h.contains("kf-inlay\">D")),
            "expected a D-rooted badge (degree 5 in G after the inline #G key change); got {badges:?}"
        );

        set_overlays_enabled(true); // restore default
    }

    #[test]
    fn section_headers_get_resolved_name_badges() {
        set_overlays_enabled(true);
        // Two verses + a chorus: the terse `VS`/`CH` markers should be badged
        // with their resolved, chart-numbered names. Two consecutive verses
        // split into `Verse 1a` / `Verse 1b`.
        let src = "VS\n1 | 5\n\nVS\n4 | 1\n\nCH\n1 | 4\n";
        let badges: Vec<String> = keyflow_decorations(&EditorState::new(src.to_string()))
            .iter()
            .filter_map(|d| match &d.kind {
                editor_state::DecorationKind::Widget { html } if html.contains("kf-section-inlay") => {
                    Some(html.clone())
                }
                _ => None,
            })
            .collect();
        assert!(
            badges.iter().any(|h| h.contains("Verse 1a")),
            "first verse should badge as 'Verse 1a'; got {badges:?}"
        );
        assert!(
            badges.iter().any(|h| h.contains("Verse 1b")),
            "second verse should badge as 'Verse 1b'; got {badges:?}"
        );
        assert!(
            badges.iter().any(|h| h.contains("Chorus")),
            "chorus should badge as 'Chorus'; got {badges:?}"
        );

        // Bare chord content has no written header → implicit section → no badge.
        let bare = keyflow_decorations(&EditorState::new("1 4 6 5\n".to_string()));
        assert!(
            !bare.iter().any(|d| matches!(
                &d.kind,
                editor_state::DecorationKind::Widget { html } if html.contains("kf-section-inlay")
            )),
            "bare chart (no section header) must not get a section badge"
        );
    }
}
