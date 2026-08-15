// AudioWorkletProcessor hosting daw-standalone's `WebRenderer` ON the audio
// thread — the browser twin of the native cpal callback. The main thread
// (crates/task/ui/src/pages/setlist_audio.rs) compiles the wasm module,
// posts it here, then streams decoded stem PCM over the port; every render
// quantum (128 frames) calls straight into the Rust graph. No jitter buffer,
// no main-thread audio work — realtime at worklet quantum size, the same
// path a signal guitar rig needs.
//
// Glue + wasm live next to this file (see `just task-worklet-wasm`):
//   assets/worklet/daw_standalone.js       wasm-bindgen glue (statically imported
//                                          — dynamic import() is unavailable in
//                                          AudioWorkletGlobalScope)
//   assets/worklet/daw_standalone_bg.wasm  compiled by the MAIN thread (no fetch
//                                          in worklets) and sent via 'init'
//
// AudioWorkletGlobalScope is minimal — polyfill what the glue needs:
//  - TextDecoder/TextEncoder (string passing)
//  - crypto.getRandomValues (track guids; non-cryptographic is fine)

if (typeof globalThis.TextDecoder === 'undefined') {
  globalThis.TextDecoder = class {
    constructor(_label, _opts) {}
    decode(input) {
      if (input === undefined) return '';
      const u8 = input instanceof Uint8Array ? input : new Uint8Array(input);
      let s = '';
      let i = 0;
      while (i < u8.length) {
        const b = u8[i++];
        let c;
        if (b < 0x80) c = b;
        else if (b < 0xe0) c = ((b & 0x1f) << 6) | (u8[i++] & 0x3f);
        else if (b < 0xf0)
          c = ((b & 0x0f) << 12) | ((u8[i++] & 0x3f) << 6) | (u8[i++] & 0x3f);
        else
          c =
            ((b & 0x07) << 18) |
            ((u8[i++] & 0x3f) << 12) |
            ((u8[i++] & 0x3f) << 6) |
            (u8[i++] & 0x3f);
        s += String.fromCodePoint(c);
      }
      return s;
    }
  };
}

if (typeof globalThis.TextEncoder === 'undefined') {
  // No encodeInto on purpose: the wasm-bindgen glue falls back to the simpler
  // (and here, correct) encode()+copy path when encodeInto is absent.
  globalThis.TextEncoder = class {
    encode(s) {
      const out = [];
      for (const ch of s) {
        let c = ch.codePointAt(0);
        if (c < 0x80) out.push(c);
        else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 0x3f));
        else if (c < 0x10000)
          out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
        else
          out.push(
            0xf0 | (c >> 18),
            0x80 | ((c >> 12) & 0x3f),
            0x80 | ((c >> 6) & 0x3f),
            0x80 | (c & 0x3f),
          );
      }
      return new Uint8Array(out);
    }
  };
}

if (typeof globalThis.crypto === 'undefined') {
  globalThis.crypto = {
    getRandomValues(arr) {
      for (let i = 0; i < arr.length; i++) arr[i] = (Math.random() * 256) | 0;
      return arr;
    },
  };
}

// `web_time::Instant::now()` (the transport soft clock) needs
// `performance.now()`; AudioWorkletGlobalScope has neither `performance`
// nor timers. Back `now()` with the audio clock (`currentTime`, seconds,
// monotonic) and queue setTimeout callbacks to run from `process()` —
// the soft clock is disabled in the worklet anyway (render() advances the
// playhead), its task just needs to not crash.
if (typeof globalThis.performance === 'undefined') {
  globalThis.performance = {
    timeOrigin: 0,
    now: () => currentTime * 1000,
  };
}
const __timers = [];
let __timerId = 1;
if (typeof globalThis.setTimeout === 'undefined') {
  globalThis.setTimeout = (fn, ms = 0, ...args) => {
    __timers.push({ id: __timerId, at: currentTime + ms / 1000, fn, args });
    return __timerId++;
  };
  globalThis.clearTimeout = (id) => {
    const i = __timers.findIndex((t) => t.id === id);
    if (i >= 0) __timers.splice(i, 1);
  };
  globalThis.setInterval = globalThis.setTimeout;
  globalThis.clearInterval = globalThis.clearTimeout;
}
function __runDueTimers() {
  if (__timers.length === 0) return;
  const due = [];
  for (let i = __timers.length - 1; i >= 0; i--) {
    if (__timers[i].at <= currentTime) due.push(...__timers.splice(i, 1));
  }
  for (const t of due) {
    try {
      t.fn(...t.args);
    } catch (_) {
      /* timer callbacks must never kill the audio thread */
    }
  }
}

