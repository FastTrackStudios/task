//! Generic trigger-autocomplete machinery — the slash-palette
//! pattern (`crate::slash`) extracted into a reusable shape and
//! used to implement `[[` wikilink completion and `#` tag
//! completion.
//!
//! The moving parts mirror CM6's `@codemirror/autocomplete`, the
//! same way the slash module does:
//!
//! - [`detect_trigger`] is the `matchBefore` analogue: scan the
//!   current line up to the caret for an open trigger (`[[…` or
//!   `#…`) and extract the query typed after it.
//! - [`CompletionState`] is the `ActiveSource`: open trigger +
//!   query + highlighted row + the fetched candidate list.
//! - [`CompletionSource`] supplies the candidates. It's a prop on
//!   `<Editor>` — `(query, kind) -> Vec<Candidate>` — so the editor
//!   stays **vault-agnostic**: the host (which owns the vault index
//!   / tag index) decides what `[[` and `#` can complete to.
//! - [`accept_candidate`] is `Completion.apply`: replace the
//!   trigger region with the final `[[Name]]` / `#tag` text through
//!   the normal transaction path.
//!
//! The slash palette keeps its own module (its "candidates" are a
//! static command catalog with run-semantics, not text inserts) and
//! its behavior is unchanged; this module covers the
//! text-completion family. When both a slash trigger and a
//! completion trigger are somehow open at once (e.g. `[[x/y`), the
//! editor's keyboard routing lets slash win — same precedence the
//! slash menu always had.

use std::rc::Rc;

use dioxus::prelude::*;
use editor_state::{Changes, EditorState, Selection, TransactionSpec};

/// Which trigger opened the completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    /// `[[` — wikilink target completion. Accepting inserts
    /// `[[<insert_text>]]`.
    Wikilink,
    /// `#` — tag completion. Accepting inserts `#<insert_text>`.
    Tag,
}

/// One completion candidate, supplied by the host's
/// [`CompletionSource`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Row text shown in the menu.
    pub label: String,
    /// Text spliced into the document (without the trigger syntax —
    /// the editor wraps it as `[[…]]` / `#…` per [`CompletionKind`]).
    pub insert_text: String,
    /// Secondary line shown under the label (path, usage count, …).
    /// Empty hides the row's detail line.
    pub detail: String,
}

impl Candidate {
    /// Candidate whose label IS the inserted text, no detail.
    pub fn simple(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            label: text.clone(),
            insert_text: text,
            detail: String::new(),
        }
    }
}

/// Candidate provider — `(query, kind) -> Vec<Candidate>`. Same
/// two-shape design as [`crate::DecorationSource`]:
///
/// - [`CompletionSource::ptr`] for a stateless fn pointer (equality
///   = fn address, stable across renders);
/// - [`CompletionSource::new`] for a capturing closure (equality =
///   `Rc` identity — create once in `use_hook` and capture
///   `Signal`s, e.g. the host's vault index signal).
///
/// The source does its own filtering/ranking against `query`; the
/// editor displays whatever comes back, in order.
#[derive(Clone)]
pub struct CompletionSource(SourceImpl);

/// The bare callable shape both [`CompletionSource`] variants share.
type CompletionFn = dyn Fn(&str, CompletionKind) -> Vec<Candidate>;

#[derive(Clone)]
enum SourceImpl {
    Ptr(fn(&str, CompletionKind) -> Vec<Candidate>),
    Dyn(Rc<CompletionFn>),
}

impl CompletionSource {
    /// Wrap a stateful closure. Create once (e.g. in `use_hook`) and
    /// capture `Signal`s — see the type-level equality contract.
    pub fn new(f: impl Fn(&str, CompletionKind) -> Vec<Candidate> + 'static) -> Self {
        Self(SourceImpl::Dyn(Rc::new(f)))
    }

    /// Wrap a plain fn pointer.
    #[must_use]
    pub fn ptr(f: fn(&str, CompletionKind) -> Vec<Candidate>) -> Self {
        Self(SourceImpl::Ptr(f))
    }

    /// Fetch candidates for `query` under `kind`.
    #[must_use]
    pub fn run(&self, query: &str, kind: CompletionKind) -> Vec<Candidate> {
        match &self.0 {
            SourceImpl::Ptr(f) => f(query, kind),
            SourceImpl::Dyn(f) => f(query, kind),
        }
    }
}

