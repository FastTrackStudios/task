//! Decorations — visual overlays on the document. The document
//! itself is plain text; decorations say "wrap these characters
//! in a `<strong>`", "hide these two characters from the
//! rendered view", or "insert a widget here".
//!
//! Mirrors `@codemirror/view`'s `Decoration` (defined in
//! `~/Development/research/codemirror/view/src/decoration.ts`).
//! We keep four variants — the four CM6 ships — but only `Mark`
//! and `Replace` are wired into the v1 view.

/// What a decoration *does* visually.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecorationKind {
    /// Wrap the covered range in an element with the given CSS
    /// class. Bold, italic, link, etc. `attrs` are extra HTML
    /// attributes copied verbatim onto the span (e.g.
    /// `data-href` for clickable links).
    Mark {
        class: String,
        attrs: Vec<(String, String)>,
    },
    /// Remove the covered range from the rendered output (the
    /// document still contains it). Used to hide markdown
    /// markers like `**` once the cursor leaves the span.
    Replace,
    /// Inject content at a point that isn't in the document.
    /// Used for inline widgets like a checkbox in front of a
    /// task. v1 stores the HTML as a string; later we can swap
    /// to a typed widget enum or Dioxus `VNode`.
    Widget { html: String },
    /// Add a CSS class to the line element containing the
    /// position. Used for the active-line highlight.
    Line { class: String },
    /// Treat the range as a single indivisible unit for cursor
    /// purposes. A caret landing inside snaps to the nearer
    /// edge; a non-empty selection that overlaps extends to
    /// cover the whole range. Behavior-only — does not change
    /// rendering. Ports CM6's `atomicRanges` facet
    /// (`view/src/extension.ts:295`, applied by
    /// `view/src/cursor.ts:skipAtomicRanges`).
    Atomic,
}

/// A decoration is just an inclusive-start, exclusive-end byte
/// range with a [`DecorationKind`]. Widgets and line decorations
/// are zero-width (`from == to`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecoratedRange {
    pub from: usize,
    pub to: usize,
    pub kind: DecorationKind,
}

impl DecoratedRange {
    pub fn mark(range: std::ops::Range<usize>, class: impl Into<String>) -> Self {
        Self {
            from: range.start,
            to: range.end,
            kind: DecorationKind::Mark {
                class: class.into(),
                attrs: Vec::new(),
            },
        }
    }
    pub fn mark_with_attrs(
        range: std::ops::Range<usize>,
        class: impl Into<String>,
        attrs: Vec<(String, String)>,
    ) -> Self {
        Self {
            from: range.start,
            to: range.end,
            kind: DecorationKind::Mark {
                class: class.into(),
                attrs,
            },
        }
    }

    #[must_use]
    pub fn replace(range: std::ops::Range<usize>) -> Self {
        Self {
            from: range.start,
            to: range.end,
            kind: DecorationKind::Replace,
        }
    }

    pub fn widget(at: usize, html: impl Into<String>) -> Self {
        Self {
            from: at,
            to: at,
            kind: DecorationKind::Widget { html: html.into() },
        }
    }

    pub fn line(at: usize, class: impl Into<String>) -> Self {
        Self {
            from: at,
            to: at,
            kind: DecorationKind::Line {
                class: class.into(),
            },
        }
    }
    #[must_use]
    pub fn atomic(range: std::ops::Range<usize>) -> Self {
        Self {
            from: range.start,
            to: range.end,
            kind: DecorationKind::Atomic,
        }
    }

    #[must_use]
    pub fn byte_range(&self) -> std::ops::Range<usize> {
        self.from..self.to
    }
}

/// Snap `pos` out of any atomic range it lands strictly inside,
/// preferring the nearer edge (CM6 picks the nearer edge when
/// `bias == 0`). Used to keep callers' selections from landing
/// in the middle of an atomic region. No-op when `pos` is at a
/// range boundary or no atomic range contains it.
///
/// Ports CM6's `skipAtomicRanges` (`view/src/cursor.ts`).
#[must_use]
pub fn skip_atomic(decs: &[DecoratedRange], pos: usize) -> usize {
    let mut p = pos;
    loop {
        let mut moved = false;
        for d in decs {
            if !matches!(d.kind, DecorationKind::Atomic) {
                continue;
            }
            if p > d.from && p < d.to {
                p = if p - d.from <= d.to - p { d.from } else { d.to };
                moved = true;
            }
        }
        if !moved {
            return p;
        }
    }
}

