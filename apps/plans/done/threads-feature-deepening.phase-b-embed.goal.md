Threads Phase B-embed per plans/threads-feature-deepening.md — embeddable ThreadEmbed UI in threads-ui. Phases B-markers, C, D, E out of scope.

Prereq: Phase A landed (commit 09db5ac on feat/threads-phase-a). Branch off it via `git worktree add` then EnterWorktree by path. architect-ui primitives + theme tokens only; no hex colors.

Done when ALL hold AND evidence in transcript:

P-B-embed.1 — features/threads/threads-ui/src/embed/mod.rs (new):
- `ThreadEmbed` component, dumb (route layer owns repo writes). Props: entity_kind: String, entity_id: Uuid, anchor: Option<threads_proto::Anchor>, comments: Vec<Comment>, mode: EmbedMode { Sidebar (default), Inline, Margin }, plus EventHandler callbacks: on_create<CommentCreate>, on_reply<(Uuid, String)>, on_resolve<Uuid>, on_reopen<Uuid>, on_promote_to_task<Uuid>, on_react<(Uuid, String)>, on_delete<Uuid>, on_edit<(Uuid, String)>.
- Dispatches layout via mode prop.

P-B-embed.2 — features/threads/threads-ui/src/embed/thread_card.rs (new):
- Renders one comment + reply tree (recursive over comments filtered by reply_to). Header: author + anchor breadcrumb (AnchorChip) + kind badge (StatusBadgeVariant per kind) + resolved status. Body: plain text (no markdown yet — flagged FUTURE per plan v1 limits). Footer: reactions row, actions menu (edit/delete/resolve/copy-link/promote-to-task when kind=action).
- Reply composer inline at bottom, collapsible.

P-B-embed.3 — features/threads/threads-ui/src/embed/composer.rs (new):
- Rich composer: Textarea (architect-ui) + mention autocomplete over a `Vec<String>` mention pool prop (route supplies) + attachment paperclip stub (no upload wired in this goal — emits an event the route ignores until Phase C) + thread-kind picker chip (default discussion) + "Promote to task" inline button visible when kind=action.

P-B-embed.4 — features/threads/threads-ui/src/embed/anchor_chip.rs (new):
- Breadcrumb pill rendering per Anchor variant:
  - Entity → no chip
  - TextQuoteSelector → "“<truncated exact>”"
  - TextPositionSelector → "Block #<short-id>"
  - FragmentSelector → "MM:SS–MM:SS"
  - RegionSelector → "Region p<page>" or "Region"
  - CanvasNodeSelector → "Canvas node"
  - CellSelector → "<table>.<col>[row N]"
- Unit test renders one of each via Dioxus testing harness OR asserts a `breadcrumb_label(&Anchor)` pure helper returns the expected String for every variant.

P-B-embed.5 — re-export from threads-ui lib.rs; keep existing CommentDashboard etc. exports.

Verify (each exits 0):
- `cargo test -p threads-ui` (breadcrumb_label tests pass)
- `cargo check -p threads-proto -p threads-crdt -p threads-db -p threads -p threads-ui`
- `cargo check -p task-ui`
- `cargo check -p task-app-web --target wasm32-unknown-unknown`

Commit one commit on a new feat/threads-phase-b-embed branch off feat/threads-phase-a. Reference plan Phase B-embed in message. Show `git log --oneline -3`.

Constraints:
- EnterWorktree off feat/threads-phase-a; do not modify the user's primary worktree.
- architect-ui primitives only (Textarea, Button, StatusBadge with Success/Warning/Danger/Neutral variants only, Popover, IconButton). Theme tokens for colors; never hex/Tailwind palette like bg-red-500.
- lucide icons by current names (CircleCheck not CheckCircle2; see CLAUDE.md gotchas).
- ThreadEmbed is wholly dumb — no repo calls, no signals owned. All state lives at the caller.
- Use .peek() vs .read() inside use_effect to avoid update loops.
- contenteditable inputs: avoid the prefix-in-textContent bug — read via the existing knowledge-ui pattern.
- No `// TODO`/`// removed`/`// kept for compat` litter. `// FUTURE:` only for plan v1 limits (markdown body, edit history, threaded reactions, mention notifications).
- Fix root causes; no --no-verify unless user authorizes.
- Stop after 30 turns. If blocked, report blocker + last green check + smallest repro.

Each turn: state which P-B-embed.N just satisfied, which is next, surface any UX divergence from the plan.
