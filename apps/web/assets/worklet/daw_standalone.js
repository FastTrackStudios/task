/* @ts-self-types="./daw_standalone.d.ts" */

/**
 * Browser-side wrapper. Owns a `Standalone` + selected project guid
 * + sample-rate-aware shared transport. Cheap to construct;
 * expensive operations (decode, parse) live on the calling side.
 */
export class WebRenderer {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WebRendererFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_webrenderer_free(ptr, 0);
    }
    /**
     * Seed one MORE project (a setlist song). Idempotent per guid. The new
     * project's transport is configured like the default one (no soft clock,
     * worklet sample rate) but it is NOT selected — call
     * [`select_project`](Self::select_project).
     * @param {string} guid
     * @param {string} name
     */
    addProject(guid, name) {
        const ptr0 = passStringToWasm0(guid, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        wasm.webrenderer_addProject(this.__wbg_ptr, ptr0, len0, ptr1, len1);
    }
    /**
     * Seed one stem: a track + an audio item/take whose source points at
     * `source_path`, keyed by the stable `take_guid` the caller will later
     * pass to [`attach_audio_source_pcm`] once the browser has decoded the
     * file. Mirrors the native `media_seed` layout (track → MIDI item → flip
     * the active take to audio) but *pins* the take's guid to `take_guid` so
     * the async decode can attach its PCM by that stable key
     * (`ProjectState.audio_sources` is keyed by the active take's guid).
     *
     * The item is placed at 0s with a generously long length; the decoded
     * source itself bounds the audible region (past its end reads silence),
     * so no per-song length is needed here.
     * @param {string} display_name
     * @param {string} take_guid
     * @param {string} source_path
     */
    addStemTrack(display_name, take_guid, source_path) {
        const ptr0 = passStringToWasm0(display_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(take_guid, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(source_path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        wasm.webrenderer_addStemTrack(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2);
    }
    /**
     * [`add_stem_track`] targeting an EXPLICIT project (a setlist song).
     * @param {string} project
     * @param {string} display_name
     * @param {string} take_guid
     * @param {string} source_path
     */
    addStemTrackIn(project, display_name, take_guid, source_path) {
        const ptr0 = passStringToWasm0(project, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(display_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(take_guid, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ptr3 = passStringToWasm0(source_path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len3 = WASM_VECTOR_LEN;
        wasm.webrenderer_addStemTrackIn(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
    }
    /**
     * Attach an already-decoded source for `take_guid`. JS callers
     * typically get the PCM via `AudioContext.decodeAudioData` →
     * `AudioBuffer.getChannelData(ch)` → `Float32Array`.
     * @param {string} take_guid
     * @param {Float32Array} interleaved_pcm
     * @param {number} channels
     * @param {number} sample_rate
     */
    attachAudioSource(take_guid, interleaved_pcm, channels, sample_rate) {
        const ptr0 = passStringToWasm0(take_guid, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF32ToWasm0(interleaved_pcm, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        wasm.webrenderer_attachAudioSource(this.__wbg_ptr, ptr0, len0, ptr1, len1, channels, sample_rate);
    }
    /**
     * [`attach_audio_source_pcm`] targeting an EXPLICIT project — the
     * setlist path, where decodes race song switches and must land on the
     * song they were started for.
     * @param {string} project
     * @param {string} take_guid
     * @param {Float32Array} interleaved_pcm
     * @param {number} channels
     */
    attachAudioSourceIn(project, take_guid, interleaved_pcm, channels) {
        const ptr0 = passStringToWasm0(project, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(take_guid, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArrayF32ToWasm0(interleaved_pcm, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        wasm.webrenderer_attachAudioSourceIn(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, channels);
    }
    /**
     * @returns {number}
     */
    audioSourceCount() {
        const ret = wasm.webrenderer_audioSourceCount(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Drop a previously-attached source (searched in the SELECTED project).
     * @param {string} take_guid
     */
    detachAudioSource(take_guid) {
        const ptr0 = passStringToWasm0(take_guid, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.webrenderer_detachAudioSource(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * Drop a previously-attached source from an EXPLICIT project — used by
     * the setlist path to free the outgoing song's PCM on a song switch.
     * @param {string} project
     * @param {string} take_guid
     */
    detachAudioSourceIn(project, take_guid) {
        const ptr0 = passStringToWasm0(project, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(take_guid, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        wasm.webrenderer_detachAudioSourceIn(this.__wbg_ptr, ptr0, len0, ptr1, len1);
    }
    /**
     * @returns {boolean}
     */
    isPlaying() {
        const ret = wasm.webrenderer_isPlaying(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Construct a fresh renderer + seed a project. The worklet should
     * hand its actual `sampleRate` so the sample clock matches the
     * browser's output.
     * @param {number} sample_rate
     */
    constructor(sample_rate) {
        const ret = wasm.webrenderer_new(sample_rate);
        this.__wbg_ptr = ret;
        WebRendererFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Every distinct source path referenced by takes in the loaded
     * project, sorted. Browsers fetch each one (e.g. via `fetch()`
     * to a Nextcloud / S3 / static host base URL), decode via
     * `AudioContext.decodeAudioData`, then call
     * [`attachAudioSource`](Self::attach_audio_source_pcm) for each
     * matching take.
     * @returns {any[]}
     */
    pathsToResolve() {
        const ret = wasm.webrenderer_pathsToResolve(this.__wbg_ptr);
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    pause() {
        wasm.webrenderer_pause(this.__wbg_ptr);
    }
    play() {
        wasm.webrenderer_play(this.__wbg_ptr);
    }
    /**
     * @returns {number}
     */
    positionSeconds() {
        const ret = wasm.webrenderer_positionSeconds(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {string}
     */
    projectGuid() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.webrenderer_projectGuid(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Render `frames` stereo frames into the two output channels.
     * The worklet calls this with the buffers AudioWorkletProcessor
     * provides each `process()` invocation.
     * @param {Float32Array} out_left
     * @param {Float32Array} out_right
     */
    render(out_left, out_right) {
        var ptr0 = passArrayF32ToWasm0(out_left, wasm.__wbindgen_malloc);
        var len0 = WASM_VECTOR_LEN;
        var ptr1 = passArrayF32ToWasm0(out_right, wasm.__wbindgen_malloc);
        var len1 = WASM_VECTOR_LEN;
        wasm.webrenderer_render(this.__wbg_ptr, ptr0, len0, out_left, ptr1, len1, out_right);
    }
    /**
     * @param {number} seconds
     */
    seekSeconds(seconds) {
        wasm.webrenderer_seekSeconds(this.__wbg_ptr, seconds);
    }
    /**
     * Queue a QUANTIZED seek: when the transport reaches `at` seconds the
     * render callback jumps to `target` seconds (see [`render`]). A later
     * call replaces the queued jump; an immediate [`seek_seconds`] cancels
     * it.
     * @param {number} target
     * @param {number} at
     */
    seekSecondsAt(target, at) {
        wasm.webrenderer_seekSecondsAt(this.__wbg_ptr, target, at);
    }
    /**
     * Select which project [`render`] renders (a setlist song switch). The
     * graph, tracks, and any attached PCM persist across switches — this
     * swaps the transport + meter bank only. No-op for the already-selected
     * project or an unknown guid.
     * @param {string} guid
     */
    selectProject(guid) {
        const ptr0 = passStringToWasm0(guid, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.webrenderer_selectProject(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * @param {number} index
     * @param {boolean} muted
     */
    setTrackMute(index, muted) {
        wasm.webrenderer_setTrackMute(this.__wbg_ptr, index, muted);
    }
    /**
     * @param {number} index
     * @param {boolean} soloed
     */
    setTrackSolo(index, soloed) {
        wasm.webrenderer_setTrackSolo(this.__wbg_ptr, index, soloed);
    }
    /**
     * Fader gain, linear (1.0 = unity).
     * @param {number} index
     * @param {number} volume
     */
    setTrackVolume(index, volume) {
        wasm.webrenderer_setTrackVolume(this.__wbg_ptr, index, volume);
    }
    stop() {
        wasm.webrenderer_stop(this.__wbg_ptr);
    }
    /**
     * All `(take_guid, source_path)` pairs in the loaded project,
     * returned as a flat `[take, path, take, path, …]` JS array.
     * Browsers use this to map fetched files back to the takes
     * they belong to.
     * @returns {any[]}
     */
    takeSources() {
        const ret = wasm.webrenderer_takeSources(this.__wbg_ptr);
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * @returns {number}
     */
    trackCount() {
        const ret = wasm.webrenderer_trackCount(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Per-track peak levels (0.0..=1.0, max of L/R), indexed by track order —
     * the render path writes these into the `Meters` cells every block, so
     * this is a lock-free read for UI VU meters.
     * @returns {Float32Array}
     */
    trackPeaks() {
        const ret = wasm.webrenderer_trackPeaks(this.__wbg_ptr);
        var v1 = getArrayF32FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
}
if (Symbol.dispose) WebRenderer.prototype[Symbol.dispose] = WebRenderer.prototype.free;
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg___wbindgen_copy_to_typed_array_4db0cbe2cc60dbee: function(arg0, arg1, arg2) {
            new Uint8Array(arg2.buffer, arg2.byteOffset, arg2.byteLength).set(getArrayU8FromWasm0(arg0, arg1));
        },
        __wbg___wbindgen_is_function_1ff95bcc5517c252: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_undefined_c05833b95a3cf397: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_throw_344f42d3211c4765: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_fffb441def202758: function(arg0) {
            arg0._wbg_cb_unref();
        },
        __wbg_clearTimeout_113b1cde814ec762: function(arg0) {
            const ret = clearTimeout(arg0);
            return ret;
        },
        __wbg_getRandomValues_bf16787eede473f5: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_now_e7c6795a7f81e10f: function(arg0) {
            const ret = arg0.now();
            return ret;
        },
        __wbg_performance_3fcf6e32a7e1ed0a: function(arg0) {
            const ret = arg0.performance;
            return ret;
        },
        __wbg_queueMicrotask_0ab5b2d2393e99b9: function(arg0) {
            const ret = arg0.queueMicrotask;
            return ret;
        },
        __wbg_queueMicrotask_6a09b7bc46549209: function(arg0) {
            queueMicrotask(arg0);
        },
        __wbg_resolve_2191a4dfe481c25b: function(arg0) {
            const ret = Promise.resolve(arg0);
            return ret;
        },
        __wbg_setTimeout_ef24d2fc3ad97385: function() { return handleError(function (arg0, arg1) {
            const ret = setTimeout(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_static_accessor_GLOBAL_4ef717fb391d88b7: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_8d1badc68b5a74f4: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_146583524fe1469b: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_f2829a2234d7819e: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_then_6ec10ae38b3e92f7: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 730, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h5ca89d924f5a02f7);
            return ret;
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 687, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen__convert__closures_____invoke__h0ddf7788afd622d7);
            return ret;
        },
        __wbindgen_cast_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./daw_standalone_bg.js": import0,
    };
}

function wasm_bindgen__convert__closures_____invoke__h0ddf7788afd622d7(arg0, arg1) {
    wasm.wasm_bindgen__convert__closures_____invoke__h0ddf7788afd622d7(arg0, arg1);
}

function wasm_bindgen__convert__closures_____invoke__h5ca89d924f5a02f7(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen__convert__closures_____invoke__h5ca89d924f5a02f7(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

const WebRendererFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_webrenderer_free(ptr, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => wasm.__wbindgen_destroy_closure(state.a, state.b));

function getArrayF32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayJsValueFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(wasm.__wbindgen_externrefs.get(mem.getUint32(i, true)));
    }
    wasm.__externref_drop_slice(ptr, len);
    return result;
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

let cachedFloat32ArrayMemory0 = null;
function getFloat32ArrayMemory0() {
    if (cachedFloat32ArrayMemory0 === null || cachedFloat32ArrayMemory0.byteLength === 0) {
        cachedFloat32ArrayMemory0 = new Float32Array(wasm.memory.buffer);
    }
    return cachedFloat32ArrayMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function makeMutClosure(arg0, arg1, f) {
    const state = { a: arg0, b: arg1, cnt: 1 };
    const real = (...args) => {

        // First up with a closure we increment the internal reference
        // count. This ensures that the Rust closure environment won't
        // be deallocated while we're invoking it.
        state.cnt++;
        const a = state.a;
        state.a = 0;
        try {
            return f(a, state.b, ...args);
        } finally {
            state.a = a;
            real._wbg_cb_unref();
        }
    };
    real._wbg_cb_unref = () => {
        if (--state.cnt === 0) {
            wasm.__wbindgen_destroy_closure(state.a, state.b);
            state.a = 0;
            CLOSURE_DTORS.unregister(state);
        }
    };
    CLOSURE_DTORS.register(real, state, state);
    return real;
}

function passArrayF32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getFloat32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedFloat32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('daw_standalone_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