/// Type alias used by extensions that produce many decorations
/// per render. Sources still hand back a plain `Vec`; the view
/// wraps it in a [`DecoRangeSet`] for efficient span queries.
pub type DecorationSet = Vec<DecoratedRange>;

/// A decoration set indexed for fast "what overlaps this byte
/// span?" queries — the read side of CM6's `RangeSet`
/// (`@codemirror/state/src/rangeset.ts`).
///
/// Decoration *sources* stay as `fn(&EditorState) -> Vec<…>`; the
/// view builds a `DecoRangeSet` from that `Vec` once per render and
/// queries it. The win is [`overlapping`]: when the viewport only
/// renders the visible lines, it needs the decorations touching a
/// byte window without scanning the whole document's set.
///
/// Storage is the ranges sorted by `from`, plus a prefix-maximum of
/// `to` (`max_end[i] = max(ranges[0..=i].to)`). To find everything
/// overlapping `[from, to)` we binary-search the last range whose
/// `from < to`, then walk left while `max_end` still reaches past
/// `from` — stopping as soon as it can't, since the prefix max is
/// monotonic. That's `O(log n + k)` in the common case (short,
/// line-local decorations); a pathological long-spanning decoration
/// near the document start degrades the left-walk toward `O(n)`,
/// which CM6's chunk tree avoids and we can swap in later if real
/// charts ever need it.
///
/// We deliberately do *not* port CM6's balanced chunk tree: it earns
/// its keep on huge, incrementally-mapped sets, and our sources
/// rebuild a `Vec` each pass anyway.
///
/// [`overlapping`]: DecoRangeSet::overlapping
#[derive(Clone, Debug, Default)]
pub struct DecoRangeSet {
    /// Ranges sorted by `from` ascending (ties keep input order).
    ranges: Vec<DecoratedRange>,
    /// `max_end[i] = max(ranges[j].to for j in 0..=i)`. Monotonic
    /// non-decreasing, so a left-walk can stop once it drops to or
    /// below the query's `from`.
    max_end: Vec<usize>,
}

impl DecoRangeSet {
    /// Build from an unsorted decoration list. Sorts by `from` and
    /// computes the prefix-max-end index. `O(n log n)`.
    #[must_use]
    pub fn new(mut ranges: Vec<DecoratedRange>) -> Self {
        ranges.sort_by_key(|d| d.from);
        let mut max_end = Vec::with_capacity(ranges.len());
        let mut running = 0usize;
        for r in &ranges {
            running = running.max(r.to);
            max_end.push(running);
        }
        Self { ranges, max_end }
    }

    /// Number of decorations in the set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    /// `true` when the set holds no decorations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// The full sorted slice — for consumers that want every
    /// decoration (e.g. building the whole-document tile tree).
    #[must_use]
    pub fn as_slice(&self) -> &[DecoratedRange] {
        &self.ranges
    }

    /// Every decoration overlapping the byte span `[from, to)`,
    /// returned in sorted-by-`from` order.
    ///
    /// "Overlap" is half-open: a range `[a, b)` overlaps `[from, to)`
    /// when `a < to && b > from`. Zero-width decorations (widgets,
    /// line marks, where `a == b`) count as overlapping when `from <=
    /// a < to`, *or* `a == from` (so a widget exactly at the window
    /// start is included) — they sit *at* a point inside the window.
    #[must_use]
    pub fn overlapping(&self, from: usize, to: usize) -> Vec<DecoratedRange> {
        let mut out = Vec::new();
        self.for_each_overlapping(from, to, |d| out.push(d.clone()));
        out.sort_by_key(|d| d.from);
        out
    }

    /// Visit each decoration overlapping `[from, to)` (unspecified
    /// order — sort if you need one). Avoids the clone+alloc of
    /// [`overlapping`] for hot scan loops.
    pub fn for_each_overlapping(&self, from: usize, to: usize, mut f: impl FnMut(&DecoratedRange)) {
        // Upper bound: first index whose `from >= to`. Everything at
        // or past it starts after the window and can't overlap.
        let hi = self.ranges.partition_point(|d| d.from < to);
        // Walk left from `hi-1`. The prefix-max-end is monotonic, so
        // once `max_end[i] <= from` no earlier range can reach into
        // the window and we stop.
        let mut i = hi;
        while i > 0 {
            i -= 1;
            if self.max_end[i] <= from {
                // A zero-width point exactly at `from` has `to == from`
                // and would be excluded by the `<= from` cutoff, so
                // check it explicitly before bailing.
                if self.ranges[i].from == from && self.ranges[i].to == from && from < to {
                    f(&self.ranges[i]);
                }
                break;
            }
            let d = &self.ranges[i];
            let overlaps = if d.from == d.to {
                // Zero-width point: inside the half-open window, or
                // exactly at its start.
                d.from >= from && d.from < to
            } else {
                d.from < to && d.to > from
            };
            if overlaps {
                f(d);
            }
        }
    }

