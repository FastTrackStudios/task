//! Reactive store + event vocabulary.
//!
//! `GanttState` is plain data. The root [`crate::components::Gantt`]
//! holds it in a `Signal<GanttState>`. Mutations from inner components
//! bubble up as [`GanttEvent`] through `on_event` — the *consumer*
//! decides whether to write through to a CRDT, debounce, etc. (Per
//! AGENTS.md: dumb components, data in, events out.)

use chrono::{DateTime, Duration, Utc};
use std::collections::{HashMap, HashSet};

use crate::scales::{self, ScaleGrid};
use crate::time::{MONDAY_START, WeekStart, add, diff_f, unit_start};
use crate::types::{
    GanttLink, GanttTask, LinkType, Marker, ScaleConfig, TaskId, TaskType, ZoomConfig,
    default_zoom_levels,
};

pub const DEFAULT_ROW_HEIGHT: f32 = 38.0;
pub const DEFAULT_BAR_HEIGHT: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    None,
    Name,
    Start,
    End,
    Progress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// One laid-out row in display order. Includes geometry (`x`, `w`)
/// resolved against the active [`ScaleGrid`].
#[derive(Clone, Debug, PartialEq)]
pub struct LaidOutTask {
    pub task: GanttTask,
    pub level: u32,
    pub index: usize,
    pub x: f32,
    pub w: f32,
    pub y: f32,
    pub h: f32,
    pub has_children: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaidOutLink {
    pub link: GanttLink,
    /// Static SVG path `d` (no drag offset applied). Drag-aware
    /// recomputation lives in the [`super::components::links`]
    /// layer using the raw endpoint fields below.
    pub path: String,
    pub source_id: TaskId,
    pub target_id: TaskId,
    /// Raw endpoint coordinates. Re-elbowed by the link layer when
    /// either bar is being dragged so the arrows animate with the
    /// bars instead of staying glued to their pre-drag positions.
    pub sx: f32,
    pub sy: f32,
    pub tx: f32,
    pub ty: f32,
}

#[derive(Clone, Debug)]
pub struct GanttState {
    pub tasks: Vec<GanttTask>,
    pub links: Vec<GanttLink>,
    pub markers: Vec<Marker>,
    pub zoom: ZoomConfig,
    pub cell_width: f32,
    pub row_height: f32,
    pub bar_height: f32,
    pub week_start: WeekStart,
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    /// Multi-select set. Single-click clears + inserts one; shift-
    /// click extends; ctrl/meta-click toggles. Drag-moves the whole
    /// set together.
    pub selected: HashSet<TaskId>,
    /// Currently open in the side editor.
    pub editing: Option<TaskId>,
    /// Sidebar sort. `None` keeps natural tree order.
    pub sort: (SortKey, SortDir),
    /// Sidebar text filter (case-insensitive substring on `text`).
    pub filter: String,
    /// Read-only mode disables drag/resize/link/edit and dims the
    /// affordances.
    pub readonly: bool,
}

impl Default for GanttState {
    fn default() -> Self {
        Self {
            tasks: Vec::new(),
            links: Vec::new(),
            markers: Vec::new(),
            zoom: ZoomConfig::default(),
            cell_width: 60.0,
            row_height: DEFAULT_ROW_HEIGHT,
            bar_height: DEFAULT_BAR_HEIGHT,
            week_start: MONDAY_START,
            start: None,
            end: None,
            selected: HashSet::new(),
            editing: None,
            sort: (SortKey::None, SortDir::Asc),
            filter: String::new(),
            readonly: false,
        }
    }
}

impl GanttState {
    #[must_use]
    pub fn active_scales(&self) -> &[ScaleConfig] {
        let i = self
            .zoom
            .level
            .min(self.zoom.levels.len().saturating_sub(1));
        &self.zoom.levels[i].scales
    }

    /// Resolve auto start/end from tasks if not pinned.
    pub fn resolved_range(&self) -> (DateTime<Utc>, DateTime<Utc>) {
        let mut s = self.start;
        let mut e = self.end;
        for t in &self.tasks {
            if s.is_none_or(|cur| t.start < cur) {
                s = Some(t.start);
            }
            if e.is_none_or(|cur| t.end > cur) {
                e = Some(t.end);
            }
        }
        let unit = crate::time::min_unit(
            &self
                .active_scales()
                .iter()
                .map(|s| s.unit)
                .collect::<Vec<_>>(),
        );
        let s = s.unwrap_or_else(Utc::now);
        let e = e.unwrap_or_else(|| add(unit, s, 30));
        // Pad one unit on each side so first/last bars don't clip.
        (add(unit, s, -1), add(unit, e, 1))
    }

