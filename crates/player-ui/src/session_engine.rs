//! Stage 4a — the in-**browser** session engine.
//!
//! The wasm sibling of `apps/fasttrackstudio/src/session_engine.rs`: it
//! builds a headless `daw-standalone` setlist player entirely inside the
//! browser tab — no server, no REAPER, no cpal — so the setlist becomes
//! DATA the page can eventually PLAY. This is the payoff of the architect
//! framework work in this branch: `architect::LocalServer` +
//! `Services::into_router()` now compile for wasm, so the `!Send`
//! single-threaded `Standalone` backend can host its own RPC in-process
//! over architect's in-memory `Link`.
//!
//! Construction mirrors the native engine's `bootstrap()`, trimmed to the
//! browser reality:
//!
//! 1. `Standalone::new()` + one `seed_project` per demo song +
//!    `stamp_song_with_default_tempo_native` — a playable demo setlist as
//!    ONE standalone project per song (each 0-based, its own tempo). No
//!    media stems, no FX factory (those are the native audio path).
//! 2. `build_in_process_daw` serves Standalone's bundle over architect's
//!    in-memory link and wires the global `daw::` facade to it
//!    (`daw::init_from_parts` — the 1-arg wasm form, no block_on runtime).
//! 3. `SetlistServiceImpl::with_daw` + an `architect::LocalServer` hosting
//!    the setlist RPC (+ its `#[subscribe]` stream sibling) behind a
//!    `LayerRouter`; `build_from_open_projects()` builds the setlist and
//!    `start_stream_pumps()` starts the per-hub pumps.
//!
//! Everything runs under `wasm_bindgen_futures::spawn_local` (the browser
//! has no tokio runtime and no `block_on`). The built engine is parked in
//! a wasm `thread_local` singleton — it and every client it holds are
//! `!Send`, which is exactly why it lives thread-local rather than in a
//! `static`.
//!
//! STAGE 4b (dormant): the UI wiring — `session_ui::Session::init` and the
//! [`SessionEventBridge`] that folds the service's streams into session-ui's
//! global signals — is present but OFF, so this stage does not disturb the
//! existing browser `type: song` data path (see `pages/song_session.rs`
//! and `pages/setlist_session.rs`). Flip [`STAGE_4B`] to wire it up.

#![cfg(target_arch = "wasm32")]

use std::cell::{Cell, RefCell};

use daw::service::{ExtState as _, ProjectContext, ProjectInfo};
use daw_standalone::bootstrap::{InProcessDaw, build_in_process_daw};
use daw_standalone::sync::Standalone;
use session::keyflow::actions::SectionKind;
use session::services::setlist_service::{
    SetlistServiceStreamClient, setlist_service_stream_service_descriptor,
    stream_serve as setlist_service_stream_serve,
};
use session::setlist::service::demo::{
    DemoSection, DemoSong, demo_chart_for, demo_songs_base, stamp_song_with_default_tempo_native,
};
use session::{
    SetlistServiceClient, SetlistServiceImpl, serve_setlist_service,
    setlist_service_service_descriptor,
};

use crate::song_session::imp as media;

/// STAGE 4b master switch — Stage 4b-1 flips this ON. The setlist page now
/// drives [`build_for_setlist`], which installs the session-ui `Session`
/// client and mounts [`SessionEventBridge`] unconditionally; this flag only
/// still gates whether the (now app-boot-detached) demo [`bootstrap`] wires
/// its client into `session_ui::Session`.
pub const STAGE_4B: bool = true;

/// Engine key for the demo setlist (the fixed-content [`bootstrap`] build).
const DEMO_KEY: &str = "::demo::";

/// GUID for the demo song at setlist index `i` (one project per song).
fn demo_song_guid(i: usize) -> String {
    format!("demo-song-{i:02}")
}

/// A stable key for the engine currently parked / being built: the ordered
/// slug list (demo uses [`DEMO_KEY`]). Re-mounting the SAME setlist re-keys to
/// the same value → no rebuild; opening a DIFFERENT setlist re-keys → rebuild.
fn setlist_key(slugs: &[String]) -> String {
    slugs.join("\u{1f}")
}

