//! The owner's half of B4/B5: who outside this org may subscribe to one
//! of its asset shelves or projects.
//!
//! The rules and the storage are `wiki_live::source_access`. This module
//! is the gate in front of them: owner or admin only (the same
//! [`super::SharingServiceImpl::admin`] check every other `sharing`
//! method makes), the source named checked against this org's own tree,
//! and one audit line per change, beside the durable row in the log.

use share_proto::{SharingError, SourceTier};

#[cfg(feature = "plugin-wiki")]
use share_proto::{SourceGrantInfo, SourceVisibility};
#[cfg(feature = "plugin-wiki")]
use wiki_live::source_access::{AccessError, AccessGrant, AccessGrants};
#[cfg(feature = "plugin-wiki")]
use wiki_proto::config::Visibility;
#[cfg(feature = "plugin-wiki")]
use wiki_proto::subscription::SourceKind;

#[cfg(feature = "plugin-wiki")]
pub(super) const fn kind_of(tier: SourceTier) -> SourceKind {
    match tier {
        SourceTier::Assets => SourceKind::Assets,
        SourceTier::Projects => SourceKind::Projects,
    }
}

#[cfg(feature = "plugin-wiki")]
const fn tier_of(kind: SourceKind) -> SourceTier {
    match kind {
        SourceKind::Projects => SourceTier::Projects,
        // `AccessGrants::grant` refuses every other kind, so a row of
        // another kind cannot be in the log.
        _ => SourceTier::Assets,
    }
}

#[cfg(feature = "plugin-wiki")]
pub(super) const fn visibility_of(v: SourceVisibility) -> Visibility {
    match v {
        SourceVisibility::Public => Visibility::Public,
        SourceVisibility::Unlisted => Visibility::Unlisted,
        SourceVisibility::Private => Visibility::Private,
    }
}

/// The directory `slug` names on `tier` of the org at `org_root`, when
/// it is a source this org actually has.
///
/// The slug is the owner's own string here, not a stranger's, but it is
/// still joined onto a path, so it gets the same guard
/// `LocalOrgs::source_root` applies: every segment its own slug, and only
/// a project may have more than one.
pub(super) fn source_dir(
    org_root: &std::path::Path,
    tier: SourceTier,
    slug: &str,
) -> Result<std::path::PathBuf, SharingError> {
    let segments: Vec<&str> = match tier {
        SourceTier::Projects => slug.split('/').collect(),
        SourceTier::Assets => vec![slug],
    };
    if segments
        .iter()
        .any(|s| s.is_empty() || *s != org_proto::wiki_slug(s))
    {
        return Err(SharingError::Invalid(format!(
            "`{slug}` is not a {} name",
            tier.as_str()
        )));
    }
    let mut dir = org_root.join(tier.as_str());
    for s in segments {
        dir.push(s);
    }
    let exists = match tier {
        SourceTier::Assets => dir.is_dir(),
        SourceTier::Projects => dir.join(org_proto::PROJECT_PAGE).is_file(),
    };
    if exists {
        Ok(dir)
    } else {
        Err(SharingError::NotFound)
    }
}

#[cfg(feature = "plugin-wiki")]
pub(super) fn info(owner_org: &str, g: &AccessGrant) -> SourceGrantInfo {
    SourceGrantInfo {
        id: g.id,
        owner_org: owner_org.to_owned(),
        tier: tier_of(g.kind),
        slug: g.slug.clone(),
        subscriber: g.subscriber.clone(),
        granted_by: g.granted_by.clone(),
        granted_at: g.granted_at,
        revoked_at: g.revoked_at,
        revoked_by: g.revoked_by.clone(),
        live: g.is_live(),
    }
}

#[cfg(feature = "plugin-wiki")]
pub(super) fn grants(org_root: &std::path::Path) -> AccessGrants {
    AccessGrants::open(org_root)
}

#[cfg(feature = "plugin-wiki")]
pub(super) fn access_err(e: AccessError) -> SharingError {
    match e {
        AccessError::Invalid(m) => SharingError::Invalid(m),
        AccessError::AlreadyGranted { id, subscriber } => SharingError::Invalid(format!(
            "`{subscriber}` already holds a live grant on this source ({id}); revoke it \
             first, since a grant is never edited"
        )),
        AccessError::Io(e) => {
            tracing::warn!(target: "task_server::sharing", error = %e, "access-grant log error");
            SharingError::Storage("the access-grant log could not be written".into())
        }
    }
}

/// One audit line for a source-access change (`t[wiki.access.source-grant]`):
/// onto the call's wide event, and as one `info` line. The durable copy
/// is the row in the org's access-grant log.
#[cfg(feature = "plugin-wiki")]
pub(super) fn audit(
    org: &str,
    event: &str,
    tier: SourceTier,
    slug: &str,
    detail: &str,
    actor: &str,
) {
    use architect_telemetry::wide;
    wide::set("sharing.event", event.to_owned());
    wide::set("sharing.source", format!("{}/{slug}", tier.as_str()));
    wide::set("sharing.detail", detail.to_owned());
    wide::set("sharing.actor", actor.to_owned());
    tracing::info!(
        target: "task_server::sharing",
        slug = %org,
        event = %event,
        source = %format!("{}/{slug}", tier.as_str()),
        detail = %detail,
        actor = %actor,
        "sharing: {event}",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_is_named_by_its_own_slug_and_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let songs = dir.path().join("assets/songs");
        std::fs::create_dir_all(&songs).unwrap();
        let album = dir.path().join("projects/crescendum/track-two");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join(org_proto::PROJECT_PAGE), "---\n---\n").unwrap();

        assert_eq!(
            source_dir(dir.path(), SourceTier::Assets, "songs").unwrap(),
            songs
        );
        assert_eq!(
            source_dir(dir.path(), SourceTier::Projects, "crescendum/track-two").unwrap(),
            album
        );
        // A directory without a `project.md` is not a project.
        assert_eq!(
            source_dir(dir.path(), SourceTier::Projects, "crescendum"),
            Err(SharingError::NotFound)
        );
        assert_eq!(
            source_dir(dir.path(), SourceTier::Assets, "charts"),
            Err(SharingError::NotFound)
        );
        for bad in ["../songs", "songs/x", "/etc", "", "Songs"] {
            assert!(
                matches!(
                    source_dir(dir.path(), SourceTier::Assets, bad),
                    Err(SharingError::Invalid(_))
                ),
                "{bad:?}"
            );
        }
    }
}
