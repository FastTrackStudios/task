# Project Overview + Task UI — design

**Status:** design (2026-05-28). Target surface: `crates/ui` pages + `features/{project,task}/…-ui` components.
**Why:** Projects/Goals render live from vox, but there is **no project drill-down, no progress rollup, and no task↔project integration**; the Task UI is feature-rich (list/kanban/detail/quick-add) but **demo-seeded**, with no filtering, search, drag-drop, or project assignment. This is the core "tasks + projects vertical slice" the app collapsed to — make it actually usable end to end.

## Principles (AGENTS.md)
- architect-ui primitives only; theme tokens (`bg-card`, `text-muted-foreground`, …), never hex; dark mode default.
- Dumb components: feed props in, emit `EventHandler<Mutation>` out. No CRDT/vox awareness below the page layer.
- `StatusBadgeVariant` ∈ {Success, Warning, Danger, Neutral}. `ButtonVariant`/`ButtonSize` always explicit. Verify lucide names.
- Data wiring stays at the **page** layer via `use_resource` → architect `*ServiceClient` (mirror `projects.rs`/`goals.rs`). Components never fetch.

## Information architecture / routes
```
/                home — real overview dashboard (was a static stub)
/projects        project list (exists) → cards link into detail
/projects/:id    project OVERVIEW (new) — the drill-down
/tasks           task workspace (exists; upgrade: filter/search/drag/assign)
/goals /schedule /gantt /wiki  (exist)
```
Add `ProjectDetailRoute { id: String }` to `routes.rs` (+ page, nav stays list-level).

## Screen 1 — Home as overview dashboard
Replace the static stub with a cross-project rollup answering "what needs me now / where are things."
```
┌ Home ───────────────────────────────────────────────┐
│ This cycle: Q2 · Cycle 2 (May 12–Jun 8)              │
│ ┌ Active projects ─┐ ┌ Due this week ─┐ ┌ Overdue ─┐ │
│ │  6   ▓▓▓▓░ 62%   │ │  9 tasks       │ │  3 tasks │ │
│ └──────────────────┘ └────────────────┘ └──────────┘ │
│ Projects (by progress)        My open tasks (top)     │
│  ▸ Wiki engine    ▓▓▓▓▓░ 80%   ☐ Fix sync race  P1   │
│  ▸ Finance        ▓▓░░░░ 30%   ☐ Email STARTTLS test │
│  ▸ Email          ▓▓▓░░░ 48%   ☐ …                   │
└──────────────────────────────────────────────────────┘
```
Components: reuse `Card`; new `StatCard` (label + big number + optional mini progress), `ProgressBar` (token-filled `bg-primary` over `bg-muted`). Rollups computed client-side from the project+task lists (see "Rollups").

## Screen 2 — Project Overview (`/projects/:id`) — primary new work
```
┌ ‹ Projects   Wiki engine                    [Edit] ┐
│ active · P1 · lead: Cody · due Jun 8 · ▓▓▓▓▓░ 80%  │
├────────────────────────────────────────────────────┤
│ Tabs:  Overview │ Tasks (12) │ Milestones (3) │ ...  │
├────────────────────────────────────────────────────┤
│ OVERVIEW                                             │
│  ┌ Progress ──────┐  ┌ Next milestone ───────────┐  │
│  │ 8 / 12 done     │  │ ◑ v1 graph parity · Jun 1 │  │
│  │ ▓▓▓▓▓▓▓░ 67%    │  │   5/7 tasks               │  │
│  └────────────────┘  └───────────────────────────┘  │
│  Sub-projects        Recent activity / details (md)  │
│   ▸ Graph layer 90%                                  │
│  TASKS tab → embedded ProjectTaskList (filtered)     │
│  MILESTONES tab → milestone cards (state/due/rollup) │
└──────────────────────────────────────────────────────┘
```
- **Project detail edits** via a right-side `ProjectDetailSheet` (mirror `TaskDetail`): title, status, priority, lead, tags, `target_date`, `details` (md), `archived`. Emits `ProjectMutation::Update`.
- **Tasks tab** = `TaskList` filtered to tasks whose `projects: Vec<String>` contains this project (wikilink match). Reuses the existing component verbatim.
- **Milestones tab** = grid of milestone cards (`Milestone`: title, `state` open/closed → StatusBadge, `due_on`, task-count rollup, optional `goal_id` chip). First UI for milestones (currently zero).
- **Sub-projects** = `parent_id` children, each with its own progress bar.

## Screen 3 — Task workspace upgrades (`/tasks`)
Keep `TasksApp` (list/kanban/detail/quick-add); add the missing affordances:
1. **Filter + search bar** (new `TaskToolbar`): text search (title), and toggles for status / priority / context / project / due-range. Pure filter applied before render; no new mutation.
2. **Kanban drag-drop**: column drop → `SetStatus`. Use the architect-ui drag primitive if present (check catalog: `dx components` — do NOT hand-roll listeners); else reuse the gantt port's pointer-drag pattern.
3. **Inline edit** in `TaskRow`: due-date `<input type=date>` + priority cycle, emitting `SetPriority`/`Update` (mirrors `view-table` cells).
4. **Project assignment**: in `TaskDetail`, a project multi-select chip editor → new `TaskMutation::SetProjects { id, projects }`.