/// The parked in-browser engine. All fields are `!Send` (single-threaded
/// wasm vox: `Rc<RefCell<..>>` sinks), so this lives in a `thread_local`.
pub struct SessionEngine {
    /// Shared service handle — a Stage-4b UI bridge attaches to its
    /// `#[subscribe]` hubs (in-process, no wire) through the stream client.
    pub setlist: SetlistServiceImpl<Standalone>,
    /// RPC client over the in-process `LocalServer` — the same
    /// `SetlistServiceClient` a remote gets over a WebSocket, here over
    /// architect's in-memory link.
    pub client: SetlistServiceClient,
    /// Stream client for the `#[subscribe]` events + active_indices streams.
    pub stream_client: SetlistServiceStreamClient,
    /// The standalone backend itself (kept for future direct native-trait
    /// access from the browser engine).
    #[allow(dead_code)]
    pub standalone: Standalone,
    /// The [`setlist_key`] this engine was built for (dedup / rebuild key).
    #[allow(dead_code)]
    pub key: String,
    /// Keeps the daw-facade in-memory link's acceptor alive.
    _daw: InProcessDaw,
    /// Keeps the setlist RPC `LocalServer`'s acceptor + lanes alive.
    _scope: std::sync::Arc<architect::Scope>,
}

thread_local! {
    /// The one browser engine, once a build has succeeded. `RefCell`
    /// (not `OnceCell`) so accessors can hand out clones of its `Clone`
    /// clients without borrowing across an `.await`, and so a new setlist can
    /// replace it (dropping the old `_scope`/`_daw` releases its links).
    static ENGINE: RefCell<Option<SessionEngine>> = const { RefCell::new(None) };

    /// The [`setlist_key`] of the engine that is parked OR currently building.
    /// Set synchronously at build entry; used to dedup re-mounts of the same
    /// setlist and to detect a switch to a different one.
    static ENGINE_KEY: RefCell<Option<String>> = const { RefCell::new(None) };

    /// "A build is in flight" latch, set true synchronously at build entry and
    /// cleared on completion/failure. Fixes the boot race where two calls both
    /// pass the async-lagging `is_running()` check before either build lands.
    static BUILDING: Cell<bool> = const { Cell::new(false) };
    /// Whether the PARKED engine has had its one-time "open on song 0 /
    /// section 0" cursor seed. Reset on every park. A re-mounted
    /// `SessionEventBridge` must NOT re-open: the indices hub replays the
    /// latest cursor to new subscribers, and re-seeking song 0 yanks a live
    /// session back to the top of the set.
    static OPENED: Cell<bool> = const { Cell::new(false) };
}

/// One-shot per parked engine: `true` exactly once after each [`park`],
/// telling the bridge to seed the initial cursor (song 0 / section 0).
pub fn needs_initial_open() -> bool {
    OPENED.with(|o| !o.replace(true))
}

/// Whether the engine has finished building and is parked.
pub fn is_running() -> bool {
    ENGINE.with(|e| e.borrow().is_some())
}

/// Whether a build is currently in flight (the synchronous latch).
pub fn is_building() -> bool {
    BUILDING.with(Cell::get)
}

/// The key of the parked-or-building engine, if any.
fn current_key() -> Option<String> {
    ENGINE_KEY.with(|k| k.borrow().clone())
}

/// A clone of the setlist RPC client, once the engine is up. `Clone` is
/// cheap (an `Arc`'d `Caller`); returning a clone avoids holding the
/// thread-local `RefCell` borrow across `.await` points in callers.
pub fn client() -> Option<SetlistServiceClient> {
    ENGINE.with(|e| e.borrow().as_ref().map(|s| s.client.clone()))
}

/// A clone of the `#[subscribe]` stream client, once the engine is up.
pub fn stream_client() -> Option<SetlistServiceStreamClient> {
    ENGINE.with(|e| e.borrow().as_ref().map(|s| s.stream_client.clone()))
}