impl From<fn(&str, CompletionKind) -> Vec<Candidate>> for CompletionSource {
    fn from(f: fn(&str, CompletionKind) -> Vec<Candidate>) -> Self {
        Self::ptr(f)
    }
}

impl PartialEq for CompletionSource {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (SourceImpl::Ptr(a), SourceImpl::Ptr(b)) => std::ptr::fn_addr_eq(*a, *b),
            (SourceImpl::Dyn(a), SourceImpl::Dyn(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl std::fmt::Debug for CompletionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            SourceImpl::Ptr(p) => f.debug_tuple("CompletionSource::Ptr").field(p).finish(),
            SourceImpl::Dyn(_) => f.write_str("CompletionSource::Dyn(..)"),
        }
    }
}

/// Open-state of the completion menu. `None` when closed. Mirrors
/// `slash::SlashState`, plus the fetched candidates (so keyboard
/// routing doesn't re-query the source on every keypress).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionState {
    /// Which trigger is open.
    pub kind: CompletionKind,
    /// Byte offset of the trigger's first char (`[` of `[[`, or `#`).
    pub trigger_start: usize,
    /// Body typed after the trigger (does NOT include the trigger).
    pub query: String,
    /// Currently highlighted row.
    pub selected: usize,
    /// Candidates fetched from the source for `query`.
    pub candidates: Vec<Candidate>,
}

/// `true` for bytes allowed inside a `#tag` body. ASCII alnum plus
/// `_-/` (Obsidian's nested-tag separator), and any non-ASCII byte
/// so unicode tags work (all bytes of a multi-byte char are
/// `>= 0x80`, so the byte-wise scan stays on char boundaries).
fn is_tag_byte(b: u8) -> bool {
    b >= 0x80 || b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'/')
}

/// Scan back from the caret for an open completion trigger on the
/// current line. Returns `(kind, trigger_start, query)`.
///
/// Wikilink wins over tag when both could match (so `[[Page#Head`
/// keeps completing the wikilink — the host sees the full
/// `Page#Head` query and can offer heading targets).
///
/// - **Wikilink**: the last `[[` on the line with no closing `]]`
///   between it and the caret. The query may contain spaces (page
///   names do). An embed's `![[` triggers too — the `!` sits before
///   `trigger_start` and is preserved on accept.
/// - **Tag**: a `#` preceded by line start or whitespace, followed
///   only by tag bytes up to the caret. `##` never triggers (the
///   second `#`'s predecessor isn't whitespace), so typing heading
///   markers doesn't pop the menu; a lone `#` at line start does
///   trigger, and typing the heading's space closes it immediately
///   — matching Obsidian's behavior.
#[must_use]
pub fn detect_trigger(doc: &str, caret: usize) -> Option<(CompletionKind, usize, String)> {
    let caret = caret.min(doc.len());
    if !doc.is_char_boundary(caret) {
        return None;
    }
    let line_start = doc[..caret].rfind('\n').map_or(0, |n| n + 1);
    let seg = &doc[line_start..caret];

    // ── `[[` wikilink ─────────────────────────────────────────
    if let Some(i) = seg.rfind("[[") {
        let query = &seg[i + 2..];
        if !query.contains("]]") {
            return Some((CompletionKind::Wikilink, line_start + i, query.to_string()));
        }
    }

    // ── `#` tag ───────────────────────────────────────────────
    let bytes = seg.as_bytes();
    let mut i = bytes.len();
    while i > 0 && is_tag_byte(bytes[i - 1]) {
        i -= 1;
    }
    if i > 0 && bytes[i - 1] == b'#' {
        let hash = i - 1;
        let prev_ok = hash == 0
            || seg[..hash]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if prev_ok {
            return Some((CompletionKind::Tag, line_start + hash, seg[i..].to_string()));
        }
    }
    None
}

