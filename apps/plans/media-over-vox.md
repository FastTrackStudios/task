# Media over vox — retire the HTTP side-channels

**Status:** partially shipped — needs triage (2026-07-27). `features/task/media/media-proto` and `apps/task/server/src/media.rs` exist, but the HTTP side-channel this plan set out to retire is still live: `apps/task/server/src/main.rs` still mounts an always-on `/media/{*path}` route.

## Why

Song stems (and soon part bundles, sample previews, video) currently
reach clients through HTTP paths bolted on next to vox: the signed-URL
`/blobs/download` route (now with Range/206), and the interim `/media`
ServeDir. Every one of those is a workaround for "the browser's `<audio>`
element wants a URL". The architecture we actually want is the architect
one: **all data — including media bytes — travels the per-org vox lane**,
same origin, same auth, one protocol.

## What exists now (this PR)

- `media-proto` — `MediaService`: `stat(hash) -> MediaInfo`,
  `read(hash, start, len, Tx<MediaChunk>)`. Content-addressed, same hash
  namespace as attachments; ranged reads; server chunks at 256 KiB.
- `task-server` mounts it per org next to `AttachmentService`
  (`MediaServiceImpl` is a read-side view over the attachment blob
  store + catalog).
- `crates/task/ui::vox_clients::media_client(slug)`.
- e2e: upload through the attachment flow → stream back over vox
  (`tests/media_stream_e2e.rs`).
- `task media stat|get|verify-song` — the binary↔binary E2E surface:
  `verify-song` streams every stem on a song note over the real vox
  wire, sha256s the bytes against the frontmatter `content_hash`, and
  reports throughput. Audio-streaming smoke tests need no browser.

## Migration steps (follow-ups)

1. **SongView playback over vox** — DONE (same PR): per-stem
   `StemSource::Vox` creates a MediaSource-backed element fed by
   `media_client.read(...)` chunks (`pages/vox_media_source.rs`,
   progressive whole-file append v1); `task song ingest` now emits
   **webm/opus** (MSE-compatible; ogg-opus is not). Selection is
   per-stem at load: `MediaSource.isTypeSupported` + the blob's mime
   from `stat` — webm streams over vox, anything else falls back to
   the signed HTTP URL. Seek stays within the buffered range (fine
   once the progressive append catches up). REMAINING: drive a real
   Play with ears/Playwright (issue #30 item 6) and then tighten
   buffering (step 5).
2. **Setlist player** — same switch (it still uses `/media` today).
3. **Retire `/media`** — once songs are ingested as attachments and both
   players stream over vox, drop `TASK_SERVER_MEDIA_DIR` + the chart
   `serverPaths` `/media` entry.
4. **Uploads over vox** (optional, later) — `write(Tx<MediaChunk>)`-style
   ingest so `task song ingest` needs no HTTP PUT either; the signed-URL
   upload flow stays for third-party/browser drag-drop compatibility.
5. **Backpressure + prefetch tuning** — vox lane throughput vs 20-36
   concurrent stem streams; likely one `read` per stem with a small
   look-ahead window driven by the transport clock, not N full-file
   streams (this also replaces the browser's 6-connection HTTP cap
   concern from issue #30 item 6 — multiplexed over ONE WebSocket).

## Proxy media (originals + derived streams)

Today ingest ships ONLY the compressed proxy (webm/opus 96k) — which is
exactly what casual-practice streaming wants (23 stems ≈ 2.2 Mbps
total), but the full-quality master never enters the store. The DAW
"proxy media" pattern we should land:

1. **Original is canonical** — ingest uploads the master too
   (`--keep-original`, FLAC for lossless at ~half of WAV), content-
   addressed like everything else. Part bundles / performance
   downloads / re-transcodes derive from it.
2. **Proxies are derived, not first-class** — a derived-media record
   keyed by `(source_hash, profile)`; profile `practice` = webm/opus
   96k. Frontmatter keeps pointing at the stream the player should
   use; the derivation record links back to the master.
3. **Server-side lazy derivation** — MediaService grows a
   "hash X at profile Y" lookup; missing proxies transcode once
   server-side (ffmpeg in the image) and cache. New profiles (48k
   cellular tier, stereo practice-mix bus) become rows, not code.
   Folds into the server-side-mix work (issue #30 item 6).

## Non-goals

- Server-side mixing (issue #30 item 6) is orthogonal: when it lands it
  becomes ONE `read`-style stream of the mixed bus over the same
  MediaService shape.
