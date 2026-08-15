//! The `<Editor>` Dioxus component.
//!
//! v1 architecture: a `<div contenteditable="plaintext-only">`
//! whose text content is bound to `state.doc`. Phase A here is
//! plain text only (a single rendered text child). Phase B adds
//! decoration rendering as per-segment spans.
//!
//! ## Why contenteditable now (and how the cursor stays put)
//!
//! Contenteditable lets us render styled text (decorations) and
//! still get a real caret. Textarea can't show inline styles.
//!
//! The trick to not eating the cursor on every re-render is:
//! **render the same text Dioxus already has in the DOM**.
//! Typing flow:
//!
//!   1. user presses 'a' → browser updates DOM textContent to "...a"
//!   2. our JS bridge reads the new textContent, computes a diff
//!      against the old `state.doc`, and applies a Transaction
//!   3. Dioxus re-renders. The text child is `"{text}"` where
//!      text == the new doc, which equals what the DOM already
//!      contains. Dioxus's reconciler sees text node value
//!      unchanged → emits no DOM mutation → caret untouched.
//!
//! For programmatic edits (a command, undo, remote CRDT op) the
//! state diverges from the DOM. Dioxus updates the text node;
//! the browser parks the caret at offset 0. A `use_effect`
//! restores it from `state.selection` — but only when the DOM
//! text already matches state.doc (preventing the writeback
//! from fighting in-flight typing).

// `DecorationSource`'s fn-pointer fast path compares by fn
// address; within a single binary fn-ptr equality is reliable
// enough for prop-diff purposes. The lint guards against
// codegen-unit splits that don't happen in our build.
#![allow(unpredictable_function_pointer_comparisons)]

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use dioxus::prelude::*;
use editor_state::{
    Changes, DecoratedRange, EditorState, KeySpec, Keymap, Range, Selection, TransactionSpec,
};

use crate::tile::build::build_tiles;
// Only the web/desktop JS-patch effect serializes the tile tree this way;
// native renders it as rsx via `tile::render_dx` instead.
#[cfg(not(feature = "native"))]
use crate::tile::patch::build_patch;

/// Decoration source — a callable that produces decorations for
/// the current state. Multiple sources can be combined; the view
/// concatenates and sorts before rendering.
///
/// Conceptually mirrors CM6's `EditorView.decorations` facet —
/// extensions contribute decorations, the view merges. Two
/// shapes:
///
/// - **Fn pointer** (the original v1 surface): a pure
///   `fn(&EditorState) -> Vec<DecoratedRange>`. Build via
///   [`DecorationSource::ptr`] or `From<fn>`. Equality is fn
///   address comparison, so passing the same fn every render
///   never re-renders the editor.
/// - **Stateful closure**: anything capturing environment — a
///   vault index for wikilink resolution, presence cursors from
///   a CRDT peer set, settings signals. Build via
///   [`DecorationSource::new`]. Equality is `Rc` identity, so
///   hosts should create the source **once** (e.g. inside
///   `use_hook` / `use_memo`) and capture `Signal`s rather than
///   values; rebuilding the `Rc` every render forces an editor
///   re-render every parent render.
///
/// The plan docs anticipated this: "we can swap to a trait
/// object for stateful sources later" — this is that swap, with
/// the fn-pointer path kept as a zero-cost compatibility shape.
#[derive(Clone)]
pub struct DecorationSource(SourceImpl);

/// The bare callable shape both [`DecorationSource`] variants share.
type SourceFn = dyn Fn(&EditorState) -> Vec<DecoratedRange>;

#[derive(Clone)]
enum SourceImpl {
    Ptr(fn(&EditorState) -> Vec<DecoratedRange>),
    Dyn(Rc<SourceFn>),
}

impl DecorationSource {
    /// Wrap a stateful closure. Create once (e.g. in `use_hook`)
    /// and capture `Signal`s — see the type-level docs for the
    /// equality contract.
    pub fn new(f: impl Fn(&EditorState) -> Vec<DecoratedRange> + 'static) -> Self {
        Self(SourceImpl::Dyn(Rc::new(f)))
    }

    /// Wrap a plain fn pointer. Identical behavior (including
    /// prop-diff equality) to the pre-stateful `type
    /// DecorationSource = fn(..)` alias.
    #[must_use]
    pub fn ptr(f: fn(&EditorState) -> Vec<DecoratedRange>) -> Self {
        Self(SourceImpl::Ptr(f))
    }

    /// Produce the decoration set for `state`.
    #[must_use]
    pub fn run(&self, state: &EditorState) -> Vec<DecoratedRange> {
        match &self.0 {
            SourceImpl::Ptr(f) => f(state),
            SourceImpl::Dyn(f) => f(state),
        }
    }
}

impl From<fn(&EditorState) -> Vec<DecoratedRange>> for DecorationSource {
    fn from(f: fn(&EditorState) -> Vec<DecoratedRange>) -> Self {
        Self::ptr(f)
    }
}

impl PartialEq for DecorationSource {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (SourceImpl::Ptr(a), SourceImpl::Ptr(b)) => std::ptr::fn_addr_eq(*a, *b),
            (SourceImpl::Dyn(a), SourceImpl::Dyn(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl std::fmt::Debug for DecorationSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            SourceImpl::Ptr(p) => f.debug_tuple("DecorationSource::Ptr").field(p).finish(),
            SourceImpl::Dyn(_) => f.write_str("DecorationSource::Dyn(..)"),
        }
    }
}

/// Per-instance id allocator — each `<Editor>` mount gets a
/// unique `data-editor-id` for the JS bridge to find it.
static EDITOR_INSTANCE: AtomicU64 = AtomicU64::new(0);

/// `true` for bridge message kinds that mutate the document. Used to
/// drop the decoration pass to [`DecoPhase::Structural`] while the user
/// is actively editing (so expensive analysis debounces). Pure caret
/// moves (`sel`), hover, and widget/UI messages are *not* edits — they
/// leave the phase alone so overlays stay put during navigation.
///
/// [`DecoPhase::Structural`]: editor_state::DecoPhase::Structural
/// Cheap structural hash of one line patch, used by the incremental
/// patcher to find the first changed line. Position-inclusive (the
/// patch embeds `data-tile-pos`), so an edit busts the hash of the
/// edited line and every line after it (their offsets shifted).
#[cfg(not(feature = "native"))]
fn hash_patch(p: &crate::tile::patch::Patch) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    h.finish()
}

fn is_edit_kind(kind: &str) -> bool {
    matches!(
        kind,
        "input"
            | "before-input-insert"
            | "before-input-delete-backward"
            | "before-input-delete-forward"
            | "insert-bracket"
            | "enter-continue-list"
            | "composition-end"
            | "task-toggle"
            | "prop-set"
            | "prop-add"
            | "prop-remove"
            | "prop-list-add"
            | "prop-list-remove"
    )
}

/// Wall-clock milliseconds. wasm-safe: `Instant::now()` traps on
/// `wasm32-unknown-unknown`, so we hop through `performance.now()`
/// there. Used by perf-trace spans.
#[cfg(all(not(target_arch = "wasm32"), not(feature = "native")))]
fn now_ms() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let s = START.get_or_init(std::time::Instant::now);
    s.elapsed().as_secs_f64() * 1000.0
}
/// Synchronously query the DOM for "is a decoration widget
/// cell currently focused?". Used by the editor's keydown
/// dispatch to bail out before any vim/keymap/slash handler
/// fires, so cell-owned keystrokes (Mod-A inside a property
/// text cell, Enter in a chip-add input, etc.) aren't double-
/// handled by the doc.
///
/// The JS-side `focusin`/`focusout` listeners flip
/// `dataset.widgetFocused` on the editor root the moment focus
/// crosses into / out of an `[data-edit-role]` element. We
/// read that attribute here from Rust via `web_sys` — same
/// thread, same tick, no bridge latency.
#[cfg(target_arch = "wasm32")]
fn widget_focused_dom() -> bool {
    use web_sys::wasm_bindgen::JsCast;
    let Some(win) = web_sys::window() else {
        return false;
    };
    let Some(doc) = win.document() else {
        return false;
    };
    let Some(el) = doc.query_selector("[data-editor-id]").ok().flatten() else {
        return false;
    };
    let html: web_sys::HtmlElement = match el.dyn_into() {
        Ok(h) => h,
        Err(_) => return false,
    };
    html.dataset().get("widgetFocused").is_some()
}
#[cfg(not(target_arch = "wasm32"))]
fn widget_focused_dom() -> bool {
    false
}

/// One-shot device-class probe: is the primary pointer coarse (a
/// touchscreen phone / tablet)? Hosts consult this ONCE at page
/// mount to decide whether to wire vim modal editing — soft
/// keyboards have no Esc key, so landing in Normal mode on a touch
/// device is a trap (letters become motions and "typing looks
/// broken"). Pass `vim: None` when this returns `true`.
///
/// Deliberately not reactive: a convertible flipping between
/// laptop and tablet mid-session keeps whatever it mounted with —
/// swapping the modal-editing model under the user's fingers would
/// be worse than either steady state.
#[cfg(target_arch = "wasm32")]
#[must_use]
pub fn coarse_pointer() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(pointer: coarse)").ok().flatten())
        .is_some_and(|m| m.matches())
}
/// Non-wasm builds (desktop webview, Blitz native) have no
/// `matchMedia` to ask synchronously; they're keyboard-first
/// environments, so report a fine pointer.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn coarse_pointer() -> bool {
    false
}

#[cfg(all(target_arch = "wasm32", not(feature = "native")))]
fn now_ms() -> f64 {
    // Cheap call — browsers cache it. Falls back to 0 if for
    // some reason there's no `performance` (Workers etc.).
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}