    #[must_use]
    pub fn build_grid(&self) -> ScaleGrid {
        let (s, e) = self.resolved_range();
        scales::build_scales(self.active_scales(), s, e, self.cell_width, self.week_start)
    }

    /// Linearize the task tree in render order respecting `open` flags.
    ///
    /// Summary bars are *derived*: their `task.start` / `task.end`
    /// fields are ignored in favour of `min(child.start)` /
    /// `max(child.end)` over all descendants. This matches svar's
    /// behaviour (summaries are read-only rollups; you move them by
    /// moving their children, or by move-dragging the whole bar,
    /// which cascades to every descendant — see [`descendants_of`]).
    #[must_use]
    pub fn layout(&self, grid: &ScaleGrid) -> (Vec<LaidOutTask>, Vec<LaidOutLink>) {
        let mut children: HashMap<Option<TaskId>, Vec<usize>> = HashMap::new();
        for (i, t) in self.tasks.iter().enumerate() {
            children.entry(t.parent).or_default().push(i);
        }
        let has_children: HashSet<TaskId> = self.tasks.iter().filter_map(|t| t.parent).collect();

        // Sort siblings when a sort key is active. Tree structure
        // stays intact; only the within-parent order changes.
        let (skey, sdir) = self.sort;
        if skey != SortKey::None {
            let tasks = &self.tasks;
            let cmp = |a: &usize, b: &usize| -> std::cmp::Ordering {
                let ta = &tasks[*a];
                let tb = &tasks[*b];
                let ord = match skey {
                    SortKey::None => std::cmp::Ordering::Equal,
                    SortKey::Name => ta.text.cmp(&tb.text),
                    SortKey::Start => ta.start.cmp(&tb.start),
                    SortKey::End => ta.end.cmp(&tb.end),
                    SortKey::Progress => ta
                        .progress
                        .partial_cmp(&tb.progress)
                        .unwrap_or(std::cmp::Ordering::Equal),
                };
                if sdir == SortDir::Desc {
                    ord.reverse()
                } else {
                    ord
                }
            };
            for kids in children.values_mut() {
                kids.sort_by(cmp);
            }
        }

        // Filter — case-insensitive substring on `text`. A task
        // matches if it OR any descendant matches (so summaries stay
        // visible when their kids match).
        let filter_norm = self.filter.trim().to_lowercase();
        let filter_active = !filter_norm.is_empty();
        let match_set: HashSet<TaskId> = if filter_active {
            let direct: HashSet<TaskId> = self
                .tasks
                .iter()
                .filter(|t| t.text.to_lowercase().contains(&filter_norm))
                .map(|t| t.id)
                .collect();
            // Walk parents up so summaries holding a match stay in.
            let mut out = direct.clone();
            for id in direct.iter().copied() {
                let mut cur = self.tasks.iter().find(|t| t.id == id);
                while let Some(t) = cur {
                    if let Some(p) = t.parent {
                        out.insert(p);
                        cur = self.tasks.iter().find(|t| t.id == p);
                    } else {
                        break;
                    }
                }
            }
            out
        } else {
            HashSet::new()
        };

        let mut out: Vec<LaidOutTask> = Vec::new();
        let mut row_y = 0.0_f32;
        let mut id_to_index: HashMap<TaskId, usize> = HashMap::new();
        let mut stack: Vec<(Option<TaskId>, u32, usize)> = vec![(None, 0, 0)];
        while let Some((parent, level, mut child_i)) = stack.pop() {
            let kids = match children.get(&parent) {
                Some(k) => k,
                None => continue,
            };
            while child_i < kids.len() {
                let idx = kids[child_i];
                child_i += 1;
                let t = &self.tasks[idx];
                if filter_active && !match_set.contains(&t.id) {
                    continue;
                }
                let bar_top = (self.row_height - self.bar_height) / 2.0;
                // Derive summary bounds from descendants when present.
                let (effective_start, effective_end) =
                    if matches!(t.task_type, crate::types::TaskType::Summary) {
                        descendant_bounds(self, t.id).unwrap_or((t.start, t.end))
                    } else {
                        (t.start, t.end)
                    };
                let x = scales::x_for_date(grid, effective_start);
                let w = (scales::x_for_date(grid, effective_end) - x).max(2.0);
                let mut task_view = t.clone();
                task_view.start = effective_start;
                task_view.end = effective_end;
                id_to_index.insert(t.id, out.len());
                out.push(LaidOutTask {
                    task: task_view,
                    level,
                    index: out.len(),
                    x,
                    w,
                    y: row_y + bar_top,
                    h: self.bar_height,
                    has_children: has_children.contains(&t.id),
                });
                row_y += self.row_height;
                if t.open && children.contains_key(&Some(t.id)) {
                    // Resume parent later, then descend.
                    stack.push((parent, level, child_i));
                    stack.push((Some(t.id), level + 1, 0));
                    break;
                }
            }
        }

        let links = self
            .links
            .iter()
            .filter_map(|l| {
                let si = *id_to_index.get(&l.source)?;
                let ti = *id_to_index.get(&l.target)?;
                let src = &out[si];
                let tgt = &out[ti];
                let (sx, sy) = anchor(src, l.link_type, true);
                let (tx, ty) = anchor(tgt, l.link_type, false);
                let path = link_path(sx, sy, tx, ty, l.link_type);
                Some(LaidOutLink {
                    link: l.clone(),
                    path,
                    source_id: l.source,
                    target_id: l.target,
                    sx,
                    sy,
                    tx,
                    ty,
                })
            })
            .collect();

        (out, links)
    }
}

/// Walk descendants of `root` and return `(min start, max end)`
/// across all non-summary leaves. Used to derive summary-bar bounds.
fn descendant_bounds(state: &GanttState, root: TaskId) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let mut by_parent: HashMap<TaskId, Vec<usize>> = HashMap::new();
    for (i, t) in state.tasks.iter().enumerate() {
        if let Some(p) = t.parent {
            by_parent.entry(p).or_default().push(i);
        }
    }
    let mut stack = vec![root];
    let mut min_start: Option<DateTime<Utc>> = None;
    let mut max_end: Option<DateTime<Utc>> = None;
    let mut had_any = false;
    while let Some(id) = stack.pop() {
        if let Some(kids) = by_parent.get(&id) {
            for &ki in kids {
                had_any = true;
                let kid = &state.tasks[ki];
                // For non-summary kids, contribute their bounds. For
                // summary kids, recurse — their own stored bounds may
                // be stale.
                if matches!(kid.task_type, crate::types::TaskType::Summary) {
                    stack.push(kid.id);
                } else {
                    if min_start.is_none_or(|m| kid.start < m) {
                        min_start = Some(kid.start);
                    }
                    if max_end.is_none_or(|m| kid.end > m) {
                        max_end = Some(kid.end);
                    }
                }
            }
        }
    }
    if had_any {
        match (min_start, max_end) {
            (Some(s), Some(e)) => Some((s, e)),
            _ => None,
        }
    } else {
        None
    }
}

