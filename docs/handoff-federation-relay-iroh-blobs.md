# Handoff: the federation relay now moves bytes over iroh-blobs

**Status as of 2026-09-22:** done, all green, committed locally on
`test/large-media`, **not pushed**. This doc is for whoever picks this branch
up next — to push it, extend it, or just understand why the code looks the
way it does.

## Why this happened

The user asked "does this work for large file projects like Session and
Signal will need?" That sent me looking for a GB-scale test of the
federation relay path (`files.topology.federation` — one server reading a
file that actually lives on another server, offer/accept, `read` →
`redeem_bytes`). There wasn't one; the existing coverage
(`features/files/files/tests/federation_stream.rs`, `tests/integration/tests/it/scale.rs`) only proved the mechanics at a few megabytes.

## What was actually wrong

I wrote `tests/integration/tests/it/large_media.rs` to test at real scale and
found a **hard, deterministic hang**, not a slowdown: any relayed read past
roughly 47 MiB never completed. Traced with `vox_core=trace`:

- The old relay (`ByteSource::Relay` in `features/files/files/src/lane/media.rs`)
  called the origin's `fetch_offered` RPC once per 1 MiB
  (`ByteRange::MAX_LEN`), and **each call opened a brand-new vox
  connection** (`IrohRemotes::lane()` → `open_bi` + a fresh
  `architect::vox::initiator_on(...).establish()` handshake) instead of
  reusing anything.
- Every individual chunk request completed fast and cleanly (1–2ms). After
  ~50–100 of them in a burst, the wire went completely silent — no more
  vox traffic of any kind, forever. Not a vox-level limit (vox's own
  `max_concurrent_requests` correctly resets to 0 after every call, checked
  in the trace) — looked like QUIC-level stream/connection churn, but I
  didn't chase it further into `iroh`/`vox-core` internals once the fix
  path was clear.
- The user's own reaction, verbatim: *"rememebr, iroh-blobs already has an
  amazing way to send large files, make sure we arent reinventing the
  wheel"* — and they were right. `features/files/files-store/src/chunk/store.rs`
  already wraps `iroh_blobs::store::fs::FsStore` for **local** storage; the
  relay was reinventing chunked transfer badly on top of vox instead of
  using the library built for exactly this.

## The fix

**One authorization round trip, then the real bytes move over iroh-blobs —
not vox — using a second ALPN on the same endpoint.**

### The design decision (the user's call, not mine)

Real iroh-blobs GET/provider transfer has no per-chunk revocation hook. The
old relay re-checked the offer's secret **on every 1 MiB chunk**, so
withdrawing an offer stopped an in-flight transfer mid-file. I asked the
user how strict that needed to stay once bytes move over a real transport
instead of vox. They chose:

> **"Check once per file/session"** — the secret is validated once, when the
> relay authorizes; after that, iroh-blobs streams the whole file with no
> further checks. A revoke lands on the *next* relay, not one already
> authorized.

This is implemented and documented at the point it matters: the doc comment
on `FederationService::open_relay` in
`features/files/files-proto/src/service/federation.rs`, and again on
`FilesBackend::open_relay`'s impl in `features/files/files/src/lane/federation.rs`.
If a future requirement needs finer-grained revocation, **this is the one
place to revisit** — see "Known limitations" below.

### What changed, file by file