/// Resolve a picked candidate into the transaction that splices the
/// final text over the trigger region. Returns `None` when the open
/// state no longer matches the document (stale offsets).
///
/// The replaced region runs from `trigger_start` to the caret; for
/// wikilinks an auto-paired `]]` sitting right after the caret (the
/// bracket auto-pair inserts `[[]]` as a unit) is swallowed so
/// accepting doesn't leave a dangling `]]`.
#[must_use]
pub fn accept_candidate(
    state: &EditorState,
    comp: &CompletionState,
    cand: &Candidate,
) -> Option<TransactionSpec> {
    let doc = state.doc.to_string();
    let caret = state.selection.primary().head.min(doc.len());
    if comp.trigger_start >= caret || caret > doc.len() || !doc.is_char_boundary(caret) {
        return None;
    }
    let mut end = caret;
    let replacement = match comp.kind {
        CompletionKind::Wikilink => {
            if doc[caret..].starts_with("]]") {
                end += 2;
            }
            format!("[[{}]]", cand.insert_text)
        }
        CompletionKind::Tag => format!("#{}", cand.insert_text),
    };
    let new_caret = comp.trigger_start + replacement.len();
    Some(
        TransactionSpec::new()
            .changes(Changes::replace(comp.trigger_start..end, replacement))
            .selection(Selection::caret(new_caret))
            .annotate("origin", "completion"),
    )
}

