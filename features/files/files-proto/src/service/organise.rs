//! Hand-organisation and accountability — `files.organise.*`.
//!
//! Extraction gives you what the machine can infer. This lane is the
//! layer the machine will never infer: the tags you chose, the takes you
//! marked, and the record of who did what.
//!
//! A tag produces a **view**, never folder membership. Tagging does not
//! move a file and does not change its path.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::error::FilesFault;
use crate::id::{ActivityId, PrincipalId, RootId};
use crate::path::RootPath;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Facet)]
#[serde(transparent)]
#[repr(C)]
pub struct Tag(pub String);

/// A file and what has been said about it by hand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Marks {
    pub root_id: RootId,
    pub path: RootPath,
    pub tags: Vec<Tag>,
    /// Per-principal, not global — one person's shortlist is not
    /// everyone's.
    pub favourite: bool,
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum Action {
    Created,
    Modified,
    Renamed,
    Moved,
    Copied,
    Deleted,
    Restored,
    Uploaded,
    Downloaded,
    Viewed,
    Shared,
    Commented,
}

/// One entry in the activity feed.
///
/// Derived from the same events that drive live propagation, so what
/// propagates is what is recorded — an audit trail and a change feed
/// cannot disagree if they are the same stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Activity {
    pub id: ActivityId,
    pub root_id: RootId,
    pub path: RootPath,
    pub action: Action,
    pub actor: PrincipalId,
    pub actor_name: String,
    /// Set for renames and moves.
    pub from: Option<RootPath>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum OrganiseEvent {
    MarksChanged(Marks),
    Happened(Activity),
}

#[architect::rpc]
pub trait OrganiseService {
    /// Tags and favourite state for a path.
    async fn marks(&self, root_id: RootId, path: RootPath) -> Result<Marks, FilesFault>;

    /// Replace a path's tags. The file does not move.
    async fn set_tags(
        &self,
        root_id: RootId,
        path: RootPath,
        tags: Vec<Tag>,
    ) -> Result<Marks, FilesFault>;

    /// Mark or unmark a favourite, for the calling principal only.
    async fn set_favourite(
        &self,
        root_id: RootId,
        path: RootPath,
        favourite: bool,
    ) -> Result<Marks, FilesFault>;

    /// Everything carrying a tag — the view a tag produces.
    async fn tagged(&self, tags: Vec<Tag>, root_id: Option<RootId>)
    -> Result<Vec<Marks>, FilesFault>;

    /// Tags in use, for completion and for showing what exists.
    async fn all_tags(&self, root_id: Option<RootId>) -> Result<Vec<Tag>, FilesFault>;

    /// The activity feed, scoped to a root or to a subtree within it,
    /// newest first.
    async fn activity(
        &self,
        root_id: RootId,
        under: Option<RootPath>,
        limit: Option<u32>,
    ) -> Result<Vec<Activity>, FilesFault>;
}
