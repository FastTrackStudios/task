Threads Phase B-markers + media per plans/threads-feature-deepening.md — per-surface marker components + audio/video recorder + waveform helpers. Can ship in parallel with Phase B-embed (no overlap). Phases C, D, E out of scope.

Prereq: Phase A landed (09db5ac on feat/threads-phase-a). Branch off it.

Done when ALL hold AND evidence in transcript:

P-B-markers.1 — features/threads/threads-ui/src/markers/{block,range,media,region,canvas}.rs (new):
- `BlockThreadMarker` — gutter dot props: count: usize, first_body: Option<String>. On hover shows count + preview; on click emits open event.
- `TextRangeHighlight` — wraps a span with a faint underline. Props: range_label: String, on_click. Renders an inline `<span>` styled with theme token `text-primary/30` underline.
- `MediaRangeMarker` — bar on a timeline. Props: time_start_ms, time_end_ms, total_ms, on_click. Position computed as a CSS calc() in tokens.
- `RegionMarker` — overlaid rectangle. Props: rect: Rect, scale: f64, on_click.
- `CanvasNodeMarker` — small badge for a node id.
- Each ~50–80 LoC, dumb (no state), tested via a pure helper where possible (e.g. position math for MediaRangeMarker).

P-B-markers.2 — features/threads/threads-ui/src/media/waveform.rs (new, pure helpers):
- `peaks_from_f32(samples: &[f32], bucket_count: usize) -> Vec<u8>` — RMS or peak-abs bucketing into 0..255.
- `waveform_svg_path(peaks: &[u8], width: f64, height: f64) -> String` — emits an SVG path.
- Unit tests for both.

P-B-markers.3 — features/threads/threads-ui/src/media/recorder.rs (new):
- `AudioCommentRecorder` component using `web_sys::MediaRecorder`. Props: max_duration_secs (default 300), on_record<RecordedAudio>, on_cancel.
- `RecordedAudio { mime: String, bytes: Vec<u8>, duration_ms: u32, waveform: Vec<u8> }`.
- Detect MIME via `MediaRecorder::is_type_supported` — Chrome "audio/webm;codecs=opus", Safari "audio/mp4".
- Wasm-only behaviour gated by `#[cfg(target_arch = "wasm32")]`; native build provides a no-op stub so cargo check on native succeeds.
- `VideoCommentRecorder` analogue (camera + mic).

P-B-markers.4 — features/threads/threads-ui/src/media/player.rs (new):
- `AudioCommentPlayer` — waveform SVG + play/pause + click-to-seek; uses Html5 `<audio>` element.
- `VideoCommentPlayer` — `<video>` with overlay timeline rendering `MediaRangeMarker` for each comment anchored to the asset (consumes a `Vec<(Uuid, FragmentSelector)>` prop).

Verify (each exits 0):
- `cargo test -p threads-ui` (waveform helper tests pass)
- `cargo check -p threads-ui`
- `cargo check -p task-ui`
- `cargo check -p task-app-web --target wasm32-unknown-unknown` (recorder compiles for wasm)

Commit one commit on feat/threads-phase-b-markers off feat/threads-phase-a. Reference plan Phase B in message. Show `git log --oneline -3`.

Constraints:
- EnterWorktree off feat/threads-phase-a; do not modify primary worktree.
- architect-ui primitives + theme tokens only. Record-button color uses `bg-destructive`, NOT `bg-red-500`.
- web_sys access via existing wasm-bindgen conventions in this codebase (see knowledge-ui editor for patterns).
- MediaRecorder MIME detection at runtime — don't assume Opus.
- All marker components dumb — no repo calls.
- // FUTURE: only for video frame thumbnails, multi-track audio, per-frame screenshots (plan v1 limits).
- Fix root causes; no --no-verify unless user authorizes.
- Stop after 30 turns. If blocked, report blocker + last green check + smallest repro.

Each turn: state which P-B-markers.N just satisfied, which is next, surface any divergence.
