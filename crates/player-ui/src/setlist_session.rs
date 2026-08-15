//! `type: setlist` vault-note view — a browser multitrack **setlist session
//! player**: it loads a whole ordered set of songs, tracks a **current song**,
//! lets you navigate the whole set (prev / next / pick), and presents the
//! session view + chart as tabs that FOLLOW the current song.
//!
//! Built on the same streaming Web-Audio engine as the single-song
//! [`SongView`](crate::song_session) — the reusable primitives (manifest
//! model, engine, mixer, session-proto mapping) are shared from
//! `song_session::imp`; this module only adds the multi-song orchestration.
//!
//! ## The model — the current song drives everything
//!
//! The session-ui views are designed around a setlist + an **active song
//! index** (`ACTIVE_INDICES.song_index`). On load we hydrate the WHOLE set:
//!
//! - `SETLIST_STRUCTURE` — a `session_proto::Setlist` with EVERY song
//!   (sections/tempo/key + `project_guid = web-session:{slug}`).
//! - `SONG_CHARTS[guid]` — each song's `chart.kf` text.
//!
//! Then the chart pane, section bar, navigator sidebar and transport ALL
//! follow the active song for free — switching songs is just a write to
//! `ACTIVE_INDICES.song_index` (via `current_song`) plus an audio swap.
//!
//! ## Audio — only the current song is loaded
//!
//! Loading every song's stems at once would mean 100+ media elements. Instead
//! the streaming graph holds ONLY the current song; when `current_song`
//! changes we tear the old graph down (`EngineInner::teardown` — pause,
//! detach, close the `AudioContext`) and build the new song's from its
//! manifest. Transport (play/pause/seek) drives the current song; a song
//! switch stops and resets to the head of the new song.

// ─────────────────────────────────────────────────────────────────────────────
// wasm32: the real player.
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(target_arch = "wasm32")]
mod imp {
    use std::rc::Rc;
    use task_ui_core::format::duration_mmss;

    use dioxus::prelude::*;

    use daw_proto::{MusicalPosition, TimeSignature};
    use session_ui::components::{
        MeasureIndicator, MixerView, SectionProgressBar, SongProgressBar, TransportControlBar,
    };
    use session_ui::{PerformanceSidebar, SETLIST_STRUCTURE};

    use crate::session_chart_pane::SessionChartPane;
    // Stage 4b-2: the WebRenderer audio graph for the active song (the SAME
    // daw-standalone render graph as native cpal), driven in lockstep with the
    // engine transport.
    use crate::setlist_audio::SetlistAudio;
    // The streaming engine + manifest model + session-proto mapping are shared
    // with the single-song player (see `song_session::imp`).
    use crate::song_session::imp as media;

    /// One row in the setlist navigator: display title plus the at-a-glance
    /// facts (key / tempo) and the song's accent color (its first section's
    /// bright color — the same per-section palette the timeline uses).
    #[derive(Clone, PartialEq)]
    struct SongMeta {
        title: String,
        key: Option<String>,
        bpm: Option<f64>,
        accent: String,
    }

    /// A song's fetched artifacts: its slug, manifest, and optional chart text.
    type LoadedSong = (String, media::Manifest, Option<String>);

    /// Which tab is showing in the embedded player.
    #[derive(Clone, Copy, PartialEq)]
    enum Tab {
        Session,
        Chart,
    }

    /// The right-hand pane of the full-screen experience's center. The
    /// left pane is always the chart; the selector (on the RIGHT) switches
    /// what sits beside it. Future options (lyric editor, etc.) slot in here.
    #[derive(Clone, Copy, PartialEq)]
    enum CenterRight {
        /// Keyflow source editor (charts as code) — the default.
        Editor,
        Mixer,
        Comments,
    }

    /// What the LEFT (chart) pane shows. For now the engraved Master Rhythm
    /// chart; a lyric scroller (for singers) and per-part views come later.
    #[derive(Clone, Copy, PartialEq)]
    enum ChartLeft {
        MasterRhythm,
        Lyrics,
    }

    /// One selectable right-hand pane in the full-screen center: a right-aligned
    /// selector (Editor / Mixer / Comments, extensible) over the chosen view.
    /// Shared so the center can render one pane normally and a second on
    /// ultrawide, both drawing from the same option set. `root_class` carries the
    /// flex-item sizing + responsive visibility (the second pane is
    /// `hidden … min-[1700px]:flex`).
    #[component]
    fn RightPane(
        root_class: &'static str,
        selected: CenterRight,
        on_select: EventHandler<CenterRight>,
        /// Remount key for the keyflow editor (distinct per pane + song).
        editor_key: String,
        source: String,
        guid: String,
        tracks: Vec<daw_proto::Track>,
        on_volume: Callback<(String, f64)>,
        on_mute: Callback<String>,
        on_solo: Callback<String>,
        guide_present: bool,
        guide_on: bool,
        on_guide: Callback<()>,
    ) -> Element {
        // Width cap by content: the keyflow editor stays a narrow reading column
        // (~2xl), comments a bit wider, and the mixer gets the full width for the
        // console. `root_class` no longer carries a `max-w-*` (this wins).
        let max_w = match selected {
            CenterRight::Editor => "max-w-2xl",
            CenterRight::Comments => "max-w-3xl",
            CenterRight::Mixer => "max-w-none",
        };
        rsx! {
            div { class: "{root_class} {max_w}",
                div { class: "flex shrink-0 items-center justify-end gap-1 border-b border-border px-3 py-1.5",
                    for (v , label) in [
                        (CenterRight::Editor, "Editor"),
                        (CenterRight::Mixer, "Mixer"),
                        (CenterRight::Comments, "Comments"),
                    ] {
                        button {
                            key: "{label}",
                            class: if selected == v {
                                "rounded px-2 py-0.5 text-xs font-medium bg-accent text-foreground"
                            } else {
                                "rounded px-2 py-0.5 text-xs text-muted-foreground hover:text-foreground"
                            },
                            onclick: move |_| on_select.call(v),
                            "{label}"
                        }
                    }
                }
                div { class: "flex min-h-0 flex-1 flex-col overflow-auto",
                    if selected == CenterRight::Editor {
                        crate::keyflow_chart_editor::KeyflowChartEditor {
                            key: "{editor_key}",
                            source: source.clone(),
                            guid: guid.clone(),
                        }
                    } else if selected == CenterRight::Mixer {
                        MeteredMixer {
                            tracks: tracks.clone(),
                            on_volume,
                            on_mute,
                            on_solo,
                            guide_present,
                            guide_on,
                            on_guide,
                        }
                    } else {
                        div { class: "p-4 text-sm text-muted-foreground",
                            "Song comments — coming soon. This pane will show the current song's notes + comments."
                        }
                    }
                }
            }
        }
    }

    /// Per-stem peak levels (indexed by stem order, 0.0..=1.0) for the mixer VU
    /// meters. A pure display signal, written by the playback meter loop.
    static SETLIST_LEVELS: GlobalSignal<Vec<f32>> = Signal::global(Vec::new);

    /// The mixer plus its live meters. Isolated in its own component so the
    /// ~20 fps meter updates (it subscribes to [`SETLIST_LEVELS`]) re-render
    /// only the desk — never the editor sitting beside it in the other pane.
    #[component]
    fn MeteredMixer(
        tracks: Vec<daw_proto::Track>,
        on_volume: Callback<(String, f64)>,
        on_mute: Callback<String>,
        on_solo: Callback<String>,
        guide_present: bool,
        guide_on: bool,
        on_guide: Callback<()>,
    ) -> Element {
        // Map per-index peaks onto track guids (guid == stem file, index == i).
        let levels: std::collections::HashMap<String, f32> = {
            let peaks = SETLIST_LEVELS.read();
            tracks
                .iter()
                .filter_map(|t| peaks.get(t.index as usize).map(|&p| (t.guid.clone(), p)))
                .collect()
        };
        rsx! {
            if guide_present {
                div { class: "flex shrink-0 items-center gap-3 border-b border-border p-3",
                    span { class: "flex-1 text-sm font-semibold text-foreground", "Guide / Click" }
                    button {
                        class: if guide_on {
                            "rounded-md bg-primary px-4 py-1.5 text-sm font-semibold text-primary-foreground hover:bg-primary/90"
                        } else {
                            "rounded-md bg-muted px-4 py-1.5 text-sm font-semibold text-muted-foreground hover:bg-accent"
                        },
                        onclick: move |_| on_guide.call(()),
                        if guide_on { "On" } else { "Off" }
                    }
                }
            }
            div { class: "min-h-0 flex-1",
                MixerView {
                    tracks,
                    on_volume,
                    on_mute,
                    on_solo,
                    levels,
                }
            }
        }
    }

