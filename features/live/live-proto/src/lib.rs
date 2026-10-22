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
//! **Observers.** A peer that watches without editing (an assistant with no
//! edit rights, a screen in the room) [`LiveSessions::observe`]s the set
//! instead: it gets the same [`LiveSet`], marked [`LiveSet::read_only`],
//! follows every song's doc and the set's presence, and may put itself in
//! the presence channel (who is watching, where) — but whatever it sends
//! up a song's `DocSync` is refused. Members and bots with edit rights
//! join and write live.
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, facet::Facet)]
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
    /// When this run ends, on Task's clock ([`LiveSessions::now`],
    /// microseconds) — what a playground counts down to. `None` for a set
    /// that does not reset.
    pub resets_at: Option<f64>,
    /// Whether this peer only watches ([`LiveSessions::observe`]): the
    /// server refuses what it sends up a song's doc. Its presence still
    /// counts. `false` for a [`LiveSessions::join`].
    #[serde(default)]
    #[facet(default)]
    pub read_only: bool,
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

    /// Watch `setlist`'s live session without editing it: the same set as
    /// [`Self::join`] (opening it if nobody is in it), with `read_only`
    /// set. This connection's `DocSync` serves the set's songs down to it
    /// and refuses every update it sends up; its `DocPresence` on the set
    /// is a member's. A later `join` on the same connection lifts it. On
    /// a live share link's guest lane, empty observes the link's set.
    async fn observe(&self, setlist: String) -> Result<LiveSet, LiveError>;

    /// Every set's epoch as it moves on.
    #[subscribe]
    fn epochs(&self) -> LiveEpoch;
}
