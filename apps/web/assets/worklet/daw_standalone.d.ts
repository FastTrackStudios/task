/* tslint:disable */
/* eslint-disable */

/**
 * Browser-side wrapper. Owns a `Standalone` + selected project guid
 * + sample-rate-aware shared transport. Cheap to construct;
 * expensive operations (decode, parse) live on the calling side.
 */
export class WebRenderer {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Seed one MORE project (a setlist song). Idempotent per guid. The new
     * project's transport is configured like the default one (no soft clock,
     * worklet sample rate) but it is NOT selected — call
     * [`select_project`](Self::select_project).
     */
    addProject(guid: string, name: string): void;
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
     */
    addStemTrack(display_name: string, take_guid: string, source_path: string): void;
    /**
     * [`add_stem_track`] targeting an EXPLICIT project (a setlist song).
     */
    addStemTrackIn(project: string, display_name: string, take_guid: string, source_path: string): void;
    /**
     * Attach an already-decoded source for `take_guid`. JS callers
     * typically get the PCM via `AudioContext.decodeAudioData` →
     * `AudioBuffer.getChannelData(ch)` → `Float32Array`.
     */
    attachAudioSource(take_guid: string, interleaved_pcm: Float32Array, channels: number, sample_rate: number): void;
    /**
     * [`attach_audio_source_pcm`] targeting an EXPLICIT project — the
     * setlist path, where decodes race song switches and must land on the
     * song they were started for.
     */
    attachAudioSourceIn(project: string, take_guid: string, interleaved_pcm: Float32Array, channels: number): void;
    audioSourceCount(): number;
    /**
     * Drop a previously-attached source (searched in the SELECTED project).
     */
    detachAudioSource(take_guid: string): void;
    /**
     * Drop a previously-attached source from an EXPLICIT project — used by
     * the setlist path to free the outgoing song's PCM on a song switch.
     */
    detachAudioSourceIn(project: string, take_guid: string): void;
    isPlaying(): boolean;
    /**
     * Construct a fresh renderer + seed a project. The worklet should
     * hand its actual `sampleRate` so the sample clock matches the
     * browser's output.
     */
    constructor(sample_rate: number);
    /**
     * Every distinct source path referenced by takes in the loaded
     * project, sorted. Browsers fetch each one (e.g. via `fetch()`
     * to a Nextcloud / S3 / static host base URL), decode via
     * `AudioContext.decodeAudioData`, then call
     * [`attachAudioSource`](Self::attach_audio_source_pcm) for each
     * matching take.
     */
    pathsToResolve(): any[];
    pause(): void;
    play(): void;
    positionSeconds(): number;
    projectGuid(): string;
    /**
     * Render `frames` stereo frames into the two output channels.
     * The worklet calls this with the buffers AudioWorkletProcessor
     * provides each `process()` invocation.
     */
    render(out_left: Float32Array, out_right: Float32Array): void;
    seekSeconds(seconds: number): void;
    /**
     * Queue a QUANTIZED seek: when the transport reaches `at` seconds the
     * render callback jumps to `target` seconds (see [`render`]). A later
     * call replaces the queued jump; an immediate [`seek_seconds`] cancels
     * it.
     */
    seekSecondsAt(target: number, at: number): void;
    /**
     * Select which project [`render`] renders (a setlist song switch). The
     * graph, tracks, and any attached PCM persist across switches — this
     * swaps the transport + meter bank only. No-op for the already-selected
     * project or an unknown guid.
     */
    selectProject(guid: string): void;
    setTrackMute(index: number, muted: boolean): void;
    setTrackSolo(index: number, soloed: boolean): void;
    /**
     * Fader gain, linear (1.0 = unity).
     */
    setTrackVolume(index: number, volume: number): void;
    stop(): void;
    /**
     * All `(take_guid, source_path)` pairs in the loaded project,
     * returned as a flat `[take, path, take, path, …]` JS array.
     * Browsers use this to map fetched files back to the takes
     * they belong to.
     */
    takeSources(): any[];
    trackCount(): number;
    /**
     * Per-track peak levels (0.0..=1.0, max of L/R), indexed by track order —
     * the render path writes these into the `Meters` cells every block, so
     * this is a lock-free read for UI VU meters.
     */
    trackPeaks(): Float32Array;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_webrenderer_free: (a: number, b: number) => void;
    readonly webrenderer_addProject: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly webrenderer_addStemTrack: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => void;
    readonly webrenderer_addStemTrackIn: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => void;
    readonly webrenderer_attachAudioSource: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => void;
    readonly webrenderer_attachAudioSourceIn: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => void;
    readonly webrenderer_audioSourceCount: (a: number) => number;
    readonly webrenderer_detachAudioSource: (a: number, b: number, c: number) => void;
    readonly webrenderer_detachAudioSourceIn: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly webrenderer_isPlaying: (a: number) => number;
    readonly webrenderer_new: (a: number) => number;
    readonly webrenderer_pathsToResolve: (a: number) => [number, number];
    readonly webrenderer_pause: (a: number) => void;
    readonly webrenderer_play: (a: number) => void;
    readonly webrenderer_positionSeconds: (a: number) => number;
    readonly webrenderer_projectGuid: (a: number) => [number, number];
    readonly webrenderer_render: (a: number, b: number, c: number, d: any, e: number, f: number, g: any) => void;
    readonly webrenderer_seekSeconds: (a: number, b: number) => void;
    readonly webrenderer_seekSecondsAt: (a: number, b: number, c: number) => void;
    readonly webrenderer_selectProject: (a: number, b: number, c: number) => void;
    readonly webrenderer_setTrackMute: (a: number, b: number, c: number) => void;
    readonly webrenderer_setTrackSolo: (a: number, b: number, c: number) => void;
    readonly webrenderer_setTrackVolume: (a: number, b: number, c: number) => void;
    readonly webrenderer_stop: (a: number) => void;
    readonly webrenderer_takeSources: (a: number) => [number, number];
    readonly webrenderer_trackCount: (a: number) => number;
    readonly webrenderer_trackPeaks: (a: number) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h5ca89d924f5a02f7: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h0ddf7788afd622d7: (a: number, b: number) => void;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_destroy_closure: (a: number, b: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_drop_slice: (a: number, b: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
