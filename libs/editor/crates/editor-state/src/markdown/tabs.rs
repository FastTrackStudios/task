//! Tabbed panels for ```` ```tabs ``` ```` fences.
//!
//! A `tabs` fence body is split into panels by delimiter lines
//! (`=== Tab Name`). Each panel renders its own markdown and the
//! whole thing becomes ONE self-contained HTML widget that
//! switches panels with **pure CSS** — hidden radio inputs +
//! `<label>` tabs + `input:checked ~ .panels .panel-{i}` rules.
//! No JavaScript, no Dioxus reactivity: a live-preview widget is
//! injected as a static HTML string (via `dangerous_inner_html`
//! in `editor-view`), so it can carry no event listeners.
//!
//! Mirrors the `mermaid`/`typst`/`keyflow` submodules: a
//! thread-local LRU cache keyed on `(source, unique)` plus a
//! per-`live_preview`-pass budget so a note full of fresh tab
//! blocks never blocks typing.

use std::cell::Cell;

use super::escape_html;

const CACHE_CAP: usize = 64;
/// Tab rendering is pure string work (no external compiler), so
/// the budget is generous — it exists only to bound a
/// pathological note with dozens of never-before-seen blocks.
const RENDER_BUDGET_PER_PASS: u8 = 24;

thread_local! {
    static RENDER_BUDGET: Cell<u8> = const { Cell::new(RENDER_BUDGET_PER_PASS) };
}

/// Re-arm the per-pass budget. Call at the top of every
/// `live_preview` pass.
pub(crate) fn reset_render_budget() {
    RENDER_BUDGET.with(|c| c.set(RENDER_BUDGET_PER_PASS));
}

/// A single parsed tab: its title and raw (unescaped) body.
struct Tab {
    title: String,
    body: String,
}

/// Split a `tabs` fence body into `(title, body)` panels.
///
/// A line whose trimmed form starts with `=== ` opens a new tab
/// titled by the rest of that line. Content before the first
/// delimiter becomes an untitled leading tab ("Tab 1") when it
/// has any non-whitespace; otherwise it's dropped. When the body
/// has no delimiter at all, the whole body is a single "Tab 1".
fn parse_tabs(body: &str) -> Vec<Tab> {
    let mut tabs: Vec<Tab> = Vec::new();
    let mut leading = String::new();
    let mut cur: Option<Tab> = None;

    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("=== ") {
            if let Some(t) = cur.take() {
                tabs.push(t);
            }
            cur = Some(Tab {
                title: rest.trim().to_string(),
                body: String::new(),
            });
        } else if trimmed == "===" {
            // Bare delimiter — a tab with an empty title.
            if let Some(t) = cur.take() {
                tabs.push(t);
            }
            cur = Some(Tab {
                title: String::new(),
                body: String::new(),
            });
        } else if let Some(t) = cur.as_mut() {
            t.body.push_str(line);
            t.body.push('\n');
        } else {
            leading.push_str(line);
            leading.push('\n');
        }
    }
    if let Some(t) = cur.take() {
        tabs.push(t);
    }

    // Promote leading content to a first tab when it's non-empty
    // (either no delimiter was present, or prose preceded the
    // first `===`).
    if !leading.trim().is_empty() {
        tabs.insert(
            0,
            Tab {
                title: String::new(),
                body: leading,
            },
        );
    }

    // Fill in default titles for untitled tabs.
    for (i, t) in tabs.iter_mut().enumerate() {
        if t.title.is_empty() {
            t.title = format!("Tab {}", i + 1);
        }
    }
    tabs
}

/// Render a `tabs` fence body to a self-contained, CSS-only
/// tab widget. `focus_pos` (the fence's byte offset) is folded
/// into the scope hash so two blocks with *identical* content
/// still get distinct radio-group names/ids.
///
/// Returns `None` when the body has no tabs, or when the
/// per-pass budget is exhausted on a cache miss (caller then
/// falls back to the raw source, exactly like the sibling
/// renderers).
pub(crate) fn render_tabs(body: &str, focus_pos: usize) -> Option<String> {
    let unique = scope_id(body, focus_pos);
    if let Some(cached) = with_cache(|c| c.get(&unique)) {
        return Some(cached);
    }
    let budget = RENDER_BUDGET.with(std::cell::Cell::get);
    if budget == 0 {
        return None;
    }
    RENDER_BUDGET.with(|c| c.set(budget - 1));

    let tabs = parse_tabs(body);
    if tabs.is_empty() {
        return None;
    }

    let html = build_html(&tabs, &unique);
    with_cache(|c| c.put(unique, html.clone()));
    Some(html)
}