    // ── the component ───────────────────────────────────────────────────────

    /// The `type: setlist` player. `songs` is the ordered list of media slugs
    /// (from the note's `songs:` frontmatter). Rendered above the note editor
    /// (embedded) or inside the full-screen setlist Experience.
    #[component]
    pub fn SetlistPlayer(
        songs: Vec<String>,
        org: String,
        #[props(default)] fullscreen: bool,
    ) -> Element {
        // Current song in the set. Mirrored FROM the engine's `ACTIVE_INDICES`
        // (published by `SessionEventBridge`), and set optimistically by the
        // navigation callbacks so the UI responds immediately.
        let current_song = use_signal(|| 0usize);
        let playing = use_signal(|| false);
        let position = use_signal(|| 0.0_f64);
        // Stage 4b-2: the active song's WebRenderer audio graph. `None` until
        // the current song's stems are seeded; rebuilt on song switch (one
        // render graph at a time). Held behind an `Rc` (it's `!Send` + owns a
        // Web Audio context + closure) so callbacks can clone a handle out.
        let audio = use_signal(|| None::<Rc<SetlistAudio>>);
        // No streamed-element buffering to track (the WebRenderer decodes
        // stems into PCM asynchronously; the graph renders silence until they
        // land, so there's no explicit buffering gate).
        let buffering = use_signal(|| false);
        // Per-stem mixer state for the CURRENT song. The mixer strips render
        // from this AND it is pushed into the worklet renderer (the effect
        // below `set_mutes`), so mute/solo/fader are audible. Reset from the
        // current song's manifest on song switch.
        let stem_ui = use_signal(Vec::<media::StemUi>::new);
        // The org whose vault this setlist lives in — media serves at
        // /org/{slug}/media/... . Follows the org switcher.

        // Fetch EVERY song's manifest + chart.kf once. Keyed on the slug list
        // via `use_reactive!` so it runs exactly once per setlist (charts are
        // optional — a missing chart.kf just yields `None`). This is now DISPLAY
        // data only (navigator rows, header facts, progress-bar sections, mixer
        // strips); the setlist STRUCTURE / charts / playhead are driven by the
        // in-process engine (see the `build_for_setlist` effect below).
        let songs_r = songs.clone();
        let org_v = org.clone();
        let loaded = use_resource(use_reactive!(|songs_r, org_v| {
            let songs = songs_r.clone();
            let org = org_v.clone();
            async move {
                let mut out: Vec<LoadedSong> = Vec::with_capacity(songs.len());
                for slug in &songs {
                    // Colocated-schema-first (song.md → arrangement.md), then a
                    // legacy manifest.json — so migrated, manifest-less songs
                    // load in a setlist exactly like the single-song player.
                    let manifest = media::load_song_manifest(&org, slug).await?;
                    let tok = crate::media_grant::suffix(&org, slug).await;
                    let chart =
                        media::fetch_text(&format!("/org/{org}/media/songs/{slug}/chart.kf{tok}"))
                            .await
                            .ok()
                            .filter(|t| !t.is_empty());
                    out.push((slug.clone(), manifest, chart));
                }
                Ok::<Vec<LoadedSong>, String>(out)
            }
        }));

        // ── Stage 4b-1: build the in-process engine for THIS setlist ─────────
        // Keyed on the slug list so re-mounting the same setlist does NOT
        // rebuild; opening a different one rebuilds (dropping the old engine).
        // The engine is seeded from the setlist's real songs (manifest.json +
        // chart.kf) and hosts the setlist RPC + `#[subscribe]` streams
        // in-process. `SessionEventBridge` (mounted in the render below) folds
        // those streams into the session-ui globals (SETLIST_STRUCTURE /
        // SONG_CHARTS / ACTIVE_INDICES / SONG_TRANSPORT), so the navigator,
        // chart pane, section bars and playhead all follow the engine — no
        // local `SETLIST_STRUCTURE.write()` hydration anymore.
        {
            let songs = songs.clone();
            let org_v = org.clone();
            use_effect(use_reactive!(|songs, org_v| {
                crate::session_engine::build_for_setlist(org_v.clone(), songs.clone());
            }));
        }

        // Follow the engine: mirror `ACTIVE_INDICES` (published by the bridge)
        // into the local signals the layout renders from. Playback is
        // soft-clock (SILENT) until Stage 4b-2 adds WebRenderer audio, so
        // `position` advances only on seeks / section jumps, not smoothly.
        {
            let mut current_song = current_song;
            let mut playing = playing;
            let mut position = position;
            let audio = audio;
            use_effect(move || {
                let ai = session_ui::ACTIVE_INDICES.read();
                let si = ai.song_index.unwrap_or(0);
                if si != *current_song.peek() {
                    current_song.set(si);
                }
                if ai.is_playing != *playing.peek() {
                    playing.set(ai.is_playing);
                }
                let dur = SETLIST_STRUCTURE
                    .read()
                    .songs
                    .get(si)
                    .map(|s| s.duration())
                    .unwrap_or(0.0);
                let secs = ai.song_progress.unwrap_or(0.0) * dur;
                let prev = *position.peek();
                if (secs - prev).abs() > 0.01 {
                    position.set(secs);
                }
                // Keep the WebRenderer aligned to the engine cursor via the
                // guarded `nudge` (it re-seeks only on genuine divergence and
                // never while a user/quantized seek is in flight — the old
                // unconditional ">0.5 s ⇒ seek" cancelled bar-click seeks
                // with stale engine ticks). `audio` is read via `peek` so
                // this effect doesn't churn on a song rebuild.
                if let Some(a) = audio.peek().clone() {
                    a.nudge(secs);
                }
            });
        }

        // Reset the DISPLAY mixer strips from the current song's manifest when
        // the song changes (or the fetch first lands). No audio graph — the
        // strips are silent placeholders until Stage 4b-2.
        // STAGE 4b-2: WebRenderer AudioWorklet audio replaces the removed
        // Web-Audio graph (`media::build_engine` / `apply_mix` / meter loop).
        {
            let mut stem_ui = stem_ui;
            let current_song = current_song;
            use_effect(move || {
                let idx = current_song();
                let guard = loaded.read();
                let Some(Ok(list)) = &*guard else {
                    return;
                };
                let Some((_, manifest, _)) = list.get(idx) else {
                    return;
                };
                let v: Vec<media::StemUi> = manifest
                    .stems
                    .iter()
                    .map(|s| media::StemUi {
                        muted: s.default_muted,
                        soloed: false,
                        volume: 1.0,
                    })
                    .collect();
                stem_ui.set(v);
            });
        }

        // ── ONE SetlistAudio per SETLIST (the native engine's layout): every
        // song is a project inside the shared worklet renderer, seeded up
        // front; a song switch SELECTS its project + swaps the decoded PCM —
        // the graph / node / AudioContext are never torn down mid-set.
        {
            let mut audio = audio;
            let current_song = current_song;
            let loaded = loaded;
            // The slug list the live renderer was built for — only a
            // DIFFERENT setlist rebuilds.
            let mut built_key = use_signal(String::new);
            let playing = playing;
            use_effect(move || {
                let idx = current_song();
                let guard = loaded.read();
                let Some(Ok(list)) = &*guard else {
                    return;
                };
                if list.is_empty() {
                    return;
                }
                let key = list
                    .iter()
                    .map(|(s, _, _)| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\u{1f}");
                if *built_key.peek() != key || audio.peek().is_none() {
                    audio.set(None); // drop a previous setlist's graph first
                    let songs: Vec<(String, media::Manifest)> = list
                        .iter()
                        .map(|(s, m, _)| (s.clone(), m.clone()))
                        .collect();
                    match SetlistAudio::build(&org, &songs) {
                        Ok(a) => {
                            audio.set(Some(Rc::new(a)));
                            built_key.set(key);
                        }
                        Err(e) => {
                            tracing::warn!("setlist audio: build failed: {e}");
                            return;
                        }
                    }
                }
                // Song switch (or first select): pick the project inside the
                // shared renderer; keep rolling if the set was playing.
                if let Some(a) = audio.peek().clone() {
                    a.select_song(idx.min(list.len() - 1));
                    if *playing.peek() {
                        a.play();
                    }
                }
            });
        }

        // ~20 fps meter pump: publish per-stem peaks from the render graph's
        // meter cells (written by `render_block` on the audio pump) into
        // `SETLIST_LEVELS` while playing; clear them when stopped. Only
        // `MeteredMixer` subscribes, so this re-renders the meters alone.
        {
            let audio = audio;
            use_future(move || async move {
                loop {
                    architect::platform::sleep(std::time::Duration::from_millis(45)).await;
                    let playing_peaks = audio
                        .peek()
                        .clone()
                        .filter(|a| a.is_playing())
                        .map(|a| a.peaks());
                    match playing_peaks {
                        Some(peaks) => *SETLIST_LEVELS.write() = peaks,
                        None => {
                            if !SETLIST_LEVELS.peek().is_empty() {
                                *SETLIST_LEVELS.write() = Vec::new();
                            }
                        }
                    }
                }
            });
        }

        // ── transport actions — command the in-process engine over RPC ─────────
        // The UI commands the engine; the resulting state flows back through
        // `SessionEventBridge` (ACTIVE_INDICES → the follow-effect above). We
        // also nudge the local signals optimistically so the UI is snappy.
        // STAGE 4b-2: real audio playback replaces the (silent) soft clock.
        let np_ctl = use_context::<crate::now_playing::NowPlayingCtl>();
        let play_pause: Callback<()> = use_callback({
            let mut playing = playing;
            let audio = audio;
            move |()| {
                let want = !*playing.peek();
                playing.set(want); // optimistic; the bridge confirms
                // Stage 4b-2: drive the WebRenderer transport in lockstep with
                // the engine RPC. The Play click is the required audio user
                // gesture, so `play()` resumes the AudioContext.
                if let Some(a) = audio.peek().clone() {
                    if want {
                        // Starting the multitrack rig → stop the global Now
                        // Playing stream so they don't play over each other.
                        // (Merely opening the player doesn't do this — only Play.)
                        let mut cmd = np_ctl.cmd;
                        let g = cmd.peek().0 + 1;
                        cmd.set((g, crate::now_playing::NpCmd::Pause));
                        a.play();
                    } else {
                        a.pause();
                    }
                }
                if let Some(client) = crate::session_engine::client() {
                    spawn(async move {
                        if let Err(e) = client.toggle_playback().await {
                            tracing::warn!("setlist: toggle_playback failed: {e:?}");
                        }
                    });
                }
            }
        });
        // Arbitrary-seconds seek (progress-bar clicks, measure clicks,
        // back/forward): song-relative seconds → the engine's `seek_to` RPC
        // (so the cursor pump republishes the new position) + the WebRenderer
        // transport in lockstep + an optimistic local playhead nudge.
        let seek: Callback<f64> = use_callback({
            let mut position = position;
            let audio = audio;
            let current_song = current_song;
            move |off: f64| {
                position.set(off); // optimistic; the bridge confirms
                if let Some(a) = audio.peek().clone() {
                    a.seek(off);
                }
                // HARD INVARIANT: a progress-bar / measure seek stays in the
                // CURRENTLY ACTIVE song. Pin the song index captured NOW and
                // command the song-indexed `seek_to_time` (never the
                // position-only `seek_to`, which resolves the active song at
                // apply time and can drift to another song), clamped strictly
                // inside that song so it can't spill onto a neighbour.
                let idx = *current_song.peek();
                let off = clamp_into_song(idx, off);
                if let Some(client) = crate::session_engine::client() {
                    spawn(async move {
                        if let Err(e) = client.seek_to_time(idx, off).await {
                            tracing::warn!("setlist: seek_to_time({idx}, {off:.2}) failed: {e:?}");
                        }
                    });
                }
            }
        });
        // QUANTIZED seek `(target, boundary)` — both in song seconds. The
        // worklet jumps ON the boundary (audio-thread accurate); the engine's
        // `seek_to` is scheduled to land at the same moment so the UI cursor
        // follows without fighting the audio (see `SetlistAudio::nudge`).
        let seek_quantized: Callback<(f64, f64)> = use_callback({
            let audio = audio;
            let position = position;
            let current_song = current_song;
            move |(target, boundary): (f64, f64)| {
                let delay = (boundary - *position.peek()).max(0.0);
                if let Some(a) = audio.peek().clone() {
                    a.seek_quantized(target, boundary, delay);
                }
                // Pin the song NOW: the engine apply is scheduled after `delay`,
                // during which the set could auto-advance to the next song. The
                // song-indexed `seek_to_time` re-selects THIS song and lands the
                // target inside it — so a queued section jump can never resolve
                // onto whatever song happens to be active when the timer fires.
                let idx = *current_song.peek();
                let target = clamp_into_song(idx, target);
                if let Some(client) = crate::session_engine::client() {
                    spawn(async move {
                        architect::platform::sleep(std::time::Duration::from_secs_f64(delay)).await;
                        if let Err(e) = client.seek_to_time(idx, target).await {
                            tracing::warn!(
                                "setlist: quantized seek_to_time({idx}, {target:.2}) failed: {e:?}"
                            );
                        }
                    });
                }
            }
        });

        // ── mixer mutators (by stem index) ───────────────────────────────────
        // They only write `stem_ui`; the effect below pushes the whole mixer
        // state into the worklet renderer, so every path (strip buttons, guide
        // toggle, faders) is audible through one door.
        let toggle_mute: Callback<usize> = use_callback({
            let mut stem_ui = stem_ui;
            move |i: usize| {
                let mut ui = stem_ui();
                if let Some(s) = ui.get_mut(i) {
                    s.muted = !s.muted;
                }
                stem_ui.set(ui);
            }
        });
        let toggle_solo: Callback<usize> = use_callback({
            let mut stem_ui = stem_ui;
            move |i: usize| {
                let mut ui = stem_ui();
                if let Some(s) = ui.get_mut(i) {
                    s.soloed = !s.soloed;
                }
                stem_ui.set(ui);
            }
        });
        let set_volume: Callback<(usize, f32)> = use_callback({
            let mut stem_ui = stem_ui;
            move |(i, v): (usize, f32)| {
                let mut ui = stem_ui();
                if let Some(s) = ui.get_mut(i) {
                    s.volume = v;
                }
                stem_ui.set(ui);
            }
        });
        let set_mutes: Callback<(Vec<usize>, bool)> = use_callback({
            let mut stem_ui = stem_ui;
            move |(idxs, muted): (Vec<usize>, bool)| {
                let mut ui = stem_ui();
                for i in idxs {
                    if let Some(s) = ui.get_mut(i) {
                        s.muted = muted;
                    }
                }
                stem_ui.set(ui);
            }
        });

        // Push the FULL mixer state into the worklet renderer whenever it
        // changes or a new song's graph comes up (both reads are reactive).
        // `apply_mix` queues while the pump is still attaching, so the
        // manifest's default-muted stems (click / guide) never sound.
        {
            let audio = audio;
            let stem_ui = stem_ui;
            use_effect(move || {
                let ui = stem_ui();
                let Some(a) = audio.read().clone() else {
                    return;
                };
                a.apply_mix(
                    ui.iter().map(|s| s.muted).collect(),
                    ui.iter().map(|s| s.soloed).collect(),
                    ui.iter().map(|s| s.volume).collect(),
                );
            });
        }

        // ── set navigation: pick / prev / next a whole song → engine RPC ───────
        let goto_song: Callback<usize> = use_callback({
            let mut current_song = current_song;
            let loaded = loaded;
            move |i: usize| {
                let count = match &*loaded.read_unchecked() {
                    Some(Ok(list)) => list.len(),
                    _ => 0,
                };
                if count == 0 {
                    return;
                }
                let i = i.min(count - 1);
                if i != *current_song.peek() {
                    current_song.set(i); // optimistic; the bridge confirms
                }
                if let Some(client) = crate::session_engine::client() {
                    spawn(async move {
                        if let Err(e) = client.seek_to_song(i).await {
                            tracing::warn!("setlist: seek_to_song({i}) failed: {e:?}");
                        }
                    });
                }
            }
        });

        // ── render ─────────────────────────────────────────────────────────────
        let idx = current_song();
        let body = match &*loaded.read_unchecked() {
            None => rsx! {
                div { class: "flex flex-col gap-2 py-10",
                    span { class: "text-sm text-muted-foreground", "Loading setlist…" }
                }
            },
            Some(Err(msg)) => rsx! {
                div { class: "flex flex-col gap-2 py-10",
                    span { class: "text-sm font-semibold text-destructive", "Could not load setlist" }
                    span { class: "text-sm text-muted-foreground", "{msg}" }
                }
            },
            Some(Ok(list)) if list.is_empty() => rsx! {
                div { class: "flex flex-col gap-2 py-10",
                    span { class: "text-sm text-muted-foreground",
                        "This setlist has no songs. Add a `songs:` list to the note frontmatter."
                    }
                }
            },
            Some(Ok(list)) => {
                let songs_meta: Vec<SongMeta> = list
                    .iter()
                    .map(|(slug, m, _)| SongMeta {
                        title: m.title.clone().unwrap_or_else(|| slug.clone()),
                        key: m.key.clone(),
                        bpm: m.bpm,
                        accent: media::progress_sections(m)
                            .first()
                            .map(|s| s.color.clone())
                            .unwrap_or_else(|| "#3b82f6".to_owned()),
                    })
                    .collect();
                let manifest = list
                    .get(idx.min(list.len() - 1))
                    .map(|(_, m, _)| m.clone())
                    .unwrap_or_else(|| list[0].1.clone());
                rsx! {
                    SetlistBody {
                        songs_meta,
                        manifest,
                        current_song,
                        playing,
                        position,
                        buffering,
                        stem_ui,
                        play_pause,
                        seek,
                        seek_quantized,
                        toggle_mute,
                        toggle_solo,
                        set_volume,
                        set_mutes,
                        goto_song,
                        fullscreen,
                    }
                }
            }
        };

        rsx! {
            // Stage 4b-1: fold the in-process engine's `events` /
            // `active_indices` streams into the session-ui globals
            // (SETLIST_STRUCTURE / SONG_CHARTS / ACTIVE_INDICES /
            // SONG_TRANSPORT). Renders nothing; it just runs the stream pumps.
            crate::session_engine::SessionEventBridge {}
            // Full-screen Experience: fill the overlay (no max-width / centering).
            // Embedded: the centered, capped column above the note editor.
            div {
                class: if fullscreen { "flex h-full min-h-0 w-full flex-col" } else { "mx-auto w-full max-w-6xl px-4 py-6" },
                {body}
            }
        }
    }

    /// The ready setlist player: a **navigator** of the whole set (left) beside
    /// the current song's transport + **Session / Chart** tabs (right). All the
    /// right-hand views follow the current song via the shared session-ui
    /// signals. Split out so the per-frame reactive reads (position / stem_ui)
    /// live in a child scope, away from the parent's resource/effect setup.
    #[allow(clippy::too_many_arguments)]
    #[component]
    fn SetlistBody(
        songs_meta: Vec<SongMeta>,
        manifest: media::Manifest,
        current_song: Signal<usize>,
        playing: Signal<bool>,
        position: Signal<f64>,
        buffering: Signal<bool>,
        stem_ui: Signal<Vec<media::StemUi>>,
        play_pause: Callback<()>,
        seek: Callback<f64>,
        seek_quantized: Callback<(f64, f64)>,
        toggle_mute: Callback<usize>,
        toggle_solo: Callback<usize>,
        set_volume: Callback<(usize, f32)>,
        set_mutes: Callback<(Vec<usize>, bool)>,
        goto_song: Callback<usize>,
        #[props(default)] fullscreen: bool,
    ) -> Element {
        let mut tab = use_signal(|| Tab::Chart);
        // Full-screen center: the right pane beside the chart. Default to
        // the keyflow editor (charts as code); the mixer follows later.
        let mut center_right = use_signal(|| CenterRight::Editor);
        // Second selectable right pane, shown only on ultrawide — it pulls from
        // the same option set and defaults to the Mixer, so a wide screen shows
        // chart + editor + mixer (or any two right views) at once.
        let mut center_right2 = use_signal(|| CenterRight::Mixer);
        // Full-screen center: what the LEFT (chart) pane shows.
        let mut chart_left = use_signal(|| ChartLeft::MasterRhythm);
        // Navigator sidebar open/closed (full-screen experience only).
        let mut nav_open = use_signal(|| true);
        // Target (song seconds) of a QUANTIZED seek waiting for its measure
        // boundary — drives the progress bar's queued-section pulse.
        let pending_jump = use_signal(|| None::<f64>);

        let count = songs_meta.len();
        let idx = current_song();
        // ONE timebase for the bars: the ENGINE song — the same source the
        // navigator and the `position` mirror (ACTIVE_INDICES × engine
        // duration) use. Falling back to the manifest duration while the
        // engine hydrates. Mixing the two skews every percent the bars draw.
        let duration = {
            let sl = SETLIST_STRUCTURE.read();
            sl.songs
                .get(idx)
                .map(|s| s.duration())
                .filter(|d| *d > 0.0)
                .unwrap_or(manifest.duration_sec)
                .max(0.001)
        };
        let pos = position();
        let is_playing = playing();
        let is_buffering = buffering();

        let title = manifest.title.clone().unwrap_or_default();
        let artist = manifest.artist.clone().unwrap_or_default();
        // Progress-bar segments from the current song's (chart-labelled)
        // sections, so the bars read `VS 1 A` / `CH 2 A` like the chart + the
        // navigator; fall back to the manifest's audio regions if unavailable.
        let sections = {
            let sl = SETLIST_STRUCTURE.read();
            sl.songs
                .get(idx)
                .map(media::progress_sections_from_song)
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| media::progress_sections(&manifest))
        };

        let song_progress = (pos / duration * 100.0).clamp(0.0, 100.0);

        // Guide (click + cue) stem indices, and whether the bus is on.
        let guide_idxs: Vec<usize> = manifest
            .stems
            .iter()
            .enumerate()
            .filter(|(_, s)| media::is_guide_stem(s))
            .map(|(i, _)| i)
            .collect();
        let guide_on = {
            let ui = stem_ui.read();
            guide_idxs
                .iter()
                .any(|&i| ui.get(i).map(|s| !s.muted).unwrap_or(false))
        };

        // ── session-ui mixer adapters (guid = stem file ↔ index) ──────────────
        let stems_for_lookup = manifest.stems.clone();
        let index_of = move |guid: &str| stems_for_lookup.iter().position(|s| s.file == guid);
        let mixer_volume: Callback<(String, f64)> = use_callback({
            let index_of = index_of.clone();
            move |(guid, v): (String, f64)| {
                if let Some(i) = index_of(&guid) {
                    set_volume.call((i, v as f32));
                }
            }
        });
        let mixer_mute: Callback<String> = use_callback({
            let index_of = index_of.clone();
            move |guid: String| {
                if let Some(i) = index_of(&guid) {
                    toggle_mute.call(i);
                }
            }
        });
        let mixer_solo: Callback<String> = use_callback({
            let index_of = index_of.clone();
            move |guid: String| {
                if let Some(i) = index_of(&guid) {
                    toggle_solo.call(i);
                }
            }
        });

        // ── transport bar adapters (section nav; crosses into the set at
        //     song boundaries) ──────────────────────────────────────────────
        // Section-nav targets come from the SAME `sections` the bars render
        // (engine timebase) — indexing a different list (the manifest's
        // audio-region sections) highlighted one thing and seeked another.
        let section_starts: Vec<f64> = sections
            .iter()
            .map(|s| s.start_percent / 100.0 * duration)
            .collect();
        // Tempo / meter (engine song) — measure grid anchored at song start.
        // Needed here for BOTH the quantized seek boundary and the section
        // bar's measure indicators further down.
        let (bpm, beats_per_measure, ts_denom) = {
            let sl = SETLIST_STRUCTURE.read();
            sl.songs
                .get(idx)
                .map(|s| {
                    let ts = s.time_signature.unwrap_or(TimeSignature::COMMON_TIME);
                    (
                        s.tempo.unwrap_or(120.0),
                        ts.numerator() as f64,
                        ts.denominator() as u8,
                    )
                })
                .unwrap_or((120.0, 4.0, 4))
        };
        let seconds_per_measure = (60.0 / bpm.max(1.0)) * beats_per_measure.max(1.0);
        // Section-nav seek door: while PLAYING, section jumps are QUANTIZED —
        // queued to the next measure boundary so the audio splices on the
        // grid ("wait for the bar line, then jump"). Paused (or degenerate
        // grid / end of song) seeks land immediately.
        let do_seek: Callback<f64> = use_callback({
            let position = position;
            let playing = playing;
            let mut pending_jump = pending_jump;
            move |target: f64| {
                let p = *position.peek();
                let spm = seconds_per_measure;
                if *playing.peek() && spm > 0.05 {
                    let boundary = ((p + 0.02) / spm).ceil() * spm;
                    if boundary + 0.05 < duration {
                        seek_quantized.call((target, boundary));
                        // Pulse the queued section until the jump lands.
                        pending_jump.set(Some(target));
                        let delay = (boundary - p).max(0.0) + 0.3;
                        spawn(async move {
                            architect::platform::sleep(std::time::Duration::from_secs_f64(delay))
                                .await;
                            if *pending_jump.peek() == Some(target) {
                                pending_jump.set(None);
                            }
                        });
                        return;
                    }
                }
                pending_jump.set(None);
                seek.call(target);
            }
        });
        let on_play_pause: Callback<()> = use_callback(move |()| play_pause.call(()));
        let noop: Callback<()> = use_callback(move |()| {});
        let starts_for_back = section_starts.clone();
        let on_back: Callback<()> = use_callback(move |()| {
            let p = position();
            // At the head of the song, Back steps to the previous SONG.
            if p <= 1.0 && *current_song.peek() > 0 {
                goto_song.call(*current_song.peek() - 1);
                return;
            }
            let target = starts_for_back
                .iter()
                .copied()
                .filter(|&s| s < p - 1.0)
                .fold(0.0_f64, f64::max);
            do_seek.call(target);
        });
        let starts_for_fwd = section_starts.clone();
        let on_forward: Callback<()> = use_callback(move |()| {
            let p = position();
            if let Some(next) = starts_for_fwd.iter().copied().find(|&s| s > p + 0.5) {
                do_seek.call(next);
            } else if *current_song.peek() + 1 < count {
                // Past the last section, Advance steps to the next SONG.
                goto_song.call(*current_song.peek() + 1);
            }
        });

        // Guide toggle: un/mute the click + cue stems together.
        let guide_idxs_for_toggle = guide_idxs.clone();
        let on_guide: Callback<()> = use_callback(move |()| {
            set_mutes.call((guide_idxs_for_toggle.clone(), guide_on));
        });

        // Section-click seeks to the section start — indexed into the SAME
        // list the bar renders; quantized to the measure grid while playing.
        let starts_for_click = section_starts.clone();
        let on_section_click: Callback<usize> = use_callback(move |i: usize| {
            if let Some(&s) = starts_for_click.get(i) {
                do_seek.call(s);
            }
        });

        // Navigator section click `(song_idx, section_idx)`. Same-song
        // selects go through the quantized seek door (same targets the bar
        // uses — engine sections); cross-song selects stay on the engine's
        // `seek_to_section` (it switches projects natively). The bridge
        // echoes the new cursor back into ACTIVE_INDICES.
        let starts_for_nav = section_starts.clone();
        let on_section_select: Callback<(usize, usize)> = use_callback({
            let mut current_song = current_song;
            move |(sidx, secidx): (usize, usize)| {
                if sidx == *current_song.peek() {
                    if let Some(&s) = starts_for_nav.get(secidx) {
                        do_seek.call(s);
                        return;
                    }
                }
                if sidx != *current_song.peek() {
                    current_song.set(sidx); // optimistic
                }
                if let Some(client) = crate::session_engine::client() {
                    spawn(async move {
                        if let Err(e) = client.seek_to_section(sidx, secidx).await {
                            tracing::warn!(
                                "setlist: seek_to_section({sidx}, {secidx}) failed: {e:?}"
                            );
                        }
                    });
                }
            }
        });

        let tracks = media::stems_to_tracks(&manifest, &stem_ui.read());
        // Does this song carry a guide/click bus? (Drives the mixer's guide
        // toggle, shown in both the tabbed and the ultrawide mixer panes.)
        let guide_present = !guide_idxs.is_empty();
        let active = tab();
        let at_first = idx == 0;
        let at_last = idx + 1 >= count;

        // The active song's accent (its first section color) is the panel's
        // accent — reused for the title rule, the navigator selection, and the
        // playing indicators so the whole panel reads as one color system.
        let accent = songs_meta
            .get(idx)
            .map(|s| s.accent.clone())
            .unwrap_or_else(|| "#3b82f6".to_owned());
        let prev_title = idx
            .checked_sub(1)
            .and_then(|i| songs_meta.get(i))
            .map(|s| s.title.clone());
        let next_title = songs_meta.get(idx + 1).map(|s| s.title.clone());
        // A quantized seek waiting for its boundary → the bar pulses the
        // queued section (matched by start seconds against the SAME list the
        // bar renders).
        let queued_target = pending_jump().and_then(|t| {
            let sec_idx = section_starts.iter().position(|&s| (s - t).abs() < 0.05)?;
            let song_id = SETLIST_STRUCTURE.read().songs.get(idx)?.id.clone();
            Some(session_proto::QueuedTarget::Section {
                song_id,
                song_index: idx,
                section_index: sec_idx,
            })
        });

        // The section the playhead is in — the "where am I" caption.
        let cur_section_name = sections
            .iter()
            .find(|s| song_progress >= s.start_percent && song_progress < s.end_percent)
            .or_else(|| sections.last())
            .map(|s| s.name.clone());
        let time_str = format!("{} / {}", duration_mmss(pos), duration_mmss(duration));
        let progress_clamped = song_progress.clamp(0.0, 100.0);

        // Progress WITHIN the current section (0–100) for the section bar.
        let section_progress = sections
            .iter()
            .find(|s| song_progress >= s.start_percent && song_progress < s.end_percent)
            .map(|s| {
                let w = (s.end_percent - s.start_percent).max(0.001);
                ((song_progress - s.start_percent) / w * 100.0).clamp(0.0, 100.0)
            })
            .unwrap_or(0.0);

        // Divide the current section into measures for the section bar — each is
        // clickable to seek. `musical_position.measure` carries the measure's
        // index within the section; the click seeks to its absolute time.
        // (Tempo / seconds_per_measure computed above, beside the seek door.)
        let cur_section = sections
            .iter()
            .find(|s| song_progress >= s.start_percent && song_progress < s.end_percent)
            .or_else(|| sections.first());
        let (measure_indicators, measure_section_start) = if let Some(sec) = cur_section {
            let start_sec = sec.start_percent / 100.0 * duration;
            let sec_dur = ((sec.end_percent - sec.start_percent) / 100.0 * duration).max(0.001);
            let n = (sec_dur / seconds_per_measure).round().max(1.0) as usize;
            // Absolute measure number at the section's start (measure 1 = song start).
            let first = (start_sec / seconds_per_measure).round() as i32;
            let inds = (0..n)
                .map(|i| MeasureIndicator {
                    position_percent: (i as f64 * seconds_per_measure / sec_dur * 100.0).min(100.0),
                    measure_number: first + i as i32 + 1,
                    time_signature: Some((beats_per_measure as u8, ts_denom)),
                    musical_position: MusicalPosition::new(i as i32, 0, 0),
                })
                .collect::<Vec<_>>();
            (inds, start_sec)
        } else {
            (Vec::new(), 0.0)
        };
        let on_measure_click: Callback<MusicalPosition> =
            use_callback(move |mp: MusicalPosition| {
                seek.call(measure_section_start + mp.measure as f64 * seconds_per_measure);
            });

        // ── Full-screen Experience layout ────────────────────────────────────
        // Progress on top, playlist navigator on the left, a switchable
        // Chart + (Mixer | Comments) center, and the transport pinned to the
        // bottom. Reuses every adapter above; only the arrangement differs.
        if fullscreen {
            let right = center_right();
            // The current song's ORIGINAL chart text + its guid. The editor
            // seeds from `chart_src` (the song's own `chart_text`, NOT the
            // live `SONG_CHARTS` it writes into) so pushing edits back for the
            // live engrave can't loop into a re-seed. Reactive read → this
            // re-resolves on song switch / hydration, flipping the remount key.
            let (chart_src, chart_guid) = {
                let sl = SETLIST_STRUCTURE.read();
                sl.songs
                    .get(idx)
                    .map(|s| {
                        (
                            s.chart_text.clone().unwrap_or_default(),
                            s.project_guid.clone(),
                        )
                    })
                    .unwrap_or_default()
            };
            let chart_present = !chart_src.trim().is_empty();
            return rsx! {
                div { class: "flex h-full min-h-0 flex-col",

                    // TOP — current section caption + time, then the song and
                    // section progress bars.
                    div { class: "shrink-0 border-b border-border px-5 py-2.5",
                        div { class: "mb-2 flex items-center justify-between gap-3",
                            div { class: "flex min-w-0 items-center gap-2.5",
                                div { class: "h-6 w-1 shrink-0 rounded-full", style: format!("background:{accent};") }
                                h1 { class: "truncate text-lg font-bold tracking-tight text-foreground", "{title}" }
                                if let Some(name) = cur_section_name.clone() {
                                    span { class: "shrink-0 text-xs font-semibold uppercase tracking-[0.1em] text-muted-foreground",
                                        "{name}"
                                    }
                                }
                            }
                            span { class: "shrink-0 text-xs font-medium tabular-nums text-muted-foreground", "{time_str}" }
                        }
                        if !sections.is_empty() {
                            div { class: "flex flex-col gap-1.5",
                                SongProgressBar {
                                    progress: song_progress,
                                    sections: sections.clone(),
                                    song_key: manifest.key.clone(),
                                    queued_target: queued_target.clone(),
                                    on_section_click,
                                }
                                SectionProgressBar {
                                    progress: section_progress,
                                    sections: sections.clone(),
                                    song_key: manifest.key.clone(),
                                    measure_indicators: measure_indicators.clone(),
                                    on_measure_click: Some(on_measure_click),
                                }
                            }
                        }
                    }

                    // MIDDLE — navigator (left) + Chart/right center.
                    div { class: "flex min-h-0 flex-1",

                        // LEFT — the session-ui setlist navigator (collapsible):
                        // every song with its live section-progress strip,
                        // driven by ACTIVE_INDICES / SETLIST_STRUCTURE. Hidden
                        // for a single song (a one-song set — the song page).
                        if count > 1 {
                        if nav_open() {
                            aside { class: "flex w-64 shrink-0 flex-col border-r border-border",
                                div { class: "flex shrink-0 items-center justify-between border-b border-border px-3 py-1.5",
                                    span { class: "text-[11px] font-semibold uppercase tracking-[0.14em] text-muted-foreground", "Setlist" }
                                    button {
                                        class: "rounded px-1 text-muted-foreground hover:bg-accent hover:text-foreground",
                                        title: "Hide navigator",
                                        onclick: move |_| nav_open.set(false),
                                        "‹"
                                    }
                                }
                                div { class: "min-h-0 flex-1 overflow-y-auto",
                                    PerformanceSidebar {
                                        on_song_select: goto_song,
                                        on_section_select,
                                        plain_selection: true,
                                    }
                                }
                            }
                        } else {
                            button {
                                class: "flex w-8 shrink-0 items-center justify-center border-r border-border text-muted-foreground hover:bg-accent hover:text-foreground",
                                title: "Show navigator",
                                onclick: move |_| nav_open.set(true),
                                "›"
                            }
                        }
                        }

                        // CENTER — the chart on the LEFT (always), a selectable
                        // pane on the right (its selector sits on the RIGHT), and
                        // — on ultrawide — a dedicated Mixer column so the desk
                        // sits beside the editor instead of hiding behind its tab.
                        div { class: "flex min-h-0 flex-1",

                            // LEFT — the chart, with its own view switcher (the
                            // engraved Master Rhythm chart today; a lyric scroller
                            // and per-part views land here later).
                            div { class: "flex min-w-0 flex-1 flex-col overflow-hidden border-r border-border bg-background",
                                div { class: "flex shrink-0 items-center justify-between gap-2 border-b border-border px-3 py-1.5",
                                    // Chart-view switcher (left).
                                    div { class: "flex items-center gap-1",
                                        for (v , label , enabled) in [
                                            (ChartLeft::MasterRhythm, "Master Rhythm", true),
                                            (ChartLeft::Lyrics, "Lyrics", false),
                                        ] {
                                            button {
                                                key: "{label}",
                                                disabled: !enabled,
                                                class: if chart_left() == v {
                                                    "rounded px-2 py-0.5 text-xs font-medium bg-accent text-foreground"
                                                } else if enabled {
                                                    "rounded px-2 py-0.5 text-xs text-muted-foreground hover:text-foreground"
                                                } else {
                                                    "rounded px-2 py-0.5 text-xs text-muted-foreground/40"
                                                },
                                                onclick: move |_| if enabled { chart_left.set(v); },
                                                "{label}"
                                            }
                                        }
                                    }
                                    // Key / notation / capo selector (right).
                                    crate::session_chart_pane::KeyBar {}
                                }
                                div { class: "min-h-0 flex-1 overflow-hidden",
                                    {match chart_left() {
                                        ChartLeft::MasterRhythm => rsx! { SessionChartPane {} },
                                        ChartLeft::Lyrics => rsx! {
                                            div { class: "flex h-full items-center justify-center p-6 text-center text-sm text-muted-foreground",
                                                "Lyric scroller — coming soon. A big, singer-friendly lyric view synced to the playhead."
                                            }
                                        },
                                    }}
                                }
                            }

                            // RIGHT — one selectable pane always; a SECOND appears
                            // on ultrawide. Both pull from the same option set (the
                            // selector sits on the RIGHT), so a wide screen shows
                            // chart + editor + mixer at once — the second pane
                            // defaults to the mixer. Below the breakpoint the second
                            // pane is hidden and everything lives behind pane one.
                            RightPane {
                                root_class: "flex min-w-0 flex-1 flex-col overflow-hidden border-r border-border bg-card",
                                selected: right,
                                on_select: move |v| center_right.set(v),
                                editor_key: format!("a-{idx}-{chart_present}"),
                                source: chart_src.clone(),
                                guid: chart_guid.clone(),
                                tracks: tracks.clone(),
                                on_volume: mixer_volume,
                                on_mute: mixer_mute,
                                on_solo: mixer_solo,
                                guide_present,
                                guide_on,
                                on_guide,
                            }
                            RightPane {
                                root_class: "hidden min-w-0 flex-1 flex-col overflow-hidden bg-card min-[1700px]:flex",
                                selected: center_right2(),
                                on_select: move |v| center_right2.set(v),
                                editor_key: format!("b-{idx}-{chart_present}"),
                                source: chart_src.clone(),
                                guid: chart_guid.clone(),
                                tracks: tracks.clone(),
                                on_volume: mixer_volume,
                                on_mute: mixer_mute,
                                on_solo: mixer_solo,
                                guide_present,
                                guide_on,
                                on_guide,
                            }
                        }
                    }

                    // BOTTOM — prev / transport / next, pinned. Prev/next are
                    // hidden for a single song (a one-song set — the song page).
                    div { class: "flex shrink-0 items-center gap-3 border-t border-border px-4 py-2",
                        if count > 1 {
                        button {
                            class: "flex min-w-0 max-w-[16rem] flex-1 items-center gap-2 rounded-lg border border-border px-3 py-1.5 text-left hover:bg-accent disabled:opacity-40",
                            disabled: at_first,
                            onclick: {
                                let goto_song = goto_song;
                                move |_| if idx > 0 { goto_song.call(idx - 1) }
                            },
                            span { class: "text-lg leading-none text-muted-foreground", "‹" }
                            span { class: "flex min-w-0 flex-col",
                                span { class: "text-[10px] font-semibold uppercase tracking-wide text-muted-foreground", "Prev" }
                                span { class: "truncate text-sm font-medium text-foreground", {prev_title.clone().unwrap_or_else(|| "—".to_owned())} }
                            }
                        }
                        }
                        div { class: "h-14 flex-[2] overflow-hidden rounded-lg border border-border",
                            TransportControlBar {
                                is_playing,
                                is_looping: false,
                                is_recording: false,
                                is_armed: false,
                                show_recording: false,
                                on_play_pause,
                                on_loop_toggle: noop,
                                on_record_toggle: noop,
                                on_arm_toggle: noop,
                                on_back,
                                on_forward,
                            }
                        }
                        if count > 1 {
                        button {
                            class: "flex min-w-0 max-w-[16rem] flex-1 items-center justify-end gap-2 rounded-lg border border-border px-3 py-1.5 text-right hover:bg-accent disabled:opacity-40",
                            disabled: at_last,
                            onclick: {
                                let goto_song = goto_song;
                                move |_| if idx + 1 < count { goto_song.call(idx + 1) }
                            },
                            span { class: "flex min-w-0 flex-col items-end",
                                span { class: "text-[10px] font-semibold uppercase tracking-wide text-muted-foreground", "Next" }
                                span { class: "truncate text-sm font-medium text-foreground", {next_title.clone().unwrap_or_else(|| "—".to_owned())} }
                            }
                            span { class: "text-lg leading-none text-muted-foreground", "›" }
                        }
                        }
                    }
                }
            };
        }

        rsx! {
            div { class: "flex flex-col gap-4 md:flex-row md:gap-5",

                // ── Setlist navigator: every song visible + scannable ─────────
                // Hidden for a single song (a one-song set — e.g. the standalone
                // song page): no navigator to scan.
                if count > 1 {
                aside { class: "shrink-0 md:w-64",
                    div { class: "rounded-xl border border-border bg-card overflow-hidden",
                        div { class: "flex items-baseline justify-between px-3 pt-3 pb-2",
                            span {
                                class: "text-[11px] font-semibold uppercase tracking-[0.14em] text-muted-foreground",
                                "Setlist"
                            }
                            span { class: "text-[11px] tabular-nums text-muted-foreground", "{count} songs" }
                        }
                        // Horizontal chips on a narrow pane; a vertical list on md+.
                        div {
                            class: "flex gap-1.5 overflow-x-auto px-2 pb-2 md:flex-col md:gap-0.5 md:overflow-x-visible md:overflow-y-auto md:max-h-[60vh]",
                            for (i , s) in songs_meta.iter().enumerate() {
                                {
                                    let is_cur = i == idx;
                                    let goto = goto_song;
                                    let facts = {
                                        let mut parts: Vec<String> = Vec::new();
                                        if let Some(k) = &s.key {
                                            parts.push(k.clone());
                                        }
                                        if let Some(b) = s.bpm {
                                            parts.push(format!("{b:.0} bpm"));
                                        }
                                        parts.join(" · ")
                                    };
                                    rsx! {
                                        div { key: "{i}", class: "shrink-0 md:w-full",
                                            button {
                                                r#type: "button",
                                                class: if is_cur {
                                                    "flex w-full items-center gap-2.5 rounded-lg border-l-2 px-2.5 py-2 text-left min-w-[9.5rem] md:min-w-0 transition-colors"
                                                } else {
                                                    "flex w-full items-center gap-2.5 rounded-lg border-l-2 border-transparent px-2.5 py-2 text-left min-w-[9.5rem] md:min-w-0 hover:bg-accent transition-colors"
                                                },
                                                style: if is_cur {
                                                    format!("border-color:{a}; background:{a}14;", a = s.accent)
                                                } else {
                                                    String::new()
                                                },
                                                onclick: move |_| goto.call(i),
                                                // Index / color badge
                                                span {
                                                    class: if is_cur {
                                                        "flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-[11px] font-bold tabular-nums text-white"
                                                    } else {
                                                        "flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-[11px] font-bold tabular-nums bg-muted text-muted-foreground"
                                                    },
                                                    style: if is_cur { format!("background:{};", s.accent) } else { String::new() },
                                                    "{i + 1}"
                                                }
                                                // Title + key/bpm caption
                                                span { class: "flex min-w-0 flex-1 flex-col",
                                                    span {
                                                        class: if is_cur {
                                                            "truncate text-sm font-semibold text-foreground"
                                                        } else {
                                                            "truncate text-sm font-medium text-foreground/80"
                                                        },
                                                        "{s.title}"
                                                    }
                                                    if !facts.is_empty() {
                                                        span {
                                                            class: "truncate text-[10px] tabular-nums text-muted-foreground",
                                                            "{facts}"
                                                        }
                                                    }
                                                }
                                                // Playing pulse (only the loaded/current song plays)
                                                if is_cur && is_playing {
                                                    span {
                                                        class: "ml-auto h-2 w-2 shrink-0 rounded-full animate-pulse",
                                                        style: format!("background:{};", s.accent),
                                                    }
                                                }
                                            }
                                            // Slim section strip for the active song — a glanceable
                                            // structure preview + playhead, NOT the tall boxes.
                                            if is_cur && !sections.is_empty() {
                                                div { class: "mt-1 mb-1 px-2.5",
                                                    div { class: "relative h-1.5 w-full overflow-hidden rounded-full bg-muted",
                                                        for (si , seg) in sections.iter().enumerate() {
                                                            div {
                                                                key: "{si}",
                                                                class: "absolute inset-y-0",
                                                                style: format!(
                                                                    "left:{}%; width:{}%; background:{};",
                                                                    seg.start_percent,
                                                                    seg.end_percent - seg.start_percent,
                                                                    seg.color,
                                                                ),
                                                            }
                                                        }
                                                        // Dim the not-yet-played remainder (theme-adaptive).
                                                        div {
                                                            class: "absolute inset-y-0 rounded-r-full",
                                                            style: format!(
                                                                "left:{p}%; width:{}%; background:color-mix(in oklch, var(--card) 55%, transparent);",
                                                                100.0 - progress_clamped,
                                                                p = progress_clamped,
                                                            ),
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                }

                // ── Current song: the hero — header, timeline, transport, tabs ─
                div { class: "flex min-w-0 flex-1 flex-col gap-4",

                    // Header: accent rule + title + quiet meta caption.
                    div { class: "flex items-start gap-3",
                        div {
                            class: "mt-1 h-9 w-1 shrink-0 rounded-full",
                            style: format!("background:{accent};"),
                        }
                        div { class: "min-w-0 flex-1",
                            div { class: "flex flex-wrap items-baseline gap-x-2",
                                h1 { class: "text-2xl font-bold leading-tight tracking-tight text-foreground truncate",
                                    "{title}"
                                }
                                if !artist.is_empty() {
                                    span { class: "text-sm text-muted-foreground truncate", "{artist}" }
                                }
                            }
                            div {
                                class: "mt-1.5 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-[11px] font-semibold uppercase tracking-[0.1em] text-muted-foreground",
                                if count > 1 {
                                    span { "Song {idx + 1} / {count}" }
                                    span { class: "text-border", "·" }
                                }
                                if let Some(k) = manifest.key.as_ref() {
                                    span { "Key {k}" }
                                }
                                if let Some(b) = manifest.bpm {
                                    span { class: "text-border", "·" }
                                    span { "{b:.0} BPM" }
                                }
                                if let Some(ts) = manifest.time_signature.as_ref() {
                                    span { class: "text-border", "·" }
                                    span { "{ts}" }
                                }
                                if is_buffering {
                                    span { class: "text-border", "·" }
                                    span { class: "normal-case tracking-normal text-muted-foreground/70", "buffering…" }
                                }
                            }
                        }
                    }

                    // Section timeline — the "where am I" hero. Caption above
                    // names the current section and the elapsed / total time.
                    if !manifest.sections.is_empty() {
                        div { class: "flex flex-col gap-2",
                            div { class: "flex items-center justify-between gap-2",
                                span { class: "truncate text-xs font-semibold uppercase tracking-[0.08em] text-foreground",
                                    {cur_section_name.clone().unwrap_or_default()}
                                }
                                span { class: "shrink-0 text-[11px] font-medium tabular-nums text-muted-foreground",
                                    "{time_str}"
                                }
                            }
                            SongProgressBar {
                                progress: song_progress,
                                sections: sections.clone(),
                                song_key: manifest.key.clone(),
                                queued_target: queued_target.clone(),
                                on_section_click,
                            }
                        }
                    }

                    // Transport (full-width, compact so the six controls fit) +
                    // whole-song prev/next below it.
                    div { class: "flex flex-col gap-2",
                        div { class: "h-14 rounded-lg overflow-hidden border border-border",
                            TransportControlBar {
                                is_playing,
                                is_looping: false,
                                is_recording: false,
                                is_armed: false,
                                on_play_pause,
                                on_loop_toggle: noop,
                                on_record_toggle: noop,
                                on_arm_toggle: noop,
                                on_back,
                                on_forward,
                                compact: true,
                            }
                        }
                        if count > 1 {
                        div { class: "grid grid-cols-2 gap-2",
                            button {
                                class: "flex items-center gap-2 rounded-lg border border-border px-3 py-1.5 text-left hover:bg-accent disabled:opacity-40 transition-colors",
                                disabled: at_first,
                                onclick: {
                                    let goto_song = goto_song;
                                    move |_| if idx > 0 { goto_song.call(idx - 1) }
                                },
                                span { class: "text-lg leading-none text-muted-foreground", "‹" }
                                span { class: "flex min-w-0 flex-col",
                                    span { class: "text-[10px] font-semibold uppercase tracking-wide text-muted-foreground", "Prev" }
                                    span { class: "truncate text-sm font-medium text-foreground",
                                        {prev_title.clone().unwrap_or_else(|| "—".to_owned())}
                                    }
                                }
                            }
                            button {
                                class: "flex items-center justify-end gap-2 rounded-lg border border-border px-3 py-1.5 text-right hover:bg-accent disabled:opacity-40 transition-colors",
                                disabled: at_last,
                                onclick: {
                                    let goto_song = goto_song;
                                    move |_| if idx + 1 < count { goto_song.call(idx + 1) }
                                },
                                span { class: "flex min-w-0 flex-col items-end",
                                    span { class: "text-[10px] font-semibold uppercase tracking-wide text-muted-foreground", "Next" }
                                    span { class: "truncate text-sm font-medium text-foreground",
                                        {next_title.clone().unwrap_or_else(|| "—".to_owned())}
                                    }
                                }
                                span { class: "text-lg leading-none text-muted-foreground", "›" }
                            }
                        }
                        }
                    }

                    // ── Session / Chart tab switcher ─────────────────────────
                    div { class: "flex items-center gap-1 border-b border-border",
                        button {
                            class: if active == Tab::Session {
                                "px-4 py-2 text-sm font-semibold border-b-2 border-primary text-foreground"
                            } else {
                                "px-4 py-2 text-sm font-semibold border-b-2 border-transparent text-muted-foreground hover:text-foreground transition-colors"
                            },
                            onclick: move |_| tab.set(Tab::Session),
                            "Session"
                        }
                        button {
                            class: if active == Tab::Chart {
                                "px-4 py-2 text-sm font-semibold border-b-2 border-primary text-foreground"
                            } else {
                                "px-4 py-2 text-sm font-semibold border-b-2 border-transparent text-muted-foreground hover:text-foreground transition-colors"
                            },
                            onclick: move |_| tab.set(Tab::Chart),
                            "Chart"
                        }
                    }

                    // Both tabs stay mounted (block/hidden) so audio + mixer
                    // state persist across tab switches.

                    // Session tab — per-stem mixer + the Guide/click toggle.
                    div { class: if active == Tab::Session { "flex flex-col gap-3" } else { "hidden" },
                        if !guide_idxs.is_empty() {
                            div { class: "flex items-center gap-3 p-3 border border-border rounded-lg bg-card",
                                span { class: "text-sm font-semibold text-foreground flex-1", "Guide / Click" }
                                button {
                                    class: if guide_on {
                                        "px-4 py-1.5 rounded-md text-sm font-semibold bg-primary text-primary-foreground hover:bg-primary/90 transition-colors"
                                    } else {
                                        "px-4 py-1.5 rounded-md text-sm font-semibold bg-muted text-muted-foreground hover:bg-accent transition-colors"
                                    },
                                    onclick: move |_| on_guide.call(()),
                                    if guide_on { "On" } else { "Off" }
                                }
                            }
                        }
                        div { class: "h-56 rounded-lg overflow-hidden border border-border bg-card",
                            MixerView {
                                tracks,
                                on_volume: mixer_volume,
                                on_mute: mixer_mute,
                                on_solo: mixer_solo,
                            }
                        }
                    }

                    // Chart tab — follows the active song index for free.
                    div { class: if active == Tab::Chart { "block" } else { "hidden" },
                        div { class: "border border-border rounded-lg overflow-hidden bg-card",
                            SessionChartPane {}
                        }
                    }
                }
            }
        }
    }

    /// Clamp a song-relative seek target strictly INSIDE song `idx` — a 10 ms
    /// end margin keeps it off the boundary (on a shared timeline song N's end
    /// == song N+1's start, and landing exactly there re-indexes to the next
    /// song). If the duration isn't known yet the target is only floored at 0;
    /// the song-indexed `seek_to_time` still pins the song, so it can't cross.
    fn clamp_into_song(idx: usize, target: f64) -> f64 {
        let dur = SETLIST_STRUCTURE
            .read()
            .songs
            .get(idx)
            .map(|s| s.duration())
            .unwrap_or(0.0);
        if dur > 0.0 {
            target.clamp(0.0, (dur - 0.01).max(0.0))
        } else {
            target.max(0.0)
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use imp::SetlistPlayer;

// ─────────────────────────────────────────────────────────────────────────────
// Non-wasm: a stub so the crate still compiles on native.
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(not(target_arch = "wasm32"))]
mod stub {
    use dioxus::prelude::*;

    #[component]
    pub fn SetlistPlayer(
        songs: Vec<String>,
        org: String,
        #[props(default)] fullscreen: bool,
    ) -> Element {
        let _ = (&songs, fullscreen);
        rsx! {
            div { class: "mx-auto max-w-3xl px-4 py-10",
                span { class: "text-sm text-muted-foreground",
                    "The setlist session player runs in the browser."
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use stub::SetlistPlayer;
