//! Pure day-resolution + reconcile logic, shared by every surface
//! (CLI `task plan`, the calendar UI, future server verbs).
//!
//! Two operations, both over minutes-since-midnight intervals:
//!
//! 1. **Template fallback** — [`merge_template`]. A date with no
//!    saved [`crate::DayPlan`] (or a *sparse* one — e.g. a plan
//!    holding only a scheduled meal block) still has a shape: its
//!    weekday template. The merge keeps every saved block verbatim
//!    ("explicit") and fills the remaining free time with the
//!    template's blocks marked **soft**. Soft blocks never move —
//!    they shrink in place to the largest free sub-interval of
//!    their span and disappear entirely when an explicit block
//!    covers them (≥ keep-threshold). Renderers draw soft blocks
//!    visually distinct (dashed / faded).
//!
//! 2. **Reconcile** — [`reconcile`]. After dropping a *fixed*
//!    event onto a day ("be at work 10:00–18:00"), the rest of
//!    the day reflows around it. The algorithm is deliberately
//!    simple and predictable — priority by category, then a
//!    chronological squeeze:
//!
//!    - **Fixed** blocks ([`PlannedBlock::fixed`]) and external
//!      obstacle windows (recurring calendar events) keep their
//!      exact span, always.
//!    - **Anchored** routine blocks (meal / sleep / reset /
//!      wind-down / hygiene / spiritual / exercise) are placed
//!      next, in start order. A displaced anchored block first
//!      tries to *shift* whole (≤ [`MAX_SHIFT_MIN`] from its
//!      original start, nearest fit wins), then to *compress*
//!      into the largest free interval near its span (≥
//!      [`MIN_KEEP_MIN`]), else it drops — with one exception:
//!    - **Meal embedding** — a *meal* that can neither shift nor
//!      compress, but whose natural window is fully covered by a
//!      fixed span (lunch inside a "Work 8:00–16:00" shift), is
//!      not dropped. It is *embedded*: kept at its original start
//!      (clamped into the fixed span) and compressed to at most
//!      [`MEAL_EMBED_LEN`] minutes, never below [`MIN_KEEP_MIN`].
//!      Embedded blocks intentionally overlap their host fixed
//!      block — they're a break *inside* it. Reported as
//!      [`ChangeAction::Embedded`]; renderers can re-derive
//!      embedded-ness from a saved plan (non-fixed block fully
//!      inside a fixed block's span).
//!    - **Flexible** blocks (allocatable / maintenance / other)
//!      go last, in start order. They never move: each shrinks to
//!      the largest still-free sub-interval of its original span,
//!      or drops when less than [`MIN_KEEP_MIN`] remains.
//!
//!    Nothing is lost silently: every move / shrink / drop is
//!    returned in the [`ReconcileOutcome::changes`] list for the
//!    caller to print.
//!
//! **Midnight cut**: a block whose `end <= start` wraps past
//! midnight (the sleep block). For this day's math it occupies
//! `[start, 24:00)`; the after-midnight tail belongs to the next
//! date and is left alone. If a wrapping block is shrunk such
//! that its evening segment ends before 24:00, it stops wrapping
//! (the morning tail is dropped) — this is reported as a shrink.
//! Callers that place one-off wrapping *events* (`task plan event
//! add 23:00-0:30`) materialize the `00:00–end` tail as a fixed
//! block on the next date's plan, so it participates in that
//! day's reconcile rather than silently vanishing.

use serde::{Deserialize, Serialize};

use crate::day_plan::PlannedBlock;
use crate::time_block::{BlockCategory, TimeBlock, TimeOfDay};

/// Reflowed fragments shorter than this are dropped instead.
pub const MIN_KEEP_MIN: u16 = 15;
/// How far (in minutes) an anchored block may shift from its
/// original start during reconcile.
pub const MAX_SHIFT_MIN: u16 = 120;
/// Target length of a meal embedded inside a covering fixed block
/// (the lunch break inside a work shift). The embedded span is
/// `min(original length, MEAL_EMBED_LEN)`, floored at
/// [`MIN_KEEP_MIN`].
pub const MEAL_EMBED_LEN: u16 = 30;

