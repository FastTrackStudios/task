# Browser audio: shared-memory worklet (the waw-rs pattern)

Status: PLANNED — groundwork landed (see "Landed now"), migration not started.
Owner context: PR #37 follow-up; the target architecture for running signal
guitar rigs (live input, realtime FX) entirely in the browser.

## Where we are (post-4b-2 + this branch's fixes)

The browser setlist plays through daw-standalone's `WebRenderer` hosted
inside an AudioWorklet (`apps/task/web/assets/worklet/processor.js` + a
dedicated ~650 KB release wasm bundle, `just task-worklet-wasm`). The main
thread decodes stems (`decodeAudioData`) and POSTS the PCM to the worklet
over the node's `MessagePort` (transferred buffers); transport commands and
the full mixer state travel the same port; position/peaks mirror back on a
~21 ms tick.

Landed now (this branch):

- Output connects straight to `ctx.destination()` — the media-element sink
  (`MediaStreamAudioDestinationNode → <audio srcObject>`) did live-stream
  clock sync and played FAST to catch up whenever the main thread was
  saturated (the "warped ~10 s on page load" bug). The media path remains
  available where WebAudio device streams are broken (THEBATTLESHIP's
  pipewire-pulse cycling): `localStorage.setItem("fts-audio-sink","media")`
  or `?audio=media`.
- Mixer state (mute/solo/fader) is pushed into the renderer via a `mix`
  port message → `Tracks::{set_muted,set_soloed,set_volume}` on the
  in-worklet `Standalone` — solo routing + gains are the graph's own logic,
  identical to native.

What's still wrong with the postMessage architecture:

- Two wasm instances (app bundle + worklet bundle) — every stem's PCM is
  copied across realms; per-stem `Float32Array` copies burn main-thread
  time during load.
- Control latency = port round-trip; meters/position arrive at tick rate,
  not read-on-demand.
- No live INPUT path: a guitar-rig graph needs main-thread (or
  capture-thread) parameter writes to land sample-accurately without a
  message queue.
- Worklet-scope polyfills (TextDecoder/setTimeout/performance) exist only
  because the glue runs in a bare `AudioWorkletGlobalScope`.

## Target: one shared-memory wasm across main + audio thread

The [waw-rs](https://github.com/Marcel-G/waw-rs) pattern (also
wasm-bindgen's own `threads` support / `web-thread`): build ONE module with
threaded wasm, instantiate it on the main thread AND inside the worklet
against the SAME shared `WebAssembly.Memory` (SharedArrayBuffer-backed).
Rust objects (the `Standalone`, `TransportShared`, meter cells — already
atomics-based) become directly visible from both threads:

- main thread calls `WebRenderer` methods directly (no port protocol);
- the worklet's `process()` only calls `render()`;
- meters/position are lock-free atomic reads (drop the 21 ms tick);
- decoded PCM is written once into shared memory (no transfer copies);
- live input: a capture worklet can write into a shared ring the graph
  reads — the guitar-rig path.

Decision: adopt the PATTERN, not the crate. waw-rs assumes it owns the
whole bundle + wasm-pack build; our worklet bundle wraps `WebRenderer` and
the main-thread side is the (separate) dioxus app wasm, which stays
postMessage-free by calling the worklet bundle's JS glue directly via
`js_sys` (both glues live in the page realm; only the audio-thread
instantiation crosses realms).

## Migration steps

1. **Toolchain**: threaded wasm needs `-C target-feature=+atomics,+bulk-memory,+mutable-globals`
   and a rebuilt std (`cargo -Zbuild-std=std,panic_abort` → nightly with
   `rust-src`). The flake already pins a nightly for phon-jit
   (`PHON_JIT_NIGHTLY_RUSTC`) — expose it (plus `rust-src`) for the worklet
   build and teach `just task-worklet-wasm` the flags:

   ```
   RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+mutable-globals' \
   cargo +<nightly> build -p daw-standalone --lib --release \
     --target wasm32-unknown-unknown -Zbuild-std=std,panic_abort \
     --no-default-features --features decode,web
   wasm-bindgen --target web ...   # glue grows a `memory` init option
   ```

2. **Cross-origin isolation** (SharedArrayBuffer gate): serve everything
   with `Cross-Origin-Opener-Policy: same-origin` +
   `Cross-Origin-Embedder-Policy: require-corp`.
   - task-server (axum): one `SetResponseHeaderLayer` pair on the app
     router. Check embedded assets + `/media` responses carry CORP.
   - `dx serve` (dev): no header config — add them in the app's service
     worker (it already fronts `/assets/`) or a tiny local proxy; decide
     during implementation. THIS is the main dev-workflow risk.
   - vox WebSocket + same-origin fetches are unaffected by COEP.

3. **Bootstrap**: main thread compiles the module + creates the shared
   `WebAssembly.Memory` (initial/maximum tuned to worst-case setlist PCM;
   grows are visible to all instantiations), calls `initSync({module,
   memory})` in the page realm, constructs the `WebRenderer`, then posts
   {module bytes or Module, memory} to the worklet which `initSync`s the
   same pair and wraps the SAME renderer pointer (`WebRenderer.__wrap(ptr)`
   — pass the ptr in the init message). Polyfills stay until the glue stops
   touching TextDecoder in the worklet realm (thread-init paths mostly
   don't).

4. **Drop the port protocol**: transport/mix/attach become direct main-
   thread calls; `process()` keeps only render. Keep ONE port message
   (init). Delete the tick — position/peaks read via main-thread calls
   into shared state each rAF/45 ms poll.

5. **Realtime hygiene**: `render()` currently locks project state
   (`read_project`) — audit for try_lock + last-snapshot fallback so a
   main-thread write never blocks the audio thread (with shared memory
   these become REAL contended locks, unlike today's single-thread-per-
   instance world).

6. **Live input (guitar-rig milestone)**: `AudioWorkletNode` with 1 input,
   the processor feeds input frames into a shared ring; signal FX graph
   (`features/fx/*-dsp`, already no_std/wasm-clean) renders in-worklet.
   COOP/COEP + `getUserMedia` require HTTPS in production.

## Risks / open questions

- Chrome refuses structured-cloned `WebAssembly.Module` into
  AudioWorkletGlobalScope (we hit this in 4b-2) — shared-memory examples
  pass module BYTES + shared Memory and `initSync` in the worklet; verify
  the Memory object itself clones into the worklet scope (waw-rs does
  exactly this, so expected-yes).
- wasm-bindgen glue thread-init in a worklet scope: needs
  `thread_stack_size` per instantiation; wasm-bindgen-rayon documents the
  worklet caveats.
- Safari: SharedArrayBuffer OK (COOP/COEP), but audio-worklet + threads
  support needs a real device pass.
- Bundle stays committed under `apps/task/web/assets/worklet/` — nightly
  build-std makes CI reproducibility worth a pinned-toolchain check.
