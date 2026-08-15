Threads Phase A per plans/threads-feature-deepening.md — schema + anchor module + CRDT/DB + seed. Phases B–E out of scope.

Adopt W3C Web Annotation selector names + Logseq status mapping. We are NOT cloning Logseq/Obsidian — conform to W3C where free, borrow Logseq shapes where they fit, our model is source of truth.

Done when ALL hold AND evidence in transcript:

P-A.1 — features/threads/threads-proto/src/anchor.rs (new):
- `Anchor` enum, serde tag="type": `Entity`, `TextQuoteSelector{exact,prefix,suffix,block_id}`, `TextPositionSelector{block_id,start_cursor_bytes:Vec<u8>,end_cursor_bytes:Vec<u8>}`, `FragmentSelector{asset_id,time_start_ms,time_end_ms}`, `RegionSelector{asset_id,page:Option<u32>,bounding:Rect,rects:Vec<Rect>,quote:Option<String>}`, `CanvasNodeSelector{block_id}`, `CellSelector{entity_type,entity_id,column,row_index}`. `Rect{x,y,w,h:f64}`.
- `resolve_text_quote(&Anchor,&str)->Option<(usize,usize)>`: exact then Bitap fuzzy (use `dissimilar` or hand-roll), else None. Doc: Loro Cursor resolves positions at call site, not here.
- Tests: serde round-trip every variant; resolve exact hit, fuzzy hit after edit, orphan None.

P-A.2 — Comment proto (features/threads/threads-proto/src/lib.rs):
- Add: `kind:String` (default "discussion"), `action_status/assignee/priority:Option<String>`, `action_due_date:Option<DateTime<Utc>>`, `spawned_task_id:Option<Uuid>`, `edited_at:Option<DateTime<Utc>>`, `deleted:bool`, `deleted_by:Option<String>`, `anchor_json:Option<String>`.
- Keep legacy `entity_id/entity_type/time_start_ms/time_end_ms`.
- Consts: `THREAD_KINDS=&["discussion","action","question","decision","praise"]`, `ACTION_STATUSES=&["open","in-progress","done","wont-do"]`.
- Doc-comment ACTION_STATUSES: Logseq map open→TODO, in-progress→DOING, done→DONE, wont-do→CANCELED.

P-A.3 — Attachment proto (same lib.rs):
- Add: `kind,mime:String`, `size_bytes:i64`, `duration_ms:Option<i64>`, `width/height:Option<i32>`, `blob_url/blob_loro_key/waveform_json/transcript/title:Option<String>`.
- Keep `owner_id/owner_type`.

P-A.4 — CRDT codec (features/threads/threads-crdt/src/lib.rs):
- Encode/decode every new field on Comment + Attachment.
- Tolerant decode: pre-extension snapshots load; missing fields → defaults (kind="discussion", deleted=false, rest None).
- Test: hand-built old-shape bytes decode with defaults.

P-A.5 — DB migration (features/threads/threads-db/src/lib.rs):
- Additive columns for every new field with safe defaults. No renames/drops.

P-A.6 — Seed (grep `seed` under features/threads or apps/server):
- Existing seeds default kind="discussion", anchor_json=None.
- Add ≥10 action-threads across kinds; ≥5 threads anchored via TextQuoteSelector to demo knowledge blocks.

Verify (each exits 0, show output):
- `cargo test -p threads-proto`
- `cargo test -p threads-crdt`
- `cargo check -p threads-proto -p threads-crdt -p threads-db -p threads`
- `cargo check -p task-ui`
- `cargo check -p task-app-web --target wasm32-unknown-unknown`

Commit: one commit on worktree branch, message references plan Phase A + W3C/Logseq notes. Show `git log --oneline -3`.

Constraints:
- Use EnterWorktree off feat/architect-migration; never touch user's primary worktree.
- W3C names are the public API — no parallel ad-hoc names.
- Loro Cursor is the position primitive. No byte-offset anchor fields; TextPositionSelector carries Cursor bytes; quote fallback in TextQuoteSelector.
- No `// TODO`/`// removed`/`// kept for compat` litter; `// FUTURE:` only for plan's listed v1 limitations.
- `Comment.body` stays String (LWW). Do NOT promote to LoroText.
- Fix root causes; no --no-verify.
- architect-ui primitives + theme tokens only in any UI touched (none expected; flag if it creeps).
- Stop after 30 turns; if blocked, report blocker + last green check + smallest repro.

Each turn: state which P-A.N just satisfied, which is next, surface any schema divergence from the plan.