/// `keymap` is optional. When `None` the browser handles every
/// key. When `Some`, each `onkeydown` looks for a matching
/// binding whose command returns `Some(spec)` and we
/// `preventDefault` + apply. Unmatched keys fall through.
#[component]
pub fn Editor(
    state: Signal<EditorState>,
    #[props(default)] keymap: Option<Keymap>,
    #[props(default)] decorations: Option<DecorationSource>,
    /// Optional vim modal state. When `Some`, every keydown is
    /// dispatched to `editor_vim::handle_key` first; the result
    /// (if any) is applied and preventDefault'd. Returning
    /// `None` from vim falls through to the keymap and normal
    /// browser-side text input.
    #[props(default)]
    vim: Option<Signal<editor_vim::VimState>>,
    /// Optional slash-command menu state. When `Some`, the
    /// editor watches doc changes and refreshes the open state
    /// via `slash::detect_slash`. Arrow keys, Enter, and Escape
    /// route into the menu when it's open. Owner is responsible
    /// for rendering the menu component itself — the editor just
    /// keeps the state in sync with the doc.
    #[props(default)]
    slash: Option<Signal<Option<crate::slash::SlashState>>>,
    /// Optional hover-tooltip source. When `Some`, the editor tracks the
    /// pointer (debounced), resolves it to a document offset, and calls this
    /// source; a returned [`editor_state::HoverTooltip`] is shown as a floating
    /// panel. Mirrors CM6's `hoverTooltip` — keyflow plugs `keyflow_hover` here.
    #[props(default)]
    hover: Option<editor_state::HoverSource>,
    /// Optional transaction sink. When `Some`, every transaction the
    /// editor itself applies — bridge input, keymap commands, vim,
    /// slash palette, trigger completion — is mirrored out as a
    /// [`crate::TransactionEvent`] after it lands in `state`. Built
    /// for CRDT / persistence hosts: the event carries the `Changes`,
    /// the `user_event` tag (echo-guard convention: hosts tag remote
    /// applications with `"remote"`), and the doc snapshots either
    /// side of the edit. Host-driven `state.set(..)` from outside the
    /// editor does NOT emit.
    #[props(default)]
    on_transaction: Option<Callback<crate::TransactionEvent>>,
    /// Optional trigger-autocomplete candidate source. When `Some`,
    /// the editor watches the caret for open `[[` wikilink and `#`
    /// tag triggers, asks this source for candidates (`(query,
    /// kind) -> Vec<Candidate>` — the host owns the vault/tag
    /// index; the editor stays vault-agnostic), and renders the
    /// completion popup itself. Arrow keys / Enter / Escape route
    /// into the menu while it's open; accepting splices `[[Name]]`
    /// / `#tag` through the normal transaction path (and therefore
    /// the `on_transaction` sink).
    #[props(default)]
    completion: Option<crate::trigger::CompletionSource>,
    /// When `false` the editor renders read-only: `contenteditable`
    /// off (the reading-mode root class applies) and the keydown
    /// dispatch (vim / keymap / slash) is disabled, so nothing can
    /// mutate the doc. Pair with `state.reading_mode = true` when the
    /// live-preview pass should also keep every source marker hidden
    /// regardless of caret position. Link and `[[wikilink]]` clicks
    /// still fire `on_link_click`. Defaults to `true` (a normal
    /// editable editor).
    #[props(default = true)]
    editable: bool,
    /// Fired with the raw `data-href` when the user clicks a rendered
    /// link or `[[wikilink]]` span (for wikilinks that's the link body,
    /// e.g. `Page`, `Page#Heading`, `Page|alias`). External `http(s)`
    /// links are additionally opened in a new tab by the view itself;
    /// wikilinks only fire this — the host owns vault navigation.
    #[props(default)]
    on_link_click: Option<Callback<String>>,
) -> Element {
    // True when a decoration widget cell currently has focus
    // (frontmatter property contenteditable, chip-add box, etc.).
    // Flipped via JS focusin/focusout bridge messages so the
    // Rust-side `on_keydown` can bail entirely while the cell
    // owns the keyboard — otherwise Dioxus's document-level
    // delegation would still fire commands like `Mod-A` against
    // the whole doc even when we `stopPropagation` in capture
    // phase on the editor root.
    let widget_focus = use_signal(|| false);
    // Undo/redo history. Provided as context so `apply_tx` (the
    // single transaction choke point) can record every edit and
    // resolve the `"undo"`/`"redo"`-tagged specs vim and keymaps
    // emit — without threading a parameter through 25 call sites.
    use_context_provider(|| Signal::new(editor_state::History::new()));
    // True while the editor root (or a descendant) holds DOM focus.
    // Gates the painted modal caret — like the native caret, it must
    // not render on an unfocused editor.
    //
    // Native starts TRUE: the root requests `autofocus`, and Blitz's
    // autofocus path calls `set_focus_to` directly WITHOUT dispatching
    // a focusin event — waiting for `onfocusin` would leave the signal
    // false forever and no caret would ever paint (Blitz also only
    // moves focus on click for text INPUTS, so clicking the editor
    // fires no focusin either). Web keeps the event-driven default.
    let editor_focused = use_signal(|| cfg!(feature = "native"));
    // Trigger-autocomplete open state (`[[` / `#`). Owned by the
    // editor (unlike `slash`, which the host threads in) because the
    // host's only contract is the candidate source prop.
    let completion_state = use_signal(|| None::<crate::trigger::CompletionState>);
    // Hover-tooltip popup (resolved content + anchor coords). Driven by the
    // `hover`/`hover-end` bridge messages below; rendered as a floating panel.
    let hover_state = use_signal(|| None::<crate::hover::HoverPopup>);
    // Whether editing has settled. `false` from the first text mutation
    // until the JS-side `idle` ping fires (~220ms after the last input);
    // the patch effect reads it to ask decoration sources for the cheap
    // `Structural` pass while typing and the `Full` pass once idle, so
    // expensive language analysis (diagnostics, overlays) debounces
    // instead of running on every keystroke. Starts `true` so the first
    // render shows the full set.
    let idle = use_signal(|| true);
    // Incremental patching: per-line content hashes from the last patch
    // shipped to JS. Each render diffs the new line hashes against these
    // to find the first changed line, then ships only the suffix from
    // there — the unchanged prefix stays untouched in the DOM. (The hash
    // is position-inclusive, so an edit on line K busts K and every line
    // after it, whose absolute positions shifted.)
    // (Native renders rsx directly and has no JS patch step, so this hash
    // cache goes unused there — kept as a hook so hook order matches web.)
    #[cfg_attr(feature = "native", allow(unused_variables))]
    let prev_line_hashes = use_signal(Vec::<u64>::new);
    // Force the next patch to be a full (prefix=0) reconcile. Set on the
    // first render and after IME composition — the one path where
    // `applyPatch` skips applying (so the JS DOM can drift from our cached
    // hashes and must be resynced with a complete patch).
    let force_full_patch = use_signal(|| true);
    // Tile-tree build moved into the imperative-patch
    // `use_effect` below. The component body itself no longer
    // computes a render — it just allocates the editor id and
    // schedules effects.
    let editor_id = use_hook(|| {
        let n = EDITOR_INSTANCE.fetch_add(1, Ordering::Relaxed);
        format!("editor-{n}")
    });

    // ── DOM → state: MutationObserver + selection bridge ─────────
    //
    // Ports CM6's `view/src/domobserver.ts` at v1 scope. A
    // `MutationObserver` on the editor root catches every kind
    // of edit (typing, paste, drag-drop, IME) — broader than
    // the `input` event we used before. On each mutation batch
    // we read the current textContent + selection and send to
    // Rust for diff + Transaction.
    //
    // Composition handling: between `compositionstart` and
    // `compositionend` we *skip* mutation handling — the IME's
    // intermediate states aren't useful and would corrupt the
    // doc mid-composition. On `compositionend` we flush a
    // single update with the final text. (CM6 does the same
    // pause-resume pattern.)
    //
    // Selection-only moves (keyup/mouseup/select/focus) still
    // flow via `sel` messages as in Phase 7.
    {
        let id = editor_id.clone();
        // Capture the decoration source so the spawn closure
        // can rebuild the tile tree + visible-text mirror when
        // diffing each input message.
        let deco_source = decorations.clone();
        // Hover source + popup signal captured for the recv loop below.
        let hover_source = hover;
        // Transaction sink mirrored to the host on every applied
        // transaction (Callback is Copy — cheap to capture).
        let sink = on_transaction;
        let mut hover_sig = hover_state;
        // Editing-settled flag, flipped by the recv loop on edits / `idle`.
        let mut idle_sig = idle;
        // Forces the next patch to a full reconcile — set on composition
        // boundaries so the incremental prefix-skip resyncs with the DOM
        // (the patcher skips applying while an IME composition is active).
        let mut force_full_sig = force_full_patch;
        use_hook(move || {
            spawn(async move {
                let script = format!(
                    r#"
                    (function() {{
                        function attach() {{
                            const el = document.querySelector('[data-editor-id="{id}"]');
                            if (!el) {{ setTimeout(attach, 30); return; }}
                            // Tile-tree-aware DOM → doc offset
                            // translation. Walks up from the
                            // selection's anchor/focus to the
                            // nearest ancestor carrying
                            // `data-tile-pos` (set by render_dx.rs),
                            // reads that as the tile's start
                            // position in the doc, and adds the
                            // text-node offset within the tile.
                            //
                            // Replaces the old TreeWalker-based
                            // approach which assumed visible-text
                            // offset == doc offset — wrong as soon
                            // as Hidden (Replace) decorations exist.
                            // CM6's equivalent walk is in
                            // `view/src/docview.ts:282` (posFromDOM).
                            // Rust doc positions are UTF-8 byte
                            // offsets. JS string offsets are UTF-16
                            // code units. ASCII coincides, but
                            // anything outside Latin-1 (em-dash,
                            // emoji, CJK, …) makes them diverge.
                            // Convert at the boundary.
                            function utf16ToBytes(str, off) {{
                                let bytes = 0;
                                let i = 0;
                                while (i < off) {{
                                    const c = str.charCodeAt(i);
                                    if (c < 0x80) {{ bytes += 1; i += 1; }}
                                    else if (c < 0x800) {{ bytes += 2; i += 1; }}
                                    else if (c >= 0xD800 && c <= 0xDBFF) {{ bytes += 4; i += 2; }}
                                    else {{ bytes += 3; i += 1; }}
                                }}
                                return bytes;
                            }}
                            function bytesToUtf16(str, bytes) {{
                                let b = 0;
                                let i = 0;
                                while (i < str.length && b < bytes) {{
                                    const c = str.charCodeAt(i);
                                    if (c < 0x80) {{ b += 1; i += 1; }}
                                    else if (c < 0x800) {{ b += 2; i += 1; }}
                                    else if (c >= 0xD800 && c <= 0xDBFF) {{ b += 4; i += 2; }}
                                    else {{ b += 3; i += 1; }}
                                }}
                                return i;
                            }}
                            function utf8Len(str) {{ return utf16ToBytes(str, str.length); }}

                            function tilePosOf(node) {{
                                let n = node;
                                while (n && n !== el) {{
                                    if (n.nodeType === 1 /* ELEMENT */
                                        && n.dataset && n.dataset.tilePos != null) {{
                                        return parseInt(n.dataset.tilePos, 10);
                                    }}
                                    n = n.parentNode;
                                }}
                                return 0; // fall back to doc start
                            }}
                            // Sum visible chars contributed by `node`
                            // and all descendants. Doesn't include
                            // line-break "\n"s — those live between
                            // sibling `.cm-line` divs, and posFromDOM
                            // only descends *inside* one node.
                            function visTextLen(node) {{
                                if (!node) return 0;
                                if (node.nodeType === 3) return node.nodeValue.length;
                                let n = 0;
                                for (const c of node.childNodes) n += visTextLen(c);
                                return n;
                            }}
                            // Convert a (container, offset) pair to a
                            // doc-space position. Mirrors CM6's
                            // `posFromDOM` (`docview.ts:282`).
                            //
                            // - Text node: ancestor's data-tile-pos
                            //   + offset within text. The simple
                            //   case.
                            // - Element node: offset is a *child
                            //   index*. The cursor is just before
                            //   childNodes[offset]; we recurse into
                            //   either that child (offset 0) or the
                            //   previous sibling's end (offset > 0).
                            //   Browsers place the cursor at element
                            //   level when the click hits non-text
                            //   space inside a line div.
                            // Sum the doc-length each prior sibling
                            // of `node` contributes to its parent tile.
                            // Used to anchor positions when the
                            // browser has split text nodes within a
                            // line (typing inside an auto-paired
                            // bracket, IME composition, paste).
                            function priorSiblingBytes(node) {{
                                let bytes = 0;
                                let n = node.previousSibling;
                                while (n) {{
                                    if (n.nodeType === 3) {{
                                        bytes += utf8Len(n.nodeValue);
                                    }} else if (n.nodeType === 1) {{
                                        // Widget tile carries its doc
                                        // length explicitly; everything
                                        // else (mark spans, line
                                        // children) sums via recursion.
                                        if (n.dataset && n.dataset.tileLen != null) {{
                                            bytes += parseInt(n.dataset.tileLen, 10);
                                        }} else {{
                                            bytes += utf8Len(visText(n));
                                        }}
                                    }}
                                    n = n.previousSibling;
                                }}
                                return bytes;
                            }}
                            function posFromDOM(container, offset) {{
                                if (!container) return 0;
                                if (container.nodeType === 3) {{
                                    return tilePosOf(container)
                                        + priorSiblingBytes(container)
                                        + utf16ToBytes(container.nodeValue, offset);
                                }}
                                const kids = container.childNodes;
                                // Element-level "after last child":
                                // if the container itself is a
                                // tile (`data-tile-pos` +
                                // `data-tile-len`), return its
                                // END position. This accounts for
                                // Hidden ranges whose bytes have
                                // no visible characters but still
                                // occupy doc space (list markers,
                                // task markers, code fences).
                                // Without this, sendSel reports
                                // the widget's start position and
                                // state.selection collapses onto
                                // the *before-hidden* doc offset.
                                if (kids.length === 0
                                    && container.dataset
                                    && container.dataset.tilePos != null) {{
                                    const p = parseInt(container.dataset.tilePos, 10);
                                    const l = parseInt(container.dataset.tileLen || '0', 10);
                                    return p + (offset >= 1 ? l : 0);
                                }}
                                if (kids.length === 0) return tilePosOf(container);
                                if (offset >= kids.length) {{
                                    if (container.dataset
                                        && container.dataset.tilePos != null
                                        && container.dataset.tileLen != null) {{
                                        return parseInt(container.dataset.tilePos, 10)
                                            + parseInt(container.dataset.tileLen, 10);
                                    }}
                                    const last = kids[kids.length - 1];
                                    if (last.nodeType === 3) {{
                                        return tilePosOf(last) + utf8Len(last.nodeValue);
                                    }}
                                    let n = last;
                                    while (n.nodeType === 1 && n.lastChild) n = n.lastChild;
                                    if (n.nodeType === 3) {{
                                        return tilePosOf(n) + utf8Len(n.nodeValue);
                                    }}
                                    return tilePosOf(last);
                                }}
                                const next = kids[offset];
                                if (next.nodeType === 3) return tilePosOf(next);
                                let n = next;
                                while (n.nodeType === 1 && n.firstChild) n = n.firstChild;
                                if (n.nodeType === 3) return tilePosOf(n);
                                return tilePosOf(next);
                            }}
                            function selOffsets() {{
                                const s = window.getSelection();
                                if (!s || s.rangeCount === 0) return [0, 0];
                                // anchor/focus carry direction;
                                // start/end always sort. We send
                                // [anchor, head] so backward
                                // selections survive the round trip.
                                if (s.anchorNode && el.contains(s.anchorNode)) {{
                                    const a = posFromDOM(s.anchorNode, s.anchorOffset);
                                    const h = posFromDOM(s.focusNode, s.focusOffset);
                                    return [a, h];
                                }}
                                const r = s.getRangeAt(0);
                                const a = posFromDOM(r.startContainer, r.startOffset);
                                const b = posFromDOM(r.endContainer, r.endOffset);
                                return [a, b];
                            }}
                            // Reconstruct doc text from the
                            // tile-tree-rendered DOM. Each LineTile
                            // renders as `<div class="cm-line">`;
                            // BreakAfter on a line means there's
                            // a `\n` between it and the next line.
                            // textContent alone *doesn't* include
                            // those newlines (it just concats text
                            // descendants), so naively reading
                            // textContent would drop every `\n` in
                            // the doc — diffs would then think
                            // the user deleted every newline on
                            // every keystroke.
                            // Reconstruct doc text from the
                            // tile-tree-rendered DOM, skipping any
                            // widget content (`contenteditable=
                            // false` spans rendered as decorations
                            // — checkboxes, bullet markers, code-
                            // block lang labels, copy buttons).
                            // Without the skip, widget characters
                            // leak into `textContent`, the input
                            // diff treats them as user-typed bytes
                            // and tries to insert them into
                            // state.doc.
                            function visText(node) {{
                                if (!node) return '';
                                if (node.nodeType === 3) return node.nodeValue;
                                if (node.nodeType !== 1) return '';
                                if (node.classList && (
                                    node.classList.contains('editor-widget')
                                    || node.classList.contains('cm-widgetBuffer'))) {{
                                    return '';
                                }}
                                let s = '';
                                for (const c of node.childNodes) s += visText(c);
                                return s;
                            }}
                            function readText() {{
                                const lines = el.querySelectorAll('.cm-line');
                                if (!lines.length) return visText(el);
                                return Array.from(lines).map(visText).join('\n');
                            }}
                            function sendInput() {{
                                const t0 = window.__cm_perf ? performance.now() : 0;
                                const [a, b] = selOffsets();
                                const text = readText();
                                if (window.__cm_perf) {{
                                    window.__cm_perf_summary.inputs += 1;
                                    const dt = performance.now() - t0;
                                    window.__cm_perf_summary.totalInputMs += dt;
                                    console.log(
                                        `[cm/perf] sendInput ${{dt.toFixed(2)}}ms`,
                                        'textLen=' + text.length + ' sel=' + a + ',' + b
                                    );
                                }}
                                dioxus.send({{
                                    kind: 'input',
                                    text: text,
                                    sel: [a, b]
                                }});
                            }}
                            function sendSel() {{
                                // Skip during programmatic
                                // writes (Phase 10) and during
                                // the brief window after a DOM
                                // mutation where Selection may
                                // be orphaned and unreliable.
                                if (el.dataset.writing === '1') return;
                                if (el.dataset.muting === '1') return;
                                const [a, b] = selOffsets();
                                dioxus.send({{ kind: 'sel', sel: [a, b] }});
                            }}

                            // ── (dead) sync bracket helper ──
                            // Kept around for the perf-trace refactor
                            // — the sync path raced the patcher
                            // (re-render replaces text nodes, cursor
                            // reference lost), so brackets route
                            // through Rust today. The helpers below
                            // are still referenced by other code
                            // paths (peekNextChar etc.).
                            const OPENERS = {{ '(': ')', '[': ']', '{{': '}}' }};
                            const SAME = new Set(["'", '"', '`']);
                            const CLOSE_BEFORE = ')]}}>;,:';
                            function handleBracketSync(ch) {{
                                const sel = window.getSelection();
                                if (!sel || sel.rangeCount === 0) return false;
                                const r = sel.getRangeAt(0);
                                if (!el.contains(r.startContainer)) return false;
                                const isOpener = ch in OPENERS;
                                const isSame = SAME.has(ch);
                                const isCloser = !isOpener && !isSame
                                    && (ch === ')' || ch === ']' || ch === '}}');
                                if (!isOpener && !isSame && !isCloser) return false;

                                const collapsed = r.collapsed;
                                // For closer: skip-over if next char is the same.
                                if (isCloser && collapsed) {{
                                    const next = peekNextChar(r);
                                    if (next === ch) {{
                                        moveCursor(r, +1);
                                        return true;
                                    }}
                                    return false;
                                }}
                                // For same-char (quote): skip if next is the same.
                                if (isSame && collapsed) {{
                                    const next = peekNextChar(r);
                                    if (next === ch) {{
                                        moveCursor(r, +1);
                                        return true;
                                    }}
                                    // Only auto-pair when surrounded by
                                    // non-word chars / EOL.
                                    const prev = peekPrevChar(r);
                                    const wordRe = /[\w]/;
                                    if ((prev && wordRe.test(prev))
                                        || (next && wordRe.test(next))) {{
                                        return false;
                                    }}
                                }}
                                // For opener: check the "before" rule.
                                if (isOpener && collapsed) {{
                                    const next = peekNextChar(r);
                                    if (next && !/^\s$/.test(next)
                                        && !CLOSE_BEFORE.includes(next)) {{
                                        return false;
                                    }}
                                    // Suppress `[` auto-pair when
                                    // the user looks like they're
                                    // typing a task marker `- [`
                                    // / `* [` / `+ [` — the
                                    // closing `]` they type next
                                    // would otherwise be dropped
                                    // by skip-past, leaving a
                                    // stray `]` later.
                                    if (ch === '[') {{
                                        const lineText = enclosingLineText(r);
                                        if (lineText && /^[ ]*[-*+] $/.test(lineText)) {{
                                            return false;
                                        }}
                                    }}
                                }}
                                // Build the pair text. For same-char,
                                // the closer IS the opener.
                                const close = isOpener ? OPENERS[ch] : ch;
                                if (!collapsed) {{
                                    // Wrap selection.
                                    const text = r.toString();
                                    r.deleteContents();
                                    r.insertNode(document.createTextNode(ch + text + close));
                                    // Place caret at the end of the inserted (post-close).
                                    return true;
                                }}
                                // Empty caret — insert pair, leave caret between.
                                const pair = document.createTextNode(ch + close);
                                r.insertNode(pair);
                                const newRange = document.createRange();
                                newRange.setStart(pair, 1);
                                newRange.collapse(true);
                                sel.removeAllRanges();
                                sel.addRange(newRange);
                                // sendInput's diff will compute
                                // an intended-caret at the END of
                                // the inserted pair. We want the
                                // caret BETWEEN. Send a follow-up
                                // `sel` so once the input
                                // transaction commits, the
                                // selection lands where we want.
                                // Queued FIFO after the input msg,
                                // so the final state matches.
                                queueMicrotask(() => {{
                                    const [a, b] = selOffsets();
                                    dioxus.send({{ kind: 'sel', sel: [a, b] }});
                                }});
                                return true;
                            }}
                            // Return the visible source bytes of
                            // the line containing `r.startContainer`
                            // up to (but not including) the caret.
                            // Used by the bracket auto-pair to
                            // decide whether `[` should be a task-
                            // marker opener instead of a pair.
                            function enclosingLineText(r) {{
                                let n = r.startContainer;
                                while (n && n !== el && !(n.classList
                                    && n.classList.contains('cm-line'))) {{
                                    n = n.parentNode;
                                }}
                                if (!n || n === el) return '';
                                return visText(n);
                            }}
                            function peekNextChar(r) {{
                                const n = r.startContainer;
                                if (n.nodeType === 3) {{
                                    return n.nodeValue[r.startOffset] || '';
                                }}
                                const kid = n.childNodes[r.startOffset];
                                if (kid && kid.nodeType === 3) return kid.nodeValue[0] || '';
                                return '';
                            }}
                            function peekPrevChar(r) {{
                                const n = r.startContainer;
                                if (n.nodeType === 3) {{
                                    return r.startOffset > 0 ? n.nodeValue[r.startOffset - 1] : '';
                                }}
                                if (r.startOffset > 0) {{
                                    const prev = n.childNodes[r.startOffset - 1];
                                    if (prev && prev.nodeType === 3) {{
                                        return prev.nodeValue[prev.nodeValue.length - 1] || '';
                                    }}
                                }}
                                return '';
                            }}
                            function moveCursor(r, delta) {{
                                const n = r.startContainer;
                                if (n.nodeType !== 3) return;
                                const newRange = document.createRange();
                                newRange.setStart(n, r.startOffset + delta);
                                newRange.collapse(true);
                                const s = window.getSelection();
                                s.removeAllRanges();
                                s.addRange(newRange);
                            }}

                            // Composition guard. Mirrors CM6's
                            // pause-during-composition pattern:
                            // - sendInput skipped while composing
                            // - state→DOM selection writeback
                            //   also reads this flag (via the
                            //   `data-composing` attribute on the
                            //   root) and bails so it doesn't
                            //   fight the IME for the Selection
                            // - compositionend flushes one
                            //   sendInput with the final text
                            // - compositionend also notifies Rust
                            //   via a typed message so any
                            //   composition-aware code can react
                            let composing = false;
                            el.addEventListener('compositionstart', () => {{
                                composing = true;
                                el.dataset.composing = '1';
                                dioxus.send({{ kind: 'composition-start' }});
                            }});
                            el.addEventListener('compositionend', () => {{
                                composing = false;
                                delete el.dataset.composing;
                                sendInput();
                                dioxus.send({{ kind: 'composition-end' }});
                            }});

                            // MutationObserver — catches every
                            // kind of DOM change (typing, paste,
                            // drag-drop, IME). CM6 reference:
                            // `view/src/domobserver.ts:103+`
                            // (the `observe` method).
                            //
                            // beforeinput interception — ports
                            // CM6's `view/src/domchange.ts`
                            // strategy of authoring edits
                            // ourselves rather than reading the
                            // DOM back after a browser-chosen
                            // mutation. For inputType events we
                            // can map cleanly (Enter, Backspace,
                            // typed chars on some IMEs), we
                            // preventDefault and send a typed
                            // message; Rust applies the Change
                            // and Dioxus re-renders the DOM the
                            // way *we* want it (e.g., a new
                            // LineTile div, not a plain <br>).
                            //
                            // Inputs we don't recognize fall
                            // through to the MutationObserver
                            // path below.
                            // CM6's strategy: intercept EVERY
                            // beforeinput event we can map
                            // cleanly. preventDefault, send the
                            // authored Change to Rust, let our
                            // reconciler put the result in the
                            // DOM. The browser never gets to
                            // modify our contenteditable — which
                            // eliminates the "browser writes a
                            // shape we have to reverse-engineer"
                            // problem completely.
                            //
                            // For inputTypes we don't recognize
                            // (e.g., insertFromPaste with rich
                            // HTML, formatBold/Italic, etc.) we
                            // fall through to the
                            // MutationObserver path, which reads
                            // the resulting DOM and computes a
                            // diff. That's a degraded path —
                            // works but exposes the empty-span
                            // bug — so the surface of cleanly-
                            // mapped types should grow over time.
                            // beforeinput interception is SCOPED
                            // to the inputTypes where the
                            // browser's default behavior is
                            // problematic (Enter creates non-
                            // portable DOM, and the "type into
                            // an empty Mark span" case crashes
                            // our reconciler). For ordinary
                            // typing we let the browser handle
                            // it and reconstruct via the
                            // MutationObserver — that path has
                            // years of test coverage and works
                            // well across decoration shapes.
                            //
                            // The compromise: we get correct
                            // Enter handling + a workaround for
                            // the empty-span case without
                            // redirecting *all* user typing
                            // through Rust (which exposes
                            // selOffsets edge cases we haven't
                            // fully chased).
                            el.addEventListener('beforeinput', evt => {{
                                if (composing) return;
                                const t = evt.inputType;
                                if (t === 'insertParagraph' || t === 'insertLineBreak') {{
                                    evt.preventDefault();
                                    // Route through the list-aware
                                    // Enter handler so pressing
                                    // Enter on a `- foo` / `1. foo`
                                    // / `- [x] foo` line continues
                                    // the list. Falls back to a
                                    // plain `\n` insert if the
                                    // line isn't a list item.
                                    dioxus.send({{
                                        kind: 'enter-continue-list',
                                        sel: selOffsets(),
                                    }});
                                    return;
                                }}
                                // Cursor inside an empty Mark
                                // span: the most common path to
                                // the "Dioxus reshape crash"
                                // bug. Intercept insertText here
                                // ONLY for this case — the empty
                                // span is the destabilizing
                                // factor, not typing in general.
                                if (t === 'insertText' && typeof evt.data === 'string') {{
                                    // Bracket / quote auto-pair —
                                    // routed through Rust. Sync-
                                    // in-JS would race the
                                    // re-render that follows; the
                                    // patcher replaces the
                                    // browser-inserted text node
                                    // with a fresh span and the
                                    // cursor reference is lost.
                                    if (/^[\(\[\{{\)\]\}}\'\"`]$/.test(evt.data)) {{
                                        evt.preventDefault();
                                        dioxus.send({{
                                            kind: 'insert-bracket',
                                            text: evt.data,
                                            sel: selOffsets(),
                                        }});
                                        return;
                                    }}
                                    const s = window.getSelection();
                                    let inEmptyMark = false;
                                    if (s && s.rangeCount > 0) {{
                                        let n = s.anchorNode;
                                        while (n && n !== el) {{
                                            // Properly-parenthesized:
                                            // "element with mark class AND
                                            //  (has no children OR has only
                                            //   an empty text node)".
                                            const isMark = n.nodeType === 1
                                                && n.classList
                                                && (n.classList.contains('md-bold')
                                                    || n.classList.contains('md-italic')
                                                    || n.classList.contains('md-code'));
                                            if (isMark) {{
                                                const c = n.firstChild;
                                                const isEmpty = !c
                                                    || (c.nodeType === 3
                                                        && c.nodeValue === '');
                                                if (isEmpty) {{
                                                    inEmptyMark = true;
                                                    break;
                                                }}
                                            }}
                                            n = n.parentNode;
                                        }}
                                    }}
                                    if (inEmptyMark) {{
                                        evt.preventDefault();
                                        dioxus.send({{
                                            kind: 'before-input-insert',
                                            text: evt.data,
                                            sel: selOffsets(),
                                        }});
                                    }}
                                }}
                            }});

                            // MutationObserver — full re-read on
                            // every mutation. The `muting` flag
                            // it sets ALSO suppresses
                            // selectionchange-driven sendSel for
                            // one frame, because Dioxus's
                            // node-replace renders (driven by
                            // decoration shape changes) orphan
                            // DOM Selection and emit a bogus
                            // selectionchange BEFORE the
                            // writeback effect can resync. That
                            // bogus event would otherwise
                            // clobber state.selection with the
                            // orphaned position.
                            // CM6's `domobserver.ignore(f)`
                            // pattern: programmatic DOM writes
                            // call `observer.disconnect()`
                            // first, run their mutations, then
                            // `observe()` again — DISCARDING any
                            // queued records via takeRecords().
                            // This is more reliable than a
                            // muting flag because it stops the
                            // observer from queueing at all,
                            // eliminating the race where the
                            // observer's callback could fire
                            // mid-write.
                            const observeOpts = {{
                                childList: true,
                                characterData: true,
                                subtree: true,
                            }};
                            // Skip mutation records whose target is
                            // inside a widget tile (`.editor-widget`).
                            // Widgets host their own contenteditable
                            // inputs (frontmatter property cells,
                            // chip-add boxes) and rendered SVGs whose
                            // text nodes would otherwise be read into
                            // the doc's textContent and confuse the
                            // diff. The DOM patcher writes inside
                            // widgets all the time (re-render);
                            // forwarding those to `sendInput` would
                            // mistakenly try to apply the widget
                            // contents as a doc edit.
                            const isWidgetMutation = (rec) => {{
                                let n = rec.target;
                                while (n && n !== el) {{
                                    if (n.nodeType === 1
                                        && n.classList
                                        && n.classList.contains('editor-widget')) {{
                                        return true;
                                    }}
                                    n = n.parentNode;
                                }}
                                return false;
                            }};
                            const mo = new MutationObserver(records => {{
                                if (composing) return;
                                if (records.every(isWidgetMutation)) return;
                                el.dataset.muting = '1';
                                requestAnimationFrame(() => {{
                                    delete el.dataset.muting;
                                }});
                                sendInput();
                            }});
                            mo.observe(el, observeOpts);

                            // ── Imperative DOM patcher ──────
                            // Bypasses Dioxus's reconciler for
                            // the editor's contents. CM6 does
                            // this for the same reasons: VDOMs
                            // can't safely manage a
                            // contenteditable across the kinds
                            // of structural transitions
                            // decoration-aware rendering needs.
                            //
                            // `applyPatch(descs)` diffs the
                            // serialized tile tree against the
                            // live DOM and applies minimal
                            // mutations. Wrapped in
                            // disconnect/observe so the
                            // MutationObserver doesn't see our
                            // own writes.
                            function patchAttrs(elem, attrs) {{
                                const want = new Set();
                                for (const [k, v] of attrs) {{
                                    want.add(k);
                                    if (k === 'data-widget-html') {{
                                        if (elem.innerHTML !== v) elem.innerHTML = v;
                                    }} else if (elem.getAttribute(k) !== v) {{
                                        elem.setAttribute(k, v);
                                    }}
                                }}
                                // Remove attrs not in want.
                                for (let i = elem.attributes.length - 1; i >= 0; i--) {{
                                    const name = elem.attributes[i].name;
                                    if (!want.has(name)) {{
                                        elem.removeAttribute(name);
                                    }}
                                }}
                            }}

                            function patchChildren(parent, descs, i0) {{
                                // Two-pass: collect existing
                                // children, then walk desired
                                // descs and either reuse-by-key,
                                // reuse-by-tag-at-position, or
                                // create-new. Trailing extras
                                // get removed.
                                //
                                // `i0` lets the top-level (line) call
                                // start at the first changed line —
                                // the incremental patch ships only
                                // `descs` from that index on, so we
                                // walk/reconcile from there and leave
                                // the unchanged prefix lines untouched.
                                // Nested calls (line content) pass no
                                // `i0` and reconcile from 0.
                                let i = i0 || 0;
                                for (const d of descs) {{
                                    if (d.text !== undefined) {{
                                        const at = parent.childNodes[i];
                                        if (at && at.nodeType === 3) {{
                                            if (at.nodeValue !== d.text) {{
                                                // Chromium collapses
                                                // the Selection when
                                                // a Text node's data
                                                // is reassigned via
                                                // `nodeValue =` /
                                                // `data =`. Use
                                                // `replaceData` for
                                                // the minimal diff
                                                // so the cursor
                                                // stays where it
                                                // visually is.
                                                const oldT = at.nodeValue;
                                                const newT = d.text;
                                                let pre = 0;
                                                const maxPre = Math.min(oldT.length, newT.length);
                                                while (pre < maxPre
                                                    && oldT.charCodeAt(pre) === newT.charCodeAt(pre))
                                                    pre++;
                                                let suf = 0;
                                                const maxSuf = Math.min(
                                                    oldT.length - pre,
                                                    newT.length - pre
                                                );
                                                while (suf < maxSuf
                                                    && oldT.charCodeAt(oldT.length - 1 - suf)
                                                       === newT.charCodeAt(newT.length - 1 - suf))
                                                    suf++;
                                                const oldMid = oldT.length - pre - suf;
                                                const insMid = newT.substring(pre, newT.length - suf);
                                                at.replaceData(pre, oldMid, insMid);
                                            }}
                                        }} else {{
                                            const tn = document.createTextNode(d.text);
                                            parent.insertBefore(tn, at || null);
                                        }}
                                        i++;
                                        continue;
                                    }}
                                    const tag = d.tag;
                                    let key = null;
                                    for (const [k, v] of d.attrs) {{
                                        if (k === 'data-tile-pos') {{ key = v; break; }}
                                    }}
                                    // Try the element at `i`.
                                    let cand = parent.childNodes[i];
                                    if (cand
                                        && cand.nodeType === 1
                                        && cand.tagName.toLowerCase() === tag
                                        && (key == null
                                            || cand.dataset.tilePos === key)) {{
                                        patchAttrs(cand, d.attrs);
                                        if (tag !== 'br'
                                            && !d.attrs.some(([k]) => k === 'data-widget-html')) {{
                                            patchChildren(cand, d.kids || []);
                                        }}
                                        i++;
                                        continue;
                                    }}
                                    // Look ahead for a matching
                                    // keyed child elsewhere.
                                    let found = null;
                                    if (key != null) {{
                                        for (let j = i; j < parent.childNodes.length; j++) {{
                                            const c = parent.childNodes[j];
                                            if (c.nodeType === 1
                                                && c.tagName.toLowerCase() === tag
                                                && c.dataset
                                                && c.dataset.tilePos === key) {{
                                                found = c;
                                                break;
                                            }}
                                        }}
                                    }}
                                    if (found) {{
                                        parent.insertBefore(found, parent.childNodes[i] || null);
                                        patchAttrs(found, d.attrs);
                                        if (tag !== 'br'
                                            && !d.attrs.some(([k]) => k === 'data-widget-html')) {{
                                            patchChildren(found, d.kids || []);
                                        }}
                                        i++;
                                        continue;
                                    }}
                                    // Create.
                                    const elem = document.createElement(tag);
                                    patchAttrs(elem, d.attrs);
                                    if (tag !== 'br'
                                        && !d.attrs.some(([k]) => k === 'data-widget-html')) {{
                                        patchChildren(elem, d.kids || []);
                                    }}
                                    parent.insertBefore(elem, parent.childNodes[i] || null);
                                    i++;
                                }}
                                while (parent.childNodes.length > i) {{
                                    parent.removeChild(parent.lastChild);
                                }}
                            }}

                            // Selection placement — same data-tile-pos
                            // walk as the writeback effect, but
                            // run *inside* applyPatch so it
                            // happens AFTER DOM restructuring.
                            // Without this, a state change that
                            // reshapes the tile tree (e.g. live-
                            // preview hiding markdown markers when
                            // the caret leaves a span) destroys
                            // the cursor's anchor text node, the
                            // browser silently parks the cursor
                            // somewhere unrelated, and the next
                            // keystroke goes to the wrong place.
                            function placeSelection(anchor, head) {{
                                const tiles = el.querySelectorAll('[data-tile-pos]');
                                const textRanges = [];
                                const emptyTiles = [];
                                tiles.forEach(node => {{
                                    // Never resolve the caret ONTO an inline
                                    // uneditable point — the buffer `<img>`s
                                    // and zero-width widget badges. They share
                                    // a doc offset with the adjacent text; we
                                    // want the caret to land in that text, not
                                    // on the un-caretable node. (Length-bearing
                                    // widgets — checkboxes etc. — still resolve,
                                    // so list/task lines keep their anchor.)
                                    if (node.classList && (
                                        node.classList.contains('cm-widgetBuffer')
                                        || (node.classList.contains('editor-widget')
                                            && node.dataset.tileLen === '0'))) {{
                                        return;
                                    }}
                                    const pos = parseInt(node.dataset.tilePos, 10);
                                    const text = node.firstChild;
                                    if (text && text.nodeType === 3) {{
                                        const len = utf8Len(text.nodeValue);
                                        if (len) textRanges.push({{pos, end: pos + len, text}});
                                        else emptyTiles.push({{pos, node}});
                                    }} else {{
                                        emptyTiles.push({{pos, node}});
                                    }}
                                }});
                                textRanges.sort((a,b) => a.pos - b.pos);
                                emptyTiles.sort((a,b) => a.pos - b.pos);
                                // Returns [node, offset] for a doc
                                // byte position, using the same
                                // tile lookup the writeback used to.
                                function resolve(target) {{
                                    for (const t of textRanges) {{
                                        if (target > t.pos && target < t.end) {{
                                            return [t.text, bytesToUtf16(t.text.nodeValue, target - t.pos)];
                                        }}
                                    }}
                                    for (const t of emptyTiles) {{
                                        if (t.pos === target) return [t.node, 0];
                                    }}
                                    for (const t of textRanges) {{
                                        if (target >= t.pos && target <= t.end) {{
                                            return [t.text, bytesToUtf16(t.text.nodeValue, target - t.pos)];
                                        }}
                                    }}
                                    // Line-tile fallback: when typing a list
                                    // marker like `- ` empties the line of
                                    // text (the marker is fully replaced and
                                    // the only child is the bullet widget),
                                    // the cursor target may fall inside a
                                    // line tile but outside any text run.
                                    // Place it at the end of the matching
                                    // line div so the next keystroke lands
                                    // where the user expects.
                                    const lineTiles = el.querySelectorAll('div.cm-line[data-tile-pos]');
                                    for (const ln of lineTiles) {{
                                        const lpos = parseInt(ln.dataset.tilePos, 10);
                                        const llen = parseInt(ln.dataset.tileLen || '0', 10);
                                        if (target >= lpos && target <= lpos + llen) {{
                                            return [ln, ln.childNodes.length];
                                        }}
                                    }}
                                    return [el, el.childNodes.length];
                                }}
                                const [aNode, aOff] = resolve(anchor);
                                const [hNode, hOff] = resolve(head);
                                const sel = window.getSelection();
                                if (sel) {{
                                    // setBaseAndExtent preserves
                                    // direction (anchor → head).
                                    // Range/addRange would always
                                    // normalize to start<=end and
                                    // break shift+arrow-left.
                                    sel.setBaseAndExtent(aNode, aOff, hNode, hOff);
                                }}
                                // Keep the caret on screen — but ONLY when the
                                // selection actually moved (typing, vim motion,
                                // click). A decoration-only re-render (e.g. the
                                // debounced overlay pass) re-places the *same*
                                // selection; scrolling then would yank the view
                                // back while the user is scrolled away reading.
                                const selKey = anchor + ':' + head;
                                if (selKey !== lastScrolledKey) {{
                                    lastScrolledKey = selKey;
                                    scrollCaretIntoView(hNode, hOff);
                                }}
                            }}

                            // Nearest scrollable ancestor of the editor —
                            // resolved at call time because the editor mounts
                            // inside different host shells (playground,
                            // app, …) whose scroll container we don't own.
                            // Mirrors CM6's `scrollableParents`
                            // (`view/src/dom.ts`).
                            function scrollableAncestor() {{
                                let n = el.parentElement;
                                while (n) {{
                                    const s = getComputedStyle(n);
                                    const oy = s.overflowY;
                                    if ((oy === 'auto' || oy === 'scroll' || oy === 'overlay')
                                        && n.scrollHeight > n.clientHeight + 1) {{
                                        return n;
                                    }}
                                    n = n.parentElement;
                                }}
                                return document.scrollingElement || document.documentElement;
                            }}

                            // Scroll the nearest scrollable ancestor the
                            // minimum amount needed to bring the caret rect
                            // inside its viewport, with a small margin so the
                            // caret never sits flush against an edge. CM6's
                            // `scrollRectIntoView` reduced to the vertical +
                            // horizontal nudge we need.
                            let lastScrolledKey = null;
                            const SCROLL_MARGIN = 24;
                            function scrollCaretIntoView(node, off) {{
                                if (!node) return;
                                let rect = null;
                                try {{
                                    const r = document.createRange();
                                    r.setStart(node, Math.min(off, (node.nodeType === 3
                                        ? node.nodeValue.length
                                        : node.childNodes.length)));
                                    r.collapse(true);
                                    const rects = r.getClientRects();
                                    rect = rects.length ? rects[0] : r.getBoundingClientRect();
                                }} catch (e) {{ return; }}
                                if (!rect || (rect.top === 0 && rect.bottom === 0
                                    && rect.left === 0)) return;
                                const sc = scrollableAncestor();
                                const cont = (sc === document.scrollingElement
                                    || sc === document.documentElement)
                                    ? {{ top: 0, bottom: window.innerHeight,
                                         left: 0, right: window.innerWidth }}
                                    : sc.getBoundingClientRect();
                                let dy = 0, dx = 0;
                                if (rect.top < cont.top + SCROLL_MARGIN) {{
                                    dy = rect.top - cont.top - SCROLL_MARGIN;
                                }} else if (rect.bottom > cont.bottom - SCROLL_MARGIN) {{
                                    dy = rect.bottom - cont.bottom + SCROLL_MARGIN;
                                }}
                                if (rect.left < cont.left + SCROLL_MARGIN) {{
                                    dx = rect.left - cont.left - SCROLL_MARGIN;
                                }} else if (rect.right > cont.right - SCROLL_MARGIN) {{
                                    dx = rect.right - cont.right + SCROLL_MARGIN;
                                }}
                                if (dy !== 0 || dx !== 0) {{
                                    sc.scrollBy({{ top: dy, left: dx, behavior: 'auto' }});
                                }}
                            }}

                            // ── perf logging ──
                            // Toggle in DevTools console with:
                            //   window.__cm_perf = true
                            // Then watch `[cm/perf]` lines.
                            function perfStart(label) {{
                                if (!window.__cm_perf) return 0;
                                return performance.now();
                            }}
                            function perfEnd(label, t0, extra) {{
                                if (!window.__cm_perf || t0 === 0) return;
                                const dt = performance.now() - t0;
                                console.log(
                                    `[cm/perf] {{label}} ${{dt.toFixed(2)}}ms`,
                                    extra || ''
                                );
                            }}
                            window.__cm_perf_summary = {{
                                patches: 0, totalPatchMs: 0,
                                inputs: 0, totalInputMs: 0,
                            }};

                            function applyPatch(payloadJson) {{
                                if (composing) return;
                                const t0 = perfStart('patch');
                                let payload;
                                try {{ payload = JSON.parse(payloadJson); }}
                                catch (_) {{ return; }}
                                const descs = payload.patches || payload;
                                const sel = payload.selection || null;
                                // Incremental: the patch carries only the
                                // line patches from `firstChanged` on; the
                                // unchanged prefix lines stay as-is. 0 (or
                                // absent) means a full reconcile.
                                const firstChanged = payload.firstChanged || 0;
                                if (typeof payload.doc === 'string') {{
                                    window['__cm_doc_{id}'] = payload.doc;
                                }}
                                mo.disconnect();
                                el.dataset.writing = '1';
                                el.dataset.muting = '1';
                                try {{
                                    patchChildren(el, descs, firstChanged);
                                    if (sel) placeSelection(sel.anchor, sel.head);
                                }} finally {{
                                    mo.takeRecords();
                                    mo.observe(el, observeOpts);
                                    requestAnimationFrame(() => {{
                                        delete el.dataset.writing;
                                        delete el.dataset.muting;
                                    }});
                                }}
                                const dt = window.__cm_perf
                                    ? performance.now() - t0 : 0;
                                if (window.__cm_perf) {{
                                    window.__cm_perf_summary.patches += 1;
                                    window.__cm_perf_summary.totalPatchMs += dt;
                                    console.log(
                                        `[cm/perf] patch ${{dt.toFixed(2)}}ms`,
                                        `descs=${{descs.length}} docLen=${{(payload.doc||'').length}}`
                                    );
                                }}
                            }}
                            window['__cm_patch_{id}'] = function(descsJson) {{
                                window['__cm_pending_{id}'] = descsJson;
                                applyPatch(descsJson);
                            }};
                            // Replay anything the Dioxus effect
                            // stashed before the bridge attached.
                            const _pending = window['__cm_pending_{id}'];
                            if (_pending != null) applyPatch(_pending);


                            // Selection-only events. `selectionchange`
                            // is the canonical event for caret
                            // movement (covers programmatic
                            // updates that keyup/mouseup miss).
                            // It fires on `document`, not the
                            // element — we filter to selections
                            // that intersect our editor.
                            //
                            // The state→DOM selection writeback
                            // effect sets `el.dataset.writing`
                            // around its setSelectionRange call;
                            // we skip the listener while that
                            // flag is set so our own write
                            // doesn't loop back through the
                            // bridge and clamp the state-side
                            // selection to whatever the browser
                            // could actually represent (which
                            // is shorter than state.doc when
                            // Hidden tiles are involved).
                            document.addEventListener('selectionchange', () => {{
                                // Skip during our own writes
                                // (Phase 10) and during the
                                // frame following a DOM mutation
                                // (Dioxus decoration churn —
                                // Selection may be orphaned and
                                // reading it would clobber state
                                // with garbage).
                                if (el.dataset.writing === '1') return;
                                if (el.dataset.muting === '1') return;
                                const s = window.getSelection();
                                if (s && s.anchorNode && el.contains(s.anchorNode)) {{
                                    sendSel();
                                }}
                            }});
                            el.addEventListener('keyup',   sendSel);
                            el.addEventListener('mouseup', sendSel);
                            el.addEventListener('focus',   sendSel);
                            // Copy/cut: substitute the source
                            // markdown for the rendered DOM. The
                            // browser's default would copy what's
                            // visible (widget characters, hidden
                            // markers, etc.) which round-trips to
                            // nothing useful.
                            function sourceSlice() {{
                                const doc = window['__cm_doc_{id}'];
                                if (typeof doc !== 'string') return null;
                                const [a, b] = selOffsets();
                                const from = Math.min(a, b);
                                const to = Math.max(a, b);
                                if (from === to) return null;
                                // selOffsets returns byte offsets;
                                // slice the doc by codepoints by
                                // walking utf-8 byte counts.
                                let i = 0;
                                let bytes = 0;
                                let start = 0;
                                while (i < doc.length && bytes < from) {{
                                    const c = doc.charCodeAt(i);
                                    if (c < 0x80) {{ bytes += 1; i += 1; }}
                                    else if (c < 0x800) {{ bytes += 2; i += 1; }}
                                    else if (c >= 0xD800 && c <= 0xDBFF) {{ bytes += 4; i += 2; }}
                                    else {{ bytes += 3; i += 1; }}
                                }}
                                start = i;
                                while (i < doc.length && bytes < to) {{
                                    const c = doc.charCodeAt(i);
                                    if (c < 0x80) {{ bytes += 1; i += 1; }}
                                    else if (c < 0x800) {{ bytes += 2; i += 1; }}
                                    else if (c >= 0xD800 && c <= 0xDBFF) {{ bytes += 4; i += 2; }}
                                    else {{ bytes += 3; i += 1; }}
                                }}
                                return doc.slice(start, i);
                            }}
                            el.addEventListener('copy', evt => {{
                                const slice = sourceSlice();
                                if (slice == null) return;
                                evt.clipboardData.setData('text/plain', slice);
                                evt.preventDefault();
                            }});
                            el.addEventListener('cut', evt => {{
                                const slice = sourceSlice();
                                if (slice == null) return;
                                evt.clipboardData.setData('text/plain', slice);
                                evt.preventDefault();
                                // Let the existing input handlers
                                // delete the selected range as if
                                // the user pressed Backspace over it.
                                const [a, b] = selOffsets();
                                dioxus.send({{
                                    kind: 'before-input-delete-backward',
                                    sel: [Math.min(a,b), Math.max(a,b)],
                                }});
                            }});
                            // Link click handling. Plain click on a
                            // link/wikilink span navigates. To
                            // *edit* the link text instead, use
                            // the keyboard (arrow into it from an
                            // adjacent position). Mirrors
                            // Obsidian's preview-side behavior.
                            // ── Frontmatter properties ─────────
                            //
                            // Editable cells live inside the
                            // `.md-properties` widget. We attach
                            // delegated handlers on the root so
                            // we don't have to re-wire per
                            // render. `data-edit-role` picks the
                            // dispatch path.
                            const propValueText = (cell) => {{
                                return (cell.textContent || '').trim();
                            }};
                            const sendPropEdit = (row, kind, extra) => {{
                                const key = row.dataset.propKey;
                                const msg = {{ kind, key }};
                                Object.assign(msg, extra || {{}});
                                dioxus.send(msg);
                            }};
                            el.addEventListener('focusin', evt => {{
                                const row = evt.target.closest('.md-property-row');
                                if (row) row.classList.add('is-active');
                                if (evt.target.dataset.editRole) {{
                                    dioxus.send({{ kind: 'widget-focus', focused: true }});
                                }}
                            }});
                            el.addEventListener('focusout', evt => {{
                                const row = evt.target.closest('.md-property-row');
                                if (evt.target.dataset.editRole) {{
                                    dioxus.send({{ kind: 'widget-focus', focused: false }});
                                }}
                                if (!row) return;
                                row.classList.remove('is-active');
                                const role = evt.target.dataset.editRole;
                                if (role === 'text' || role === 'number') {{
                                    sendPropEdit(row, 'prop-set', {{
                                        value: propValueText(evt.target),
                                        ty: role,
                                    }});
                                }}
                            }});
                            el.addEventListener('change', evt => {{
                                const role = evt.target.dataset.editRole;
                                if (role !== 'date') return;
                                const row = evt.target.closest('.md-property-row');
                                if (!row) return;
                                sendPropEdit(row, 'prop-set', {{
                                    value: evt.target.value,
                                    ty: 'date',
                                }});
                            }});
                            // Synchronous focus flag — written
                            // before any keydown that follows the
                            // click that focused the cell. The
                            // editor's `onkeydown` Rust closure
                            // checks `el.dataset.widgetFocused`
                            // synchronously via an inline JS
                            // helper at the top of dispatch.
                            el.addEventListener('focusin', evt => {{
                                // Focus drift caused by the vim visual-arrow
                                // `Selection.modify` walk (flagged below) is
                                // not user intent — never flip to Insert for
                                // it, or holding `k` at the top starts
                                // typing "kkkk" into a property cell.
                                if (el.dataset.selModify) return;
                                const role = evt.target.dataset.editRole;
                                if (role) {{
                                    el.dataset.widgetFocused = '1';
                                    // Typing cells put vim into
                                    // Insert — the mode badge and
                                    // the painted modal caret must
                                    // agree with where keystrokes
                                    // actually land. (Esc blurs +
                                    // sends prop-leave → Normal.)
                                    if (role === 'text' || role === 'number'
                                        || role === 'date' || role === 'chip-add'
                                        || role === 'row-add') {{
                                        dioxus.send({{ kind: 'prop-focus' }});
                                    }}
                                }}
                            }}, true);
                            el.addEventListener('focusout', evt => {{
                                if (evt.target.dataset.editRole) {{
                                    delete el.dataset.widgetFocused;
                                }}
                            }}, true);
                            // Capture-phase keydown inside a
                            // property cell: handle cell-owned
                            // keys (Esc/Enter/Space) and stop
                            // propagation so the editor's
                            // normal vim/keymap dispatch
                            // (Dioxus document delegation)
                            // never sees them. Single handler
                            // because if we split it across
                            // capture + bubble, the
                            // stopPropagation in capture kills
                            // the bubble pass on the same
                            // element.
                            el.addEventListener('keydown', evt => {{
                                const role = evt.target.dataset.editRole;
                                if (!role) return;
                                evt.stopPropagation();
                                // Esc always blurs. Enter blurs for
                                // single-value cells (text /
                                // number / date / bool) but is
                                // handled below for the cells
                                // that commit on Enter
                                // (chip-add, row-add).
                                const enterCommits = role === 'chip-add' || role === 'row-add';
                                if (evt.key === 'Escape' || (evt.key === 'Enter' && !enterCommits)) {{
                                    evt.preventDefault();
                                    evt.target.blur();
                                    // Tell Rust the cell lost
                                    // focus due to Esc/Enter so
                                    // vim can flip back to
                                    // Normal mode.
                                    dioxus.send({{ kind: 'prop-leave' }});
                                    return;
                                }}
                                if (role === 'bool' && (evt.key === ' ' || evt.key === 'Enter')) {{
                                    evt.preventDefault();
                                    const row = evt.target.closest('.md-property-row');
                                    const next = evt.target.dataset.checked !== 'true';
                                    sendPropEdit(row, 'prop-set', {{
                                        value: next ? 'true' : 'false',
                                        ty: 'bool',
                                    }});
                                    return;
                                }}
                                if (role === 'chip-add' && evt.key === 'Enter') {{
                                    evt.preventDefault();
                                    const row = evt.target.closest('.md-property-row');
                                    const val = propValueText(evt.target);
                                    if (!val) return;
                                    sendPropEdit(row, 'prop-list-add', {{ value: val }});
                                    evt.target.textContent = '';
                                    return;
                                }}
                                if (role === 'row-add' && evt.key === 'Enter') {{
                                    evt.preventDefault();
                                    const key = propValueText(evt.target);
                                    if (!key) return;
                                    dioxus.send({{ kind: 'prop-add', key }});
                                    evt.target.textContent = '';
                                    return;
                                }}
                                // Up/down arrows: navigate to the
                                // adjacent property ROW (or the
                                // row-add cell at the bottom).
                                // The default contenteditable
                                // up-arrow bubbles to the
                                // browser, which moves the caret
                                // out of the properties widget
                                // entirely.
                                if (evt.key === 'ArrowUp' || evt.key === 'ArrowDown') {{
                                    // Each navigation target is the
                                    // first edit-cell of a property
                                    // row, plus the bottom
                                    // `row-add`. Skip `chip-remove`
                                    // × buttons — they're not
                                    // useful nav targets.
                                    const targets = [];
                                    el.querySelectorAll('.md-properties .md-property-row')
                                        .forEach(row => {{
                                            const cell = row.querySelector(
                                                '[data-edit-role="text"],'
                                                + '[data-edit-role="number"],'
                                                + '[data-edit-role="date"],'
                                                + '[data-edit-role="bool"],'
                                                + '[data-edit-role="chip-add"]'
                                            );
                                            if (cell) targets.push(cell);
                                        }});
                                    const rowAdd = el.querySelector(
                                        '[data-edit-role="row-add"]'
                                    );
                                    if (rowAdd) targets.push(rowAdd);
                                    // Find which row the current
                                    // cell belongs to (the cell
                                    // itself may not be in
                                    // `targets` — e.g. a chip-
                                    // remove button — so map via
                                    // row).
                                    const currentRow = evt.target.closest('.md-property-row');
                                    let idx = targets.findIndex(t => {{
                                        if (t === evt.target) return true;
                                        if (currentRow && t.closest('.md-property-row') === currentRow) return true;
                                        return false;
                                    }});
                                    if (idx === -1) {{
                                        if (evt.target === rowAdd) idx = targets.length - 1;
                                    }}
                                    if (idx === -1) return;
                                    const delta = evt.key === 'ArrowUp' ? -1 : 1;
                                    const next = targets[idx + delta];
                                    if (!next) return;
                                    evt.preventDefault();
                                    next.focus();
                                    if (next.isContentEditable) {{
                                        const r = document.createRange();
                                        r.selectNodeContents(next);
                                        r.collapse(false);
                                        const s = window.getSelection();
                                        s.removeAllRanges();
                                        s.addRange(r);
                                    }}
                                    return;
                                }}
                            }}, true);
                            el.addEventListener('click', evt => {{
                                const role = evt.target.dataset.editRole;
                                if (role === 'bool') {{
                                    evt.preventDefault();
                                    evt.stopPropagation();
                                    const row = evt.target.closest('.md-property-row');
                                    const next = evt.target.dataset.checked !== 'true';
                                    sendPropEdit(row, 'prop-set', {{
                                        value: next ? 'true' : 'false',
                                        ty: 'bool',
                                    }});
                                    return;
                                }}
                                if (role === 'chip-remove') {{
                                    evt.preventDefault();
                                    evt.stopPropagation();
                                    const chip = evt.target.closest('.md-property-chip');
                                    const row = evt.target.closest('.md-property-row');
                                    if (chip && row) {{
                                        sendPropEdit(row, 'prop-list-remove', {{
                                            value: chip.dataset.chipValue || '',
                                        }});
                                    }}
                                    return;
                                }}
                                if (role === 'row-remove') {{
                                    evt.preventDefault();
                                    evt.stopPropagation();
                                    const row = evt.target.closest('.md-property-row');
                                    if (row) {{
                                        dioxus.send({{
                                            kind: 'prop-remove',
                                            key: row.dataset.propKey,
                                        }});
                                    }}
                                    return;
                                }}
                            }});
                            // Mousedown anywhere inside the
                            // properties widget shouldn't steal
                            // the caret: the contenteditable
                            // cells handle their own focus.
                            el.addEventListener('mousedown', evt => {{
                                if (evt.target.closest('.md-properties')) {{
                                    evt.stopPropagation();
                                }}
                            }});
                            // Click anywhere in the row (key
                            // column, gutters, even an empty
                            // list cell) routes focus into the
                            // value's editable control. Lets the
                            // user tap a label and immediately
                            // type, the way Obsidian does.
                            el.addEventListener('click', evt => {{
                                if (evt.target.dataset.editRole) return;
                                if (evt.target.closest('.md-chip-remove')) return;
                                const row = evt.target.closest('.md-property-row');
                                if (!row) return;
                                const cell = row.querySelector('[data-edit-role]');
                                if (!cell) return;
                                if (cell.matches('input')) {{
                                    cell.focus();
                                }} else {{
                                    cell.focus();
                                    // Move caret to end of
                                    // existing text.
                                    const r = document.createRange();
                                    r.selectNodeContents(cell);
                                    r.collapse(false);
                                    const s = window.getSelection();
                                    s.removeAllRanges();
                                    s.addRange(r);
                                }}
                            }});

                            // Mousedown on a task checkbox: the
                            // browser would otherwise park the
                            // caret at the widget position
                            // before our click handler runs,
                            // overwriting wherever the user was
                            // editing. Pre-empt by preventing
                            // default selection here — the
                            // click handler below still fires
                            // and dispatches the toggle.
                            el.addEventListener('mousedown', evt => {{
                                let n = evt.target;
                                while (n && n !== el) {{
                                    if (n.nodeType === 1 && n.dataset
                                        && n.dataset.taskPos != null) {{
                                        evt.preventDefault();
                                        return;
                                    }}
                                    if (n.nodeType === 1 && n.tagName === 'LABEL'
                                        && n.closest && n.closest('.md-tabs-widget')) {{
                                        // Tab-strip label: don't let the
                                        // browser park the caret in the
                                        // widget (which would revert it
                                        // to fence source).
                                        evt.preventDefault();
                                        return;
                                    }}
                                    n = n.parentNode;
                                }}
                            }});
                            el.addEventListener('click', evt => {{
                                let n = evt.target;
                                while (n && n !== el) {{
                                    if (n.nodeType === 1 && n.classList
                                        && n.classList.contains('md-keyflow-toggle')) {{
                                        // Keyflow chart source toggle —
                                        // must win over the widget's
                                        // data-focus-pos ancestor, or
                                        // the click would drop the
                                        // caret into the fence instead.
                                        evt.preventDefault();
                                        evt.stopPropagation();
                                        const w = n.closest('.md-keyflow-widget');
                                        if (w) w.classList.toggle('md-keyflow-show-source');
                                        return;
                                    }}
                                    if (n.nodeType === 1 && n.dataset
                                        && n.dataset.copyFrom != null) {{
                                        evt.preventDefault();
                                        const from = parseInt(n.dataset.copyFrom, 10);
                                        const to   = parseInt(n.dataset.copyTo, 10);
                                        dioxus.send({{ kind: 'copy-range', from, to }});
                                        n.classList.add('copied');
                                        setTimeout(() => n.classList.remove('copied'), 800);
                                        return;
                                    }}
                                    if (n.nodeType === 1 && n.dataset
                                        && n.dataset.taskPos != null) {{
                                        evt.preventDefault();
                                        evt.stopPropagation();
                                        const p = parseInt(n.dataset.taskPos, 10);
                                        if (!isNaN(p)) {{
                                            dioxus.send({{ kind: 'task-toggle', pos: p }});
                                        }}
                                        return;
                                    }}
                                    if (n.nodeType === 1 && n.tagName === 'LABEL'
                                        && n.closest && n.closest('.md-tabs-widget')) {{
                                        // Tabs widget: switch the CSS-only
                                        // tab ourselves — preventDefault
                                        // suppresses the label's native
                                        // radio activation, and letting the
                                        // click fall through to the
                                        // data-focus-pos ancestor would
                                        // drop the caret into the fence.
                                        evt.preventDefault();
                                        evt.stopPropagation();
                                        const id = n.getAttribute('for');
                                        const r = id && document.getElementById(id);
                                        if (r) r.checked = true;
                                        return;
                                    }}
                                    if (n.nodeType === 1 && n.dataset
                                        && n.dataset.focusPos != null) {{
                                        // Click on a rendered
                                        // math/typst widget:
                                        // hop the caret inside
                                        // the source span so
                                        // the user can edit.
                                        evt.preventDefault();
                                        evt.stopPropagation();
                                        const p = parseInt(n.dataset.focusPos, 10);
                                        if (!isNaN(p)) {{
                                            dioxus.send({{ kind: 'focus-pos', pos: p }});
                                        }}
                                        return;
                                    }}
                                    if (n.nodeType === 1 && n.dataset
                                        && n.dataset.href) {{
                                        const href = n.dataset.href;
                                        // Clear caret state so the
                                        // link span goes back to
                                        // live-preview (the browser
                                        // had already parked the
                                        // caret inside it as part
                                        // of the click). Rust drops
                                        // selection to caret(0).
                                        dioxus.send({{ kind: 'link-clicked', href }});
                                        if (/^https?:/i.test(href)) {{
                                            window.open(href, '_blank', 'noopener');
                                        }}
                                        return;
                                    }}
                                    n = n.parentNode;
                                }}
                            }});
                            // ── Typing / idle tracking ──────────
                            // `lastTypeTs` powers hover suppression
                            // (don't pop a tooltip while the user is
                            // typing or keyboard-navigating). The
                            // `idleTimer` fires ~220ms after the last
                            // text input and pings Rust so it can flip
                            // back to the full decoration pass
                            // (diagnostics + overlays) once editing
                            // settles. `beforeinput` covers every doc
                            // mutation (typing, IME, bracket, delete);
                            // `keydown` additionally captures caret
                            // navigation for hover suppression only.
                            let lastTypeTs = 0;
                            let idleTimer = null;
                            el.addEventListener('beforeinput', () => {{
                                lastTypeTs = Date.now();
                                if (idleTimer) clearTimeout(idleTimer);
                                idleTimer = setTimeout(() => {{
                                    idleTimer = null;
                                    dioxus.send({{ kind: 'idle' }});
                                }}, 220);
                            }}, true);
                            el.addEventListener('keydown', () => {{
                                lastTypeTs = Date.now();
                            }}, true);
                            // ── Hover tooltips ──────────────────
                            // Debounced pointer → doc position →
                            // `hover` message. Rust runs the hover
                            // source and shows/hides the floating
                            // panel. Mirrors CM6's HoverPlugin
                            // (`view/src/tooltip.ts`): rest the
                            // pointer ~300ms, map coords to a doc
                            // offset, ask the source.
                            let hoverTimer = null;
                            el.addEventListener('mousemove', evt => {{
                                const x = evt.clientX, y = evt.clientY;
                                if (hoverTimer) clearTimeout(hoverTimer);
                                hoverTimer = setTimeout(() => {{
                                    hoverTimer = null;
                                    // Suppress while actively typing /
                                    // navigating: a tooltip popping up
                                    // under the cursor mid-edit is the
                                    // exact jank we're killing.
                                    if (Date.now() - lastTypeTs < 700) {{
                                        dioxus.send({{ kind: 'hover-end' }});
                                        return;
                                    }}
                                    let node = null, off = 0;
                                    if (document.caretPositionFromPoint) {{
                                        const cp = document.caretPositionFromPoint(x, y);
                                        if (cp) {{ node = cp.offsetNode; off = cp.offset; }}
                                    }} else if (document.caretRangeFromPoint) {{
                                        const r = document.caretRangeFromPoint(x, y);
                                        if (r) {{ node = r.startContainer; off = r.startOffset; }}
                                    }}
                                    if (!node || !el.contains(node)) {{
                                        dioxus.send({{ kind: 'hover-end' }});
                                        return;
                                    }}
                                    const pos = posFromDOM(node, off);
                                    dioxus.send({{ kind: 'hover', pos: pos, x: x, y: y }});
                                }}, 300);
                            }});
                            el.addEventListener('mouseleave', () => {{
                                if (hoverTimer) {{ clearTimeout(hoverTimer); hoverTimer = null; }}
                                dioxus.send({{ kind: 'hover-end' }});
                            }});
                            el.addEventListener('scroll', () => {{
                                if (hoverTimer) {{ clearTimeout(hoverTimer); hoverTimer = null; }}
                                dioxus.send({{ kind: 'hover-end' }});
                            }}, true);
                            sendSel();
                        }}
                        attach();
                    }})();
                    "#
                );
                let mut handle = document::eval(&script);
                while let Ok(v) = handle.recv::<serde_json::Value>().await {
                    match v.get("kind").and_then(|k| k.as_str()).unwrap_or("") {
                        // Pointer hover → run the hover source (if any) and
                        // show/hide the floating tooltip. Pure UI state, so it
                        // bypasses the transaction-producing dispatcher.
                        "hover" => {
                            if let Some(src) = hover_source {
                                let pos =
                                    v.get("pos").and_then(|p| p.as_u64()).unwrap_or(0) as usize;
                                let x = v.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0);
                                let y = v.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0);
                                let cur = state.read().clone();
                                hover_sig.set(src(&cur, pos).map(|tip| crate::hover::HoverPopup {
                                    tip,
                                    x,
                                    y,
                                }));
                            }
                        }
                        "hover-end" => hover_sig.set(None),
                        // Typing has settled (JS debounce fired) — flip to
                        // the `Full` decoration pass so the patch effect
                        // recomputes diagnostics + overlays.
                        "idle" => idle_sig.set(true),
                        other => {
                            // Surface link/wikilink clicks to the host
                            // before the generic dispatch (which only
                            // drops the selection). The host decides
                            // what a wikilink click means — e.g. a
                            // vault view navigates to the target note.
                            if other == "link-clicked" {
                                if let (Some(cb), Some(href)) = (
                                    on_link_click,
                                    v.get("href").and_then(|h| h.as_str()),
                                ) {
                                    cb.call(href.to_string());
                                }
                            }
                            // Any doc-mutating message means the user is
                            // actively editing: drop to the `Structural`
                            // pass until the next `idle` ping so expensive
                            // analysis doesn't run on each keystroke. Pure
                            // caret moves (`sel`) and widget/UI messages
                            // leave the phase alone.
                            if is_edit_kind(other) {
                                idle_sig.set(false);
                            }
                            // IME composition is the one window where the
                            // patcher skips applying, so force the next
                            // patch to a full reconcile to resync the
                            // incremental prefix-skip with the live DOM.
                            if other == "composition-start" || other == "composition-end" {
                                force_full_sig.set(true);
                            }
                            crate::bridge::handle_bridge_msg(
                                state,
                                deco_source.as_ref(),
                                vim,
                                widget_focus,
                                sink,
                                &v,
                            );
                        }
                    }
                }
            });
        });
    }

    // ── onkeydown: vim → keymap dispatch ─────────────────────────
    let keymap_for_keys = keymap.clone();
    let sink_for_keys = on_transaction;
    let completion_for_keys = completion_state;
    let vim_for_keys = vim;
    let slash_for_keys = slash;
    let widget_focus_for_keys = widget_focus;
    // Only the web visual-arrow `Selection.modify` eval references this.
    #[cfg(not(feature = "native"))]
    let editor_id_for_keys = editor_id.clone();
    let on_keydown = move |evt: Event<KeyboardData>| {
        // Read-only editor: no command may mutate the doc. The
        // browser can't edit either (contenteditable is off), so
        // bailing here makes the whole keyboard inert.
        if !editable {
            return;
        }
        // Widget cell has focus (e.g. a frontmatter property
        // contenteditable). The widget owns its keyboard, so
        // every editor-side handler — vim, slash, keymap —
        // must bail. We check the DOM directly (synchronous)
        // because the `widget_focus_for_keys` signal is
        // bridge-driven and arrives a tick late, racing the
        // first keypress after a click.
        if widget_focused_dom() || *widget_focus_for_keys.peek() {
            return;
        }
        let mods = evt.modifiers();
        let mut key_str = match evt.key() {
            Key::Character(c) => c,
            other => other.to_string(),
        };
        // Some key sources (notably the Blitz/native event path)
        // report the *unshifted* character with the shift modifier
        // set — vim then sees `o` instead of `O` and opens the
        // line below. Normalize: shift + single lowercase letter
        // becomes the uppercase letter. (No-op where the platform
        // already applied shift.)
        if mods.shift() && key_str.chars().count() == 1 {
            if let Some(c) = key_str.chars().next() {
                if c.is_lowercase() {
                    key_str = c.to_uppercase().to_string();
                }
            }
        }
        let press = KeySpec {
            key: key_str,
            ctrl: mods.ctrl(),
            alt: mods.alt(),
            shift: mods.shift(),
            meta: mods.meta(),
            r#mod: mods.ctrl() || mods.meta(),
        };
        let cur = state.read().clone();
        // ── Slash menu key routing ──
        //
        // When the menu is open, Arrow keys cycle selection,
        // Enter picks the highlighted command, Escape closes
        // without firing. Everything else falls through to vim
        // and the keymap below — including character keys, so
        // typing more after `/` keeps the trigger active and the
        // doc change re-runs `detect_slash`.
        if let Some(mut slash_sig) = slash_for_keys {
            let snapshot = slash_sig.peek().clone();
            if let Some(current) = snapshot {
                let hits = crate::slash::filter_commands(&current.query);
                match press.key.as_str() {
                    "Escape" => {
                        slash_sig.set(None);
                        evt.prevent_default();
                        return;
                    }
                    "ArrowDown" if !hits.is_empty() => {
                        let len = hits.len();
                        let next = (current.selected + 1) % len;
                        let mut new = current.clone();
                        new.selected = next;
                        slash_sig.set(Some(new));
                        evt.prevent_default();
                        return;
                    }
                    "ArrowUp" if !hits.is_empty() => {
                        let len = hits.len();
                        let next = (current.selected + len - 1) % len;
                        let mut new = current.clone();
                        new.selected = next;
                        slash_sig.set(Some(new));
                        evt.prevent_default();
                        return;
                    }
                    "Enter" => {
                        if let Some(entry) = hits.get(current.selected) {
                            let end = current.slash_start + 1 + current.query.len();
                            if let Some(spec) = crate::slash::run_command(
                                &cur,
                                current.slash_start..end,
                                entry.kind,
                            ) {
                                crate::event::apply_tx(state, &cur, spec, sink_for_keys);
                            }
                        }
                        slash_sig.set(None);
                        evt.prevent_default();
                        return;
                    }
                    _ => {}
                }
            }
        }
        // ── Completion menu key routing ──
        //
        // Same contract as the slash menu: Arrows cycle, Enter
        // accepts, Escape closes; everything else falls through so
        // continued typing updates the query via the detect effect.
        // Runs AFTER slash so slash keeps its existing precedence in
        // the (degenerate) case where both menus are open at once.
        // Unlike slash, Enter with zero candidates falls through —
        // swallowing a newline because the host had nothing to offer
        // would be hostile.
        {
            let mut comp_sig = completion_for_keys;
            let snapshot = comp_sig.peek().clone();
            if let Some(current) = snapshot {
                match press.key.as_str() {
                    "Escape" => {
                        comp_sig.set(None);
                        evt.prevent_default();
                        return;
                    }
                    "ArrowDown" if !current.candidates.is_empty() => {
                        let len = current.candidates.len();
                        let mut new = current.clone();
                        new.selected = (current.selected + 1) % len;
                        comp_sig.set(Some(new));
                        evt.prevent_default();
                        return;
                    }
                    "ArrowUp" if !current.candidates.is_empty() => {
                        let len = current.candidates.len();
                        let mut new = current.clone();
                        new.selected = (current.selected + len - 1) % len;
                        comp_sig.set(Some(new));
                        evt.prevent_default();
                        return;
                    }
                    "Enter" if !current.candidates.is_empty() => {
                        let idx = current.selected.min(current.candidates.len() - 1);
                        if let Some(spec) = crate::trigger::accept_candidate(
                            &cur,
                            &current,
                            &current.candidates[idx],
                        ) {
                            crate::event::apply_tx(state, &cur, spec, sink_for_keys);
                        }
                        comp_sig.set(None);
                        evt.prevent_default();
                        return;
                    }
                    _ => {}
                }
            }
        }
        if let Some(mut vim_sig) = vim_for_keys {
            // ── Visual-row h/j/k/l shortcut ──
            //
            // Vim's logical j/k jump whole doc lines, which is
            // jarring when soft-wrap is on (a paragraph that
            // takes 5 visual rows jumps the cursor 5 rows in one
            // press). For bare h/j/k/l without a pending count
            // we lean on the browser's `Selection.modify` so
            // movement follows the rendered rows the user sees,
            // matching Obsidian / VSCode wrap behavior. Count-
            // prefixed motions (`3j`, `5k`) still go through
            // vim for logical-line semantics.
            // Native renderer skips this wrap-aware shortcut — no JS
            // `Selection.modify` exists; `vim::handle_key` below owns
            // h/j/k/l with logical-line semantics instead.
            #[cfg(not(feature = "native"))]
            {
            let vim_snap = vim_sig.peek().clone();
            let is_visual_arrow_key = !vim_snap.is_inserting()
                && vim_snap.pending_count.is_none()
                && vim_snap.pending_operator.is_none()
                && vim_snap.pending_motion_input.is_none()
                && matches!(press.key.as_str(), "h" | "j" | "k" | "l")
                && !press.ctrl
                && !press.alt
                && !press.meta;
            if is_visual_arrow_key {
                let direction = match press.key.as_str() {
                    "h" => ("backward", "character"),
                    "l" => ("forward", "character"),
                    "k" => ("backward", "line"),
                    "j" => ("forward", "line"),
                    _ => unreachable!(),
                };
                let action = if vim_snap.is_visual() {
                    "extend"
                } else {
                    "move"
                };
                let script = format!(
                    r#"
                    (function() {{
                        const el = document.querySelector('[data-editor-id="{id}"]');
                        if (!el) return;
                        const sel = window.getSelection();
                        if (!sel) return;
                        // Snapshot: if the walk escapes the editor's line
                        // content we restore. `k` on the top row otherwise
                        // drifts the DOM selection into the frontmatter
                        // properties widget's cells — the cell focus flips
                        // vim to Insert and the next `k`s TYPE. (Regression:
                        // "holding k at the top drops into insert mode".)
                        const before = sel.rangeCount > 0
                            ? sel.getRangeAt(0).cloneRange() : null;
                        // Flag the walk so the focusin handler above knows
                        // any focus change here is drift, not user intent
                        // (focusin fires synchronously inside modify()).
                        el.dataset.selModify = '1';
                        sel.modify('{action}', '{dir}', '{gran}');
                        delete el.dataset.selModify;
                        const n = sel.anchorNode;
                        const host = n && (n.nodeType === 1 ? n : n.parentElement);
                        const escaped = !host || !host.closest
                            || !el.contains(host)
                            || !host.closest('.cm-line')
                            || host.closest('[data-edit-role]')
                            || host.closest('.md-property-row');
                        if (escaped && before) {{
                            sel.removeAllRanges();
                            sel.addRange(before);
                            // The walk may have moved focus into a widget
                            // input — hand it back to the editor root.
                            if (document.activeElement !== el) el.focus();
                        }}
                    }})();
                    "#,
                    id = editor_id_for_keys,
                    action = action,
                    dir = direction.0,
                    gran = direction.1,
                );
                let _ = document::eval(&script);
                evt.prevent_default();
                return;
            }
            } // end #[cfg(not(feature = "native"))] visual-arrow block
            // ── Frontmatter row-nav override ──────────────
            //
            // When the caret sits inside the YAML frontmatter
            // and the user is in vim Normal mode, `j`/`k`
            // shouldn't crawl through hidden YAML bytes — they
            // should hop to the next/prev property row (the
            // Obsidian-properties feel). `i`/`a` focuses the
            // current row's value cell. Only activates when the
            // widget is showing (caret outside vim insert mode
            // *and* the parsed FM exists).
            let vim_peek = vim_sig.peek().clone();
            if matches!(vim_peek.mode, editor_vim::Mode::Normal)
                && !press.ctrl
                && !press.alt
                && !press.meta
            {
                let pre = cur.selection.primary().head;
                let doc_str = cur.doc.to_string();
                if let Some(fm) = editor_state::markdown::parse_frontmatter(&doc_str) {
                    // Only hijack keys when the caret is on an
                    // actual property row. On the opener/closer
                    // `---` lines the keys fall through to vim —
                    // otherwise `i` on line 1 flips to Insert,
                    // focuses nothing, and the caret appears to
                    // jump into the properties widget.
                    let on_prop_row = fm
                        .props
                        .iter()
                        .any(|p| pre >= p.range.start && pre < p.range.end);
                    if pre >= fm.outer.start && pre < fm.outer.end && on_prop_row {
                        let key = press.key.as_str();
                        if key == "j" || key == "k" {
                            let cur_idx = fm
                                .props
                                .iter()
                                .position(|p| pre >= p.range.start && pre < p.range.end)
                                .unwrap_or(if key == "j" { 0 } else { fm.props.len() - 1 });
                            let next_idx = if key == "j" {
                                (cur_idx + 1).min(fm.props.len() - 1)
                            } else {
                                cur_idx.saturating_sub(1)
                            };
                            let target = fm.props[next_idx].range.start;
                            crate::event::apply_tx(
                                state,
                                &cur,
                                editor_state::TransactionSpec::new()
                                    .selection(editor_state::Selection::caret(target))
                                    .annotate("origin", "fm-row-nav"),
                                sink_for_keys,
                            );
                            evt.prevent_default();
                            return;
                        }
                        if key == "i" || key == "a" || key == "Enter" {
                            // Flip vim to Insert so further keys
                            // (typed into the cell) aren't read
                            // as motions. The JS capture-phase
                            // listener above also stops them
                            // from reaching this handler, but
                            // mode-state needs to be coherent
                            // so a follow-up `<Esc>` from the
                            // cell drops the user back into
                            // Normal cleanly.
                            let mut next_vim = vim_peek.clone();
                            next_vim.mode = editor_vim::Mode::Insert;
                            vim_sig.set(next_vim);
                            // Dispatch a JS focus to the row's
                            // value cell. The contenteditable
                            // handles the rest.
                            let row_key = fm
                                .props
                                .iter()
                                .find(|p| pre >= p.range.start && pre < p.range.end)
                                .map_or("", |p| p.key.as_str());
                            let safe_key = row_key.replace('"', "\\\"");
                            let script = format!(
                                r#"(()=>{{const r=document.querySelector('.md-property-row[data-prop-key="{safe_key}"]');if(!r)return;const c=r.querySelector('[data-edit-role]');if(c){{c.focus();if(c.matches('input'))return;const rng=document.createRange();rng.selectNodeContents(c);rng.collapse(false);const s=window.getSelection();s.removeAllRanges();s.addRange(rng);}}}})();"#
                            );
                            let _ = document::eval(&script);
                            evt.prevent_default();
                            return;
                        }
                    }
                }
            }

            // `/` in Normal mode opens the slash palette when the
            // host wired one (Obsidian UX) — it must win over
            // vim's `/` search, so check BEFORE handle_key. Hosts
            // without a slash source get vim search on `/`;
            // `?` (backward search) is always vim's.
            if !press.ctrl
                && !press.alt
                && !press.meta
                && press.key == "/"
                && slash_for_keys.is_some()
                && matches!(vim_sig.peek().mode, editor_vim::Mode::Normal)
                && vim_sig.peek().pending_operator.is_none()
            {
                let head = cur.selection.primary().head;
                let mut next_vim = vim_sig.peek().clone();
                next_vim.mode = editor_vim::Mode::Insert;
                vim_sig.set(next_vim);
                crate::event::apply_tx(
                    state,
                    &cur,
                    TransactionSpec::new()
                        .changes(Changes::insert(head, "/"))
                        .selection(Selection::caret(head + 1))
                        .annotate("origin", "slash-trigger"),
                    sink_for_keys,
                );
                evt.prevent_default();
                return;
            }

            let mut vim_state = vim_sig.peek().clone();
            let spec = editor_vim::handle_key(&cur, &mut vim_state, &press);
            vim_sig.set(vim_state);
            if let Some(spec) = spec {
                evt.prevent_default();
                tracing::debug!(?press, "editor.vim.fire");
                crate::event::apply_tx(state, &cur, spec, sink_for_keys);
                return;
            }
            if !vim_sig.peek().is_inserting() {
                // Native renderer: there's no contenteditable to move the
                // caret on Arrow/Home/End in Normal mode, so drive it from
                // state before the blanket swallow. (Vim already had its
                // pass at h/j/k/l + motions above.)
                #[cfg(feature = "native")]
                if crate::native::handle_navigation(state, &cur, &press, sink_for_keys) {
                    evt.prevent_default();
                    return;
                }
                evt.prevent_default();
                return;
            }
        }
        if let Some(ref km) = keymap_for_keys {
            if let Some(spec) = km.dispatch(&press, &cur) {
                evt.prevent_default();
                tracing::debug!(?press, "editor.keymap.fire");
                crate::event::apply_tx(state, &cur, spec, sink_for_keys);
            }
        }
        // ── Native default-input fallthrough ─────────────────────────
        //
        // The web path stops here: an unclaimed key (a printable char, an
        // arrow in Insert mode, Enter) is left to the browser's
        // contenteditable, and the `bridge` observes the resulting DOM
        // mutation. Blitz has no contenteditable, so we *are* the default
        // action — move the caret, or insert the typed text, straight into
        // `editor-state`.
        #[cfg(feature = "native")]
        {
            if crate::native::handle_navigation(state, &cur, &press, sink_for_keys) {
                tracing::debug!(?press, "editor.native.nav");
                evt.prevent_default();
                return;
            }
            if crate::native::handle_text_input(state, &cur, &press, sink_for_keys) {
                tracing::debug!(?press, "editor.native.input");
                evt.prevent_default();
            }
        }
    };

    // ── Imperative DOM patch effect ──────────────────────────
    //
    // Fires on every state change. Serializes the tile tree to
    // a `Patch` description and hands it to the JS-side
    // `__cm_patch_<id>` function set up in the bridge.
    //
    // Dioxus reconciler stays out of the editor's content
    // entirely — we render only an empty `<div data-editor-id>`
    // and the patcher fills + maintains everything inside.
    // CM6 model.
    // Slash-state refresh: every time the doc or selection
    // changes, re-run `detect_slash` against the caret. Open the
    // menu when a fresh `/` trigger appears; close it when the
    // trigger goes away (user typed a space, deleted the `/`,
    // or moved the caret off the line).
    if let Some(mut slash_sig) = slash {
        use_effect(move || {
            let s = state.read();
            let caret = s.selection.primary().head;
            let detected = crate::slash::detect_slash(&s.doc.to_string(), caret);
            let cur = slash_sig.peek().clone();
            match (detected, cur) {
                (Some((start, q)), Some(prev)) if prev.slash_start == start => {
                    // Same trigger, query updated. Clamp the
                    // selected row to the new hit count.
                    let hits_len = crate::slash::filter_commands(&q).len();
                    let selected = prev.selected.min(hits_len.saturating_sub(1));
                    if prev.query != q || prev.selected != selected {
                        slash_sig.set(Some(crate::slash::SlashState {
                            slash_start: start,
                            query: q,
                            selected,
                        }));
                    }
                }
                (Some((start, q)), _) => {
                    slash_sig.set(Some(crate::slash::SlashState {
                        slash_start: start,
                        query: q,
                        selected: 0,
                    }));
                }
                (None, Some(_)) => slash_sig.set(None),
                _ => {}
            }
        });
    }

    // Completion-state refresh: on every doc/selection change,
    // re-run `detect_trigger` against the caret. Open the menu on a
    // fresh `[[` / `#` trigger (fetching candidates from the host's
    // source), refresh candidates as the query grows, close it when
    // the trigger goes away. Mirrors the slash refresh above.
    if let Some(source) = completion.clone() {
        let mut comp_sig = completion_state;
        use_effect(move || {
            let s = state.read();
            let caret = s.selection.primary().head;
            let detected = crate::trigger::detect_trigger(&s.doc.to_string(), caret);
            let prev = comp_sig.peek().clone();
            match (detected, prev) {
                (Some((kind, start, q)), Some(prev))
                    if prev.kind == kind && prev.trigger_start == start
                    // Same trigger, query updated. Re-fetch and clamp
                    // the highlighted row to the new candidate count.
                    && prev.query != q => {
                        let candidates = source.run(&q, kind);
                        let selected = prev.selected.min(candidates.len().saturating_sub(1));
                        comp_sig.set(Some(crate::trigger::CompletionState {
                            kind,
                            trigger_start: start,
                            query: q,
                            selected,
                            candidates,
                        }));
                    }
                (Some((kind, start, q)), _) => {
                    let candidates = source.run(&q, kind);
                    comp_sig.set(Some(crate::trigger::CompletionState {
                        kind,
                        trigger_start: start,
                        query: q,
                        selected: 0,
                        candidates,
                    }));
                }
                (None, Some(_)) => comp_sig.set(None),
                _ => {}
            }
        });
    }

    // Web/desktop only: ship the tile tree to the JS patcher via
    // `document::eval`. The native renderer has no JS engine (eval no-ops)
    // and renders the tile tree as rsx children instead (`native_content`
    // below), so it skips this whole effect.
    #[cfg(not(feature = "native"))]
    {
        let id = editor_id.clone();
        let deco_source_patch = decorations.clone();
        // Mut copies of the incremental-patch signals for the move closure
        // (Signal is Copy; `.set()` needs a mutable binding).
        let mut prev_line_hashes = prev_line_hashes;
        let mut force_full_patch = force_full_patch;
        use_effect(move || {
            let _span = tracing::debug_span!("editor.patch_effect").entered();
            let s = state.read();
            let doc_len = s.doc.len();
            // Ask sources for the full set once editing settles, and only
            // the cheap structural set while typing. Reading `idle()` here
            // subscribes the effect, so the flip back to idle re-runs this
            // pass and the overlays/diagnostics reappear.
            editor_state::set_deco_phase(if idle() {
                editor_state::DecoPhase::Full
            } else {
                editor_state::DecoPhase::Structural
            });
            let t_decos = now_ms();
            let decorations: Vec<DecoratedRange> = {
                let mut v = match &deco_source_patch {
                    Some(src) => src.run(&s),
                    None => Vec::new(),
                };
                // Painted modal caret (block/underscore) — the native
                // caret can't take these shapes everywhere (CSS
                // `caret-shape` is Chromium-only as of 2026; Firefox
                // shows a bar), so non-insert vim modes paint theirs
                // as a decoration and CSS hides the native caret.
                v.extend(modal_caret_decoration(&s, vim, editor_focused));
                v.sort_by_key(|d| d.from);
                v
            };
            let deco_count = decorations.len();
            let decos_ms = now_ms() - t_decos;
            let t_build = now_ms();
            let (arena, root) = build_tiles(&s.doc.to_string(), &decorations);
            let build_ms = now_ms() - t_build;
            let t_patch = now_ms();
            let patch = build_patch(&arena, root);
            let patch_ms = now_ms() - t_patch;

            // Incremental diff: hash each line patch and compare against
            // the previous render's hashes to find the first changed
            // line. We ship only `patch[first_changed..]`; the JS patcher
            // leaves the unchanged prefix lines alone. A `force_full`
            // flag (first render / post-IME-composition resync) sends the
            // whole thing. `prev_line_hashes` is updated unconditionally:
            // the only time JS skips applying is during composition, and
            // `force_full` covers the resync after it.
            let new_hashes: Vec<u64> = patch.iter().map(hash_patch).collect();
            // `peek()` so the effect doesn't subscribe to a signal it also
            // writes (that would loop). The recv loop sets it true on
            // composition; this pass reads the current value at run time.
            let first_changed = if *force_full_patch.peek() {
                force_full_patch.set(false);
                0
            } else {
                let prev = prev_line_hashes.peek();
                new_hashes
                    .iter()
                    .zip(prev.iter())
                    .take_while(|(a, b)| a == b)
                    .count()
            };
            prev_line_hashes.set(new_hashes);
            let shipped: &[crate::tile::patch::Patch] =
                patch.get(first_changed..).unwrap_or(&[]);

            tracing::debug!(
                doc_len,
                deco_count,
                line_count = patch.len(),
                first_changed,
                shipped = shipped.len(),
                decos_ms = %format!("{:.2}", decos_ms),
                build_ms = %format!("{:.2}", build_ms),
                patch_ms = %format!("{:.2}", patch_ms),
                "editor.patch.build"
            );
            let primary = s.selection.primary();
            // Send anchor/head separately (not sorted) so JS uses
            // `setBaseAndExtent` and preserves direction. Otherwise
            // shift+arrow-left wouldn't extend backward — every
            // restored selection would have its head at the right
            // edge, and the keyboard would extend further right.
            // Send the full doc text along with the patch so the
            // JS-side copy handler can produce *source* markdown
            // when the user copies — the rendered DOM differs
            // (widgets replace markers, ` ``` ` fences are
            // hidden, etc.), and copying the rendered view would
            // be uneditable on paste.
            let payload = serde_json::json!({
                "patches": shipped,
                "firstChanged": first_changed,
                "doc": s.doc.to_string(),
                "selection": {
                    "anchor": primary.anchor,
                    "head": primary.head,
                },
            });
            let patch_json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
            let patch_json_lit =
                serde_json::to_string(&patch_json).unwrap_or_else(|_| "\"\"".into());
            let script = format!(
                r"
                (function() {{
                    const payload = {patch_json_lit};
                    // Always stash the latest payload — the
                    // bridge consumes it on attach so we never
                    // race patches against in-flight retries.
                    window['__cm_pending_{id}'] = payload;
                    const fn = window['__cm_patch_{id}'];
                    if (typeof fn === 'function') fn(payload);
                }})();
                "
            );
            let _ = document::eval(&script);
        });
    }

    // CSS hook so caret-shape / theming can branch on vim mode.
    // Falls back to `vim-mode-insert` (bar caret) when vim is
    // disabled — same as plain editing.
    let vim_class = vim.map_or("vim-mode-insert", |sig| match sig.peek().mode {
        editor_vim::Mode::Normal => "vim-mode-normal",
        editor_vim::Mode::Insert => "vim-mode-insert",
        editor_vim::Mode::VisualChar
        | editor_vim::Mode::VisualLine
        | editor_vim::Mode::VisualBlock => "vim-mode-visual",
        editor_vim::Mode::Replace => "vim-mode-replace",
        editor_vim::Mode::Command => "vim-mode-command",
    });
    let read_only = !editable || state.read().reading_mode;
    let root_class = if read_only {
        format!("editor-root reading-mode {vim_class}")
    } else {
        format!("editor-root {vim_class}")
    };
    let contenteditable = if read_only { "false" } else { "plaintext-only" };

    // ── Native render content ────────────────────────────────────
    //
    // On the web/desktop path the root div is empty and the JS patcher
    // fills it (above). The native renderer has no patcher, so we build the
    // tile tree here and emit it as rsx children — the Dioxus reconciler
    // diffs it against Blitz's DOM. Decorations are assembled the same way
    // the patch effect does, plus the always-on painted caret (Blitz draws
    // no OS caret on a plain div).
    #[cfg(feature = "native")]
    let native_content: Element = {
        let s = state.read();
        editor_state::set_deco_phase(if idle() {
            editor_state::DecoPhase::Full
        } else {
            editor_state::DecoPhase::Structural
        });
        let mut decos: Vec<DecoratedRange> = decorations
            .as_ref()
            .map(|src| src.run(&s))
            .unwrap_or_default();
        // Selection highlight FIRST so the caret decoration paints on top.
        decos.extend(crate::native::native_selection_decoration(&s, vim, editor_focused));
        decos.extend(modal_caret_decoration(&s, vim, editor_focused));
        decos.extend(crate::native::native_caret_decoration(&s, vim, editor_focused));
        decos.sort_by_key(|d| d.from);
        let (arena, root) = build_tiles(&s.doc.to_string(), &decos);
        // Click-to-position: Blitz has no contenteditable, so each rendered
        // element reports its doc offset here. Route through `push_selection`
        // so the caret snaps off atomic/hidden ranges the same way the web
        // selection bridge does.
        let on_click = {
            let click_deco = decorations.clone();
            let click_sink = on_transaction;
            let mut click_state = state;
            Callback::new(move |pos: usize| {
                // A click in the content is user intent to edit here —
                // count it as focus (Blitz won't fire focusin for it).
                let mut focused = editor_focused;
                if !*focused.peek() {
                    focused.set(true);
                }
                let cur = click_state.read().clone();
                push_selection(&mut click_state, &cur, click_deco.as_ref(), click_sink, pos, pos);
            })
        };
        crate::tile::render_dx::render_tile(&arena, root, on_click)
    };

    // The root div differs by renderer: native emits the tile tree as rsx
    // children; web leaves it empty for the JS patcher to own. rsx can't
    // `#[cfg]` an individual child node, so the two variants are whole
    // blocks. Everything else (focus tracking, overlays) is identical.
    //
    #[cfg(feature = "native")]
    let rendered = rsx! {
        div {
            // `ed-render-native` scopes the Blitz-specific caret metrics
            // (inline-block block caret + advance-width compensation) that
            // would shift text on the web renderer.
            class: "{root_class} ed-render-native",
            "data-editor-id": "{editor_id}",
            contenteditable: "{contenteditable}",
            spellcheck: "false",
            tabindex: "0",
            // Native: Blitz routes key events to the focused node, so the
            // editor must hold focus to be typable. `autofocus` claims it
            // on mount; the FTS blitz fork focuses the nearest focussable
            // ancestor (this div, tabindex 0) on pointerdown — upstream
            // only click-focuses text inputs, which left the editor
            // unfocusable by mouse. (MountedData::set_focus from a spawned
            // task is NOT an option: it re-borrows the doc RefCell during
            // the vdom poll and panics.)
            autofocus: "true",
            onkeydown: on_keydown,
            onfocusin: move |_| {
                let mut focused = editor_focused;
                if !*focused.peek() { focused.set(true); }
            },
            onfocusout: move |_| {
                let mut focused = editor_focused;
                if *focused.peek() { focused.set(false); }
            },
            // Rendered tile tree — the Dioxus reconciler diffs it against
            // Blitz's DOM (no JS patcher on native).
            {native_content}
        }
        if let Some(popup) = hover_state.read().clone() {
            crate::hover::HoverTooltipView { popup }
        }
        if completion.is_some() {
            crate::trigger::CompletionMenu {
                state,
                completion: completion_state,
                on_transaction,
            }
        }
    };

    #[cfg(not(feature = "native"))]
    let rendered = rsx! {
        div {
            class: "{root_class}",
            "data-editor-id": "{editor_id}",
            contenteditable: "{contenteditable}",
            spellcheck: "false",
            // Focusable so tab order is sane; the contenteditable is
            // already focusable, so this is harmless.
            tabindex: "0",
            onkeydown: on_keydown,
            // Focus tracking for the painted modal caret: like the
            // native caret, it must not render until the user
            // actually focuses (clicks into) the editor.
            onfocusin: move |_| {
                let mut focused = editor_focused;
                if !*focused.peek() { focused.set(true); }
            },
            onfocusout: move |_| {
                let mut focused = editor_focused;
                if *focused.peek() { focused.set(false); }
            },
        }
        if let Some(popup) = hover_state.read().clone() {
            crate::hover::HoverTooltipView { popup }
        }
        if completion.is_some() {
            crate::trigger::CompletionMenu {
                state,
                completion: completion_state,
                on_transaction,
            }
        }
    };

    rendered
}

