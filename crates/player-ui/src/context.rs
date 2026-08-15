//! Player contexts — the request channels the app shell provides and
//! any page can post into.
//!
//! These used to live in the shell's `chrome` module; they moved here
//! with the player so a note page can ask for playback without the
//! shell owning the player's vocabulary. The shell still installs them
//! (see [`provide_player_contexts`], called from `provide_chrome_contexts`)
//! and re-exports the types at their old paths.

use dioxus::prelude::*;

/// A song-strip play click (`data-href="song-play:<Note Name>"` from the
/// editor's inline song widgets). `(generation, note name)` — the counter
/// makes replaying the same song observable. Consumed by whichever player
/// is mounted (the setlist stream player's header today).
#[derive(Clone, Copy)]
pub struct SongPlayRequest(pub Signal<(u64, String)>);

/// A request to the GLOBAL Now Playing player (mounted in the app shell,
/// so playback survives navigation). Carries the whole queue captured at
/// play time — the player owns its copy, independent of whichever note
/// fired it, so leaving the note doesn't stop the music or break
/// skip-next. `generation` makes replays of the same request observable.
#[derive(Clone, PartialEq, Default)]
pub struct NowPlayingRequest {
    pub generation: u64,
    /// Org whose colocated `/org/{org}/media/songs/{slug}/…` serves the audio.
    pub org: String,
    /// Queue title (setlist name, or the song's own title for a 1-song queue).
    pub title: String,
    /// Ordered song slugs (`/media` slugs).
    pub songs: Vec<String>,
    /// Index in `songs` to start / jump to.
    pub start: usize,
    /// Header ▶: toggle play/pause when this queue is already loaded,
    /// rather than restarting it. Song-strip clicks set this `false`.
    pub toggle: bool,
}

#[derive(Clone, Copy)]
pub struct NowPlaying(pub Signal<NowPlayingRequest>);

/// Install every context the player reads. Call once in the app shell,
/// above both the headless engine and the status bar.
pub fn provide_player_contexts() {
    use_context_provider(|| SongPlayRequest(Signal::new((0, String::new()))));
    use_context_provider(|| NowPlaying(Signal::new(NowPlayingRequest::default())));
    crate::now_playing::provide_now_playing_ctl();
}