/// FNV-1a over the body plus the fence offset → a short hex tag
/// used to scope every id/class/selector to this one block.
fn scope_id(body: &str, focus_pos: usize) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in body.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    for b in focus_pos.to_le_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Assemble the CSS-only tab widget. Layout, structure and
/// selectors are all scoped by `.md-tabs-{u}` / `#tab-{u}-{i}`
/// so any number of blocks coexist in one note without their
/// radio groups or `:checked` rules colliding.
fn build_html(tabs: &[Tab], u: &str) -> String {
    let n = tabs.len();

    // Per-index rules: the active label indicator and the
    // panel visibility toggle. This is the whole switching
    // mechanism — a checked radio reveals its sibling panel and
    // lights its sibling label, no script involved.
    let mut rules = String::new();
    for i in 0..n {
        rules.push_str(&format!(
            "#tab-{u}-{i}:checked~.md-tabs-strip label[for=\"tab-{u}-{i}\"]{{opacity:1;border-bottom-color:currentColor;font-weight:600}}\
             #tab-{u}-{i}:checked~.md-tabs-panels .md-tabs-panel-{i}{{display:block}}"
        ));
    }

    let css = format!(
        ".md-tabs-{u}{{display:block;margin:0.4em 0}}\
         .md-tabs-{u} .md-tabs-radio{{position:absolute;width:1px;height:1px;opacity:0;pointer-events:none;clip:rect(0 0 0 0);overflow:hidden}}\
         .md-tabs-{u} .md-tabs-strip{{display:flex;flex-wrap:wrap;gap:0.15em;border-bottom:1px solid rgba(128,128,128,0.35);margin-bottom:0.2em}}\
         .md-tabs-{u} .md-tabs-strip label{{padding:0.3em 0.75em;cursor:pointer;opacity:0.6;border-bottom:2px solid transparent;margin-bottom:-1px;user-select:none;white-space:nowrap}}\
         .md-tabs-{u} .md-tabs-strip label:hover{{opacity:0.85}}\
         .md-tabs-{u} .md-tabs-panels .md-tabs-panel{{display:none;padding:0.5em 0.2em}}\
         .md-tabs-{u} .md-tabs-panels .md-tabs-panel>*:first-child{{margin-top:0}}\
         {rules}"
    );

    let mut inputs = String::new();
    let mut labels = String::new();
    let mut panels = String::new();
    for (i, tab) in tabs.iter().enumerate() {
        let checked = if i == 0 { " checked" } else { "" };
        inputs.push_str(&format!(
            "<input type=\"radio\" class=\"md-tabs-radio\" name=\"tabs-{u}\" id=\"tab-{u}-{i}\"{checked}>"
        ));
        labels.push_str(&format!(
            "<label for=\"tab-{u}-{i}\">{title}</label>",
            title = escape_html(&tab.title),
        ));
        panels.push_str(&format!(
            "<div class=\"md-tabs-panel md-tabs-panel-{i}\">{content}</div>",
            content = render_panel_markdown(&tab.body),
        ));
    }

    format!(
        "<div class=\"md-tabs md-tabs-{u}\"><style>{css}</style>{inputs}\
         <div class=\"md-tabs-strip\">{labels}</div>\
         <div class=\"md-tabs-panels\">{panels}</div></div>"
    )
}

/// Minimal, XSS-safe markdown for a panel body: HTML-escape
/// everything first, then apply a small set of inline
/// transforms over the *already-escaped* text (so no transform
/// can introduce unescaped markup). Blank-line-separated blocks
/// become `<p>`; single newlines become `<br>`.
fn render_panel_markdown(src: &str) -> String {
    let mut out = String::new();
    for block in split_blocks(src) {
        let escaped = escape_html(block);
        let inline = inline_md(&escaped);
        out.push_str("<p>");
        out.push_str(&inline);
        out.push_str("</p>");
    }
    out
}

/// Split text into blank-line-separated blocks, trimming
/// surrounding blank lines. A "blank line" is one that is empty
/// after trimming whitespace.
fn split_blocks(src: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut start: Option<usize> = None;
    let mut last_end = 0;
    let bytes = src;
    let mut idx = 0;
    for line in bytes.split_inclusive('\n') {
        let line_start = idx;
        idx += line.len();
        if line.trim().is_empty() {
            if let Some(s) = start.take() {
                blocks.push(bytes[s..last_end].trim_matches('\n'));
            }
        } else {
            if start.is_none() {
                start = Some(line_start);
            }
            last_end = idx;
        }
    }
    if let Some(s) = start.take() {
        blocks.push(bytes[s..last_end].trim_matches('\n'));
    }
    blocks.into_iter().filter(|b| !b.is_empty()).collect()
}

