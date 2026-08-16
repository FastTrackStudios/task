//! Typed identifiers.
//!
//! Every id on this surface was a bare `Uuid`, and several methods take
//! two of them — `rendition_at(root_id, version_id)`, `resolve_divergence(
//! root_id, change_id)` — where transposing the arguments compiles and
//! fails at runtime. These newtypes make that a type error.
//!
//! They are transparent on the wire: a `RootId` serialises exactly as the
//! `Uuid` it wraps, so this is a source-level change only and existing
//! payloads keep parsing.

use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Declares a transparent newtype over `Uuid` with the usual conversions.
macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Facet,
        )]
        #[serde(transparent)]
        #[repr(C)]
        pub struct $name(pub Uuid);

        impl $name {
            #[must_use]
            pub const fn new(id: Uuid) -> Self {
                Self(id)
            }

            #[must_use]
            pub fn generate() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn get(self) -> Uuid {
                self.0
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

uuid_id!(
    /// A File Root — a folder tree with a stable identity.
    RootId
);
uuid_id!(
    /// A durable version: a Session checkpoint or a curated version.
    VersionId
);
uuid_id!(
    /// An ephemeral auto-snapshot taken by the cadence engine. Distinct
    /// from [`VersionId`] because a snapshot is not a version — see
    /// `files.version.cadence`.
    SnapshotId
);
uuid_id!(
    /// A Project Version — a named lineage of a root's whole tree.
    ProjectVersionId
);
uuid_id!(
    /// An in-progress upload. Lives only until the upload completes or
    /// is abandoned.
    UploadId
);
uuid_id!(
    /// A review page — the client-facing surface over one file.
    ReviewId
);
uuid_id!(
    /// A comment on a review.
    CommentId
);
uuid_id!(
    /// A device registered for sync.
    DeviceId
);
uuid_id!(
    /// A grant of capabilities over a path to a principal.
    GrantId
);
uuid_id!(
    /// A share link.
    ShareId
);
uuid_id!(
    /// A principal — a person, an agent, or a team.
    PrincipalId
);
uuid_id!(
    /// An entry in the activity feed.
    ActivityId
);

/// A content address: the hash of a file's chunk manifest.
///
/// Distinct from every id above because it is *derived from the bytes*
/// rather than minted — two files with identical content share one
/// `ContentId` across roots, orgs and servers, which is what makes
/// `files.scale.small-files` dedup and `files.topology.federation`
/// zero-transfer sharing work.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Facet)]
#[serde(transparent)]
#[repr(C)]
pub struct ContentId(pub String);

impl ContentId {
    #[must_use]
    pub fn new(hash: impl Into<String>) -> Self {
        Self(hash.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}
