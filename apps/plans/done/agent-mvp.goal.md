Agent feature MVP per features/agent/spec/agent.md and Forgejo issue #17.

Scope is the smallest viable slice that lets us dogfood by launching real Claude Code runs from Task and watching them in a dashboard. Phase 1+2 only; pause/resume, Hermes routing, scheduled runs, PR creation, multi-adapter parity all deferred to follow-up issues.

Done when ALL hold AND evidence is in transcript:

P1 — proto + entities (features/agent/agent-proto):
- New entities exist with #[derive(Entity)]: `ToolCall`, `ConversationMessage`, plus fields added to existing `AgentRun` (parent_run_id, worktree_path, git_repo_connection_id, spawned_from_message_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, cost_cents_estimate, tool_call_count, assistant_message_count, max_tokens, max_tool_calls, max_wall_seconds). Show via grep.
- `AgentService` trait extends to: `start_run(prompt, kind, worktree_path) -> AgentRun`, `cancel(run_id)`, `approve_tool(tool_call_id)`, `deny_tool(tool_call_id, reason)`, `subscribe_logs(run_id, since_seq) -> stream`, `subscribe_workspace_events(workspace_id) -> stream`.
- `AgentRun.status` is validated by the service against the state machine in r[agent.run.status-state-machine]; invalid transitions return `InvalidInput`. Test covers each illegal transition.
- `cargo test -p agent-proto -p agent-crdt` exits 0.

P2 — claude-code adapter (features/agent/agent-claude-code, new crate):
- `AgentAdapter` trait in agent-proto with `start`, `subscribe`, `cancel`.
- `ClaudeCodeAdapter` implements it via subprocess + JSON-line stdout parsing (CodexMonitor pattern adapted for claude-code's `--output-format stream-json`).
- Normalizes claude-code's tool calls into `ToolCall` rows + emits `AgentLogLine` per event.
- Integration test: launch the adapter against a mock claude-code binary (one that emits a known stream-json sequence), assert AgentRun reaches `completed`, log lines + tool calls match expected count.
- `cargo test -p agent-claude-code` exits 0.

P3 — live update transport (features/agent/agent + apps/server):
- Service emits the three event kinds (`RunStateChanged`, `LogAppended`, `ToolCallChanged`) via vox subscription keyed on `run_id` or `workspace_id`.
- 50ms batching window for workspace-scope subscribers (r[agent.live-update.workspace-scope]).
- Test: two subscribers (one per-run, one workspace-scope) both receive events from a synthetic run; workspace-scope batches.

P4 — UI dashboard (features/agent/agent-ui):
- Run-list view sortable by status / kind / cost / duration with filter facets (status, kind, has-error, awaiting-input).
- Run-detail view: three panes (logs / tool calls / conversation) each subscribing independently.
- Tool-call row renders inline diff for `Edit`/`Write` calls using args_json.before/after.
- New-run dialog: prompt + kind picker + worktree path. Submits via `start_run`, navigates to detail view.

Commits: one per phase on `feat/agent-mvp` branch off main. Each commit message references plans/agent-mvp.goal.md AND issue #17. Show `git log --oneline -6` at end.

Constraints:
- proto Entity macro collision is fixed upstream (architect commit ea1146b); use it. If multiple structs in one file collide on Model/Column/etc., that's a regression — fix don't bypass.
- `Vec<String>` fields with sea-orm derive must use a Json wrapper or skip the column. Don't ship a broken proto.
- LoroText for `ConversationMessage.body` per r[agent.crdt.conversation-message-text]; LWW for other scalars.
- Run capn through `nix develop` (the hook handles this); no NO_CAPN unless the failure is genuinely infra.
- `cargo check -p task-ui` and `cargo check -p task-app-web --target wasm32-unknown-unknown` exit 0 after each phase commit.
- Stop after 60 turns if not done; report blocker on issue #17.

After each turn: state which subitem just satisfied and which is next.
