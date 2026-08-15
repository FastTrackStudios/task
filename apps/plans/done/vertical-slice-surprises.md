# Vertical Slice Reset — Surprises

Captured during the reset to `tasks + projects + auth`. Items here were
on the deletion list but turned out to be load-bearing for kept code.

## task-core / email + property modules retained

- `crates/task-core/src/email/` — `EmailRef` is referenced by the `Task`
  model (`task::model::EmailRefList`) and by `Project` (via re-export).
  Both are kept by the slice, so the email model module had to stay.
  No service / dispatcher surface is exposed for email.
- `crates/task-core/src/property/` — `property::JsonObject` is the
  Facet/SeaORM bridge for `Task::properties` (re-exported as
  `JsonProperties`). The `Project` model also uses it. Module stays as
  a pure data carrier; no service surface.

These were not on the keep list in the prompt; flagging here so the
follow-up cleanup knows they're intentional.

## task-core / no separate `service` for inbox + project

The original instruction kept `TaskService`, `InboxService`,
`ProjectService` in `service::model`. All three live in the
unmodified monolithic `service/model.rs` (3.2k lines) which still
declares many other traits. Trimming them out would have required
splitting the file; instead the lib.rs re-export list now only re-
exports the kept traits + their dispatchers and clients. The other
trait definitions stay alive in the file but are never re-exported,
so they're effectively dead until someone wires them.

## Project routes / agent + knowledge integration stripped from sheet

`features/project/project-ui/src/pm/sheet.rs` was tightly coupled to
`agent_ui::hermes_kit::AgentRunPanel` + `knowledge_ui::OutlinerEmbed`
(Notes tab, Agent tab, shadow blocks). Those features are gone, so
the sheet is reduced to the Comments / Activity / Subtasks / Git
tabs. The Notes + Agent tabs are no longer exposed.
