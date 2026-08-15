Threads Phase D per plans/threads-feature-deepening.md — wire ThreadEmbed into every feature route. Phase E out of scope.

Prereq: Phases A + B-embed + B-markers landed. Phase C optional (audio upload uses POST /api/attachments when C is live; without C, the recorder stays disabled and the rest still works).

Done when ALL hold AND evidence in transcript:

P-D.1 — Knowledge `PageView` (crates/task-ui/src/feature_routes/knowledge.rs + features/knowledge/knowledge-ui/src/editor/{block,outliner}.rs):
- Add a Threads tab on PageView. Renders ThreadEmbed in Sidebar mode against `(entity_kind: "knowledge_page", entity_id: page.id)`.
- BlockThreadMarker in the gutter of every Block that has at least one un-resolved thread anchored to it (TextQuoteSelector or TextPositionSelector with matching block_id).
- Selection toolbar "Comment" action in the BlockEditor — emits a `TextQuoteSelector` anchor with exact + prefix/suffix context (24 chars each side max).
- TextRangeHighlight on every anchored range; resolves via `resolve_text_quote` at render time. Orphans render in an "Orphan threads" panel at the bottom of the Threads tab.

P-D.2 — Project `TaskDetailSheet` (crates/task-ui/src/feature_routes/project.rs):
- Replace the placeholder "Comments" tab content with ThreadEmbed against `(entity_kind: "task", entity_id: task.id)`.
- Wire `on_promote_to_task`: when fired on an action thread, create a new `project_proto::Task` with title=comment.body[..80], description carrying backlink to the thread, set `Comment.spawned_task_id` to the new task id. Reverse link: extend `project_proto::Task` with `source_thread_id: Option<Uuid>` (additive). Mention this proto extension in the commit.

P-D.3 — Invoice `InvoiceEditor` (features/invoice/invoice-ui/src/ninja/editor.rs):
- Add a Threads side-rail visible while editing. ThreadEmbed against `(entity_kind: "invoice", entity_id: invoice.id)`.

P-D.4 — Timer `TimeEntryEdit` (features/timer/timer-ui/src/solidtime/...):
- ThreadEmbed with audio-comment-first UX. AudioCommentRecorder front-and-center; on record, POST to /api/attachments (if Phase C live; otherwise log a warning and keep the bytes in memory for a follow-up upload).

P-D.5 — Calendar event detail (features/calendar/calendar-ui/src/interactive/calendar.rs):
- ThreadEmbed for meeting agendas / follow-ups.

P-D.6 — Inventory item detail (features/inventory/inventory-ui/src/...InventoryItemDetailSheet*.rs):
- ThreadEmbed for maintenance notes.

P-D.7 — Top-level `/threads` route (crates/task-ui/src/feature_routes/threads.rs):
- Rebuild as a global inbox view of all un-resolved threads across all features. Group by entity_type, sort by recency. Click → navigate to the source surface with the thread auto-opened (use a deep-link query param `?thread=<id>` honored by each route).

Verify (each exits 0):
- `cargo check -p threads-proto -p threads-crdt -p threads-db -p threads -p threads-ui`
- `cargo check -p task-ui`
- `cargo check -p task-app-web --target wasm32-unknown-unknown`
- Manual: open the desktop app, comment on a knowledge block via selection, verify the BlockThreadMarker + TextRangeHighlight render. Document the manual smoke in the commit body.

Commit one commit on feat/threads-phase-d off feat/threads-phase-{a,b-embed,b-markers} merged. Reference plan Phase D in message. Show `git log --oneline -3`.

Constraints:
- EnterWorktree off whichever branch contains A+B; do not modify primary worktree.
- architect-ui primitives + theme tokens only across every wire-up.
- Re-use ThreadEmbed (do NOT fork per feature). The whole point of Phase D is one component, many mounts.
- Spawning Task from a thread: backlink BOTH ways — Comment.spawned_task_id + Task.source_thread_id.
- Deep-link `?thread=<id>` on every host route.
- Fix root causes; no --no-verify unless user authorizes.
- Stop after 40 turns (Phase D is the largest). If blocked, report blocker + last green check + smallest repro.

Each turn: state which P-D.N just satisfied, which is next, surface any UX divergence.
