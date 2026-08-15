//! Drag state shared across views.
//!
//! Three drag modes:
//! - **Move**: whole event slides in time.
//! - **ResizeStart**: only the top edge moves. Emits
//!   `Reschedule { start: new, end: unchanged }`.
//! - **ResizeEnd**: only the bottom edge moves. Emits
//!   `Reschedule { start: unchanged, end: new }`.
//!
//! Two signals:
//! - [`DragState`] — set at drag-start, cleared at drop/end. Tells
//!   chips whether they're the dragged event (for opacity tricks)
//!   and gives handlers a stable origin to compute deltas from.
//! - [`Ghost`] — snapped preview of where the drag *will land*,
//!   updated every `ondragover` frame. Drives the live block we
//!   render at the snapped position. Without this, the native HTML5
//!   drag image is the only visual feedback and it doesn't snap.

use chrono::{DateTime, NaiveDate, Utc};
use dioxus::prelude::*;

use crate::types::{ColorTag, EventId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragKind {
    Move,
    ResizeStart,
    ResizeEnd,
}

/// What the user is currently dragging. The `orig_start` /
/// `orig_end` snapshot is taken at drag-start so handlers can
/// compute deltas from a stable origin instead of relying on each
/// frame to read the latest state. `color` + `title` are carried so
/// the live ghost block can render without re-looking-up the event.
///
/// `committed` flips from `false` to `true` the first time the
/// pointer moves past the drag threshold. Until then the
/// pointerdown is treated as a click — the chip doesn't fade and
/// the root pointerup leaves the event alone, so the onclick
/// handler can fire and open the editor.
#[derive(Clone, Debug, PartialEq)]
pub struct DragState {
    pub event: EventId,
    pub kind: DragKind,
    pub orig_start: DateTime<Utc>,
    pub orig_end: DateTime<Utc>,
    pub color: ColorTag,
    pub title: String,
    /// Page x/y at pointerdown — used to compute movement delta for
    /// the commit threshold.
    pub start_page_x: f64,
    pub start_page_y: f64,
    pub committed: bool,
}

/// Pixel movement required before a pointerdown becomes a drag.
/// Matches the gantt drag handling.
pub(crate) const DRAG_THRESHOLD_PX: i64 = 4;

/// Finger movement allowed while a long-press is pending. Wider than
/// [`DRAG_THRESHOLD_PX`] because a resting fingertip jitters a few px
/// on most digitizers — anything past this reads as scroll intent and
/// cancels the pending drag.
pub(crate) const TOUCH_SLOP_PX: f64 = 10.0;

/// How long a touch must hold still on a chip / plan block before the
/// press becomes a drag. Under this it's a tap (click → editor) or the
/// start of a scroll. 400ms sits between iOS (~500) and Android (~300).
pub(crate) const LONG_PRESS_MS: u64 = 400;

/// Coarse-pointer flag + the armed long-press, shared calendar-wide.
///
/// `coarse` is `true` when the primary input is a finger
/// (`matchMedia('(pointer: coarse)')`, probed once at Calendar mount).
/// It gates the touch affordances: long-press-to-drag, fatter resize
/// handles, and month view's tap-a-day-to-zoom behavior. Defaults to
/// `false` so mouse/desktop behavior is untouched when the probe
/// can't run (native, tests).
///
/// The pending long-press lives here — NOT on the chip/block that
/// armed it — because the capture-release installer restores
/// hit-testing: the disarm events (pointermove past slop, pointerup,
/// pointercancel) land on whatever is under the finger, which during
/// a scroll or after drifting off a short chip is *not* the arming
/// element. Only the Calendar root sees every bubbled pointer event,
/// so it owns the disarm; arming sites just call [`Self::arm`] and
/// check [`Self::still_armed`] from their timer.
#[derive(Clone, Copy)]
pub struct TouchContext {
    pub coarse: Signal<bool>,
    /// Page `(x, y)` of the armed long-press; `None` = nothing pending.
    pub lp_pending: Signal<Option<(f64, f64)>>,
    /// Generation counter, bumped on every arm/disarm so a stale
    /// timer can tell it lost the race to a newer press.
    pub lp_gen: Signal<u32>,
}

impl TouchContext {
    /// Arm a long-press at page `(x, y)`. Returns the generation the
    /// caller's timer must present to [`Self::still_armed`].
    pub(crate) fn arm(&mut self, x: f64, y: f64) -> u32 {
        let generation = *self.lp_gen.peek() + 1;
        self.lp_gen.set(generation);
        self.lp_pending.set(Some((x, y)));
        generation
    }

    /// Disarm the pending long-press (finger lifted, gesture
    /// cancelled, or strayed into a scroll). Bumps the generation so
    /// any timer still in flight loses.
    pub(crate) fn disarm(&mut self) {
        if self.lp_pending.peek().is_some() {
            self.lp_pending.set(None);
            let generation = *self.lp_gen.peek() + 1;
            self.lp_gen.set(generation);
        }
    }

    /// `true` while the press armed as `generation` is still live —
    /// the timer's commit condition.
    pub(crate) fn still_armed(&self, generation: u32) -> bool {
        *self.lp_gen.peek() == generation && self.lp_pending.peek().is_some()
    }

    /// Disarm if the pointer strayed past the slop — a moving finger
    /// is scrolling, not long-pressing.
    pub(crate) fn disarm_if_strayed(&mut self, x: f64, y: f64) {
        let pending = { *self.lp_pending.peek() };
        if let Some((ox, oy)) = pending {
            if (x - ox).abs() > TOUCH_SLOP_PX || (y - oy).abs() > TOUCH_SLOP_PX {
                self.disarm();
            }
        }
    }
}

pub fn use_touch_context() -> TouchContext {
    use_context::<TouchContext>()
}

/// Snapped preview of the drag's current target. Re-computed every
/// `ondragover` tick by the column under the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ghost {
    pub event: EventId,
    pub date: NaiveDate,
    /// Minutes from the start of `date` (UTC).
    pub start_min: i64,
    pub end_min: i64,
    pub color: ColorTag,
    pub title: String,
}

