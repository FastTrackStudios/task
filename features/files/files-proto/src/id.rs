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
impl VersionId {
    /// A version's id, derived from the store's commit hex.
    ///
    /// **The canonical definition, and deliberately the only one.** Two
    /// lanes independently invented incompatible ones — a `UUIDv5` hash
    /// and a leading-bits prefix — which meant a `VersionId` handed out
    /// by `VersionService::chain` could not be resolved by
    /// `CurationService::name_version`. Anything addressing a version
    /// converts through here.
    ///
    /// The id is the commit's **leading 128 bits**, so it converts back
    /// (see [`Self::commit_prefix`]) rather than needing a side table or
    /// a scan to resolve. jj commit ids are 32 bytes for media roots and
    /// 20 for git-backed ones, so 16 bytes is always a prefix of one; a
    /// shorter input is right-padded, which keeps the conversion total
    /// rather than fallible.
    #[must_use]
    pub fn from_commit_hex(hex: &str) -> Self {
        let mut buf = [0u8; 32];
        for (slot, c) in buf.iter_mut().zip(hex.bytes()) {
            *slot = c;
        }
        // Non-hex or short input still yields a stable, distinct id —
        // this must not fail, because a caller has no useful recovery.
        let text = String::from_utf8_lossy(&buf);
        Self(Uuid::from_u128(
            u128::from_str_radix(text.trim_end_matches('\0'), 16).unwrap_or_else(|_| {
                text.bytes().fold(0u128, |acc, b| {
                    acc.wrapping_mul(31).wrapping_add(u128::from(b))
                })
            }),
        ))
    }

    /// The commit prefix this id addresses — 32 hex characters, which
    /// the store's prefix resolver accepts directly.
    #[must_use]
    pub fn commit_prefix(self) -> String {
        self.0.simple().to_string()
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_id_round_trips_to_its_commit_prefix() {
        let hex = "3f9c1e8800004000800000000000000112345678";
        let id = VersionId::from_commit_hex(hex);
        assert_eq!(id.commit_prefix(), &hex[..32]);
        assert!(
            hex.starts_with(&id.commit_prefix()),
            "the id must address the commit it came from"
        );
    }

    #[test]
    fn the_same_commit_always_yields_the_same_id() {
        let hex = "abcdef0123456789abcdef0123456789abcdef01";
        assert_eq!(
            VersionId::from_commit_hex(hex),
            VersionId::from_commit_hex(hex)
        );
    }

    #[test]
    fn commits_differing_inside_the_prefix_differ() {
        let a = VersionId::from_commit_hex("abcdef0123456789abcdef0123456789");
        let b = VersionId::from_commit_hex("abcdef0123456789abcdef012345678a");
        assert_ne!(a, b);
    }

    #[test]
    fn a_short_or_non_hex_id_is_stable_rather_than_a_panic() {
        // A caller has no useful recovery from a malformed store id, so
        // the conversion is total.
        assert_eq!(
            VersionId::from_commit_hex("zzz"),
            VersionId::from_commit_hex("zzz")
        );
        assert_ne!(
            VersionId::from_commit_hex("zzz"),
            VersionId::from_commit_hex("yyy")
        );
    }
}
