//! The Task app's browser session player.
//!
//! Everything here used to live inside `crates/task/ui` — nine modules
//! and ~6.8k lines of Web Audio, engraved-chart rendering and an
//! in-tab daw-standalone engine, sitting in the middle of a
//! productivity shell and dragging `daw`, `daw-standalone`, `session`,
//! the keyflow engraver and the editor's keyflow language into every
//! Task wasm bundle. It is one coherent feature, so it is one crate.
//!
//! ## What's in here
//!
//! - [`song_session`] — the single-song view: manifest/frontmatter →
//!   stems → a Web Audio graph, driving `session-ui`'s global signals.
//! - [`setlist_session`] — the multi-song player built on those same
//!   primitives, plus the fullscreen performance layout.
//! - [`setlist_audio`] — one AudioWorklet-backed queue player per
//!   setlist (the native engine's layout).
//! - [`setlist_stream`] — the lightweight reference-stem streamer.
//! - [`session_chart_pane`] / [`keyflow_chart_editor`] — the synced
//!   chart: CPU engraver → inline SVG, and the editor over its source.
//! - [`now_playing`] — the global mini-player: a headless engine
//!   mounted outside the route `Outlet` so music survives navigation,
//!   plus its status-bar tab.
//! - [`session_engine`] (wasm only) — a headless daw-standalone setlist
//!   player hosted in the tab via architect's in-process `LocalServer`.
//! - [`vox_media_source`] — MediaSource-backed `<audio>` fed by
//!   `MediaService` chunks over the vox lane.
//!
//! ## Why `crates/task/` and not `crates/session/`
//!
//! `session-ui` is the presentational half — dumb components plus the
//! global signals — and it is shared with the fasttrackstudio wasm
//! remote. This crate is the *stateful host* that drives those
//! components, and it is task-flavoured: it parses `type: song` vault
//! frontmatter and dials `/org/<slug>/vox`. Parking it under
//! `crates/session/` would make the session domain depend on the Task
//! app; folding it into `session-ui` would push `daw-standalone` and
//! the keyflow engraver into every session-ui consumer, which is the
//! exact weight this split removes.
//!
//! ## Wiring
//!
//! The shell calls [`provide_player_contexts`] once (from
//! `provide_chrome_contexts`), mounts [`GlobalNowPlayer`] +
//! [`NowPlayingStripHighlighter`] outside the `Outlet`, drops
//! [`NowPlayingTab`] into the status bar, and renders [`SongView`] /
//! [`SetlistPlayer`] from its note views. Nothing else is public
//! surface.
//!
//! Every module keeps its wasm/native split: the real player is
//! `cfg(target_arch = "wasm32")` (Web Audio + media elements), with a
//! stub twin so desktop/mobile/server builds still compile.

pub mod context;
pub mod keyflow_chart_editor;
// Signed media grants moved to the shared UI seam so any surface that
// builds `/org/{slug}/media`-style URLs (the review player, the stem
// player) shares one cache; re-exported so callers keep their path.
pub use task_ui_core::media_grant;
pub mod now_playing;
pub mod session_chart_pane;
pub mod session_engine;
pub mod setlist_audio;
pub mod setlist_session;
pub mod setlist_stream;
pub mod song_session;
pub mod vox_media_source;
pub mod widgets;

pub use self::widgets::widgets;
pub use context::{NowPlaying, NowPlayingRequest, SongPlayRequest, provide_player_contexts};
pub use now_playing::{
    GlobalNowPlayer, NowPlayingCtl, NowPlayingStripHighlighter, NowPlayingTab, NpCmd,
    provide_now_playing_ctl,
};
pub use setlist_session::SetlistPlayer;
pub use setlist_stream::SetlistStreamPlayer;
pub use song_session::SongView;

/// An org's `MediaServiceClient` — content-addressed blob bytes
/// streamed over the vox lane itself (`Tx<MediaChunk>`), no HTTP
/// side-channel. The forward path for the player's stems.
// Only the wasm (browser) build streams over these clients; native
// compiles the module but never calls them.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) async fn media_client(
    slug: &str,
) -> Result<media_proto::AttachmentMediaServiceClient, String> {
    task_ui_core::vox_clients::establish_for::<media_proto::AttachmentMediaServiceClient>(slug)
        .await
}

/// An org's `AttachmentServiceClient` — short-lived signed download
/// URLs for content-addressed blobs (song stems).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) async fn attachments_client(
    slug: &str,
) -> Result<attachments_proto::AttachmentServiceClient, String> {
    task_ui_core::vox_clients::establish_for::<attachments_proto::AttachmentServiceClient>(slug)
        .await
}