/// Inline formatting over an **already HTML-escaped** string:
/// `` `code` ``, `**bold**`, `*italic*`, and newline → `<br>`.
/// Because the input carries no raw `<`/`>`/`&`, and this
/// function only ever emits fixed tags plus copies of that safe
/// input, the result stays XSS-safe.
fn inline_md(escaped: &str) -> String {
    let chars: Vec<char> = escaped.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        match chars[i] {
            '`' => {
                if let Some(j) = find_char(&chars, i + 1, '`') {
                    out.push_str("<code>");
                    out.extend(chars[i + 1..j].iter());
                    out.push_str("</code>");
                    i = j + 1;
                    continue;
                }
                out.push('`');
                i += 1;
            }
            '*' if i + 1 < n && chars[i + 1] == '*' => {
                if let Some(j) = find_double(&chars, i + 2) {
                    let inner: String = chars[i + 2..j].iter().collect();
                    out.push_str("<strong>");
                    out.push_str(&inline_md(&inner));
                    out.push_str("</strong>");
                    i = j + 2;
                    continue;
                }
                out.push_str("**");
                i += 2;
            }
            '*' => {
                if let Some(j) = find_char(&chars, i + 1, '*') {
                    let inner: String = chars[i + 1..j].iter().collect();
                    out.push_str("<em>");
                    out.push_str(&inline_md(&inner));
                    out.push_str("</em>");
                    i = j + 1;
                    continue;
                }
                out.push('*');
                i += 1;
            }
            '\n' => {
                out.push_str("<br>");
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn find_char(chars: &[char], from: usize, needle: char) -> Option<usize> {
    (from..chars.len()).find(|&k| chars[k] == needle)
}

/// Find the next `**` at or after `from`, returning the index of
/// its first `*`.
fn find_double(chars: &[char], from: usize) -> Option<usize> {
    let mut k = from;
    while k + 1 < chars.len() {
        if chars[k] == '*' && chars[k + 1] == '*' {
            return Some(k);
        }
        k += 1;
    }
    None
}

struct TabsCache {
    entries: Vec<(String, String)>,
    cap: usize,
}

impl TabsCache {
    fn new(cap: usize) -> Self {
        Self {
            entries: Vec::with_capacity(cap),
            cap,
        }
    }
    fn get(&mut self, key: &str) -> Option<String> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        let hit = self.entries.remove(i);
        let html = hit.1.clone();
        self.entries.push(hit);
        Some(html)
    }
    fn put(&mut self, key: String, html: String) {
        if self.entries.len() >= self.cap {
            self.entries.remove(0);
        }
        self.entries.push((key, html));
    }
}

fn with_cache<R>(f: impl FnOnce(&mut TabsCache) -> R) -> R {
    thread_local! {
        static CACHE: std::cell::RefCell<TabsCache> =
            std::cell::RefCell::new(TabsCache::new(CACHE_CAP));
    }
    CACHE.with(|c| f(&mut c.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_tabs_yields_three_of_each() {
        let body = "=== Session\nRun of show\n=== Chart\nKey/tempo\n=== Lyrics\nVerse 1";
        let html = render_tabs(body, 42).expect("renders");
        assert_eq!(html.matches("type=\"radio\"").count(), 3);
        assert_eq!(html.matches("<label for=").count(), 3);
        assert_eq!(html.matches("class=\"md-tabs-panel ").count(), 3);
        // First radio is checked, and only the first.
        assert_eq!(html.matches(" checked>").count(), 1);
        // Titles present.
        assert!(html.contains(">Session</label>"));
        assert!(html.contains(">Chart</label>"));
        assert!(html.contains(">Lyrics</label>"));
    }

    #[test]
    fn escapes_content_and_titles() {
        let body = "=== <script>\n<img src=x onerror=alert(1)>";
        let html = render_tabs(body, 0).expect("renders");
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<img src=x"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&lt;img"));
    }

    #[test]
    fn inline_markdown_bold_italic_code() {
        let body = "=== A\nsome **bold** and *italic* and `code` text";
        let html = render_tabs(body, 7).expect("renders");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
        assert!(html.contains("<code>code</code>"));
    }

    #[test]
    fn leading_content_becomes_first_tab() {
        let body = "intro prose\n=== Second\nbody";
        let tabs = parse_tabs(body);
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].title, "Tab 1");
        assert_eq!(tabs[1].title, "Second");
    }

    #[test]
    fn distinct_scope_for_identical_bodies() {
        // Same content at different fence offsets must not
        // collide (radio group names would otherwise clash).
        let body = "=== A\nx\n=== B\ny";
        let a = scope_id(body, 10);
        let b = scope_id(body, 20);
        assert_ne!(a, b);
    }

    #[test]
    fn empty_body_is_none() {
        assert!(render_tabs("   \n  ", 0).is_none());
    }
}