/// The painted modal caret: a block over the char under the cursor
/// in Normal/Visual modes, an underscore in Replace. Insert/Command
/// (and vim disabled) keep the native bar caret. Painted rather than
/// CSS `caret-shape` — that property is Chromium-only (Firefox/Zen
/// silently fall back to a bar), and a decoration renders identically
/// everywhere; `.editor-root.vim-mode-*` CSS hides the native caret
/// in the painted modes.
///
/// Reading the vim and focus signals here (from the patch effect)
/// subscribes the effect to mode flips and focus changes, so i↔Esc
/// and click-in/click-away repaint immediately. Nothing paints while
/// the editor is unfocused — same contract as the native caret.
fn modal_caret_decoration(
    s: &EditorState,
    vim: Option<Signal<editor_vim::VimState>>,
    focused: Signal<bool>,
) -> Vec<DecoratedRange> {
    let Some(vim) = vim else {
        return Vec::new();
    };
    if !focused() || s.reading_mode {
        return Vec::new();
    }
    let class = match vim.read().mode {
        editor_vim::Mode::Normal
        | editor_vim::Mode::VisualChar
        | editor_vim::Mode::VisualLine
        | editor_vim::Mode::VisualBlock => "ed-modal-caret-block",
        editor_vim::Mode::Replace => "ed-modal-caret-underscore",
        editor_vim::Mode::Insert | editor_vim::Mode::Command => return Vec::new(),
    };
    let rope = s.doc.rope();
    let head = s.selection.primary().head.min(rope.len_bytes());
    let ch = rope.byte_to_char(head);
    if ch < rope.len_chars() && rope.char(ch) != '\n' {
        let end = rope.char_to_byte(ch + 1);
        vec![DecoratedRange::mark(head..end, class)]
    } else {
        // End of line / end of doc: no char to paint over — drop a
        // fixed-width block widget in the empty cell instead (vim's
        // caret legitimately occupies the cell past the last char).
        vec![DecoratedRange::widget(
            head,
            format!("<span class=\"{class} ed-modal-caret-empty\">\u{a0}</span>"),
        )]
    }
}

