//! What Task plays of a song, and where it sends you to rehearse one.
//!
//! Task keeps what a song IS — the note, its media, its manifest — and
//! the one thing a listener needs without leaving the app: the global Now
//! Playing stream, which survives navigation. Rehearsal (stems, mixer,
//! the engraved chart, the fullscreen performance view) is the Session
//! app's, and a song or setlist note links across to it.
//!
//! ## What's in here
//!
//! - [`now_playing`] — the global mini-player: a headless engine
//!   mounted outside the route `Outlet` so music survives navigation,
//!   plus its status-bar tab.
//! - [`song_source`] — a song's reference stem, from its colocated
//!   `song` folder or a legacy `manifest.json`.
//! - [`widgets`] — the song/setlist note widgets (Play, Open in Session)
//!   and the ```` ```kf ```` chart fence renderer.
//! - [`session_link`] — the URL into Session for a song or setlist.
//!
//! ## Wiring
//!
//! The shell calls [`provide_player_contexts`] once (from
//! `provide_chrome_contexts`), mounts [`GlobalNowPlayer`] +
//! [`NowPlayingStripHighlighter`] outside the `Outlet`, and drops
//! [`NowPlayingTab`] into the status bar. Nothing else is public surface.
//!
//! The player is `cfg(target_arch = "wasm32")` (media elements), with a
//! stub twin so desktop/mobile/server builds still compile.

pub mod context;
// Signed media grants moved to the shared UI seam so any surface that
// builds `/org/{slug}/media`-style URLs (the review player, the stem
// player) shares one cache; re-exported so callers keep their path.
pub use task_ui_core::media_grant;
pub mod now_playing;
pub mod session_link;
pub mod song_source;
pub mod widgets;

pub use self::widgets::{register_chart_fences, widgets};
pub use context::{NowPlaying, NowPlayingRequest, provide_player_contexts};
pub use now_playing::{
    GlobalNowPlayer, NowPlayingCtl, NowPlayingStripHighlighter, NowPlayingTab, NpCmd,
    provide_now_playing_ctl,
};
