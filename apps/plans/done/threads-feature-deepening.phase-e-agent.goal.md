Threads Phase E per plans/threads-feature-deepening.md — agent integration (optional, can split further).

Prereq: Phases A–D landed. Whisper transcription needs Phase C (server-side blob handling) since ASR runs server-side over uploaded audio.

Done when ALL hold AND evidence in transcript:

P-E.1 — "Summarize this thread" via ChatModel:
- Hermes plugin (or in-app command, depending on where chat_model lives — grep `ChatModel` trait) that takes a thread (parent comment + reply tree) and emits a one-paragraph summary into a new comment with kind="discussion", author=<agent username>, mentions=[original author].
- Wire as an inline button "Summarize" on each thread in ThreadEmbed (props on_summarize<Uuid>).

P-E.2 — Whisper ASR for audio attachments:
- New trait `Transcriber` in a shared place (likely apps/server or a new crate) with `async fn transcribe(&self, bytes: &[u8], mime: &str) -> Result<String, TranscribeError>`.
- Two impls: `WhisperLocal` (via Hermes Whisper skill if available) and `OpenAIWhisper` (HTTPS). Pick at runtime via env var `TASK_TRANSCRIBER=local|openai`; default to "none" (skip).
- Server-side: when a new Attachment lands with kind="audio" or kind="video", fire a background task that calls Transcriber and populates `Attachment.transcript` via `AttachmentRepoLoro::update`.
- v1: best-effort; failures log and move on.

P-E.3 — "Suggest a reply" button on each thread:
- Inline button drafts a reply via the user's selected ChatModel. Pre-populates the composer text area; user reviews & sends.
- Use the existing chat_model infrastructure; do NOT introduce a new model abstraction.

Verify (each exits 0):
- `cargo test -p task-server` (Transcriber trait + a unit test with a no-op impl)
- `cargo check -p task-server -p task-ui`
- `cargo check -p task-app-web --target wasm32-unknown-unknown`
- Manual smoke documented in commit body: record a 5-sec audio comment → transcript appears within ~30s when TASK_TRANSCRIBER=openai.

Commit one or more commits on feat/threads-phase-e off feat/threads-phase-d. Reference plan Phase E. Show `git log --oneline -5`.

Constraints:
- EnterWorktree off feat/threads-phase-d; do not modify primary worktree.
- Don't add a new model abstraction — reuse ChatModel.
- Transcriber is a trait so we can swap providers without changing call sites.
- ASR failures are non-fatal — transcript stays None, log a warning.
- v1 limit: no privacy-by-default for transcripts. They're stored as plain text on the Attachment row. Document; v1.5 adds opt-in.
- Fix root causes; no --no-verify unless user authorizes.
- Stop after 30 turns. If blocked, report blocker + last green check + smallest repro.

Each turn: state which P-E.N just satisfied, which is next, surface any divergence.
