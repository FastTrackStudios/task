# Relevancy + the single trusted inbox

**Status:** active — the first post-launch product layer. All logic is
domain-side (feature crates + architect services); CLI and web UI are
both thin renderers of the same functions. Follow-up to
[mvp.md](mvp.md).

## The product idea (Cody, 2026-07-01)

Default view = the todo list, showing only tasks that are **Active**
and **Relevant**. Relevancy is contextual visibility:

- **Time** — morning/evening routines (brush teeth, hair) only show
  in their windows; meal-prep tasks around meal times ("during food
  prep time, show what I had planned for lunch that day").
- **Location** — errands only when out, studio tasks only at studio.
- **Device** — phone-actionable vs computer-actionable.
- **Active timer** — when the timer runs on a project, prioritize
  that project's other tasks.

And one **single trusted inbox** for todos, reminders, project ideas,
notes — fleeting notes captured without deciding where they go,
processed reliably every day, optionally by an AI that promotes them
into tasks/notes. "Eliminates the guesswork of where to put
something."

## Architecture rule

Relevance scoring, context vocabulary, and inbox processing live in
the feature crates (`task`, `inbox`) and are exposed through the
architect service surface. The web UI calls the *same exported
functions* client-side (the task crate is already a UI dep) so the
optimistic store keeps working offline; the CLI gets them via
`TaskListFilter` server-side. One implementation, two renderers.

## Building blocks already there

- `TaskInfo.contexts: StringList` — GTD contexts (`@shopping`,
  `@dev`). Relevancy rides on these: `@morning`, `@evening`,
  `@mealprep`, `@home`, `@studio`, `@errands`, `@phone`, `@computer`.
- `status_is_open` — the domain's "Active" classification.
- `TaskListFilter` + `TaskService::query` — AND-filter semantics,
  already paged; relevance slots in as an optional field.
- Timer sessions (`timer` feature) — the running session's project id
  is the active-project signal.
- **Inbox already exists**: `features/inbox`, fleeting capture modal,
  `InboxMutations` incl. promote-to-task/note, ProcessReview flow,
  and [inbox-agent-ingestion.md](inbox-agent-ingestion.md) for the
  AI-processing design. The "single trusted system" is positioning +
  polish, not a new feature.

## v1 (this pass)

1. `task::relevance` module:
   - `RelevanceContext { local_hhmm, location, device, active_project }`
     — wire-serializable; every field optional.
   - Time-window contexts v1 (fixed windows, personalization later):
     `@morning` 05:00–10:00 · `@mealprep` 11:00–13:00 + 17:00–19:00 ·
     `@evening` 20:00–24:00.
   - Gating rule: a task carrying gate contexts is visible only when
     at least one matches the context; tasks with no gate contexts
     are always relevant; **due/scheduled today or overdue always
     shows** (deadlines trump gates).
   - `relevance_rank` for ordering: active-project first, then
     due/overdue, then in-window, then the rest.
2. `TaskListFilter.relevance: Option<RelevanceContext>` — server-side
   filter + rank sort (CLI parity).
3. CLI: `task task list --relevant` (auto local time; `--at`,
   `--location`, `--device` overrides).
4. Web: tasks page defaults to Active + Relevant with visible toggle
   chips (so hidden items are one click away); context built from the
   browser clock + the running timer session; toggles persisted
   per-account (`task.prefs.<email>.*` in localStorage for now).
5. `/` lands on the task list.

## Shipped since (2026-07-02)

- Checkbox click cycle (`task::click_transition`): open →
  in-progress → done; subtask under a running parent completes
  directly (the parent's timer owns the process).
- Automatic time tracking (`task::track_status_transition`):
  in-progress opens an inline `TimeEntry`, leaving it closes the
  entry; enforced in `TaskBackend::update` so cascade completions
  stop the parent's clock. In-progress tasks rank first in Relevant.
- CLI `task task start`; `[~]` list marker; web three-state checkbox
  (pulsing ring while tracking).

## Later

- **Per-user prefs entity** (architect::Entity, per-org auth user):
  default page, default filters, personal time windows, named
  locations. Replaces the localStorage stopgap so CLI + all devices
  share personalization. This is also what makes "switch account →
  see *your* setup" real.
- **Location/device signals**: manual context switcher chip first
  (an "I'm at: home/studio/out" selector), geolocation later; device
  class inferred from user-agent.
- **Meal-plan bridge**: during `@mealprep` windows, surface the
  mealplan feature's planned meal for the day alongside the tasks.
- **Inbox AI processing**: wire the agent per
  [inbox-agent-ingestion.md](inbox-agent-ingestion.md) — daily
  processing pass proposes task/note promotions; user approves in
  ProcessReview.
- **Timer-page task picker** — done: the start form's input
  fuzzy-searches open tasks (`crates/ui/src/fuzzy.rs`); picking one
  links the session via `task_note_path` + the task's
  `project_id`/`project_path`. Remaining: feed the topbar timer
  widget from task work too.
- Account switcher: switching accounts applies that user's prefs
  (today it only changes identity/presence — reads as "nothing
  happens").
