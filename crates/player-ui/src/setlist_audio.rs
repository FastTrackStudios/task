//! Stage 4b-2 — **audio** for the engine-fed browser setlist.
//!
//! `daw-standalone`'s [`WebRenderer`] runs **inside an AudioWorklet** — the
//! browser twin of the native cpal callback. The audio thread owns the render
//! graph and is called every 128-frame quantum; the main thread only:
//!
//!  1. compiles the dedicated worklet wasm bundle once
//!     (`assets/worklet/daw_standalone_bg.wasm`, built by
//!     `just task-worklet-wasm` — ~650 KB, release-opt, NOT the app bundle),
//!  2. seeds the song's tracks + fetches/decodes stem PCM
//!     (`decodeAudioData` only exists on the main thread) and streams it over
//!     the node's `MessagePort`, and
//!  3. relays transport commands (play / pause / seek).
//!
//! The worklet posts a ~21 ms state tick (position / play-state / per-track
//! peaks) that this side mirrors for the UI (VU meters, drift-kill). There is
//! NO main-thread rendering and NO jitter buffer — realtime at worklet
//! quantum size, the same path an in-browser signal guitar rig needs.
//!
//! One [`SetlistAudio`] per ACTIVE song (rebuilt on song switch — one
//! project's graph renders at a time), all on ONE app-wide shared
//! `AudioContext` (Chrome hard-caps live contexts per tab; churning them
//! mutes everything permanently).

