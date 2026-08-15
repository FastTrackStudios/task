# Vox Phase 3 — wire repo + provider services

Phase 1 wired `TaskService` / `InboxService`. Phase 2 will cover every service whose deps are only a `DatabaseConnection`. This plan finishes the remaining services — the ones whose `*ServiceDeps` carries extra Loro repos or optional external providers that `AppState` doesn't construct today.

## Services to wire (grouped by missing infrastructure)

### Need extra Loro repos

These are CRDT-backed and need their `*RepoLoro` constructed off the same workspace `CrdtDoc` that lives on `AppState::workspace_doc`:

- `ProjectService` — `ProjectServiceDeps { project_repo, task_repo }`. Need `ProjectRepoLoro::new(&doc)` on `AppState`.
- `OperatingService` — `OperatingServiceDeps { task_repo, project_repo, event_repo }`. Need `ProjectRepoLoro` + an event repo (likely `calendar_crdt::CalendarEventRepoLoro`).
- `ActivityService` — `ActivityServiceDeps { activity_repo }`. Source TBD; nothing on `AppState` exposes an activity log repo yet.
- `InvoiceService` — `InvoiceServiceDeps { invoice_repo }`. `invoice_crdt::InvoiceRepoLoro` exists; add to `AppState`.
- `AttachmentService` — `AttachmentServiceDeps { repo, nextcloud: Option<Arc<NextcloudSync>> }`.

Note: every existing wired service uses the `task_core::task::TaskRepo` *trait* whose only impl is `TaskRepoStorage<DatabaseConnection>` (sea-orm), **not** `TaskRepoLoro`. The Phase-3 services above use different repo traits (`ProjectRepo`, `EventRepo`, …) — verify each trait's available impls before assuming Loro repos satisfy them. They may also be sea-orm-only.

### Need optional providers

These compile fine with `provider: None` (most surface `provider_not_configured` at call time), but wiring them properly means standing up:

- `MailService` — `MailServiceDeps { email_repo, client: Option<Arc<MailClient>> }`.
- `PeopleService` — `PeopleServiceDeps { people_repo, provider: Option<Arc<NextcloudSync>> }`.
- `CalendarService` (in `business.rs`) — `CalendarServiceDeps { task_repo, event_repo, provider: Option<Arc<NextcloudSync>> }`.
- `ConversationService` — `ConversationServiceDeps { provider: Option<Arc<dyn CommunicationChannelProvider>> }`. Trivial; ship this in the first cut.

### Needs broadcast wiring

- `TimeService` — `TimeServiceDeps { task_repo, op_tx: Option<broadcast::Sender<TaskOp>> }`. Reuse `vox.task_op_tx` so time-entry edits show up on the existing TaskService subscription stream.

## Implementation sketch

1. Decide per service whether the bound repo trait has a Loro-backed or sea-orm-backed impl, then build the right thing inside `VoxState::new`.
2. Add a `providers` config object (env-driven) so `NextcloudSync`, `MailClient`, `OpenFoodFactsClient`, `CommunicationChannelProvider` are constructed once and shared.
3. Extend `VoxState` and the `vox_ws_handler` match arms.

## Verification

- `cargo check -p task-server` + `cargo test -p task-server`.
- Every CLI subcommand backed by these services runs against a fresh server.
- `/vox` handler logs zero "dispatcher not yet wired" lines.

## Open questions

- Is there a project / event / activity / attachment **Loro** repo at all today, or are those service traits sea-orm-only? If sea-orm-only, Phase 3 collapses into "thread the existing DB through more service constructors."
- Auth: the `/vox` WS handler currently ignores the `token` and `organization_id` query params the CLI sends. Decide whether auth lands in Phase 3 or its own slice.
