Agent P4 — UI dashboard per features/agent/spec/agent.md + issue #24. Builds on `feat/agent-mvp` (962664a P1 + 41edde0 P2 + 732f2f0 P3). Branch off `feat/agent-mvp` as `feat/agent-dashboard`.

Done when ALL hold AND evidence is in transcript:

P4A — Run list (features/agent/agent-ui/src/dashboard/):
- `AgentDashboard` component renders an AgentRun table.
- Sortable: status, kind, started_at, completed_at, cost_cents_estimate, tool_call_count. Default sort live-first (running > awaiting-input > starting > queued > terminal), then started_at desc.
- Filters: status (multi), kind (multi), has-error, awaiting-input.
- Row shows status badge, name, kind, elapsed (live for non-terminal, completed_at-started_at for terminal), cost.
- Row click → detail view. Empty state has a "Start one →" button opening the new-run dialog.

P4B — Run detail (three panes):
- Header: name, status badge, kind, prompt (collapsed), elapsed, cost. Cancel button when non-terminal.
- Logs pane: virtual-scrolling AgentLogLine list with level filter dropdown. Subscribes via `LiveUpdateBus::subscribe_run` if wired, else polls repo.
- Tool Calls pane: sortable ToolCall table. Click row to expand pretty-printed args_json + result_json. For Edit/Write/NotebookEdit, parse `args_json.{path,before,after}` and render line-by-line plus/minus diff via new `InlineDiff` component.
- Conversation pane: ConversationMessage list for the spawning conversation (AgentRun.spawned_from_message_id → conversation_id). Role-styled bubbles (user/assistant/tool/system). Empty state if no link.

P4C — New-run dialog:
- "New Run" button (top-right of dashboard) opens modal.
- Fields: prompt (textarea, required), kind (combobox autocomplete from: claude-code, codex, hermes, hermes-agent, gemini-cli, aider, cursor-agent, mock), worktree_path (optional).
- Submit calls `AgentService.start_run`. On success: close, navigate to new run detail, refresh list.
- Validation: empty prompt/kind disables submit + inline error.

P4D — Route wiring (crates/task-ui/src/feature_routes/):
- `/agent/dashboard` mounts AgentDashboard.
- `/agent/dashboard/:run_id` mounts detail view.
- `/agent-chat` route stays untouched.

Commits: one per phase on `feat/agent-dashboard`, message references plans/agent-p4-dashboard.goal.md AND issue #24. Show `git log --oneline -6`.

Constraints:
- architect-ui primitives ONLY (no raw HTML, no Tailwind hex; theme tokens always per AGENTS.md).
- Dumb components — UI takes props + emits events; data fetching at the route layer.
- ConversationMessage.body wire is plain String — LoroText is internal to the CRDT codec (already wired in P1).
- Lucide icons: `CircleCheck` not `CheckCircle2`.
- StatusBadge variants only `Success`/`Warning`/`Danger`/`Neutral` — map RunStatus → these four at the component boundary.
- Run capn through `nix develop` (hook handles); no NO_CAPN unless genuinely infra (e.g. workspace-wide Vec<String> issue being cleaned up in a parallel branch).
- `cargo check -p task-ui` and `cargo check -p task-app-web --target wasm32-unknown-unknown` exit 0 after each commit.
- Stop after 50 turns if not done; report blocker on issue #24.

After each turn: state which subitem just satisfied and which is next.