#[derive(Clone, Copy)]
pub struct DragContext {
    pub state: Signal<Option<DragState>>,
    pub ghost: Signal<Option<Ghost>>,
}

pub fn use_drag_context() -> DragContext {
    use_context::<DragContext>()
}

/// Which edge (or the whole block) a plan-block drag moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockDragKind {
    /// Whole block slides in time — can cross days.
    Move,
    /// Top edge moves (start changes), within the same day.
    ResizeStart,
    /// Bottom edge moves (end changes), within the same day.
    ResizeEnd,
}

/// An in-flight drag of a *plan block* (the day-plan overlay), separate
/// from event drags. `cur_*` track the snapped live position; `date` is
/// the target day (a `Move` follows the cursor's column); `committed`
/// flips once the pointer moves past the threshold so a plain click
/// still opens the block editor.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockDrag {
    pub block_id: String,
    pub kind: BlockDragKind,
    /// The day the block started on.
    pub orig_date: NaiveDate,
    /// The current target day (= `orig_date` unless a `Move` has
    /// crossed into another column).
    pub date: NaiveDate,
    pub orig_start_min: i64,
    pub orig_end_min: i64,
    /// Minutes between the block's start and where the user grabbed it,
    /// so the block doesn't jump to align its top with the cursor.
    pub grab_offset_min: i64,
    pub cur_start_min: i64,
    pub cur_end_min: i64,
    pub start_page_y: f64,
    pub committed: bool,
}

#[derive(Clone, Copy)]
pub struct BlockDragContext {
    pub drag: Signal<Option<BlockDrag>>,
}

pub fn use_block_drag_context() -> BlockDragContext {
    use_context::<BlockDragContext>()
}

/// MIME used by HTML5 `DataTransfer` for cross-view drag carrying
/// the event id. Same pattern as `view-kanban`.
pub(crate) const DT_MIME: &str = "text/x-calendar-event-id";

/// Install (once) a document-level dragstart listener that
/// suppresses the browser's native cursor-following preview for any
/// element marked `data-cal-drag`. Without this the native ghost
/// image floats around in addition to our snapped block, which is
/// confusing.
///
/// The trick: pass a 1×1 transparent PNG to
/// `DataTransfer.setDragImage` to displace the native preview.
pub(crate) fn install_drag_image_suppressor() {
    let _ = dioxus::document::eval(
        r"
        if (!window.__dxCalDragImgSetup) {
            window.__dxCalDragImgSetup = true;
            const img = new Image();
            img.src = 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7';
            document.addEventListener('dragstart', (e) => {
                const t = e.target;
                if (t && t.closest && t.closest('[data-cal-drag]') && e.dataTransfer) {
                    e.dataTransfer.setDragImage(img, 0, 0);
                }
            }, true);
        }
        ",
    );
}

/// Install (once) a document-level pointerdown listener that releases
/// the *implicit pointer capture* touch pointers get on their
/// pointerdown target (Pointer Events spec §implicit-capture).
///
/// Without this, every pointermove of a touch drag is delivered to
/// the chip the finger went down on — never to the day-column surface
/// underneath — so the snapped ghost + drop math (which live on the
/// column) would see nothing. Releasing the capture restores
/// mouse-style hit-testing: once the dragged chip flips itself to
/// `pointer-events: none`, moves fall through to the column exactly
/// like a mouse drag. Scoped to `[data-cal-grid]` so other widgets
/// keep the spec behavior.
///
/// Also suppresses non-mouse `contextmenu` inside the grid: Android
/// Chrome fires it ~500ms into a hold — right on top of our
/// long-press — and its follow-up pointercancel would kill the
/// just-committed drag. Mouse right-click is left alone.
pub(crate) fn install_touch_capture_release() {
    let _ = dioxus::document::eval(
        r"
        if (!window.__dxCalTouchCapSetup) {
            window.__dxCalTouchCapSetup = true;
            document.addEventListener('pointerdown', (e) => {
                if (e.pointerType !== 'touch') return;
                const t = e.target;
                if (t && t.closest && t.closest('[data-cal-grid]') && t.releasePointerCapture) {
                    try { t.releasePointerCapture(e.pointerId); } catch (_) {}
                }
            }, true);
            document.addEventListener('contextmenu', (e) => {
                if (e.pointerType === 'mouse') return;
                const t = e.target;
                if (t && t.closest && t.closest('[data-cal-grid]')) {
                    e.preventDefault();
                }
            }, true);
        }
        ",
    );
}