/// Claim the build latch for `key`, returning `false` (no-op) if the engine
/// for this exact key is already parked or already building. On `true` the
/// caller owns the in-flight build: `ENGINE_KEY` is set to `key` and
/// `BUILDING` is raised (both synchronously, before any `.await`), so a second
/// concurrent call for the same key sees the claim and bails — this is the fix
/// for the boot race (two calls both passing the async-lagging `is_running()`
/// gate).
fn claim_build(key: &str) -> bool {
    let same = current_key().as_deref() == Some(key);
    if same && (is_running() || is_building()) {
        return false;
    }
    ENGINE_KEY.with(|k| *k.borrow_mut() = Some(key.to_owned()));
    BUILDING.with(|b| b.set(true));
    true
}

/// Park a freshly built engine + install the session-ui `Session` client so
/// transport panels / the [`SessionEventBridge`] drive THIS engine. Clears the
/// build latch.
fn park(engine: SessionEngine, wire_session: bool) {
    if wire_session {
        if let Err(e) = session_ui::Session::init(engine.client.clone()) {
            tracing::warn!("session engine: Session::init failed: {e:?}");
        }
    }
    // Replacing the `Option` drops any previous engine here — its `_scope` /
    // `_daw` release the in-memory links.
    ENGINE.with(|c| *c.borrow_mut() = Some(engine));
    BUILDING.with(|b| b.set(false));
    OPENED.with(|o| o.set(false));
    tracing::info!("session engine: ready (parked in-browser setlist player)");
}

/// Clear the latch after a failed build, forgetting the key so a later
/// re-mount of the same setlist can retry.
fn fail_build(context: &str, e: impl std::fmt::Debug) {
    tracing::warn!("session engine: {context} failed: {e:?}");
    BUILDING.with(|b| b.set(false));
    ENGINE_KEY.with(|k| *k.borrow_mut() = None);
}

/// Kick off the DEMO engine build in the background (once). Safe to call at app
/// boot: it no-ops if already running/building and never blocks the caller —
/// the whole build runs under `spawn_local`.
///
/// As of Stage 4b-1 the app-boot hook no longer calls this (the setlist page
/// drives [`build_for_setlist`] instead); it is kept as a self-contained demo
/// path and to preserve the [`STAGE_4B`] gate for the demo client wiring.
pub fn bootstrap() {
    if !claim_build(DEMO_KEY) {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        match build_demo().await {
            Ok(engine) => park(engine, STAGE_4B),
            Err(e) => fail_build("demo bootstrap", e),
        }
    });
}

/// Build (or REBUILD) the in-browser engine hosting the REAL vault setlist
/// named by `slugs` — replacing the demo/previous engine. No-ops if the same
/// setlist is already parked or building (keyed on the slug list), so a
/// re-mount of the same page does not rebuild; a different setlist rebuilds
/// (the old engine is dropped, releasing its links). Runs under `spawn_local`
/// and never blocks the caller.
///
/// This is the Stage 4b-1 entry point: it wires `session_ui::Session` (so the
/// UI can command the engine over RPC) and the [`SessionEventBridge`] the page
/// mounts folds the engine's streams back into the session-ui globals.
pub fn build_for_setlist(org: String, slugs: Vec<String>) {
    // The setlist page's effect fires once with an EMPTY slug list before its
    // `songs` prop is populated. Building that produces a 0-song setlist that
    // fails ("no songs") and needlessly churns the engine (and, downstream, the
    // per-song AudioContext). Skip it — the real slugs arrive on the next fire.
    if slugs.is_empty() {
        return;
    }
    let key = setlist_key(&slugs);
    // Diagnose WHY a rebuild is claimed — a re-mount of the same setlist must
    // NOT rebuild (it resets the cursor to song 0 and orphans in-flight RPCs,
    // which replay against the fresh engine as wild cross-song seeks).
    let prev = current_key();
    let was_running = is_running();
    let was_building = is_building();
    if !claim_build(&key) {
        return;
    }
    tracing::info!(
        "session engine: build claimed (same_key={}, was_running={was_running}, \
         was_building={was_building})",
        prev.as_deref() == Some(key.as_str()),
    );
    wasm_bindgen_futures::spawn_local(async move {
        // Drop the previously parked engine BEFORE building the new one so its
        // `_scope` / `_daw` links (and the global daw facade) are released
        // ahead of the replacement.
        ENGINE.with(|c| c.borrow_mut().take());
        match build_setlist(org, slugs, key).await {
            // Always wire the session client for a real setlist — the page's
            // transport callbacks command the engine through it.
            Ok(engine) => park(engine, true),
            Err(e) => fail_build("build_for_setlist", e),
        }
    });
}