| File | What |
|---|---|
| `features/files/files-proto/src/service/federation.rs` | `ByteRange`/`fetch_offered` removed. New `RelayChunk { hash: String, len: u64 }` + `RelayManifest { chunks: Vec<RelayChunk> }`, and `FederationService::open_relay(secret, token) -> RelayManifest` replaces `fetch_offered`. |
| `features/files/files-store/src/chunk/store.rs` | New `ChunkStore::publish_for_relay(file_id, dest: &FsStore) -> Manifest` — publishes a file's chunks into another iroh-blobs store. Whole-tier files link in by reference (zero-copy); chunked files copy one bounded chunk at a time. Content already present in `dest` (by hash) is skipped — this is where "content addressing means a re-offer transfers nothing" becomes literally true. |
| `features/files/files/src/backend.rs` | `FilesBackend` gains `federation_blobs()` — a lazily-opened, per-org `iroh_blobs::store::fs::FsStore` at `<data_dir>/federation-blobs/` (a `tokio::sync::OnceCell`, **never opened unless something actually federates**). Also `read_relay_chunk()`, a `pub` "read a published chunk, ranged" seam used only by the in-process test harness (real callers go over the wire). |
| `features/files/files/src/lane/federation.rs` | `RemoteFiles` trait: `fetch_offered` → `open_relay` + `fetch_relay`. Origin-side `open_relay` impl: checks the secret once, resolves the token to `(RootId, FileId)`, publishes via `ChunkStore::publish_for_relay`, returns the manifest. Also: `trim_to_byte_range` (see below) and the `BLAKE3_CHUNK_LEN` constant. |
| `features/files/files/src/lane/media.rs` | `ByteSource::Relay { origin, manifest }` (no more `secret`/`token` — the manifest is resolved once at mint). `redeem_bytes`'s relay branch calls `RemoteFiles::fetch_relay` once instead of looping `fetch_offered`. `redeem_bytes<W>` now requires `W: Send` (needed to pass `dest` on as a trait-object writer). |
| `features/files/files/src/remotes.rs` | `IrohRemotes` gets a second connection pool (`blobs_pool`, ALPN is per-connection so this can't share the vox pool) and a lazily-opened on-disk `scratch` store. `open_relay` proxies the vox call unchanged. `fetch_relay` is the real thing: windows the requested range (4 MiB, bounded regardless of "chunk" size — a whole-tier file is *one* chunk spanning the whole object), fetches each window via `iroh_blobs::api::remote::Remote::execute_get` against a **pooled** blobs connection, reads the bytes back out of the scratch store, and trims BLAKE3's chunk-rounding before writing to `dest`. |
| `features/files/files/src/peer.rs` | `serve_over_iroh` takes an `Option<FilesBackend>` (renamed `relay`) and dispatches an incoming connection by `connection.alpn()`: vox goes to the existing router, `iroh_blobs::ALPN` goes to a `BlobsProtocol` built from `relay.federation_blobs()` — **opened lazily, on the first such connection**, not at boot. This lazy-open is load-bearing; see "The regression I introduced and fixed" below. |
| `apps/server/src/lib.rs` | `serve_org_iroh` passes `Some(org.files.clone())` instead of eagerly building a `BlobsProtocol`. |
| `features/files/files-sync/src/lib.rs` | Its `serve_over_iroh` call passes `None` — a device replica never serves a federation relay. |
| `apps/server/src/permits.rs` | `fetch_offered` → `open_relay` in the `FILES_FEDERATION` permit table (still `.audited()`, still on `public/files-offer`). |
| `features/files/files/tests/federation_stream.rs` | Rewritten. `open_relay` is still tested in-process (it's ordinary Rust, no network needed). The actual byte transfer's correctness is `tests/integration`'s claim now, since it needs a real iroh-blobs connection either way — `Direct::fetch_relay` reads straight out of the origin's published store via `read_relay_chunk` instead of simulating a wire fetch. `revocation_lands_mid_transfer` was rewritten as `a_withdrawn_offer_stops_the_next_relay_not_one_already_open`, matching the new, weaker (deliberately, per the user's choice) guarantee. |
| `tests/integration/tests/it/large_media.rs` | **New.** Opt-in (`#[ignore]`, `TASK_SCALE_BYTES` env, default 1 GiB). Proves the whole path at real scale: byte-exact via blake3, bounded memory via `/proc/self/status` peak RSS, a working seek, and a second read that's measurably faster (proof the receiver's on-disk cache works). |
| `.config/nextest.toml` | Two additions: an override giving `large_media::*` a long timeout budget, and — separately, see below — `test-threads = 16` on `[profile.default]`. |

### `trim_to_byte_range` — the bug that made partial-range fetches wrong

`iroh_blobs::protocol::ChunkRangesExt::bytes(range)` rounds a byte range **up**
to whole BLAKE3 chunks (1024 raw bytes each — BLAKE3's own spec constant,
not an iroh-blobs implementation detail) before fetching, because that's the
smallest unit its own Merkle tree can prove. I didn't know this going in;
`federation_stream.rs`'s seek/small-range tests caught it immediately (wrong
bytes, off by up to ~2047 either side). `trim_to_byte_range` in
`features/files/files/src/lane/federation.rs` undoes the rounding — used by
both the real `fetch_relay` (`remotes.rs`) and the test-only
`read_relay_chunk` (`backend.rs`), so there's one place this math lives.

### The regression I introduced, found, and fixed within the same session

First working version opened the origin's `federation_blobs()` store
**eagerly, for every org, at server boot** (to build the `BlobsProtocol`
handler before the first connection could arrive). Every
`iroh_blobs::store::fs::FsStore::load` spawns **its own dedicated
multi-thread tokio runtime** — sized to every CPU core by default. Running
the full test suite at full concurrency (32 cores, dozens of test binaries,
each with a couple of these stores) hit `EAGAIN` on `pthread_create` — "OS
can't spawn worker thread." That surfaced as a scary, unrelated-looking
cascade: `notify`'s `INotifyWatcher` panics in its own `Drop` impl when its
event-loop thread is already dead, which is a `panic!` during another
panic's unwind → `SIGABRT` → **the whole test binary process dies**, taking
down every other test that happened to share it. That's why the first full
run showed failures in things like `device`, `form`, `ingest` — collateral
damage, not real bugs in those areas.

Fixed two ways:
1. Made the open genuinely lazy — `peer.rs::serve_over_iroh` now opens
   `federation_blobs()` on the **first** incoming `iroh_blobs::ALPN`
   connection, not at boot, and never for an org that doesn't federate.
2. Added `test-threads = 16` to `[profile.default]` in `.config/nextest.toml`
   — this exact number already exists in `[profile.ci]` for the same
   underlying reason (every File Root's chunk store is *also* one of these
   dedicated-runtime stores, so this fragility was already latent, just
   not tipped over before). Confirmed empirically: 32 (uncapped) flakes, 16
   and 6 both ran the full `integration` suite clean.

## Verification performed

- `large_media.rs` at 1 GiB and 2 GiB: pass, bounded memory (~16 MiB peak
  growth regardless of file size), byte-exact, seek works, second read ~4x
  faster than the first (genuine on-disk caching, answering the original
  "does the receiver cache?" question: **yes, now**).
- `federation_stream.rs`: all 7 tests pass.
- `cargo nextest run -p files -p files-store -p files-sync -p integration -p task-server`: clean after the thread-cap fix (two flakes seen before the cap, both pre-existing documented-flaky categories unrelated to this change — a real-time inotify test and the `chunk_whole_file` disk-measurement group nextest.toml already flags as load-sensitive — confirmed by reproducing them in isolation).
- Full `just ci` (fmt, manifests, clippy `-D warnings`, wasm): clean.
- Full `cargo nextest run --profile ci --workspace`: **3160/3160 passed** (1
  flaky-then-passed on retry, the same pre-existing inotify test, exactly
  what `retries = 2` in `[profile.ci]` exists for).

## Current repo state

- Branch `test/large-media`, one commit (`caa2f14` at last check — verify
  with `git log -1`), **not pushed**.
- Working tree clean.
- Per the user's standing instruction, pushing/PR is their call — don't push
  without being asked. When asked: `just ci` already passed against this
  exact tree, so `[skip checks]` on the PR is legitimate *if* it's the very
  next commit and main hasn't moved (see CLAUDE.md's "The PR gate" section
  for the exact caveat).