const DAY_END: u16 = 24 * 60;

/// How a block behaves under [`reconcile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockRole {
    /// Never moves, shrinks, or drops.
    Fixed,
    /// Routine block — shifts (bounded) or compresses.
    Anchored,
    /// Shrinks in place around higher-priority blocks, or drops.
    Flexible,
}

#[must_use]
pub fn role_for(category: BlockCategory, fixed: bool) -> BlockRole {
    if fixed {
        return BlockRole::Fixed;
    }
    match category {
        BlockCategory::Meal
        | BlockCategory::Sleep
        | BlockCategory::Reset
        | BlockCategory::WindDown
        | BlockCategory::Hygiene
        | BlockCategory::Spiritual
        | BlockCategory::Exercise => BlockRole::Anchored,
        BlockCategory::Allocatable | BlockCategory::Maintenance | BlockCategory::Other => {
            BlockRole::Flexible
        }
    }
}

/// One block of a resolved day: the block + whether it's a soft
/// template-fallback block (vs. an explicit saved one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedBlock {
    pub block: PlannedBlock,
    pub soft: bool,
}

/// What [`reconcile`] did to one block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChangeAction {
    Moved {
        from: (u16, u16),
        to: (u16, u16),
    },
    Shrunk {
        from: (u16, u16),
        to: (u16, u16),
    },
    Dropped {
        from: (u16, u16),
        reason: String,
    },
    /// A meal compressed *inside* a covering fixed block instead
    /// of dropping (see the module docs). `to` overlaps the host
    /// fixed span by design.
    Embedded {
        from: (u16, u16),
        to: (u16, u16),
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileChange {
    pub label: String,
    pub action: ChangeAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileOutcome {
    /// The reflowed blocks, sorted by start.
    pub blocks: Vec<PlannedBlock>,
    /// Everything that moved / shrank / dropped, in processing
    /// order. Empty = the day already fit.
    pub changes: Vec<ReconcileChange>,
}

/// The `[start, end)` span a block occupies *on its own date*,
/// cutting wrap-past-midnight blocks at 24:00.
#[must_use]
pub fn day_span(start: TimeOfDay, end: TimeOfDay) -> (u16, u16) {
    let s = start.minutes_since_midnight.min(DAY_END);
    let e = end.minutes_since_midnight.min(DAY_END);
    if e <= s { (s, DAY_END) } else { (s, e) }
}

/// Map a [`BlockCategory::Meal`] block to a mealplan slot name
/// (`"breakfast"` / `"lunch"` / `"dinner"` / `"snack"`), so the
/// schedule surfaces can preview what's planned for that block.
///
/// The mealplan model already carries a free-form `slot` string
/// per meal (canonical set: breakfast / lunch / dinner / snack),
/// so no proto extension is needed — the mapping is by **label
/// keyword first** ("Breakfast prep" → breakfast), with a
/// **start-time fallback** for unnamed meal blocks: before 10:30
/// → breakfast, 10:30–15:30 → lunch, after 15:30 → dinner.
#[must_use]
pub fn meal_slot_for_block(label: &str, start: TimeOfDay) -> &'static str {
    let l = label.to_ascii_lowercase();
    if l.contains("breakfast") || l.contains("brunch") {
        return "breakfast";
    }
    if l.contains("lunch") {
        return "lunch";
    }
    if l.contains("dinner") || l.contains("supper") {
        return "dinner";
    }
    if l.contains("snack") {
        return "snack";
    }
    let m = start.minutes_since_midnight;
    if m < 10 * 60 + 30 {
        "breakfast"
    } else if m < 15 * 60 + 30 {
        "lunch"
    } else {
        "dinner"
    }
}

// ── interval timeline ────────────────────────────────────────────

/// Sorted, merged set of occupied `[start, end)` intervals over
/// the day.
#[derive(Debug, Default, Clone)]
struct Timeline {
    occ: Vec<(u16, u16)>,
}

impl Timeline {
    fn occupy(&mut self, s: u16, e: u16) {
        if e <= s {
            return;
        }
        self.occ.push((s, e));
        self.occ.sort_unstable();
        // Merge touching/overlapping intervals.
        let mut merged: Vec<(u16, u16)> = Vec::with_capacity(self.occ.len());
        for &(s, e) in &self.occ {
            match merged.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        self.occ = merged;
    }

    fn is_free(&self, s: u16, e: u16) -> bool {
        self.occ.iter().all(|&(os, oe)| e <= os || oe <= s)
    }

    /// Complement of the occupied set within `[0, 1440)`.
    fn free_gaps(&self) -> Vec<(u16, u16)> {
        let mut gaps = Vec::new();
        let mut cursor = 0u16;
        for &(s, e) in &self.occ {
            if s > cursor {
                gaps.push((cursor, s));
            }
            cursor = cursor.max(e);
        }
        if cursor < DAY_END {
            gaps.push((cursor, DAY_END));
        }
        gaps
    }

    /// Free sub-intervals of `[s, e)`.
    fn free_within(&self, s: u16, e: u16) -> Vec<(u16, u16)> {
        self.free_gaps()
            .into_iter()
            .filter_map(|(gs, ge)| {
                let lo = gs.max(s);
                let hi = ge.min(e);
                (hi > lo).then_some((lo, hi))
            })
            .collect()
    }
}

// ── template fallback ────────────────────────────────────────────

/// Merge a (possibly absent / sparse) saved plan with the date's
/// template: saved blocks stay verbatim, template blocks fill the
/// free time as **soft** blocks (shrunk in place, never moved;
/// dropped when < [`MIN_KEEP_MIN`] of their span remains free).
/// Result is sorted by start.
#[must_use]
pub fn merge_template(saved: &[PlannedBlock], template: &[TimeBlock]) -> Vec<ResolvedBlock> {
    let mut tl = Timeline::default();
    for b in saved {
        let (s, e) = day_span(b.start, b.end);
        tl.occupy(s, e);
    }

    let mut out: Vec<ResolvedBlock> = saved
        .iter()
        .map(|b| ResolvedBlock {
            block: b.clone(),
            soft: false,
        })
        .collect();

    let mut tpl: Vec<&TimeBlock> = template.iter().collect();
    tpl.sort_by_key(|b| day_span(b.start, b.end).0);
    for tb in tpl {
        // An explicit block with the same id overrides the
        // template row outright.
        if saved.iter().any(|b| b.id.0 == tb.id.0) {
            continue;
        }
        let (s, e) = day_span(tb.start, tb.end);
        let Some(&(ps, pe)) = tl.free_within(s, e).iter().max_by_key(|(ps, pe)| pe - ps) else {
            continue;
        };
        if pe - ps < MIN_KEEP_MIN {
            continue;
        }
        // Keep the original end when the block wraps and its
        // evening segment still reaches 24:00.
        let wraps = tb.end.minutes_since_midnight <= tb.start.minutes_since_midnight;
        let (new_start, new_end) = if wraps && pe == DAY_END {
            (
                TimeOfDay {
                    minutes_since_midnight: ps,
                },
                tb.end,
            )
        } else {
            (
                TimeOfDay {
                    minutes_since_midnight: ps,
                },
                TimeOfDay {
                    minutes_since_midnight: pe,
                },
            )
        };
        tl.occupy(ps, pe);
        out.push(ResolvedBlock {
            block: PlannedBlock {
                id: tb.id.clone(),
                start: new_start,
                end: new_end,
                label: tb.label.clone(),
                category: tb.category,
                note: tb.note.clone(),
                assignment: None,
                fixed: false,
            },
            soft: true,
        });
    }

    out.sort_by_key(|rb| day_span(rb.block.start, rb.block.end).0);
    out
}

// ── reconcile ────────────────────────────────────────────────────

/// Reflow `blocks` around the fixed blocks among them plus the
/// external `obstacles` (e.g. recurring calendar events on this
/// date, as `(start_min, end_min)` windows). See the module docs
/// for the exact priority/squeeze rules.
#[must_use]
pub fn reconcile(blocks: Vec<PlannedBlock>, obstacles: &[(u16, u16)]) -> ReconcileOutcome {
    let mut tl = Timeline::default();
    // Fixed spans (blocks + obstacles) double as embedding hosts
    // for meals — see the module docs.
    let mut fixed_spans: Vec<(u16, u16)> = Vec::new();
    for &(s, e) in obstacles {
        let (s, e) = (s.min(DAY_END), e.min(DAY_END));
        if e > s {
            tl.occupy(s, e);
            fixed_spans.push((s, e));
        }
    }

    let mut fixed = Vec::new();
    let mut anchored = Vec::new();
    let mut flexible = Vec::new();
    for b in blocks {
        match role_for(b.category, b.fixed) {
            BlockRole::Fixed => fixed.push(b),
            BlockRole::Anchored => anchored.push(b),
            BlockRole::Flexible => flexible.push(b),
        }
    }

    let mut out = Vec::new();
    let mut changes = Vec::new();

    // 1. Fixed blocks claim their spans verbatim.
    for b in fixed {
        let (s, e) = day_span(b.start, b.end);
        tl.occupy(s, e);
        fixed_spans.push((s, e));
        out.push(b);
    }

    // 2. Anchored routine blocks: keep → shift → compress → drop.
    anchored.sort_by_key(|b| day_span(b.start, b.end).0);
    for mut b in anchored {
        let (s, e) = day_span(b.start, b.end);
        let dur = e - s;
        if tl.is_free(s, e) {
            tl.occupy(s, e);
            out.push(b);
            continue;
        }
        // Shift whole: nearest gap start within MAX_SHIFT_MIN.
        let best_shift = tl
            .free_gaps()
            .into_iter()
            .filter(|(gs, ge)| ge - gs >= dur)
            .map(|(gs, ge)| {
                let cand = s.clamp(gs, ge - dur);
                (cand.abs_diff(s), cand)
            })
            .filter(|&(shift, _)| shift <= MAX_SHIFT_MIN)
            .min();
        if let Some((_, ns)) = best_shift {
            let ne = ns + dur;
            tl.occupy(ns, ne);
            changes.push(ReconcileChange {
                label: b.label.clone(),
                action: ChangeAction::Moved {
                    from: (s, e),
                    to: (ns, ne),
                },
            });
            apply_span(&mut b, ns, ne);
            out.push(b);
            continue;
        }
        // Compress: largest free interval near the original span.
        let window = (
            s.saturating_sub(MAX_SHIFT_MIN),
            (e + MAX_SHIFT_MIN).min(DAY_END),
        );
        let best_gap = tl
            .free_within(window.0, window.1)
            .into_iter()
            .max_by_key(|&(gs, ge)| ge - gs);
        match best_gap {
            Some((gs, ge)) if ge - gs >= MIN_KEEP_MIN => {
                let len = dur.min(ge - gs);
                let ns = s.clamp(gs, ge - len);
                let ne = ns + len;
                tl.occupy(ns, ne);
                changes.push(ReconcileChange {
                    label: b.label.clone(),
                    action: ChangeAction::Shrunk {
                        from: (s, e),
                        to: (ns, ne),
                    },
                });
                apply_span(&mut b, ns, ne);
                out.push(b);
            }
            // Meal embedding: no free time anywhere near, but a
            // fixed span covers the meal's natural window — keep
            // it compressed *inside* the fixed block instead of
            // dropping (lunch break inside the work shift).
            _ if b.category == BlockCategory::Meal
                && fixed_spans.iter().any(|&(fs, fe)| fs <= s && e <= fe)
                && dur >= MIN_KEEP_MIN =>
            {
                let &(fs, fe) = fixed_spans
                    .iter()
                    .find(|&&(fs, fe)| fs <= s && e <= fe)
                    .expect("checked by the guard");
                let len = dur.min(MEAL_EMBED_LEN);
                let ns = s.clamp(fs, fe - len);
                let ne = ns + len;
                changes.push(ReconcileChange {
                    label: b.label.clone(),
                    action: ChangeAction::Embedded {
                        from: (s, e),
                        to: (ns, ne),
                    },
                });
                apply_span(&mut b, ns, ne);
                // Deliberately NOT occupied on the timeline: the
                // span already belongs to the host fixed block.
                out.push(b);
            }
            _ => changes.push(ReconcileChange {
                label: b.label.clone(),
                action: ChangeAction::Dropped {
                    from: (s, e),
                    reason: "no free time near its span".into(),
                },
            }),
        }
    }

    // 3. Flexible blocks shrink in place (largest free piece of
    //    their own span), or drop.
    flexible.sort_by_key(|b| day_span(b.start, b.end).0);
    for mut b in flexible {
        let (s, e) = day_span(b.start, b.end);
        let best = tl
            .free_within(s, e)
            .into_iter()
            .max_by_key(|&(ps, pe)| pe - ps);
        match best {
            Some((ps, pe)) if pe - ps >= MIN_KEEP_MIN => {
                tl.occupy(ps, pe);
                if (ps, pe) != (s, e) {
                    changes.push(ReconcileChange {
                        label: b.label.clone(),
                        action: ChangeAction::Shrunk {
                            from: (s, e),
                            to: (ps, pe),
                        },
                    });
                }
                apply_span(&mut b, ps, pe);
                out.push(b);
            }
            _ => changes.push(ReconcileChange {
                label: b.label.clone(),
                action: ChangeAction::Dropped {
                    from: (s, e),
                    reason: "fully covered by fixed blocks".into(),
                },
            }),
        }
    }

    out.sort_by_key(|b| day_span(b.start, b.end).0);
    ReconcileOutcome {
        blocks: out,
        changes,
    }
}

/// Write a reflowed `[ns, ne)` day-span back onto a block,
/// preserving a wrap-past-midnight tail when the evening segment
/// still reaches 24:00.
fn apply_span(b: &mut PlannedBlock, ns: u16, ne: u16) {
    let wraps = b.end.minutes_since_midnight <= b.start.minutes_since_midnight
        && b.start.minutes_since_midnight < DAY_END;
    b.start = TimeOfDay {
        minutes_since_midnight: ns,
    };
    if !(wraps && ne == DAY_END) {
        b.end = TimeOfDay {
            minutes_since_midnight: ne,
        };
    }
}

/// Is `blocks[idx]` an *embedded* block — non-fixed, fully inside
/// some other fixed block's day span? Re-derives what
/// [`ChangeAction::Embedded`] produced from a saved plan, so any
/// renderer can mark embedded meals without extra state.
#[must_use]
pub fn is_embedded(blocks: &[PlannedBlock], idx: usize) -> bool {
    let Some(b) = blocks.get(idx) else {
        return false;
    };
    if b.fixed {
        return false;
    }
    let (s, e) = day_span(b.start, b.end);
    blocks.iter().enumerate().any(|(i, f)| {
        if i == idx || !f.fixed {
            return false;
        }
        let (fs, fe) = day_span(f.start, f.end);
        fs <= s && e <= fe
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time_block::TimeBlockId;

    fn tod(h: u16, m: u16) -> TimeOfDay {
        TimeOfDay {
            minutes_since_midnight: h * 60 + m,
        }
    }

    fn pb(id: &str, s: (u16, u16), e: (u16, u16), cat: BlockCategory, fixed: bool) -> PlannedBlock {
        PlannedBlock {
            id: TimeBlockId(id.into()),
            start: tod(s.0, s.1),
            end: tod(e.0, e.1),
            label: id.into(),
            category: cat,
            note: None,
            assignment: None,
            fixed,
        }
    }

    fn tb(id: &str, s: (u16, u16), e: (u16, u16), cat: BlockCategory) -> TimeBlock {
        TimeBlock {
            id: TimeBlockId(id.into()),
            start: tod(s.0, s.1),
            end: tod(e.0, e.1),
            label: id.into(),
            category: cat,
            note: None,
        }
    }

    fn span(b: &PlannedBlock) -> (u16, u16) {
        (b.start.minutes_since_midnight, b.end.minutes_since_midnight)
    }

    // ── merge_template ──────────────────────────────────────────

    #[test]
    fn empty_plan_yields_full_soft_template() {
        let template = vec![
            tb("morning", (6, 0), (9, 0), BlockCategory::Reset),
            tb("work", (9, 0), (17, 0), BlockCategory::Allocatable),
        ];
        let out = merge_template(&[], &template);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|rb| rb.soft));
    }

    #[test]
    fn sparse_plan_keeps_explicit_and_fills_soft() {
        // The real-world bug: a saved plan holding only one meal
        // block cleared the whole day. The merge must keep the
        // meal and fill the rest of the day from the template.
        let saved = vec![pb("oats", (7, 30), (8, 0), BlockCategory::Meal, false)];
        let template = vec![
            tb("reset", (6, 0), (7, 0), BlockCategory::Reset),
            tb("breakfast", (7, 30), (8, 0), BlockCategory::Meal),
            tb("block-1", (9, 0), (12, 0), BlockCategory::Allocatable),
        ];
        let out = merge_template(&saved, &template);
        let labels: Vec<(&str, bool)> = out
            .iter()
            .map(|rb| (rb.block.label.as_str(), rb.soft))
            .collect();
        // breakfast is fully covered by the explicit oats block →
        // dropped; reset + block-1 come through soft.
        assert_eq!(
            labels,
            vec![("reset", true), ("oats", false), ("block-1", true)]
        );
    }

    #[test]
    fn soft_block_shrinks_in_place_around_explicit() {
        let saved = vec![pb("standup", (9, 0), (10, 0), BlockCategory::Other, true)];
        let template = vec![tb("block-1", (9, 0), (12, 0), BlockCategory::Allocatable)];
        let out = merge_template(&saved, &template);
        let soft: Vec<_> = out.iter().filter(|rb| rb.soft).collect();
        assert_eq!(soft.len(), 1);
        assert_eq!(span(&soft[0].block), (600, 720)); // 10:00–12:00
    }

    #[test]
    fn template_row_overridden_by_same_id_is_skipped() {
        let saved = vec![pb(
            "block-1",
            (10, 0),
            (13, 0),
            BlockCategory::Allocatable,
            false,
        )];
        let template = vec![tb("block-1", (9, 0), (12, 0), BlockCategory::Allocatable)];
        let out = merge_template(&saved, &template);
        assert_eq!(out.len(), 1);
        assert!(!out[0].soft);
    }

    #[test]
    fn wrapping_sleep_block_merges_and_keeps_tail() {
        let template = vec![tb("sleep", (22, 30), (6, 0), BlockCategory::Sleep)];
        let out = merge_template(&[], &template);
        assert_eq!(out.len(), 1);
        // Tail preserved: still 22:30 → 06:00.
        assert_eq!(span(&out[0].block), (1350, 360));
    }

    // ── reconcile ───────────────────────────────────────────────

    #[test]
    fn untouched_day_reports_no_changes() {
        let blocks = vec![
            pb("a", (9, 0), (12, 0), BlockCategory::Allocatable, false),
            pb("lunch", (12, 0), (13, 0), BlockCategory::Meal, false),
        ];
        let out = reconcile(blocks, &[]);
        assert!(out.changes.is_empty());
        assert_eq!(out.blocks.len(), 2);
    }

    /// Scenario from the brief: a "Work 8:00–16:00" fixed block
    /// dropped onto a weekday-style template day.
    #[test]
    fn workday_squeezes_template() {
        let blocks = vec![
            pb("work", (8, 0), (16, 0), BlockCategory::Other, true),
            pb("reset", (6, 0), (6, 30), BlockCategory::Reset, false),
            pb("breakfast", (7, 0), (8, 0), BlockCategory::Meal, false),
            pb("gym", (8, 0), (9, 0), BlockCategory::Exercise, false),
            pb(
                "block-1",
                (9, 30),
                (12, 30),
                BlockCategory::Allocatable,
                false,
            ),
            pb("lunch", (12, 30), (13, 30), BlockCategory::Meal, false),
            pb(
                "block-2",
                (13, 30),
                (16, 30),
                BlockCategory::Allocatable,
                false,
            ),
            pb("dinner", (17, 30), (19, 0), BlockCategory::Meal, false),
            pb(
                "block-3",
                (19, 0),
                (22, 0),
                BlockCategory::Allocatable,
                false,
            ),
            pb("sleep", (22, 30), (6, 0), BlockCategory::Sleep, false),
        ];
        let out = reconcile(blocks, &[]);
        let work = out.blocks.iter().find(|b| b.label == "work").unwrap();
        assert_eq!(span(work), (480, 960)); // untouched
        // Gym (anchored, 8:00–9:00) can't fit inside work — shifts
        // earlier to 6:30–7:00? No: needs 60min, free gap before
        // work is 6:30–7:00 (30m after reset+breakfast). It should
        // compress or shift within 120m. Whatever it does, nothing
        // overlaps and everything is reported.
        assert_no_overlaps(&out.blocks);
        // block-2 fully inside work except 16:00–16:30 → shrunk.
        let b2 = out.blocks.iter().find(|b| b.label == "block-2").unwrap();
        assert_eq!(span(b2), (960, 990));
        // block-1 fully covered → dropped + reported.
        assert!(out.blocks.iter().all(|b| b.label != "block-1"));
        assert!(
            out.changes
                .iter()
                .any(|c| c.label == "block-1" && matches!(c.action, ChangeAction::Dropped { .. }))
        );
        // Lunch must survive: no free time anywhere near its span,
        // but the work block covers its natural window → embedded
        // compressed inside the fixed span (≥ MIN_KEEP_MIN).
        let lunch = out.blocks.iter().find(|b| b.label == "lunch").unwrap();
        assert_eq!(span(lunch), (750, 780)); // 12:30–13:00, inside work
        assert!(
            out.changes
                .iter()
                .any(|c| c.label == "lunch" && matches!(c.action, ChangeAction::Embedded { .. }))
        );
        let lunch_idx = out.blocks.iter().position(|b| b.label == "lunch").unwrap();
        assert!(is_embedded(&out.blocks, lunch_idx));
        // Sleep untouched.
        let sleep = out.blocks.iter().find(|b| b.label == "sleep").unwrap();
        assert_eq!(span(sleep), (1350, 360));
    }

    #[test]
    fn meal_embeds_inside_covering_obstacle_window() {
        // Same rule when the cover is an external event window
        // (recurring calendar event) rather than a fixed block.
        // Free time exists before 8:00 but is > MAX_SHIFT_MIN away,
        // so shift + compress both fail.
        let blocks = vec![pb("lunch", (12, 30), (13, 30), BlockCategory::Meal, false)];
        let out = reconcile(blocks, &[(8 * 60, 16 * 60)]);
        assert_eq!(out.blocks.len(), 1);
        assert_eq!(span(&out.blocks[0]), (750, 780));
        assert!(matches!(
            out.changes[0].action,
            ChangeAction::Embedded {
                from: (750, 810),
                to: (750, 780)
            }
        ));
    }

    #[test]
    fn meal_prefers_real_free_time_over_embedding() {
        // Lunch 12:30–13:30 vs work 12:00–14:00: it can shift whole
        // (90min ≤ MAX_SHIFT_MIN; the 11:00 slot ties with 14:00
        // and the earlier candidate wins), so it does — embedding
        // is strictly the last resort.
        let blocks = vec![
            pb("work", (12, 0), (14, 0), BlockCategory::Other, true),
            pb("lunch", (12, 30), (13, 30), BlockCategory::Meal, false),
        ];
        let out = reconcile(blocks, &[]);
        let lunch = out.blocks.iter().find(|b| b.label == "lunch").unwrap();
        assert_eq!(span(lunch), (660, 720));
        assert!(matches!(out.changes[0].action, ChangeAction::Moved { .. }));
    }

    #[test]
    fn non_meal_anchored_still_drops_under_fixed_cover() {
        // Embedding is meal-only: a gym block fully covered by a
        // fixed span (and nothing free nearby) still drops.
        let blocks = vec![pb("gym", (12, 0), (13, 0), BlockCategory::Exercise, false)];
        let out = reconcile(blocks, &[(8 * 60, 16 * 60)]);
        assert!(out.blocks.is_empty());
        assert!(matches!(
            out.changes[0].action,
            ChangeAction::Dropped { .. }
        ));
    }

    /// Scenario from the brief: concert 19:00–23:00 + dinner-out
    /// 23:00–00:30 (wraps midnight; cut at 24:00 for this date).
    #[test]
    fn concert_evening_pushes_routine() {
        let blocks = vec![
            pb("concert", (19, 0), (23, 0), BlockCategory::Other, true),
            pb("dinner-out", (23, 0), (0, 30), BlockCategory::Other, true),
            pb("dinner", (17, 30), (19, 0), BlockCategory::Meal, false),
            pb(
                "block-3",
                (19, 0),
                (22, 0),
                BlockCategory::Allocatable,
                false,
            ),
            pb(
                "wind-down",
                (22, 0),
                (22, 30),
                BlockCategory::WindDown,
                false,
            ),
            pb("sleep", (22, 30), (6, 0), BlockCategory::Sleep, false),
        ];
        let out = reconcile(blocks, &[]);
        assert_no_overlaps(&out.blocks);
        // Fixed wrap block occupies 23:00–24:00 on this date.
        let dout = out.blocks.iter().find(|b| b.label == "dinner-out").unwrap();
        assert_eq!(span(dout), (1380, 30));
        // Home dinner untouched (ends as concert starts).
        let dinner = out.blocks.iter().find(|b| b.label == "dinner").unwrap();
        assert_eq!(span(dinner), (1050, 1140));
        // Evening is solid 19:00–24:00 → block-3, wind-down and
        // sleep can't exist on this date; all reported.
        for lost in ["block-3", "wind-down", "sleep"] {
            assert!(
                out.changes.iter().any(|c| c.label == lost),
                "{lost} should be reported"
            );
        }
    }

    #[test]
    fn anchored_block_shifts_around_obstacle() {
        // Lunch 12:30–13:30 vs a 12:00–13:00 obstacle → shifts to
        // 13:00–14:00 (nearest full fit, 30min shift).
        let blocks = vec![pb("lunch", (12, 30), (13, 30), BlockCategory::Meal, false)];
        let out = reconcile(blocks, &[(720, 780)]);
        assert_eq!(span(&out.blocks[0]), (780, 840));
        assert!(matches!(out.changes[0].action, ChangeAction::Moved { .. }));
    }

    #[test]
    fn fixed_blocks_never_change_even_when_overlapping_obstacles() {
        let blocks = vec![pb("work", (10, 0), (18, 0), BlockCategory::Other, true)];
        let out = reconcile(blocks, &[(540, 660)]);
        assert_eq!(span(&out.blocks[0]), (600, 1080));
        assert!(out.changes.is_empty());
    }

    fn assert_no_overlaps(blocks: &[PlannedBlock]) {
        // Embedded meals overlap their host fixed block by design —
        // exclude them from the pairwise check.
        let mut spans: Vec<(u16, u16)> = blocks
            .iter()
            .enumerate()
            .filter(|&(i, _)| !is_embedded(blocks, i))
            .map(|(_, b)| day_span(b.start, b.end))
            .collect();
        spans.sort_unstable();
        for w in spans.windows(2) {
            assert!(
                w[0].1 <= w[1].0,
                "overlap between {:?} and {:?}",
                w[0],
                w[1]
            );
        }
    }
}