/// Seed a demo setlist (fixed content) into a fresh `Standalone`, then assemble
/// the in-process RPC engine over it. Mirrors the native `bootstrap()` minus
/// audio/guide.
async fn build_demo() -> eyre::Result<SessionEngine> {
    tracing::info!("session engine: seeding demo setlist …");
    let standalone = Standalone::new();
    let songs = demo_songs_base();
    let mut song_guids: Vec<String> = Vec::with_capacity(songs.len());
    for (i, song) in songs.iter().enumerate() {
        let guid = demo_song_guid(i);
        seed_song(
            &standalone,
            &guid,
            &format!("{i:02} {}", song.name),
            song,
            demo_chart_for(song.name),
        )?;
        song_guids.push(guid);
    }
    finish_seed(&standalone, &song_guids, "demo setlist")?;
    assemble(standalone, DEMO_KEY.to_owned()).await
}

/// Build the in-browser engine for the REAL setlist `slugs`: fetch each song's
/// `manifest.json` + `chart.kf`, derive a [`DemoSong`] per manifest, seed one
/// standalone project per song (stable slug-derived guid, zero-padded name for
/// authored order), then assemble the in-process RPC engine — the SAME
/// pipeline the demo uses, just fed from manifests.
async fn build_setlist(
    org: String,
    slugs: Vec<String>,
    key: String,
) -> eyre::Result<SessionEngine> {
    tracing::info!(
        "session engine: seeding setlist from {} song(s) …",
        slugs.len()
    );
    let standalone = Standalone::new();
    let mut song_guids: Vec<String> = Vec::with_capacity(slugs.len());
    for (i, slug) in slugs.iter().enumerate() {
        // Fetch this song's manifest (+ optional chart) from `/media`.
        let tok = crate::media_grant::suffix(&org, slug).await;
        let manifest =
            media::fetch_manifest(&format!("/org/{org}/media/songs/{slug}/manifest.json{tok}"))
                .await
                .map_err(|e| eyre::eyre!("fetch manifest for {slug}: {e}"))?;
        let chart = media::fetch_text(&format!("/org/{org}/media/songs/{slug}/chart.kf{tok}"))
            .await
            .ok()
            .filter(|t| !t.is_empty());

        // Map the manifest (+ chart) → a DemoSong the standalone can stamp.
        let title = manifest.title.clone().unwrap_or_else(|| slug.clone());
        // `DemoSong::name` is `&'static str`; leak the runtime title (small,
        // once per setlist build). Used only for the SONG-lane region name.
        let name: &'static str = Box::leak(title.clone().into_boxed_str());
        let demo_song = manifest_to_demo_song(name, &manifest, chart.as_deref());

        // Stable, slug-derived guid; zero-padded prefix keeps authored order
        // (the setlist build lists projects name-sorted).
        let guid = format!("setlist-{i:02}-{slug}");
        seed_song(
            &standalone,
            &guid,
            &format!("{i:02} {title}"),
            &demo_song,
            chart.as_deref(),
        )?;
        song_guids.push(guid);
    }
    finish_seed(&standalone, &song_guids, "setlist")?;
    assemble(standalone, key).await
}

/// Seed ONE song project: create it, stamp its regions/markers at its own
/// default tempo/time-sig, and attach its keyflow chart (ext-state
/// `FTS/chart_text`) so setlist hydration can serve it to the chart pane.
fn seed_song(
    standalone: &Standalone,
    guid: &str,
    name: &str,
    song: &DemoSong,
    chart: Option<&str>,
) -> eyre::Result<()> {
    standalone.seed_project(ProjectInfo {
        guid: guid.to_owned(),
        name: name.to_owned(),
        path: String::new(),
    });
    stamp_song_with_default_tempo_native(
        standalone,
        ProjectContext::Project(guid.to_owned()),
        song,
    )
    .map_err(|e| eyre::eyre!("stamp song {name}: {e:?}"))?;
    if let Some(chart) = chart {
        standalone
            .set_project(
                ProjectContext::Project(guid.to_owned()),
                session::setlist::service::CHART_EXT_STATE_SECTION,
                session::setlist::service::CHART_EXT_STATE_KEY,
                chart,
            )
            .map_err(|e| eyre::eyre!("stamp chart for {name}: {e:?}"))?;
    }
    Ok(())
}

