Threads Phase C per plans/threads-feature-deepening.md — server attachments endpoint. Phases D, E out of scope.

Prereq: Phase A landed (09db5ac). Phase B can run in parallel — this goal is independent.

Done when ALL hold AND evidence in transcript:

P-C.1 — apps/server/src/attachments.rs (new module):
- `POST /api/attachments` (multipart upload). Body: file part + JSON metadata part {owner_id, owner_type, kind, title?}. Returns `{attachment_id: Uuid, blob_url: String}`.
- `GET /blobs/{id}` — streams the bytes back with Content-Type from the stored Attachment.mime; supports Range requests for media seeking.
- `DELETE /api/attachments/{id}` — removes both the blob file and the Attachment row.
- Inserts Attachment row via `AttachmentRepoLoro` so all clients see metadata immediately; bytes serve from the HTTP endpoint.

P-C.2 — apps/server/Cargo.toml:
- Add multipart support (axum has `axum::extract::Multipart` already; verify version) + tokio file IO.

P-C.3 — Storage:
- Blobs land at `<data_dir>/blobs/<uuid>` (data_dir is the existing server data_dir; same place as the SQLite snapshot).
- Filename safety: store under the Attachment UUID, NOT the user-supplied filename, to prevent traversal and collisions.
- Size cap: reject uploads larger than 50 MB (configurable via env `TASK_SERVER_ATTACHMENT_MAX_BYTES`, default 50_000_000).
- MIME allowlist at the API boundary: `audio/*`, `video/*`, `image/*`, `application/pdf`, `text/*`. Reject `application/octet-stream`, executables, anything else with 415.

P-C.4 — Wire routes onto the server's existing axum Router; mount under the same prefix as the other API routes (grep `Router::new` in apps/server/src/main.rs to find the integration point).

P-C.5 — Tests:
- Integration test (in apps/server/tests/ if that pattern exists, else inline): POST → 200 + attachment_id; GET /blobs/{id} → 200 + bytes match; oversize body → 413; bad mime → 415; missing metadata → 400. Use a tempdir as data_dir.

Verify (each exits 0):
- `cargo test -p task-server` (new integration tests pass)
- `cargo check -p task-server`
- `cargo check -p threads-proto -p threads-crdt -p threads-db -p threads`
- `curl -F file=@/tmp/test.txt -F 'metadata={"owner_id":"<uuid>","owner_type":"comment","kind":"file"};type=application/json' http://localhost:<port>/api/attachments` produces a 200 in the commit message body, plus `curl /blobs/<id>` echoing the file. Smoke run only — don't commit the test fixtures.

Commit one commit on feat/threads-phase-c off feat/threads-phase-a. Reference plan Phase C in message. Show `git log --oneline -3`.

Constraints:
- EnterWorktree off feat/threads-phase-a; do not modify primary worktree.
- No bytes-in-Loro path; the Attachment row carries metadata only (already true per Phase A schema). Reject `blob_loro_key` on the POST path with 400 for v1.
- Block path traversal aggressively: never use user-supplied filename for disk path.
- Stream files (don't read whole body into memory); cap at 50 MB.
- v1 limit explicitly accepted: any URL holder can fetch the blob (document in code). Signed URLs are v1.5.
- Fix root causes; no --no-verify unless user authorizes.
- Stop after 30 turns. If blocked, report blocker + last green check + smallest repro.

Each turn: state which P-C.N just satisfied, which is next, surface any divergence.