import { initSync, WebRenderer } from './daw_standalone.js';

class FtsDawProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.renderer = null;
    this.tick = 0;
    this.port.onmessage = (e) => this.handle(e.data);
    // A message whose payload can't deserialize in THIS realm arrives as a
    // silent `messageerror` — surface it (this is how a structured-cloned
    // WebAssembly.Module dies in an AudioWorkletGlobalScope).
    this.port.onmessageerror = () =>
      this.port.postMessage({ kind: 'error', message: 'messageerror: payload not deserializable' });
    // DIAG: proves processor construction + the worklet→main port path.
    this.port.postMessage({ kind: 'hello' });
  }

  handle(msg) {
    try {
      switch (msg.kind) {
        case 'init': {
          // `wasmBytes` is the raw wasm binary (ArrayBuffer, transferred) —
          // NOT a pre-compiled WebAssembly.Module: cloned Modules fail to
          // deserialize into an AudioWorkletGlobalScope (silent messageerror).
          // Synchronous compilation is allowed off the main thread; `initSync`
          // accepts a BufferSource directly. `sampleRate` is the worklet
          // global.
          initSync({ module: new Uint8Array(msg.wasmBytes) });
          this.renderer = new WebRenderer(sampleRate);
          this.port.postMessage({ kind: 'ready' });
          break;
        }
        case 'add_project':
          // One project per setlist song — ONE renderer hosts the whole set;
          // a song switch is 'select_project', never a rebuild.
          this.renderer?.addProject(msg.guid, msg.name);
          break;
        case 'select_project':
          this.renderer?.selectProject(msg.guid);
          break;
        case 'add_stem':
          if (msg.project) {
            this.renderer?.addStemTrackIn(msg.project, msg.name, msg.guid, msg.path);
          } else {
            this.renderer?.addStemTrack(msg.name, msg.guid, msg.path);
          }
          break;
        case 'attach':
          if (msg.project) {
            // Explicit project: decodes race song switches and must land on
            // the song they were started for (resampled to the worklet rate
            // by decodeAudioData already).
            this.renderer?.attachAudioSourceIn(msg.project, msg.guid, msg.pcm, msg.channels);
          } else {
            this.renderer?.attachAudioSource(msg.guid, msg.pcm, msg.channels, msg.sampleRate);
          }
          break;
        case 'detach':
          if (msg.project) {
            this.renderer?.detachAudioSourceIn(msg.project, msg.guid);
          } else {
            this.renderer?.detachAudioSource(msg.guid);
          }
          break;
        case 'play':
          this.renderer?.play();
          break;
        case 'pause':
          this.renderer?.pause();
          break;
        case 'stop':
          this.renderer?.stop();
          break;
        case 'seek':
          this.renderer?.seekSeconds(msg.seconds);
          break;
        case 'seek_q':
          // Quantized seek: jump to `seconds` when the transport reaches
          // `at` (executed inside render(), on the boundary block).
          this.renderer?.seekSecondsAt(msg.seconds, msg.at);
          break;
        case 'mix': {
          // Full mixer state, idempotent (track order == stem order). Goes
          // through the graph's own Tracks ops, so solo routing + fader gain
          // apply on the next render block.
          const n = msg.muted?.length ?? 0;
          for (let i = 0; i < n; i++) {
            this.renderer?.setTrackMute(i, !!msg.muted[i]);
            this.renderer?.setTrackSolo(i, !!msg.soloed[i]);
            this.renderer?.setTrackVolume(i, msg.volumes[i]);
          }
          break;
        }
      }
    } catch (err) {
      this.port.postMessage({ kind: 'error', message: String(err) });
    }
  }

  process(_inputs, outputs) {
    __runDueTimers();
    const out = outputs[0];
    if (!out || out.length === 0) return true;
    const l = out[0];
    const r = out[1] ?? out[0];
    if (this.renderer) {
      this.renderer.render(l, r);
    } else {
      l.fill(0);
      if (r !== l) r.fill(0);
    }
    // ~21 ms state tick (position / play-state / per-track peaks) for the UI.
    if (this.renderer && ++this.tick >= 8) {
      this.tick = 0;
      this.port.postMessage({
        kind: 'tick',
        pos: this.renderer.positionSeconds(),
        playing: this.renderer.isPlaying(),
        peaks: this.renderer.trackPeaks(),
      });
    }
    return true;
  }
}

registerProcessor('fts-daw-processor', FtsDawProcessor);