/// Focus the first project + log — the tail of a seeding pass.
fn finish_seed(standalone: &Standalone, song_guids: &[String], what: &str) -> eyre::Result<()> {
    let first_guid = song_guids
        .first()
        .cloned()
        .ok_or_else(|| eyre::eyre!("{what} produced no songs"))?;
    standalone.set_current_project(&first_guid);
    tracing::info!(
        "session engine: stamped {} per-song projects ('{}' …)",
        song_guids.len(),
        first_guid,
    );
    Ok(())
}

/// Steps 2–3, shared by every seed path: install the in-process daw facade,
/// host the setlist service (RPC + `#[subscribe]` stream sibling) behind a
/// `LocalServer`, establish clients, do the initial build, and start the
/// stream pumps.
async fn assemble(standalone: Standalone, key: String) -> eyre::Result<SessionEngine> {
    // 2. In-process daw facade over architect's in-memory link. The setlist
    //    build/hydration path resolves the daw through `daw::get()`, so
    //    install the global facade (1-arg wasm form — no block_on runtime).
    tracing::info!("session engine: building in-process daw facade …");
    let bundle = build_in_process_daw(standalone.clone()).await?;
    daw::init_from_parts(bundle.daw.clone());

    // 3. The setlist service over the standalone backend, hosted in-process
    //    behind a LocalServer (RPC + its `#[subscribe]` stream sibling).
    let setlist = SetlistServiceImpl::with_daw(standalone.clone());
    let router = daw::LayerRouter::new()
        .with(
            setlist_service_service_descriptor(),
            serve_setlist_service(setlist.clone()),
        )
        // The `#[subscribe]` stream sibling (events + active_indices),
        // served from the impl's PubSub hubs. Without it the stream client's
        // subscribe calls return `UnknownMethod`.
        .with(
            setlist_service_stream_service_descriptor(),
            setlist_service_stream_serve(setlist.clone()),
        );
    let scope = architect::Scope::new();
    let server = architect::LocalServer::serve(router, std::sync::Arc::clone(&scope));
    tracing::info!("session engine: LocalServer up; establishing clients …");
    let caller = server
        .caller()
        .await
        .map_err(|e| eyre::eyre!("local setlist caller: {e:?}"))?;
    let client = SetlistServiceClient::new(caller);
    let stream_client = server
        .establish::<SetlistServiceStreamClient>()
        .await
        .map_err(|e| eyre::eyre!("local setlist stream client: {e:?}"))?;

    // Initial build from the seeded projects. Later (UI-driven) builds
    // republish through the hub.
    client
        .build_from_open_projects()
        .await
        .map_err(|e| eyre::eyre!("build_from_open_projects: {e:?}"))?;
    tracing::info!("session engine: setlist built from standalone projects");

    // Start the `#[subscribe]` stream pumps AFTER the build so the events
    // pump snapshots the populated setlist (same ordering constraint as the
    // native engine).
    setlist.start_stream_pumps();

    Ok(SessionEngine {
        setlist,
        client,
        stream_client,
        standalone,
        key,
        _daw: bundle,
        _scope: scope,
    })
}

