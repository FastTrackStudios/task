//! Live sessions: a setlist played together, hosted by Task.
//!
//! Task is the broker behind Session: a live session is kept open there
//! whoever is in it, and every peer — the desktop app, a browser, a phone —
//! meets in it. A peer [`LiveSessions::join`]s a set and gets its
//! [`LiveSet`]: each song's shared doc (synced over Task's `DocSync`, the
//! song's arrangement as a Loro doc), the set's presence channel (who is
//! where, cursors, the transport's position), and where each song's files
//! are. It follows Task's clock ([`LiveSessions::now`]), so positions
//! stamped by anyone mean the same moment to everyone.
//!
//! A song's doc starts empty: the first peer on it seeds it from the song
//! it opened (the prepared session in Task), and everyone after adopts it.
//!
//! **Epochs.** A set can be a playground that resets (the public demo):
//! every so often its epoch moves on, its songs' docs start fresh under new
//! ids, and peers ([`LiveSessions::epochs`]) re-open their songs from the
//! files and join again.

use serde::{Deserialize, Serialize};

/// A song of a live set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, facet::Facet)]
pub struct LiveSong {
    /// The library's song id (`always-on-time`).
    pub slug: String,
    pub title: String,
    /// The song's shared doc (a UUID), for `DocSync`.
    pub doc_id: String,
    /// A share link to the song's session folder (its documents, and its
    /// proxies as renditions) — what a peer with no account streams the
    /// song from. `None` for a member, who reads the library directly.
    pub files: Option<String>,
}

/// A live set, as a peer joins it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, facet::Facet)]
pub struct LiveSet {
    /// The setlist it plays.
    pub setlist: String,
    pub title: String,
    /// Which run of the set this is (see the module docs on epochs).
    pub epoch: u64,
    /// The set's presence channel (a UUID), for `DocPresence`.
    pub presence_id: String,
    /// Its songs, in order.
    pub songs: Vec<LiveSong>,
    /// Seconds between resets, for a playground; `None` for a set that
    /// keeps what is done in it.
    pub resets_every_secs: Option<u32>,
}

/// A set's epoch moved on: its docs start fresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, facet::Facet)]
pub struct LiveEpoch {
    pub setlist: String,
    pub epoch: u64,
}

/// Why a live call failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, facet::Facet, thiserror::Error)]
#[repr(u8)]
pub enum LiveError {
    #[error("no such setlist: {0}")]
    NotFound(String),
    #[error("not allowed: {0}")]
    NotAllowed(String),
    #[error("{0}")]
    Failed(String),
}

/// Live sessions, as a peer reaches them.
#[architect::rpc]
pub trait LiveSessions {
    /// Join `setlist`'s live session (opening it if nobody is in it). On a
    /// live share link's guest lane, empty joins the link's set.
    async fn join(&self, setlist: String) -> Result<LiveSet, LiveError>;

    /// Task's monotonic clock now, microseconds — the clock every peer's
    /// positions are stamped in. Pinged a few times a second.
    async fn now(&self) -> f64;

    /// Every set's epoch as it moves on.
    #[subscribe]
    fn epochs(&self) -> LiveEpoch;
}