/// Completion popup. Rendered by `<Editor>` itself (unlike
/// `SlashMenu`, which the host mounts) — the host only supplies the
/// `completion` candidate source. Reuses the slash menu's CSS
/// classes so existing stylesheets cover it; `.completion-menu` is
/// added for targeted overrides. Renders nothing when there are no
/// candidates (an empty box flashing during normal `[[`/`#` typing
/// would be noise).
#[component]
pub fn CompletionMenu(
    state: Signal<EditorState>,
    completion: Signal<Option<CompletionState>>,
    #[props(default)] on_transaction: Option<Callback<crate::TransactionEvent>>,
) -> Element {
    let snapshot = completion.read().clone();
    let Some(current) = snapshot else {
        return rsx! {
            Fragment {}
        };
    };
    // Caret-anchored positioning — same approach as `SlashMenu`.
    use_effect(|| {
        let script = r"(()=>{
            const menu = document.querySelector('.completion-menu');
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
    if current.candidates.is_empty() {
        return rsx! {
            Fragment {}
        };
    }
    let selected = current.selected.min(current.candidates.len() - 1);
    let kind_icon = match current.kind {
        CompletionKind::Wikilink => "[[",
        CompletionKind::Tag => "#",
    };
    rsx! {
        div { class: "slash-menu completion-menu",
            for (idx , cand) in current.candidates.iter().cloned().enumerate() {
                {
                    let is_selected = idx == selected;
                    let state_for_click = state;
                    let mut comp_for_click = completion;
                    let sink_for_click = on_transaction;
                    let current_for_click = current.clone();
                    let cand_for_click = cand.clone();
                    rsx! {
                        div {
                            key: "{idx}",
                            class: if is_selected { "slash-row selected" } else { "slash-row" },
                            // Keep the editor's caret from blurring as
                            // the click lands (same trick as SlashMenu).
                            onmousedown: move |e: Event<MouseData>| e.prevent_default(),
                            onclick: move |_| {
                                let cur = state_for_click.read().clone();
                                if let Some(spec) = accept_candidate(
                                    &cur,
                                    &current_for_click,
                                    &cand_for_click,
                                ) {
                                    crate::event::apply_tx(
                                        state_for_click,
                                        &cur,
                                        spec,
                                        sink_for_click,
                                    );
                                }
                                comp_for_click.set(None);
                            },
                            div { class: "slash-row-icon", "{kind_icon}" }
                            div { class: "slash-row-body",
                                div { class: "slash-row-label", "{cand.label}" }
                                if !cand.detail.is_empty() {
                                    div { class: "slash-row-desc", "{cand.detail}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_open_wikilink() {
        assert_eq!(
            detect_trigger("see [[Pa", 9),
            Some((CompletionKind::Wikilink, 4, "Pa".to_string()))
        );
    }

    #[test]
    fn wikilink_query_may_contain_spaces() {
        assert_eq!(
            detect_trigger("[[My Page Na", 12),
            Some((CompletionKind::Wikilink, 0, "My Page Na".to_string()))
        );
    }

    #[test]
    fn closed_wikilink_does_not_trigger() {
        let doc = "see [[Page]] after";
        assert_eq!(detect_trigger(doc, doc.len()), None);
    }

    #[test]
    fn wikilink_with_autopaired_close_triggers_between() {
        // Bracket auto-pair leaves `[[]]` with the caret between —
        // the closing `]]` sits after the caret, so the segment up
        // to the caret is an open trigger.
        assert_eq!(
            detect_trigger("[[]]", 2),
            Some((CompletionKind::Wikilink, 0, String::new()))
        );
    }

    #[test]
    fn embed_bang_form_triggers() {
        assert_eq!(
            detect_trigger("![[img", 6),
            Some((CompletionKind::Wikilink, 1, "img".to_string()))
        );
    }

    #[test]
    fn wikilink_scoped_to_current_line() {
        assert_eq!(detect_trigger("[[open\nnext", 11), None);
    }

    #[test]
    fn detects_tag_after_whitespace() {
        assert_eq!(
            detect_trigger("note #ta", 8),
            Some((CompletionKind::Tag, 5, "ta".to_string()))
        );
    }

    #[test]
    fn detects_tag_at_line_start() {
        assert_eq!(
            detect_trigger("#proj", 5),
            Some((CompletionKind::Tag, 0, "proj".to_string()))
        );
    }

    #[test]
    fn nested_tag_and_unicode_allowed() {
        assert_eq!(
            detect_trigger("x #a/b-c_ü", 11),
            Some((CompletionKind::Tag, 2, "a/b-c_ü".to_string()))
        );
    }

    #[test]
    fn heading_hashes_do_not_trigger() {
        // `##` — second hash's predecessor is a hash, not whitespace.
        assert_eq!(detect_trigger("##", 2), None);
        // `# ` — the space after the hash is not a tag byte, and the
        // hash isn't immediately before the caret scan.
        assert_eq!(detect_trigger("# h", 3), None);
        // mid-word hash (URLs, code) — no trigger.
        assert_eq!(detect_trigger("a#b", 3), None);
    }

    #[test]
    fn wikilink_wins_over_tag_inside_open_link() {
        assert_eq!(
            detect_trigger("[[Page#Head", 11),
            Some((CompletionKind::Wikilink, 0, "Page#Head".to_string()))
        );
    }

    fn st(text: &str, head: usize) -> EditorState {
        EditorState {
            doc: editor_state::Doc::from_str(text),
            selection: Selection::caret(head),
            folds: Vec::new(),
            reading_mode: false,
        }
    }

    fn comp(kind: CompletionKind, start: usize, query: &str) -> CompletionState {
        CompletionState {
            kind,
            trigger_start: start,
            query: query.to_string(),
            selected: 0,
            candidates: Vec::new(),
        }
    }

    #[test]
    fn accept_wikilink_inserts_full_link() {
        let state = st("see [[Pa", 8);
        let spec = accept_candidate(
            &state,
            &comp(CompletionKind::Wikilink, 4, "Pa"),
            &Candidate::simple("Page Name"),
        )
        .unwrap();
        let next = state.update(spec);
        assert_eq!(next.doc.to_string(), "see [[Page Name]]");
        assert_eq!(next.selection.primary().head, 17);
    }

    #[test]
    fn accept_wikilink_swallows_autopaired_close() {
        // `[[]]` with the caret between the pairs.
        let state = st("[[]]", 2);
        let spec = accept_candidate(
            &state,
            &comp(CompletionKind::Wikilink, 0, ""),
            &Candidate::simple("Page"),
        )
        .unwrap();
        let next = state.update(spec);
        assert_eq!(next.doc.to_string(), "[[Page]]");
        assert_eq!(next.selection.primary().head, 8);
    }

    #[test]
    fn accept_tag_inserts_hash_form() {
        let state = st("note #pr", 8);
        let spec = accept_candidate(
            &state,
            &comp(CompletionKind::Tag, 5, "pr"),
            &Candidate::simple("project/active"),
        )
        .unwrap();
        let next = state.update(spec);
        assert_eq!(next.doc.to_string(), "note #project/active");
        assert_eq!(next.selection.primary().head, 20);
    }

    #[test]
    fn accept_rejects_stale_state() {
        // Caret before the recorded trigger — stale offsets.
        let state = st("ab", 1);
        assert!(accept_candidate(
            &state,
            &comp(CompletionKind::Tag, 5, "x"),
            &Candidate::simple("t"),
        )
        .is_none());
    }

    #[test]
    fn unicode_around_trigger_round_trips() {
        let doc = "émoji 🦀 [[Pa";
        let state = st(doc, doc.len());
        let start = doc.find("[[").unwrap();
        let spec = accept_candidate(
            &state,
            &comp(CompletionKind::Wikilink, start, "Pa"),
            &Candidate::simple("Pärt 🎵"),
        )
        .unwrap();
        let next = state.update(spec);
        assert_eq!(next.doc.to_string(), "émoji 🦀 [[Pärt 🎵]]");
    }
}