    /// Map every decoration through a document edit, returning the
    /// shifted set. A range's `from` binds *before* (stays put when
    /// text is inserted at its left edge); its `to` binds *after* (so
    /// the range grows to swallow text appended at its right edge) —
    /// CM6's default mark inclusivity. Ranges that collapse to nothing
    /// inside a deletion are dropped; zero-width points map their
    /// single offset with `Before`.
    ///
    /// This is the hook for incrementally-maintained decoration
    /// providers (map the old set forward instead of recomputing) —
    /// CM6's `RangeSet.map`. Today's sources recompute, so it's used
    /// by tests and future providers.
    #[must_use]
    pub fn map_through(&self, changes: &crate::Changes) -> Self {
        use crate::change::Assoc;
        let mut mapped = Vec::with_capacity(self.ranges.len());
        for d in &self.ranges {
            if d.from == d.to {
                let at = changes.map_position(d.from, Assoc::Before);
                mapped.push(DecoratedRange {
                    from: at,
                    to: at,
                    kind: d.kind.clone(),
                });
                continue;
            }
            let from = changes.map_position(d.from, Assoc::Before);
            let to = changes.map_position(d.to, Assoc::After);
            if to > from {
                mapped.push(DecoratedRange {
                    from,
                    to,
                    kind: d.kind.clone(),
                });
            }
        }
        Self::new(mapped)
    }
}

/// Which pass a decoration source is being asked to produce.
///
/// The view re-runs decoration sources on *every* keystroke (to keep
/// hidden-marker / widget layout in sync with the text). Cheap,
/// structural work — syntax highlighting, live-preview marker hiding —
/// has to run on each of those passes. Expensive, whole-document
/// analysis — language diagnostics, resolved-chord overlays — does
/// not: it can lag a beat behind the cursor without anyone noticing.
///
/// So the view tags each pass: [`Structural`] while the user is
/// actively typing (skip the expensive analysis), [`Full`] once typing
/// settles (recompute everything). A source consults [`deco_phase`] to
/// decide whether to do the expensive work. Sources that are entirely
/// cheap — and sources whose output the layout depends on, like
/// marker-hiding `Replace` decorations — ignore the phase and always
/// produce their full set.
///
/// This is a process-global, set by the view immediately before it
/// calls a source and read synchronously inside it (both happen on the
/// UI thread, so there's no interleaving). Mirrors the global-flag
/// pattern the keyflow overlay toggle already uses.
///
/// [`Structural`]: DecoPhase::Structural
/// [`Full`]: DecoPhase::Full
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecoPhase {
    /// Active-typing pass — produce only cheap, layout-affecting
    /// decorations; skip expensive whole-document analysis.
    Structural,
    /// Settled pass — produce everything, including analysis-derived
    /// diagnostics and overlays.
    Full,
}

static DECO_PHASE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

/// Set the [`DecoPhase`] for the next decoration-source call. Called by
/// the view right before invoking a source.
pub fn set_deco_phase(phase: DecoPhase) {
    let v = match phase {
        DecoPhase::Structural => 0,
        DecoPhase::Full => 1,
    };
    DECO_PHASE.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// The current [`DecoPhase`]. Decoration sources read this to decide
/// whether to run expensive analysis. Defaults to [`DecoPhase::Full`]
/// so direct callers (tests, non-view consumers) get the complete set
/// without opting in.
#[must_use]
pub fn deco_phase() -> DecoPhase {
    if DECO_PHASE.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        DecoPhase::Structural
    } else {
        DecoPhase::Full
    }
}

/// Marker type re-exported so callers can `use editor_state::Decoration;`
/// and reach the constructors without ceremony.
pub struct Decoration;

