Notifications feature MVP. New issue to be filed after this lands.

Goal: fire a notification when an `AgentRun` transitions to a terminal status (completed / failed / cancelled / timed-out) or blocking status (awaiting-input / paused). Build the substrate generically so other features can emit notifications later (calendar reminders, task due dates, etc.).

Branch off `main` as `feat/notifications-mvp`.

Done when ALL hold AND evidence is in transcript:

P1 — proto + entity (features/notifications/notifications-proto):
- `Notification` entity: id, kind (string — `run.completed` / `run.failed` / `run.blocked` / `run.awaiting-input` / future), title, body, severity (info/warning/error), entity_kind (string — `agent_run`/`task`/...), entity_id (Uuid), action_url (Option), created_at, read_at (Option), dismissed_at (Option). #[architect(Entity)] for repo + CRDT codec.
- `NotificationChannel` entity: id, kind enum (`browser-toast`, `browser-push`, `desktop-libnotify`, `hermes-relay`), enabled, config_json (per-channel knobs like push subscription endpoints).
- `NotificationRule` entity: when_kind (matches Notification.kind glob), to_channel_id, min_severity, enabled. Multiple rules per channel; first-match wins.
- `NotificationService` vox trait: list/get/mark_read/dismiss/create_rule/list_rules/list_channels.

P2 — agent integration + router (features/notifications/notifications + apps/server):
- `NotificationRouter` server-side service subscribes to `agent::LiveUpdateBus` and emits a Notification on every status transition matching: `Running → Completed|Failed|Cancelled|TimedOut|AwaitingInput|Paused`.
- Title/body templated per kind: `"Run #{run_id} completed"` / `"Run #{run_id} needs your input"`. action_url points to `/agent/dashboard/{run_id}`.
- Rules applied: for each emitted Notification, look up matching `NotificationRule`s, dispatch through each rule's channel via a `ChannelDeliver` trait. Default install: one `browser-toast` rule for ALL kinds (so MVP works zero-config).
- Idempotent: same `(kind, entity_id, transition_at)` tuple within 5s of an existing Notification is deduped — covers re-emits during sync reconnect.

P3 — UI (features/notifications/notifications-ui):
- `NotificationInbox` component — list of unread + recent-dismissed Notifications, mark-read on click, dismiss-all button.
- `NotificationToast` — auto-dismissing top-right toast that fires on every new Notification arriving via subscription.
- `NotificationBell` — header icon with unread count badge; click opens inbox dropdown.
- Browser Notification API delivery: when `browser-push` channel is enabled, request permission once and `new Notification(title, {body, icon})` on each matching notif.
- Route `/notifications` mounts the full inbox view; bell + toast mount globally in the AppShell.

Commits: one per phase on `feat/notifications-mvp`. Each commit message references plans/notifications-mvp.goal.md.

Constraints:
- architect-ui primitives only.
- Vec<String> fields tagged #[architect(json)] — proto compiles under --features server (known macro gap separately; if it bites, document but don't block this work).
- LoroText for Notification.body if multi-line.
- Run capn through `nix develop`; no NO_CAPN unless infra.
- `cargo test -p notifications-proto -p notifications -p notifications-ui` exits 0.
- `cargo check -p task-ui` + `cargo check -p task-app-web --target wasm32-unknown-unknown` clean.
- Stop after 80 turns; report blocker on a new issue.

After each turn: state which subitem satisfied + which is next.