/// True if `candidate` is somewhere underneath `ancestor` in the
/// task tree (used to decide whether a child bar should follow its
/// summary ancestor during a live drag preview).
#[must_use]
pub fn is_descendant(state: &GanttState, ancestor: TaskId, candidate: TaskId) -> bool {
    let mut current = state.tasks.iter().find(|t| t.id == candidate);
    while let Some(t) = current {
        match t.parent {
            None => return false,
            Some(p) if p == ancestor => return true,
            Some(p) => current = state.tasks.iter().find(|t| t.id == p),
        }
    }
    false
}

/// Collect every descendant id of `root` (excluding `root` itself).
/// Used for cascade-move when the user drags a summary bar.
#[must_use]
pub fn descendants_of(state: &GanttState, root: TaskId) -> Vec<TaskId> {
    let mut by_parent: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for t in &state.tasks {
        if let Some(p) = t.parent {
            by_parent.entry(p).or_default().push(t.id);
        }
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if let Some(kids) = by_parent.get(&id) {
            for &k in kids {
                out.push(k);
                stack.push(k);
            }
        }
    }
    out
}

fn anchor(t: &LaidOutTask, link: LinkType, is_source: bool) -> (f32, f32) {
    let y = t.y + t.h / 2.0;
    let (left, right) = (t.x, t.x + t.w);
    let from_start = matches!(
        (link, is_source),
        (LinkType::S2s | LinkType::S2e, true) | (LinkType::S2s | LinkType::E2s, false)
    );
    let x = if from_start { left } else { right };
    (x, y)
}