impl Decoration {
    pub fn mark(range: std::ops::Range<usize>, class: impl Into<String>) -> DecoratedRange {
        DecoratedRange::mark(range, class)
    }
    pub fn mark_with_attrs(
        range: std::ops::Range<usize>,
        class: impl Into<String>,
        attrs: Vec<(String, String)>,
    ) -> DecoratedRange {
        DecoratedRange::mark_with_attrs(range, class, attrs)
    }
    #[must_use]
    pub fn replace(range: std::ops::Range<usize>) -> DecoratedRange {
        DecoratedRange::replace(range)
    }
    pub fn widget(at: usize, html: impl Into<String>) -> DecoratedRange {
        DecoratedRange::widget(at, html)
    }
    pub fn line(at: usize, class: impl Into<String>) -> DecoratedRange {
        DecoratedRange::line(at, class)
    }
    #[must_use]
    pub fn atomic(range: std::ops::Range<usize>) -> DecoratedRange {
        DecoratedRange::atomic(range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_constructor() {
        let d = Decoration::mark(2..5, "bold");
        assert_eq!(d.from, 2);
        assert_eq!(d.to, 5);
        assert_eq!(
            d.kind,
            DecorationKind::Mark {
                class: "bold".into(),
                attrs: Vec::new(),
            }
        );
    }

    #[test]
    fn replace_constructor() {
        let d = Decoration::replace(0..2);
        assert!(matches!(d.kind, DecorationKind::Replace));
    }

    #[test]
    fn widget_is_zero_width() {
        let d = Decoration::widget(7, "<span/>");
        assert_eq!(d.from, d.to);
    }

    fn classes(decs: &[DecoratedRange]) -> Vec<(usize, usize)> {
        decs.iter().map(|d| (d.from, d.to)).collect()
    }

    #[test]
    fn rangeset_overlapping_basic() {
        let set = DecoRangeSet::new(vec![
            Decoration::mark(0..3, "a"),
            Decoration::mark(5..9, "b"),
            Decoration::mark(12..15, "c"),
        ]);
        // Window squarely over the middle range.
        assert_eq!(classes(&set.overlapping(6, 8)), vec![(5, 9)]);
        // Window touching the boundary of `a` (to is exclusive at 3).
        assert_eq!(classes(&set.overlapping(3, 5)), vec![]);
        // Window spanning all three.
        assert_eq!(classes(&set.overlapping(0, 20)), vec![(0, 3), (5, 9), (12, 15)]);
    }

    #[test]
    fn rangeset_overlapping_long_span_from_earlier() {
        // A long decoration that starts well before the query window
        // must still be found (the prefix-max-end left-walk).
        let set = DecoRangeSet::new(vec![
            Decoration::mark(0..100, "block"),
            Decoration::mark(50..52, "tok"),
        ]);
        assert_eq!(classes(&set.overlapping(60, 62)), vec![(0, 100)]);
        assert_eq!(classes(&set.overlapping(50, 52)), vec![(0, 100), (50, 52)]);
    }

    #[test]
    fn rangeset_overlapping_includes_zero_width_points() {
        let set = DecoRangeSet::new(vec![
            Decoration::widget(5, "<x/>"),
            Decoration::mark(2..8, "m"),
        ]);
        // Widget at 5 is inside [4,6).
        assert_eq!(classes(&set.overlapping(4, 6)), vec![(2, 8), (5, 5)]);
        // Widget exactly at the window start is included.
        assert_eq!(classes(&set.overlapping(5, 9)), vec![(2, 8), (5, 5)]);
        // Widget just past the (exclusive) window end is not.
        assert_eq!(classes(&set.overlapping(0, 5)), vec![(2, 8)]);
    }

    #[test]
    fn rangeset_map_through_shifts_and_grows() {
        let set = DecoRangeSet::new(vec![Decoration::mark(4..8, "m")]);
        // Insert 3 chars before the range → it slides right by 3.
        let mapped = set.map_through(&crate::Changes::insert(0, "abc"));
        assert_eq!(classes(mapped.as_slice()), vec![(7, 11)]);
    }

    #[test]
    fn rangeset_map_through_drops_fully_deleted() {
        let set = DecoRangeSet::new(vec![Decoration::mark(4..8, "m")]);
        // Delete the whole range → it collapses and is dropped.
        let mapped = set.map_through(&crate::Changes::delete(3..9));
        assert!(mapped.is_empty());
    }

    #[test]
    fn rangeset_map_through_point_stays_before_insert() {
        let set = DecoRangeSet::new(vec![Decoration::widget(5, "<x/>")]);
        // Insert exactly at the point — Before bias keeps it put.
        let mapped = set.map_through(&crate::Changes::insert(5, "Z"));
        assert_eq!(classes(mapped.as_slice()), vec![(5, 5)]);
    }
}