/// Clamp-detection guard: when state has a selection that
/// covers more doc than the DOM can currently represent (e.g.,
/// after `select_all` on a doc with Hidden markdown markers,
/// state is `(0, doc.len)` but DOM Selection clamps to visible
/// content's end), the next `selectionchange`/`keyup` would
/// otherwise read the clamped value and shrink state. We
/// recognize this as "DOM is a strict subset of cur" and skip
/// the update — state remains authoritative. Mirrors CM6's
/// `domobserver` ignoring DOM selection changes that are
/// derivable from current state.
pub(crate) fn push_selection(
    state: &mut Signal<EditorState>,
    cur: &EditorState,
    deco_source: Option<&DecorationSource>,
    sink: Option<Callback<crate::TransactionEvent>>,
    s: usize,
    e: usize,
) {
    let doc_len = cur.doc.len();
    let mut s = s.min(doc_len);
    let mut e = e.min(doc_len);
    // Atomic-range snap: if either endpoint lands strictly
    // inside an atomic decoration, jump it to the nearer edge.
    // Mirrors CM6's `skipAtomicRanges` (`view/src/cursor.ts`).
    // Atomic ranges are structural (behavior-only, cheap), so this
    // never needs the expensive analysis pass — pin Structural.
    if let Some(src) = deco_source {
        editor_state::set_deco_phase(editor_state::DecoPhase::Structural);
        let decs = src.run(cur);
        s = editor_state::decoration::skip_atomic(&decs, s);
        e = editor_state::decoration::skip_atomic(&decs, e);
    }
    let cur_primary = cur.selection.primary();
    if cur_primary.anchor == s && cur_primary.head == e {
        return;
    }
    // Clamp detection: cur has a non-caret selection extending
    // past where DOM ends (head/anchor at doc end), and the
    // incoming range is a subset of cur. Trust state.
    let cur_from = cur_primary.from();
    let cur_to = cur_primary.to();
    let incoming_from = s.min(e);
    let incoming_to = s.max(e);
    let cur_nontrivial = cur_from != cur_to;
    let incoming_is_subset = incoming_from >= cur_from && incoming_to <= cur_to;
    let cur_reaches_doc_end = cur_to == doc_len;
    if cur_nontrivial && incoming_is_subset && cur_reaches_doc_end && incoming_to < cur_to {
        tracing::trace!(
            cur_from,
            cur_to,
            incoming_from,
            incoming_to,
            "editor.selection.ignored_clamp"
        );
        return;
    }
    // Orphaned-selection guard: a (0, 0) coming in when cur is
    // non-zero is almost always the Dioxus reconciler removing
    // the text node our Selection was anchored to and the
    // browser falling back to editor-root position 0. Real
    // jumps-to-doc-start come from `Home` / arrow / click —
    // those events have separate paths (keyup, mouseup,
    // click) AND the user can re-place the caret if our guess
    // was wrong.
    if s == 0 && e == 0 && cur_primary.head != 0 && cur_primary.anchor != 0 {
        tracing::trace!(
            cur_anchor = cur_primary.anchor,
            cur_head = cur_primary.head,
            "editor.selection.ignored_orphan"
        );
        return;
    }
    tracing::trace!(
        old_anchor = cur_primary.anchor,
        old_head = cur_primary.head,
        new_start = s,
        new_end = e,
        "editor.selection"
    );
    let new_sel = Selection::single(Range::new(s, e));
    crate::event::apply_tx(
        *state,
        cur,
        TransactionSpec::new()
            .selection(new_sel)
            .annotate("origin", "input"),
        sink,
    );
}