/// Right-angle path connector with proper lead-in / lead-out
/// segments. Two regimes:
///
/// - **Forward** (target is to the right of source by at least
///   `2 * lead`): single elbow with `lead`-px horizontal stubs at
///   both ends so the arrow doesn't fuse into the bar.
/// - **Backward** (target is to the left, or above/below very close):
///   six-segment route that goes out from the source, jumps to a
///   bypass row above or below the target, then comes back into the
///   target — avoiding drawing through bars between source and
///   target.
#[must_use]
pub fn link_path(sx: f32, sy: f32, tx: f32, ty: f32, link_type: LinkType) -> String {
    let lead: f32 = 12.0;
    // Source-side stub direction: if anchored at end, stub points
    // right; at start, points left.
    let s_right = matches!(link_type, LinkType::E2s | LinkType::E2e);
    let t_right = matches!(link_type, LinkType::S2s | LinkType::E2s);
    let sx_lead = if s_right { sx + lead } else { sx - lead };
    let tx_lead = if t_right { tx - lead } else { tx + lead };

    // "Forward enough" — there's room for a straight elbow.
    let forward = (tx_lead - sx_lead).abs() >= lead && {
        if s_right {
            tx_lead >= sx_lead
        } else {
            tx_lead <= sx_lead
        }
    };

    if forward {
        // Forward link: a single elbow whose vertical leg sits at
        // the midpoint between the two lead points. The previous
        // version dropped vertical immediately at `sx_lead`, which
        // made arrows feel like they "lunged" out of the source bar
        // before swinging across — confusing when source and target
        // were close together.
        let mid = f32::midpoint(sx_lead, tx_lead);
        if (sy - ty).abs() < 0.5 {
            // Same row — straight horizontal segment is cleaner.
            format!("M {sx} {sy} L {tx} {ty}")
        } else {
            format!("M {sx} {sy} L {mid} {sy} L {mid} {ty} L {tx} {ty}")
        }
    } else {
        // Backward link: jog past the bars before swinging across.
        // Bypass 15px below the target if target is above the source
        // (so neither bar is crossed), else 15px above.
        let bypass = if ty > sy { ty + 15.0 } else { ty - 15.0 };
        format!(
            "M {sx} {sy} L {sx_lead} {sy} L {sx_lead} {bypass} L {tx_lead} {bypass} L {tx_lead} {ty} L {tx} {ty}"
        )
    }
}

/// Selection-update semantics. Mirrors common file-manager / list
/// conventions: plain click replaces, ctrl/meta toggles, shift
/// extends from the previously-clicked anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectMode {
    Replace,
    Toggle,
    Range,
    Clear,
}