/// Map a `media::Manifest` (+ optional keyflow chart) → a [`DemoSong`] the
/// standalone can stamp. When a chart is present its layout wins (accurate
/// tempo / time-signature / section timing, exactly like the demo's
/// `chart_song`); otherwise the manifest's audio-region sections are used.
fn manifest_to_demo_song(
    name: &'static str,
    manifest: &media::Manifest,
    chart: Option<&str>,
) -> DemoSong {
    const TAIL: f64 = 4.0; // ring-out after the last section

    // Chart path — the accurate skeleton (drops the leading CountIn, which the
    // stamp path re-derives as the count-in marker).
    if let Some(chart_text) = chart {
        if let Ok(layout) = session::setlist::chart_import::chart_to_layout(chart_text) {
            let sections: Vec<DemoSection> = layout
                .sections
                .iter()
                .filter(|s| s.kind != SectionKind::CountIn)
                .map(|s| DemoSection {
                    kind: s.kind,
                    start: s.start_seconds,
                    end: s.end_seconds,
                })
                .collect();
            return DemoSong {
                name,
                region_start: 0.0,
                region_end: layout.song_end_seconds + TAIL,
                count_in: 0.0,
                song_start: layout.song_start_seconds,
                song_end: layout.song_end_seconds,
                abs_end: layout.song_end_seconds + TAIL,
                tempo_bpm: layout.tempo_bpm,
                time_sig_num: layout.time_sig_num,
                time_sig_den: layout.time_sig_den,
                sections,
            };
        }
    }

    // Fallback — the manifest's own audio-region sections (0-based seconds).
    let (num, den) = parse_time_sig(manifest.time_signature.as_deref());
    let sections: Vec<DemoSection> = manifest
        .sections
        .iter()
        .map(|s| DemoSection {
            kind: label_to_kind(&s.name),
            start: s.start_sec,
            end: s.end_sec,
        })
        .collect();
    let song_start = sections.first().map(|s| s.start).unwrap_or(0.0);
    let song_end = manifest
        .duration_sec
        .max(sections.last().map(|s| s.end).unwrap_or(0.0));
    DemoSong {
        name,
        region_start: 0.0,
        region_end: song_end + TAIL,
        count_in: 0.0,
        song_start,
        song_end,
        abs_end: song_end + TAIL,
        tempo_bpm: manifest.bpm.unwrap_or(120.0),
        time_sig_num: num,
        time_sig_den: den,
        sections,
    }
}

/// Parse a `"num/denom"` time signature, defaulting to 4/4 — the task-ui
/// mirror of `media`'s private `parse_time_sig`.
fn parse_time_sig(s: Option<&str>) -> (u32, u32) {
    s.and_then(|t| {
        let (n, d) = t.split_once('/')?;
        Some((n.trim().parse().ok()?, d.trim().parse().ok()?))
    })
    .filter(|(n, d)| *n > 0 && *d > 0)
    .unwrap_or((4, 4))
}

/// Map a manifest section label → a [`SectionKind`] (colour / marker driver).
/// Mirrors the demo's private `label_to_kind`; unknown labels fall back to a
/// generic instrumental section so they still render.
fn label_to_kind(label: &str) -> SectionKind {
    let l = label.trim().to_ascii_lowercase();
    if l.starts_with("pre") {
        SectionKind::PreChorus
    } else if l.starts_with("verse") || l.starts_with("vs") {
        SectionKind::Verse
    } else if l.starts_with("chorus") || l.starts_with("ch") {
        SectionKind::Chorus
    } else if l.starts_with("refrain") {
        SectionKind::Refrain
    } else if l.starts_with("bridge") || l.starts_with("br") {
        SectionKind::Bridge
    } else if l.starts_with("interlude") {
        SectionKind::Interlude
    } else if l.starts_with("instrumental") || l.starts_with("inst") {
        SectionKind::Instrumental
    } else if l.starts_with("intro") {
        SectionKind::Intro
    } else if l.starts_with("outro") || l.starts_with("ending") || l.starts_with("tag") {
        SectionKind::Outro
    } else {
        SectionKind::Instrumental
    }
}

// ── STAGE 4b (dormant) ──────────────────────────────────────────────────

use dioxus::prelude::*;