## Known limitations / things a future agent should know before extending this

1. **No ADR was written.** This is a mechanism-level fix within ADR 0001's
   already-decided "iroh-blobs as the blob store" — extending it to also be
   the *transport*, not a new data-model decision. If this grows (e.g. the
   HashSeq/collection format, a real eviction policy) it may deserve one
   then.
2. **Revocation is coarser than before, by explicit user choice.** A
   withdrawn offer stops the *next* `read()`, not a `redeem_bytes` call
   already in flight or already authorized. If a product requirement shows
   up needing stricter mid-transfer revocation, the right lever is
   `open_relay`'s call frequency (e.g., re-authorize every N chunks instead
   of once) — the plumbing for a coarser-than-per-byte, finer-than-once
   recheck already exists in `fetch_relay`'s windowing loop, it just isn't
   wired to a re-check today.
3. **`federation_blobs` and the receiver's `scratch` store have no eviction
   policy.** They're plain `FsStore`s with no GC configured (deliberately —
   see the doc comments), so they grow forever as content is
   published/fetched. Fine for now (disk, not RAM, and content-addressed so
   duplicates cost nothing), but a long-running production server will
   eventually want either a size cap or periodic GC on these two stores
   specifically. Nothing sweeps them today.
4. **Why the QUIC-level hang happened is still not fully understood.**
   I found and fixed the *design* bug (one handshake per megabyte) but
   didn't chase down exactly which layer refused new streams after ~50-100
   of them in a burst — vox's own accounting was ruled out (traced,
   confirmed 0 after every call). If this class of bug resurfaces
   elsewhere (anything opening many short-lived vox/iroh connections in a
   tight loop), that's the next place to look, and `vox_core=trace`
   logging (see the git history of `large_media.rs` for how I temporarily
   wired `tracing-subscriber` into the test to get it — since removed, it
   was diagnostic-only) is how I'd start again.
5. **`ChunkStore::publish_for_relay` eagerly publishes the whole file at
   `open_relay` time**, not lazily per requested byte range. This is a
   real (small) regression against `read()`'s documented "nothing is
   downloaded up front" property — but for whole-tier files (the default
   for anything not explicitly small, and the case that actually motivated
   this work — big single audio/video takes) it's a zero-copy link, so the
   cost is negligible in the case that matters. Chunked files pay a real
   bounded-memory copy of their full size at mint time. If someone reports
   this as a real cost (e.g. very large *chunked* — not whole-tier —
   files), the fix is deferring the publish to be per-chunk, lazily, on
   the origin's serving side — which needs a way for the `BlobsProtocol`
   handler to resolve "which root's `ChunkStore` owns this hash" on demand;
   I considered and rejected this for this pass as too large a change for
   the value at hand (see the conversation's design discussion for the
   fuller reasoning).
6. **This was scoped to the federation relay specifically.** Device-to-device
   sync (`files-sync`) already has its own, different, working large-file
   mechanism (`ChunkStore::export_ranges`/`import_ranges`, bao-verified
   ranges shipped as vox RPC payloads, proven in `tests/integration/tests/it/scale.rs`)
   and was deliberately left alone — it never had this bug (no per-1MiB-chunk
   handshake loop) and didn't need this fix.

## If you're picking this up to push it

1. `git log -1 --stat` to see the full diff at a glance.
2. `just ci` one more time if any time has passed (main may have moved).
3. `gh pr create --fill && gh pr merge --auto --squash` per CLAUDE.md, or
   ask the user how they want it merged if anything about the branch has
   changed since this doc was written.