## New components (all dumb, architect-ui only)
| Component | Crate | Props (sketch) |
|---|---|---|
| `StatCard` | `crates/ui` or architect-ui | `label`, `value`, `progress: Option<f32>` |
| `ProgressBar` | architect-ui (reusable) | `percent: f32`, `variant` |
| `ProjectDetailSheet` | `features/project/project-ui` (new crate) | `project`, `on_event: EventHandler<ProjectMutation>`, `on_close` |
| `ProjectOverview` | `project-ui` | `project`, `tasks`, `milestones`, `children`, `on_event` |
| `ProjectTaskList` | thin wrapper over `task-ui::TaskList` | `tasks` (pre-filtered), `on_*` |
| `MilestoneCard` | `features/milestone/milestone-ui` (new) | `milestone`, `task_done`, `task_total` |
| `TaskToolbar` | `task-ui` | `filter: TaskFilter`, `on_change` |

`project-ui` / `milestone-ui` are new feature-trio UI crates mirroring `task-ui` (dumb, architect-ui). `crates/ui` pages own the data + compose them.

## Data model touchpoints
- `ProjectMutation` (new, in `project` or `project-ui`): `Update { project }`, `SetStatus`, `SetArchived`. Reducer `apply` like `task-ui::store`.
- `TaskMutation::SetProjects { id, projects: Vec<String> }` (new variant) — task→project assignment.
- **Task↔Project link** is the existing `TaskInfo.projects: Vec<String>` (wikilink `[[Project]]`). Match by project title/slug. (Future: stable id link, but ship on the existing field.)
- **Milestone↔Task**: `Milestone.project_id` exists; tasks don't yet reference milestones. Rollup v1 = "tasks in project" only; per-milestone rollup is a follow-up once tasks carry a milestone ref.

## Rollups (pure functions, unit-testable, no UI)
- `project_progress(project, tasks) -> {done, total, percent}` — percent from done/total when `progress_percent < 0`, else the stored value.
- `cycle_window(now) -> Cycle` (reuse `features/cycle` generator) for the Home "this cycle" framing.
- Overdue/due-this-week counts from `TaskInfo.due` vs today. Put these in a `rollup.rs` in `project-ui` (or a shared `ui` util) with tests.

## Data wiring seam (live data)
- New page `crates/ui/src/pages/project_detail.rs`: `use_resource` → `ProjectServiceClient.get(id)` + `TaskServiceClient.list()` (filter client-side) + milestones list. Same wasm-only pattern as `projects.rs` (native client TODO already noted there).
- **Tasks page persistence**: replace `seed_state()` with `use_resource` → task service (the documented follow-up). Until the task service client exists on the web target, keep the seed but structure the page so the swap is one call site. **This is the one real backend dependency — confirm the web task client exists before committing to live tasks.**

## Phasing
1. **P1 — Project Overview** (highest value, mostly composition over existing data): `ProjectDetailRoute` + `project_detail.rs` + `ProjectOverview`/`ProjectDetailSheet` + `project_progress` rollup + Tasks tab reusing `TaskList`. Projects already live → real data immediately.
2. **P2 — Task workspace upgrades**: `TaskToolbar` (filter/search) + inline edit + `SetProjects`. Pure UI, demo-seed fine.
3. **P3 — Milestones UI**: `milestone-ui` + `MilestoneCard` + Milestones tab + milestone service wiring.
4. **P4 — Home dashboard**: `StatCard`/`ProgressBar` + cross-project rollups + cycle framing.
5. **P5 — Kanban drag-drop** (catalog-primitive first) + **task persistence seam** (gated on the web task service client).

## Resolved dependencies (checked 2026-05-28)
- **`TaskServiceClient` exists** (`features/task/task/src/lib.rs:51`) — tasks can go live on the web target with the same `use_resource` pattern as projects. The `/tasks` demo seed is a choice, not a missing client → P1 Tasks-tab and P5 persistence are unblocked.
- **`ProjectService` has `get(id)`, `get_by_path(path)`, `update(project)`** (`features/project/project/src/service.rs:46–62`) — project detail fetch + `ProjectMutation::Update` persistence are both backed. **P1 can be fully live, no backend work needed.**

## Open questions
- Project↔task link: stay on `projects: Vec<String>` wikilinks now, or introduce a stable `project_id` on `TaskInfo`? (Wikilink ships faster; id is more correct.)
- `project-ui`/`milestone-ui` as new crates vs folding into `crates/ui` — new crates match the feature-trio convention and keep `crates/ui` thin.