/// Events bubbled up to the consumer.
#[derive(Clone, Debug)]
pub enum GanttEvent {
    /// Move + resize via drag. `start`/`end` are the new dates.
    UpdateDates {
        id: TaskId,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    UpdateText {
        id: TaskId,
        text: String,
    },
    UpdateProgress {
        id: TaskId,
        progress: f32,
    },
    UpdateType {
        id: TaskId,
        task_type: TaskType,
    },
    ToggleOpen {
        id: TaskId,
    },
    Select {
        id: Option<TaskId>,
        mode: SelectMode,
    },
    SetSort {
        key: SortKey,
        dir: SortDir,
    },
    SetFilter {
        text: String,
    },
    SetReadOnly {
        readonly: bool,
    },
    OpenEditor {
        id: TaskId,
    },
    CloseEditor,
    AddLink {
        source: TaskId,
        target: TaskId,
        link_type: LinkType,
    },
    DeleteLink {
        id: TaskId,
    },
    AddTask {
        task: GanttTask,
    },
    DeleteTask {
        id: TaskId,
    },
    /// Reposition `id` in the task list. The new sibling order is
    /// determined by `before` (insert immediately before this id, or
    /// at the end of `new_parent`'s children when `None`).
    /// `new_parent` defaults to the task's current parent if `None`
    /// is passed; pass `Some(None_)` semantics via the
    /// [`ReorderParent`] sentinel (omitted for simplicity — we just
    /// always require an explicit decision).
    ReorderTask {
        id: TaskId,
        before: Option<TaskId>,
        new_parent: Option<TaskId>,
    },
    ZoomTo {
        level: usize,
    },
}

/// Apply an event mutation locally (for unit tests & demo data).
/// Real consumers usually intercept events and apply via their own
/// repo; this helper exists so the demo route can stand alone.
pub fn apply(state: &mut GanttState, event: &GanttEvent) {
    match event {
        GanttEvent::UpdateDates { id, start, end } => {
            if let Some(t) = state.tasks.iter_mut().find(|t| &t.id == id) {
                t.start = *start;
                t.end = *end;
            }
        }
        GanttEvent::UpdateText { id, text } => {
            if let Some(t) = state.tasks.iter_mut().find(|t| &t.id == id) {
                t.text = text.clone();
            }
        }
        GanttEvent::UpdateProgress { id, progress } => {
            if let Some(t) = state.tasks.iter_mut().find(|t| &t.id == id) {
                t.progress = progress.clamp(0.0, 1.0);
            }
        }
        GanttEvent::UpdateType { id, task_type } => {
            if let Some(t) = state.tasks.iter_mut().find(|t| &t.id == id) {
                t.task_type = *task_type;
            }
        }
        GanttEvent::ToggleOpen { id } => {
            if let Some(t) = state.tasks.iter_mut().find(|t| &t.id == id) {
                t.open = !t.open;
            }
        }
        GanttEvent::Select { id, mode } => match (id, mode) {
            (None, _) | (_, SelectMode::Clear) => state.selected.clear(),
            (Some(id), SelectMode::Replace) => {
                state.selected.clear();
                state.selected.insert(*id);
            }
            (Some(id), SelectMode::Toggle) => {
                if !state.selected.insert(*id) {
                    state.selected.remove(id);
                }
            }
            (Some(id), SelectMode::Range) => {
                // Range needs a layout-aware anchor. Without one we
                // degrade to a Replace; the route can prefill a
                // smarter range if it tracks the anchor itself.
                state.selected.insert(*id);
            }
        },
        GanttEvent::SetSort { key, dir } => state.sort = (*key, *dir),
        GanttEvent::SetFilter { text } => state.filter = text.clone(),
        GanttEvent::SetReadOnly { readonly } => state.readonly = *readonly,
        GanttEvent::OpenEditor { id } => state.editing = Some(*id),
        GanttEvent::CloseEditor => state.editing = None,
        GanttEvent::AddLink {
            source,
            target,
            link_type,
        } => {
            state.links.push(GanttLink {
                id: uuid::Uuid::new_v4(),
                source: *source,
                target: *target,
                link_type: *link_type,
                lag: 0,
            });
        }
        GanttEvent::DeleteLink { id } => state.links.retain(|l| &l.id != id),
        GanttEvent::AddTask { task } => state.tasks.push(task.clone()),
        GanttEvent::DeleteTask { id } => {
            state.tasks.retain(|t| &t.id != id);
            state.links.retain(|l| &l.source != id && &l.target != id);
        }
        GanttEvent::ReorderTask {
            id,
            before,
            new_parent,
        } => {
            // Disallow dropping a task onto its own descendant —
            // that would form a cycle.
            if let Some(new_p) = new_parent {
                if *new_p == *id || is_descendant(state, *id, *new_p) {
                    return;
                }
            }
            let Some(src_idx) = state.tasks.iter().position(|t| &t.id == id) else {
                return;
            };
            let mut task = state.tasks.remove(src_idx);
            task.parent = *new_parent;
            let insert_at = match before {
                Some(target) => state
                    .tasks
                    .iter()
                    .position(|t| &t.id == target)
                    .unwrap_or(state.tasks.len()),
                None => state.tasks.len(),
            };
            state.tasks.insert(insert_at, task);
        }
        GanttEvent::ZoomTo { level } => {
            state.zoom.level = (*level).min(state.zoom.levels.len().saturating_sub(1));
        }
    }
}

/// Helper for drag/resize: snap a date to the current minimum unit.
#[must_use]
pub fn snap(grid: &ScaleGrid, date: DateTime<Utc>, week_start: WeekStart) -> DateTime<Utc> {
    unit_start(grid.min_unit, date, week_start)
}

/// Compute the duration a drag should move, given pixel delta.
#[must_use]
pub fn dx_to_duration(grid: &ScaleGrid, dx: f32) -> Duration {
    let units = f64::from(dx / grid.min_unit_width);
    let secs_per_unit = {
        let next = add(grid.min_unit, grid.start, 1);
        (next - grid.start).num_seconds() as f64
    };
    Duration::seconds((units * secs_per_unit) as i64)
}

/// Convenience for default zoom + scales (used by docs/tests).
#[must_use]
pub fn default_state() -> GanttState {
    GanttState {
        zoom: ZoomConfig {
            level: 3,
            levels: default_zoom_levels(),
        },
        ..GanttState::default()
    }
}

#[allow(dead_code)]
fn _diff_f_doc(unit: crate::types::LengthUnit, a: DateTime<Utc>, b: DateTime<Utc>) -> f64 {
    diff_f(unit, a, b)
}