/// Compute a minimal `Changes` between two strings by trimming
/// common prefix + suffix and replacing the diff in the middle.
/// O(n) — good enough for typing and small pastes; replace with
/// a proper diff algorithm later if we want minimal ops for
/// large pastes too.
pub(crate) fn diff_text(old: &str, new: &str) -> Changes {
    let ob = old.as_bytes();
    let nb = new.as_bytes();
    let mut start = 0;
    while start < ob.len() && start < nb.len() && ob[start] == nb[start] {
        start += 1;
    }
    let mut o_end = ob.len();
    let mut n_end = nb.len();
    while o_end > start && n_end > start && ob[o_end - 1] == nb[n_end - 1] {
        o_end -= 1;
        n_end -= 1;
    }
    // Walk back to a UTF-8 boundary in case our binary trim
    // landed in the middle of a multi-byte sequence.
    while start > 0 && !old.is_char_boundary(start) {
        start -= 1;
    }
    while o_end < ob.len() && !old.is_char_boundary(o_end) {
        o_end += 1;
    }
    while n_end < nb.len() && !new.is_char_boundary(n_end) {
        n_end += 1;
    }
    let inserted = &new[start..n_end];
    Changes::replace(start..o_end, inserted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_appends_one_char() {
        let c = diff_text("hello", "helloa");
        let cs: Vec<_> = c.iter().cloned().collect();
        assert_eq!(cs.len(), 1);
        let only = &cs[0];
        assert_eq!(only.from, 5);
        assert_eq!(only.to, 5);
        assert_eq!(only.inserted, "a");
    }

    #[test]
    fn diff_inserts_in_middle() {
        let c = diff_text("hello world", "hello big world");
        let cs: Vec<_> = c.iter().cloned().collect();
        assert_eq!(cs.len(), 1);
        let only = &cs[0];
        assert_eq!(only.from, 6);
        assert_eq!(only.to, 6);
        assert_eq!(only.inserted, "big ");
    }

    #[test]
    fn diff_deletes_one_char() {
        let c = diff_text("helloa", "hello");
        let cs: Vec<_> = c.iter().cloned().collect();
        assert_eq!(cs.len(), 1);
        let only = &cs[0];
        assert_eq!(only.from, 5);
        assert_eq!(only.to, 6);
        assert_eq!(only.inserted, "");
    }

    #[test]
    fn diff_replaces_range() {
        let c = diff_text("hello world", "hello RUST");
        let cs: Vec<_> = c.iter().cloned().collect();
        assert_eq!(cs.len(), 1);
        let only = &cs[0];
        assert_eq!(only.from, 6);
        assert_eq!(only.to, 11);
        assert_eq!(only.inserted, "RUST");
    }

    #[test]
    fn diff_identical_is_empty() {
        let c = diff_text("hello", "hello");
        let cs: Vec<_> = c.iter().cloned().collect();
        assert_eq!(cs.len(), 1);
        // Trim-and-replace algorithm returns a no-op
        // `replace(5..5, "")`. Functionally equivalent to empty
        // for our apply path; could collapse in a follow-up.
        let only = &cs[0];
        assert_eq!(only.from, only.to);
        assert_eq!(only.inserted, "");
    }
}