/// STAGE 4b (dormant): bridges the setlist service's `#[subscribe]`
/// streams into session-ui's global signals — the in-process flavor of the
/// web remote's subscription. Ported from
/// `apps/fasttrackstudio/src/session_view.rs` with the engine accessor
/// swapped for [`client`]/[`stream_client`] and all `guide::*` calls
/// dropped.
///
/// Mounted **nowhere** while [`STAGE_4B`] is `false`; kept compiling so 4b
/// is a one-line flip. It must NOT be mounted alongside the live
/// `type: song` path (which owns session-ui's globals today) until 4b.
#[component]
pub fn SessionEventBridge() -> Element {
    // ── Events stream: setlist structure + per-song transport ──────────
    use_future(move || async move {
        let Some((client, stream_client)) = wait_for_engine().await else {
            tracing::warn!("session engine never became ready; setlist events unavailable");
            return;
        };

        // Consume the `events` `#[subscribe]` stream through the stream
        // client so the vox lane pumps it (a raw in-process hub attach is
        // never drained — the lane is what moves data).
        let (tx, mut rx) = vox::channel::<session::SetlistEvent>();
        spawn(async move {
            if let Err(e) = stream_client.events(tx).await {
                tracing::warn!("events subscription ended: {e:?}");
            }
        });

        // Deterministic initial snapshot. The events pump publishes its
        // startup SetlistChanged + SongChartHydrated once, at pump init —
        // BEFORE this late subscription attaches — so we must re-fetch rather
        // than rely on that missed republish.
        match client.setlist().await {
            Ok(setlist) => {
                let songs = setlist.songs.clone();
                session_ui::apply_setlist_event(&session::SetlistEvent::SetlistChanged(setlist));
                // Charts are stripped from the Setlist/Song structural payload
                // and only ride the `SongChartHydrated` deltas — whose startup
                // publish this late subscriber also missed. Backfill each song's
                // chart via the dedicated `song_chart` query (exactly its
                // documented purpose: a remote that connected after hydration
                // ran uses it to seed the chart pane's initial snapshot).
                for (index, song) in songs.iter().enumerate() {
                    match client.song_chart(index).await {
                        Ok(Some(chart)) => session_ui::apply_setlist_event(
                            &session::SetlistEvent::SongChartHydrated {
                                song_id: song.id.clone(),
                                index,
                                chart,
                            },
                        ),
                        Ok(None) => {}
                        Err(e) => tracing::warn!("song_chart({index}) failed: {e:?}"),
                    }
                }
            }
            Err(e) => tracing::warn!("initial setlist snapshot failed: {e:?}"),
        }

        while let Ok(Some(ev)) = rx.recv().await {
            session_ui::apply_setlist_event(ev.get());
        }
        tracing::warn!("setlist event stream ended");
    });

    // ── Active-indices stream: the cursor (which song/section is current) ─
    use_future(move || async move {
        let Some((client, stream_client)) = wait_for_engine().await else {
            return;
        };

        let (tx, mut rx) = vox::channel::<session_proto::ActiveIndices>();
        spawn(async move {
            if let Err(e) = stream_client.active_indices(tx).await {
                tracing::warn!("active_indices subscription ended: {e:?}");
            }
        });

        // Open on song 0 / section 0 — ONCE per parked engine (fired
        // concurrently so `rx` is already polling when the cursor publish
        // arrives). A re-mounted bridge skips this: the indices hub replays
        // the latest cursor (`with_replay(1)`), and re-opening would yank a
        // live session back to the top of the set.
        if needs_initial_open() {
            spawn(async move {
                match client.seek_to_section(0, 0).await {
                    Ok(_) => tracing::info!("opened setlist on song 0 / section 0"),
                    Err(e) => tracing::warn!("initial seek to song 0 failed: {e:?}"),
                }
            });
        }

        while let Ok(Some(ai)) = rx.recv().await {
            session_ui::apply_active_indices(ai.get());
        }
        tracing::warn!("active-indices stream ended");
    });

    rsx! {}
}

/// Await the in-process engine becoming ready, returning clones of its RPC +
/// stream clients.
///
/// [`build_for_setlist`] builds the engine ASYNCHRONOUSLY (it fetches every
/// song's manifest + chart over the network) and only [`park`]s it — populating
/// the thread-local [`ENGINE`] — once that finishes. [`SessionEventBridge`] is
/// mounted in the same render that kicks the build, so on its first poll the
/// engine is never ready yet. The bridge must WAIT (a `use_future` runs once and
/// never retries), not bail — bailing is why the navigator / chart / playhead
/// stayed dark while the section bar (which falls back to the fetched manifest)
/// looked fine. Polls every 50 ms, up to ~15 s.
async fn wait_for_engine() -> Option<(SetlistServiceClient, SetlistServiceStreamClient)> {
    for _ in 0..300 {
        if let (Some(c), Some(s)) = (client(), stream_client()) {
            return Some((c, s));
        }
        architect::platform::sleep(std::time::Duration::from_millis(50)).await;
    }
    None
}