// Browser-only. The module still has to compile on native (pages/mod.rs
// declares it unconditionally), so gate the whole implementation on wasm32.
#[cfg(target_arch = "wasm32")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        AudioBuffer, AudioContext, AudioWorkletNode, AudioWorkletNodeOptions, MessageEvent,
        MessagePort, Response,
    };

    use crate::song_session::imp::Manifest;

    /// Stable take-guid for stem `j` of song `i` — the async decode attaches
    /// PCM by the same key the seed used. Unique across the whole setlist
    /// (one renderer hosts every song).
    fn stem_take_guid(song: usize, j: usize) -> String {
        format!("setlist-{song:02}-take-{j:02}")
    }

    /// One setlist song inside the shared renderer: its project guid (mirrors
    /// the engine's `setlist-XX-slug` layout) + its stems
    /// `(display_name, take_guid, url)`.
    struct SongStems {
        project: String,
        name: String,
        stems: Vec<(String, String, String)>,
    }

    // The worklet bundle, embedded at COMPILE time (rebuilt by
    // `just task-worklet-wasm`; cargo tracks the files). Embedding — instead
    // of fetching `/assets/worklet/...` at runtime — sidesteps two dev-server
    // traps that both abort `addModule`: dx serves static assets with no
    // `Content-Type` (module loads hard-require a JS MIME), and the app's
    // service worker (scoped to `/assets/`) answers asset fetches with the
    // HTML shell. A Blob URL carries its own MIME and needs no server.
    const WORKLET_GLUE: &str = include_str!("../../../apps/web/assets/worklet/daw_standalone.js");
    const WORKLET_PROC: &str = include_str!("../../../apps/web/assets/worklet/processor.js");
    const WORKLET_WASM: &[u8] =
        include_bytes!("../../../apps/web/assets/worklet/daw_standalone_bg.wasm");

    thread_local! {
        /// ONE `AudioContext` for the whole app, created lazily and NEVER
        /// closed (see module docs).
        static SHARED_CTX: RefCell<Option<AudioContext>> = const { RefCell::new(None) };
        /// Whether `processor.js` has been registered on the shared context.
        static WORKLET_REGISTERED: Cell<bool> = const { Cell::new(false) };
        /// Whether the shared context's output sink has been cycled (see
        /// [`cycle_sink`]) — once per context.
        static SINK_CYCLED: Cell<bool> = const { Cell::new(false) };
    }

    /// Force the context to open a FRESH physical output stream by cycling
    /// `setSinkId` → `{type:"none"}` → `""` (default). Chrome opens a
    /// context's device stream ONCE and never retries it — if that open
    /// happened while the OS audio path was briefly broken (this rig's
    /// pipewire-pulse restarts every few minutes, stranding the browser's
    /// audio service), the context is stuck on a silent FAKE stream forever:
    /// graph runs, meters move, zero sound, no PipeWire node. New stream
    /// opens recover (that's why other tabs play) — so force one. Same-ID
    /// `setSinkId` calls are spec'd no-ops, hence the none→default cycle.
    /// Done via `Reflect` (`setSinkId` is Chrome 110+; web-sys coverage
    /// varies).
    async fn cycle_sink(ctx: &AudioContext) {
        let f = match js_sys::Reflect::get(ctx.as_ref(), &JsValue::from_str("setSinkId")) {
            Ok(f) if f.is_function() => js_sys::Function::from(f),
            _ => return, // pre-110 browser: nothing to do
        };
        let none = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &none,
            &JsValue::from_str("type"),
            &JsValue::from_str("none"),
        );
        for arg in [JsValue::from(none), JsValue::from_str("").into()] {
            match f.call1(ctx.as_ref(), &arg) {
                Ok(p) => {
                    if let Ok(p) = p.dyn_into::<js_sys::Promise>() {
                        let _ = JsFuture::from(p).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("setlist audio: setSinkId failed: {e:?}");
                    return;
                }
            }
        }
        tracing::info!("setlist audio: output sink cycled (fresh device stream)");
    }

    /// The app-wide shared [`AudioContext`], created on first use.
    fn shared_ctx() -> Result<AudioContext, String> {
        SHARED_CTX.with(|c| {
            let mut slot = c.borrow_mut();
            if let Some(ctx) = slot.as_ref() {
                return Ok(ctx.clone());
            }
            // `latencyHint: "playback"`: Chrome's default WebAudio stream is
            // the low-latency "interactive" category, and when the device
            // open for it fails (this rig's PipeWire runs a large fixed
            // quantum for the Dante/console chain), Chrome silently hands the
            // context a FAKE output stream — the graph runs, meters move,
            // zero sound, no PipeWire node. The "playback" category opens the
            // same higher-latency stream media elements use (which audibly
            // works here). Latency is fine for a setlist player; the future
            // low-latency rig path needs the rig's quantum unlocked instead.
            let opts = web_sys::AudioContextOptions::new();
            opts.set_latency_hint(&JsValue::from_str("playback"));
            let ctx = AudioContext::new_with_context_options(&opts)
                .map_err(|e| format!("AudioContext: {e:?}"))?;
            *slot = Some(ctx.clone());
            Ok(ctx)
        })
    }

    /// Register the worklet module on the context (once) and compile the
    /// worklet wasm bundle (once).
    ///
    /// The module is assembled as a self-contained **Blob** module: dx's dev
    /// server serves static assets with NO `Content-Type`, and module loading
    /// hard-fails without a JS MIME type ("Unable to load a worklet's
    /// module") — a Blob URL carries its own type, on any server. We fetch
    /// `processor.js` + the wasm-bindgen glue as TEXT, strip the module
    /// syntax (the glue's `export`s; processor's `import` line — polyfills
    /// stay ABOVE the glue so its guarded `TextDecoder` consts see them), and
    /// register the concatenation.
    async fn ensure_worklet_assets(ctx: &AudioContext) -> Result<(), String> {
        if !WORKLET_REGISTERED.with(Cell::get) {
            let proc = WORKLET_PROC;

            // Glue: `export class X {` → `class X {`; drop the final
            // `export { initSync, __wbg_init as default };` line.
            let glue: String = WORKLET_GLUE
                .lines()
                .filter(|l| !l.trim_start().starts_with("export {"))
                .map(|l| l.replacen("export class ", "class ", 1))
                .map(|l| l.replacen("export function ", "function ", 1))
                .map(|l| l.replacen("export const ", "const ", 1))
                .collect::<Vec<_>>()
                .join("\n");
            // Processor: split at its `import` line — polyfills above, the
            // processor class below; glue goes in between.
            let (pre, post) = match proc
                .lines()
                .position(|l| l.trim_start().starts_with("import "))
            {
                Some(idx) => {
                    let lines: Vec<&str> = proc.lines().collect();
                    (lines[..idx].join("\n"), lines[idx + 1..].join("\n"))
                }
                None => (String::new(), proc.to_string()),
            };
            let src = format!("{pre}\n{glue}\n{post}");

            let parts = js_sys::Array::of1(&JsValue::from_str(&src));
            let opts = web_sys::BlobPropertyBag::new();
            opts.set_type("application/javascript");
            let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts)
                .map_err(|e| format!("worklet blob: {e:?}"))?;
            let url = web_sys::Url::create_object_url_with_blob(&blob)
                .map_err(|e| format!("worklet blob url: {e:?}"))?;
            let promise = ctx
                .audio_worklet()
                .map_err(|e| format!("audio_worklet(): {e:?}"))?
                .add_module(&url)
                .map_err(|e| format!("add_module: {e:?}"))?;
            JsFuture::from(promise)
                .await
                .map_err(|e| format!("add_module await: {e:?}"))?;
            let _ = web_sys::Url::revoke_object_url(&url);
            WORKLET_REGISTERED.with(|r| r.set(true));
        }

        Ok(())
    }

    /// Whether the media-element output pipeline is FORCED (see the connect
    /// site in [`attach`]): `?audio=media` in the URL, or the persistent
    /// `localStorage["fts-audio-sink"] = "media"` rig flag.
    fn media_sink_forced() -> bool {
        let Some(win) = web_sys::window() else {
            return false;
        };
        if win
            .location()
            .search()
            .map(|s| s.contains("audio=media"))
            .unwrap_or(false)
        {
            return true;
        }
        matches!(
            win.local_storage()
                .ok()
                .flatten()
                .and_then(|s| s.get_item("fts-audio-sink").ok().flatten())
                .as_deref(),
            Some("media")
        )
    }

    /// Build a `{kind: ...}` message object.
    fn msg(kind: &str) -> js_sys::Object {
        let o = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str("kind"), &JsValue::from_str(kind));
        o
    }
    fn set(o: &js_sys::Object, k: &str, v: &JsValue) {
        let _ = js_sys::Reflect::set(o, &JsValue::from_str(k), v);
    }

    /// The live worklet node + the tick-handler closure (kept alive together).
    struct Pump {
        node: AudioWorkletNode,
        port: MessagePort,
        /// The `<audio>` element playing the rendered MediaStream — only when
        /// the media-path output sink is FORCED (see the connect site); the
        /// default path connects straight to `ctx.destination()`.
        sink: Option<web_sys::HtmlAudioElement>,
        _onmsg: Closure<dyn FnMut(MessageEvent)>,
    }

    /// Full mixer state, track order == stem order: (muted, soloed, volumes).
    type MixState = (Vec<bool>, Vec<bool>, Vec<f32>);

    /// The WHOLE setlist's audio — ONE worklet renderer hosting every song as
    /// a separate project (the native engine's exact layout). A song switch
    /// is a `select_project` message + a PCM swap (decode the incoming song,
    /// detach the outgoing song's sources — a full set decoded at once is
    /// multi-GB), never a graph teardown. Dropping it stops the worklet
    /// transport and disconnects the node (the shared context stays alive).
    pub(crate) struct SetlistAudio {
        ctx: AudioContext,
        pump: Rc<RefCell<Option<Pump>>>,
        /// Per-song projects + stems, in setlist order.
        songs: Rc<Vec<SongStems>>,
        /// Index of the SELECTED song (`usize::MAX` until the first select).
        current: Rc<Cell<usize>>,
        /// Decode epoch — bumped on every song switch so in-flight decodes
        /// for the outgoing song abort instead of attaching stale PCM.
        epoch: Rc<Cell<u64>>,
        /// Song selected while the pump was still attaching.
        pending_song: Rc<Cell<Option<usize>>>,
        /// Transport intent while the worklet is still attaching (the async
        /// module setup takes ~a frame; a Play click can beat it).
        want_play: Rc<Cell<bool>>,
        pending_seek: Rc<Cell<Option<f64>>>,
        /// Latest mixer state queued while attaching — flushed on attach so
        /// default-muted stems (click / guide) are silent from the first block.
        pending_mix: Rc<RefCell<Option<MixState>>>,
        /// AudioContext time until which engine-driven [`nudge`]s are
        /// suppressed. Every explicit seek arms it — otherwise the bridge's
        /// next (still-stale) engine tick would seek the worklet straight
        /// BACK to the old position and cancel the user's jump.
        nudge_guard: Rc<Cell<f64>>,
        // ── mirrors of the worklet's ~21 ms state tick ──
        pos: Rc<Cell<f64>>,
        playing: Rc<Cell<bool>>,
        peaks: Rc<RefCell<Vec<f32>>>,
    }

    impl SetlistAudio {
        /// Wire the SETLIST's audio: (async) register + compile the worklet
        /// assets, create the node, seed every song as its own project with a
        /// track per stem. PCM decoding is per-song, driven by
        /// [`select_song`](Self::select_song).
        pub(crate) fn build(
            org: &str,
            songs_in: &[(String, Manifest)],
        ) -> Result<SetlistAudio, String> {
            let ctx = shared_ctx()?;
            let songs: Vec<SongStems> = songs_in
                .iter()
                .enumerate()
                .map(|(i, (slug, manifest))| SongStems {
                    project: format!("setlist-{i:02}-{slug}"),
                    name: manifest.title.clone().unwrap_or_else(|| slug.clone()),
                    stems: manifest
                        .stems
                        .iter()
                        .enumerate()
                        .map(|(j, s)| {
                            (
                                s.name.clone(),
                                stem_take_guid(i, j),
                                format!(
                                    "/org/{org}/media/songs/{slug}/{}{}",
                                    s.file,
                                    crate::media_grant::cached_suffix(org, slug)
                                ),
                            )
                        })
                        .collect(),
                })
                .collect();
            let audio = SetlistAudio {
                ctx: ctx.clone(),
                pump: Rc::new(RefCell::new(None)),
                songs: Rc::new(songs),
                current: Rc::new(Cell::new(usize::MAX)),
                epoch: Rc::new(Cell::new(0)),
                pending_song: Rc::new(Cell::new(None)),
                want_play: Rc::new(Cell::new(false)),
                pending_seek: Rc::new(Cell::new(None)),
                pending_mix: Rc::new(RefCell::new(None)),
                nudge_guard: Rc::new(Cell::new(0.0)),
                pos: Rc::new(Cell::new(0.0)),
                playing: Rc::new(Cell::new(false)),
                peaks: Rc::new(RefCell::new(Vec::new())),
            };

            {
                let ctx = ctx.clone();
                let pump = audio.pump.clone();
                let songs = audio.songs.clone();
                let current = audio.current.clone();
                let epoch = audio.epoch.clone();
                let pending_song = audio.pending_song.clone();
                let want_play = audio.want_play.clone();
                let pending_seek = audio.pending_seek.clone();
                let pending_mix = audio.pending_mix.clone();
                let pos = audio.pos.clone();
                let playing = audio.playing.clone();
                let peaks = audio.peaks.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(e) = attach(
                        &ctx,
                        AttachState {
                            songs,
                            current,
                            epoch,
                            pending_song,
                            pump,
                            want_play,
                            pending_seek,
                            pending_mix,
                        },
                        pos,
                        playing,
                        peaks,
                    )
                    .await
                    {
                        tracing::warn!("setlist audio: worklet attach failed: {e}");
                    }
                });
            }

            tracing::info!(
                "setlist audio: built (ONE renderer, {} song projects, v4b4-set2)",
                audio.songs.len()
            );
            Ok(audio)
        }

        /// Switch the renderer to song `idx`: `select_project` + swap the
        /// decoded PCM (detach the outgoing song's sources, decode the
        /// incoming song's stems). The graph, tracks, and transport states
        /// persist — this is the browser twin of the native project switch.
        pub(crate) fn select_song(&self, idx: usize) {
            if idx >= self.songs.len() || self.current.get() == idx {
                return;
            }
            let prev = self.current.replace(idx);
            let e = self.epoch.get() + 1;
            self.epoch.set(e);
            match self.pump.borrow().as_ref() {
                Some(p) => kick_song(
                    &self.ctx,
                    &p.port,
                    &self.songs,
                    idx,
                    (prev != usize::MAX).then_some(prev),
                    e,
                    &self.epoch,
                ),
                None => self.pending_song.set(Some(idx)),
            }
        }

        /// Start playback. Resumes the context (the caller's Play click is the
        /// required user gesture) and rolls the worklet transport.
        pub(crate) fn play(&self) {
            if let Ok(promise) = self.ctx.resume() {
                let ctx = self.ctx.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(e) = JsFuture::from(promise).await {
                        tracing::warn!("setlist audio: ctx.resume() failed: {e:?}");
                    }
                    // Once per context (on the Play gesture, after resume):
                    // force a fresh physical output stream in case the
                    // original open landed on a fake sink (see cycle_sink).
                    if !SINK_CYCLED.with(Cell::get) {
                        SINK_CYCLED.with(|c| c.set(true));
                        cycle_sink(&ctx).await;
                    }
                });
            }
            self.want_play.set(true);
            if let Some(p) = self.pump.borrow().as_ref() {
                let _ = p.port.post_message(&msg("play"));
                // The Play click is the autoplay gesture for the media-path
                // sink element too (only present when that path is forced).
                if let Some(sink) = &p.sink {
                    let _ = sink.play();
                }
            }
        }

        /// Pause playback (context stays alive so Play resumes instantly).
        pub(crate) fn pause(&self) {
            self.want_play.set(false);
            if let Some(p) = self.pump.borrow().as_ref() {
                let _ = p.port.post_message(&msg("pause"));
            }
        }

        /// Seek the worklet transport to `seconds` (playing or paused).
        pub(crate) fn seek(&self, seconds: f64) {
            self.nudge_guard.set(self.ctx.current_time() + 1.5);
            if let Some(p) = self.pump.borrow().as_ref() {
                let m = msg("seek");
                set(&m, "seconds", &JsValue::from_f64(seconds));
                let _ = p.port.post_message(&m);
            } else {
                self.pending_seek.set(Some(seconds));
            }
        }

        /// QUANTIZED seek: the worklet jumps to `target` (song seconds) when
        /// its transport reaches `at` (song seconds) — executed on the audio
        /// thread, on the boundary block. `delay` is the wall-clock estimate
        /// until the boundary (arms the nudge guard past the jump so the
        /// engine's catch-up ticks can't cancel it).
        pub(crate) fn seek_quantized(&self, target: f64, at: f64, delay: f64) {
            self.nudge_guard
                .set(self.ctx.current_time() + delay.max(0.0) + 1.5);
            if let Some(p) = self.pump.borrow().as_ref() {
                let m = msg("seek_q");
                set(&m, "seconds", &JsValue::from_f64(target));
                set(&m, "at", &JsValue::from_f64(at));
                let _ = p.port.post_message(&m);
            } else {
                self.pending_seek.set(Some(target));
            }
        }

        /// Engine-driven drift-kill. Re-seeks the worklet to the engine's
        /// position ONLY when it has genuinely diverged (> 1 s vs the
        /// worklet's own mirrored position) and no explicit / quantized seek
        /// is in flight — the previous unconditional ">0.5 s ⇒ seek" fought
        /// user seeks with stale engine ticks (a bar click seeked, the next
        /// old-position tick seeked it straight back).
        pub(crate) fn nudge(&self, secs: f64) {
            if self.ctx.current_time() < self.nudge_guard.get() {
                return;
            }
            if (secs - self.pos.get()).abs() <= 1.0 {
                return;
            }
            self.seek(secs);
        }

        /// The render transport's position (audio truth, mirrored from the
        /// worklet tick) — for the drift-kill refinement.
        #[allow(dead_code)]
        pub(crate) fn position(&self) -> f64 {
            self.pos.get()
        }

        /// Whether the render transport is rolling (mirrored).
        pub(crate) fn is_playing(&self) -> bool {
            self.playing.get()
        }

        /// Per-stem peak levels (track order == stem order, mirrored from the
        /// worklet tick) — feeds the mixer VU meters.
        pub(crate) fn peaks(&self) -> Vec<f32> {
            self.peaks.borrow().clone()
        }

        /// Push the FULL mixer state (track order == stem order) into the
        /// worklet renderer — mute / solo / fader are audible, not display
        /// state. Idempotent; queued while the pump is still attaching (the
        /// attach flushes the LATEST state), so default-muted stems (click /
        /// guide) are silent from the first rendered block.
        pub(crate) fn apply_mix(&self, muted: Vec<bool>, soloed: Vec<bool>, volumes: Vec<f32>) {
            tracing::debug!(
                "setlist audio: mix push ({} stems, muted={muted:?}, soloed={soloed:?})",
                muted.len()
            );
            match self.pump.borrow().as_ref() {
                Some(p) => post_mix(&p.port, &muted, &soloed, &volumes),
                None => *self.pending_mix.borrow_mut() = Some((muted, soloed, volumes)),
            }
        }
    }

    /// Post one `mix` message carrying the full mixer state.
    fn post_mix(port: &MessagePort, muted: &[bool], soloed: &[bool], volumes: &[f32]) {
        let m = msg("mix");
        let jm = js_sys::Array::new();
        for &b in muted {
            jm.push(&JsValue::from_bool(b));
        }
        let js = js_sys::Array::new();
        for &b in soloed {
            js.push(&JsValue::from_bool(b));
        }
        set(&m, "muted", &jm);
        set(&m, "soloed", &js);
        set(&m, "volumes", &js_sys::Float32Array::from(volumes));
        let _ = port.post_message(&m);
    }

    impl Drop for SetlistAudio {
        fn drop(&mut self) {
            // Stop the worklet transport and unhook the node. The shared
            // AudioContext is NOT closed — it's app-wide.
            if let Some(p) = self.pump.borrow_mut().take() {
                let _ = p.port.post_message(&msg("stop"));
                p.port.set_onmessage(None);
                let _ = p.node.disconnect();
                if let Some(sink) = &p.sink {
                    let _ = sink.pause();
                    sink.set_src_object(None);
                }
            }
        }
    }

    /// Register assets, create the worklet node, init the in-worklet renderer,
    /// seed the stems, flush any queued transport intent, and start the
    /// decodes. Port messages are ordered, so `init` → `add_project`* /
    /// `add_stem`* → `select_project` / `attach`* / transport all land
    /// against a live renderer.
    ///
    /// The shared handles the attach needs (they outlive this async fn).
    struct AttachState {
        songs: Rc<Vec<SongStems>>,
        current: Rc<Cell<usize>>,
        epoch: Rc<Cell<u64>>,
        pending_song: Rc<Cell<Option<usize>>>,
        pump: Rc<RefCell<Option<Pump>>>,
        want_play: Rc<Cell<bool>>,
        pending_seek: Rc<Cell<Option<f64>>>,
        pending_mix: Rc<RefCell<Option<MixState>>>,
    }

    async fn attach(
        ctx: &AudioContext,
        st: AttachState,
        pos: Rc<Cell<f64>>,
        playing: Rc<Cell<bool>>,
        peaks: Rc<RefCell<Vec<f32>>>,
    ) -> Result<(), String> {
        ensure_worklet_assets(ctx).await?;

        let opts = AudioWorkletNodeOptions::new();
        opts.set_number_of_inputs(0);
        opts.set_output_channel_count(&js_sys::Array::of1(&JsValue::from(2u32)));
        let node = AudioWorkletNode::new_with_options(ctx, "fts-daw-processor", &opts)
            .map_err(|e| format!("AudioWorkletNode: {e:?}"))?;
        // A processor that throws during construction or process() surfaces
        // ONLY through this event — without it the node just goes silent.
        let onprocerr = Closure::wrap(Box::new(move |e: web_sys::Event| {
            tracing::warn!("setlist audio: worklet PROCESSOR ERROR: {:?}", e.type_());
        }) as Box<dyn FnMut(web_sys::Event)>);
        node.set_onprocessorerror(Some(onprocerr.as_ref().unchecked_ref()));
        onprocerr.forget(); // node-lifetime handler, tiny leak per song build
        let port = node.port().map_err(|e| format!("worklet port: {e:?}"))?;

        // Tick handler: mirror the worklet's position / play-state / peaks.
        let onmsg = Closure::wrap(Box::new(move |e: MessageEvent| {
            let data = e.data();
            let kind = js_sys::Reflect::get(&data, &JsValue::from_str("kind"))
                .ok()
                .and_then(|k| k.as_string())
                .unwrap_or_default();
            match kind.as_str() {
                "tick" => {
                    if let Ok(p) = js_sys::Reflect::get(&data, &JsValue::from_str("pos")) {
                        if let Some(p) = p.as_f64() {
                            pos.set(p);
                        }
                    }
                    if let Ok(pl) = js_sys::Reflect::get(&data, &JsValue::from_str("playing")) {
                        if let Some(pl) = pl.as_bool() {
                            playing.set(pl);
                        }
                    }
                    if let Ok(pk) = js_sys::Reflect::get(&data, &JsValue::from_str("peaks")) {
                        if let Ok(arr) = pk.dyn_into::<js_sys::Float32Array>() {
                            *peaks.borrow_mut() = arr.to_vec();
                        }
                    }
                }
                "hello" => tracing::info!("setlist audio: worklet processor constructed"),
                "ready" => tracing::info!("setlist audio: worklet renderer ready"),
                "error" => {
                    let m = js_sys::Reflect::get(&data, &JsValue::from_str("message"))
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_default();
                    tracing::warn!("setlist audio: worklet error: {m}");
                }
                _ => {}
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        port.set_onmessage(Some(onmsg.as_ref().unchecked_ref()));

        // init (the renderer constructs at the worklet's global sampleRate).
        // Raw wasm BYTES, transferred — a pre-compiled WebAssembly.Module
        // fails to deserialize into an AudioWorkletGlobalScope (the message
        // dies as a silent `messageerror`); `initSync` compiles the bytes on
        // the audio thread, where synchronous compilation is allowed.
        let bytes = js_sys::Uint8Array::from(WORKLET_WASM);
        let init = msg("init");
        set(&init, "wasmBytes", &bytes.buffer());
        let transfer = js_sys::Array::of1(&bytes.buffer().into());
        port.post_message_with_transferable(&init, &transfer)
            .map_err(|e| format!("post init: {e:?}"))?;

        // Seed EVERY song: one project each (the native engine's layout),
        // one track + take per stem. Cheap — no PCM until a song is selected.
        for song in st.songs.iter() {
            let m = msg("add_project");
            set(&m, "guid", &JsValue::from_str(&song.project));
            set(&m, "name", &JsValue::from_str(&song.name));
            let _ = port.post_message(&m);
            for (name, guid, path) in &song.stems {
                let m = msg("add_stem");
                set(&m, "project", &JsValue::from_str(&song.project));
                set(&m, "name", &JsValue::from_str(name));
                set(&m, "guid", &JsValue::from_str(guid));
                set(&m, "path", &JsValue::from_str(path));
                let _ = port.post_message(&m);
            }
        }

        // Output path. DEFAULT: straight into `ctx.destination()` — the spec
        // path, and the only one immune to media-element live-stream clock
        // sync (an `<audio srcObject>` sink plays FAST to catch up latency
        // accumulated while the main thread is saturated — the "warped /
        // sped-up ~10 s on page load" bug). On rigs where WebAudio device
        // streams land on a silent FAKE sink (THEBATTLESHIP's pipewire-pulse
        // cycling strands the browser's audio service; see `cycle_sink`),
        // FORCE the media-element pipeline — the one that provably plays
        // there — with `localStorage.setItem("fts-audio-sink", "media")` or
        // `?audio=media`.
        let sink = if media_sink_forced() {
            let msd = ctx
                .create_media_stream_destination()
                .map_err(|e| format!("create_media_stream_destination: {e:?}"))?;
            node.connect_with_audio_node(&msd)
                .map_err(|e| format!("connect worklet node: {e:?}"))?;
            let sink =
                web_sys::HtmlAudioElement::new().map_err(|e| format!("HtmlAudioElement: {e:?}"))?;
            sink.set_src_object(Some(&msd.stream()));
            sink.set_autoplay(true);
            let _ = sink.play();
            tracing::info!("setlist audio: media-element output sink FORCED");
            Some(sink)
        } else {
            node.connect_with_audio_node(&ctx.destination())
                .map_err(|e| format!("connect worklet node: {e:?}"))?;
            None
        };

        // Flush the song selected while attaching, then transport + mixer
        // intent (order matters: select first, so seeks/mix land on it).
        let flush_song = st.pending_song.take().or_else(|| {
            let c = st.current.get();
            (c != usize::MAX).then_some(c)
        });
        if let Some(i) = flush_song {
            kick_song(ctx, &port, &st.songs, i, None, st.epoch.get(), &st.epoch);
        }
        if let Some(s) = st.pending_seek.take() {
            let m = msg("seek");
            set(&m, "seconds", &JsValue::from_f64(s));
            let _ = port.post_message(&m);
        }
        if let Some((muted, soloed, volumes)) = st.pending_mix.borrow_mut().take() {
            post_mix(&port, &muted, &soloed, &volumes);
        }
        if st.want_play.get() {
            let _ = port.post_message(&msg("play"));
        }

        *st.pump.borrow_mut() = Some(Pump {
            node,
            port: port.clone(),
            sink,
            _onmsg: onmsg,
        });

        tracing::info!(
            "setlist audio: worklet pump attached ({} song projects)",
            st.songs.len()
        );
        Ok(())
    }

    /// Song switch inside the shared renderer: post `select_project`, detach
    /// the outgoing song's PCM, and decode the incoming song's stems —
    /// THROTTLED (a handful of range requests at a time; ~23 at once 503s the
    /// media server), each attach epoch-guarded so a decode outlived by
    /// another switch aborts instead of attaching stale PCM.
    fn kick_song(
        ctx: &AudioContext,
        port: &MessagePort,
        songs: &Rc<Vec<SongStems>>,
        idx: usize,
        prev: Option<usize>,
        my_epoch: u64,
        epoch: &Rc<Cell<u64>>,
    ) {
        let Some(song) = songs.get(idx) else { return };
        let m = msg("select_project");
        set(&m, "guid", &JsValue::from_str(&song.project));
        let _ = port.post_message(&m);
        if let Some(p) = prev.and_then(|p| songs.get(p)) {
            for (_, guid, _) in &p.stems {
                let m = msg("detach");
                set(&m, "project", &JsValue::from_str(&p.project));
                set(&m, "guid", &JsValue::from_str(guid));
                let _ = port.post_message(&m);
            }
        }

        const DECODE_CONCURRENCY: usize = 5;
        let project = Rc::new(song.project.clone());
        let jobs = Rc::new(song.stems.clone());
        let next = Rc::new(Cell::new(0usize));
        for _ in 0..DECODE_CONCURRENCY.min(jobs.len().max(1)) {
            let project = project.clone();
            let jobs = jobs.clone();
            let next = next.clone();
            let ctx = ctx.clone();
            let port = port.clone();
            let epoch = epoch.clone();
            wasm_bindgen_futures::spawn_local(async move {
                loop {
                    if epoch.get() != my_epoch {
                        break; // song switched away — stop decoding this one
                    }
                    let i = next.get();
                    if i >= jobs.len() {
                        break;
                    }
                    next.set(i + 1);
                    let (_, guid, url) = &jobs[i];
                    if let Err(e) =
                        decode_and_send(&ctx, &port, &project, guid, url, &epoch, my_epoch).await
                    {
                        tracing::warn!("setlist audio: stem `{url}` decode failed: {e}");
                    }
                }
            });
        }
        tracing::info!(
            "setlist audio: song {idx} selected ({} stems decoding)",
            jobs.len()
        );
    }

    /// Fetch `url`, decode it through the context (`decodeAudioData` resamples
    /// to the context's rate == the worklet renderer's rate), interleave the
    /// channels, and post the PCM to the worklet (buffer transferred).
    #[allow(clippy::too_many_arguments)]
    async fn decode_and_send(
        ctx: &AudioContext,
        port: &MessagePort,
        project: &str,
        take_guid: &str,
        url: &str,
        epoch: &Rc<Cell<u64>>,
        my_epoch: u64,
    ) -> Result<(), String> {
        let array_buffer = fetch_array_buffer(url).await?;
        let promise = ctx
            .decode_audio_data(&array_buffer)
            .map_err(|e| format!("decode_audio_data: {e:?}"))?;
        let decoded = JsFuture::from(promise)
            .await
            .map_err(|e| format!("decode await: {e:?}"))?;
        let buffer: AudioBuffer = decoded
            .dyn_into()
            .map_err(|_| "decodeAudioData did not return an AudioBuffer".to_string())?;

        let channels = buffer.number_of_channels();
        let frames = buffer.length() as usize;
        let ch = channels.max(1) as usize;
        let mut pcm = vec![0.0_f32; frames * ch];
        for c in 0..channels {
            let data = buffer
                .get_channel_data(c)
                .map_err(|e| format!("get_channel_data({c}): {e:?}"))?;
            for (i, &s) in data.iter().enumerate() {
                pcm[i * ch + c as usize] = s;
            }
        }

        if epoch.get() != my_epoch {
            return Ok(()); // song switched away while decoding — drop it
        }
        let jarr = js_sys::Float32Array::from(&pcm[..]);
        let m = msg("attach");
        set(&m, "project", &JsValue::from_str(project));
        set(&m, "guid", &JsValue::from_str(take_guid));
        set(&m, "pcm", &jarr);
        set(&m, "channels", &JsValue::from(channels));
        set(&m, "sampleRate", &JsValue::from(ctx.sample_rate() as u32));
        let transfer = js_sys::Array::of1(&jarr.buffer().into());
        port.post_message_with_transferable(&m, &transfer)
            .map_err(|e| format!("post attach: {e:?}"))?;
        Ok(())
    }

    /// Same-origin `fetch` → `arrayBuffer()`.
    async fn fetch_array_buffer(url: &str) -> Result<js_sys::ArrayBuffer, String> {
        let win = web_sys::window().ok_or_else(|| "no window".to_string())?;
        let resp_val = JsFuture::from(win.fetch_with_str(url))
            .await
            .map_err(|e| format!("fetch {url}: {e:?}"))?;
        let resp: Response = resp_val
            .dyn_into()
            .map_err(|_| "fetch did not return a Response".to_string())?;
        if !resp.ok() {
            return Err(format!("{url}: HTTP {}", resp.status()));
        }
        let promise = resp
            .array_buffer()
            .map_err(|e| format!("{url}: array_buffer: {e:?}"))?;
        let val = JsFuture::from(promise)
            .await
            .map_err(|e| format!("{url}: array_buffer await: {e:?}"))?;
        val.dyn_into::<js_sys::ArrayBuffer>()
            .map_err(|_| format!("{url}: response was not an ArrayBuffer"))
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use imp::SetlistAudio;
