# Gantt port — follow-ups

## Round 2 — landed since the first cut

- Date pickers in the side editor (`<input type="date">`)
- Double-click a bar to inline-rename; Enter/Esc commits/cancels
- Bar tooltips (architect-ui `Tooltip`): name + dates + progress
- Drag-to-scroll: pointer near pane edge auto-scrolls chart pane
  horizontally / body vertically (via `document::eval`)
- Sidebar ↔ chart vertical scroll sync via single outer y-scroll
  container; chart pane retains its own x-scroll
- Progress drag handle on each leaf bar (drag the white marker)
- Resize handles widened to 6px hit zones; cursors flip to `ew-resize`
- Link anchors render on milestones too
- Toolbar "Today" + "Find selected" buttons (scroll-into-view)
- Sidebar filter input + sort dropdown wired in the toolbar; sort
  preserves tree structure (siblings re-order within parent)
- Hotkeys on the focused gantt: Del/Backspace delete, Enter opens
  editor, ←/→ nudge dates, Ctrl/Meta+A select all, Esc clears
- Multi-select (Ctrl/Meta-click toggles, plain click replaces); drag
  on a selected bar moves the whole set
- `EmptyState` when `tasks` is empty
- `readonly: bool` prop disables drag/resize/link/edit affordances and
  surfaces a `Read-only` badge in the toolbar
- Calendar-correct month/quarter/year diff in `time::diff_f`
- Smarter link routing: forward elbows get lead-in/lead-out stubs;
  backward links route via a bypass row above/below the target
- Demo palette switched to `var(--color-*-500, …)` so retinting via
  theme tokens works without recompiling the demo

## Round 3 — landed since the second cut

- **Context menu** (right-click bar): Edit details / Duplicate /
  Convert task↔milestone / Delete. Anchored at the pointer; click
  outside closes; respects read-only.
- **Row reorder by drag**: HTML5 drag-and-drop in the sidebar. Drop on
  the top quarter inserts before, bottom quarter inserts after, middle
  reparents (cycle check prevents dropping a task onto its own
  descendant). New `GanttEvent::ReorderTask { id, before, new_parent }`.
- **Virtualized rendering**: bars + sidebar rows outside `[scroll_top -
  buffer, scroll_top + viewport + buffer]` are skipped at render time.
  Sidebar uses top/bottom spacers to keep scrollbar geometry and
  per-row alignment with the chart correct.
- **Custom sidebar columns API**: `columns: Option<Vec<GanttColumn>>`
  prop with built-in kinds (`Name` / `Start` / `End` / `Progress` /
  `Duration` / `Type`). When set, the sidebar width is computed from
  the column widths sum.

## Still open



Initial port of [svar-widgets/gantt](https://github.com/svar-widgets/gantt)
lives in `features/gantt/gantt-ui/` with a stub demo route at `/gantt`.
Substantial core landed; this file enumerates the gaps so a follow-up
arc can pick them off.

## Landed (this arc)

- Types: `GanttTask`, `GanttLink` (FS/SS/FF/SF), `ScaleConfig`, `ZoomConfig`
- Date math: `unit_start`, `add`, `diff_f` (minute → year)
- Scale-grid builder with `x_for_date` / `date_for_x` inverses
- Layout pass: hierarchical linearization, geometry, expand/collapse
- Components: `Gantt` (root), `Toolbar`, `Grid` (sidebar), `TimeScale`,
  `Chart`, `Bars`, `LinkLayer`, `TaskEditor`
- Drag/resize/link-create via shared pointer state + chart-level
  `onpointermove`/`onpointerup`
- Weekend shading, today line, custom markers
- Six default zoom levels (year/quarter → day/hour)
- Side-sheet editor via architect-ui `Sheet`
- Stub `/gantt` route in `task-ui` with seeded summary + tasks +
  milestone + links

## Not yet ported (svar parity gap)

These are the meaningful features still missing relative to svar's
open-source build. PRO-only features are out of scope.

1. **Virtualized rendering**. We render every row/cell. Fine for
   demos, but svar windows the chart based on `scrollLeft` /
   `scrollTop`. Needed before this can show >1k tasks.
2. **Drag preview snapping**. Currently snaps on `pointerup`. Svar
   snaps live to the active scale's step.
3. **Reorder rows by drag**. Vertical drag in the sidebar grid.
4. **Sort + filter** in the sidebar (incl. natural-language search).
5. **Custom columns** on the sidebar grid (svar accepts `IGanttColumn`).
6. **Context menu** + **hotkeys** (Ctrl+Z/Y, Del, arrows for nudge).
7. **Auto-scheduling**. PRO in svar — skip.
8. **Bar tooltips** (architect-ui has `Tooltip`; just wire it onto each bar).
9. **Inline edit of the bar label** (double-click → contenteditable).
10. **Date entry in the editor**. Currently read-only; needs
    architect-ui `DatePicker` integration + an `UpdateDates` event from the
    form.
11. **Multi-select**. Single only today.
12. **Undo/redo** (PRO in svar).
13. **Theme polish**: bar colors per `task_type` via theme tokens
    rather than hardcoded primary/foreground.
14. **Wire to `TaskRepoLoro`**. The route currently uses local
    signal state; a `GanttLive` wrapper should subscribe to the
    org doc and translate `Task` ↔ `GanttTask`. Open question:
    where do dependency *links* live in the existing model? May need
    a small `task-proto` addition.
15. **Tests**. Native unit tests for `time.rs` and `scales.rs` (the
    pure math layers) would be cheap and high-value.

## Known papercuts

- `time::diff_f` for `Month`/`Quarter`/`Year` is approximate.
  Calendar-correct diff (matching svar's `lib-schedule`) needs more
  work for months that don't have 30 days. Demo-quality is fine.
- The link arrow path uses a fixed elbow `dx`; long links can render
  through bars. Svar routes around bars — non-trivial.
- `Bar` mounting overlay element on milestones short-circuits before
  the link-handle render, so milestones can't be link sources via the
  edge dot. Workaround: shift-pointerdown on the milestone diamond.
- The `Sheet` editor cannot edit start/end dates yet (text-only).
