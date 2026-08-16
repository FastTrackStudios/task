//! `CurationService` — Named Versions and Project Versions.
//!
//! The lane over what a human *decided* was worth pointing at, as
//! opposed to [`super::version`], which is what happened to a tree on
//! its own. Both curated shapes are Vault entities referencing
//! `(root, change)`, so nothing here writes to a version store: naming
//! is a markdown page with frontmatter, and the store learns about it
//! only when the GC pass reads the protect set back out.
//!
//! ## Delegation, and what is actually new
//!
//! Every method below is the v1 `FilesService` method under its v2
//! name. The value added is exactly three things and deliberately no
//! more: typed ids in place of bare `Uuid`s, [`FilesFault`] variants a
//! caller can branch on in place of four prose-carrying `FilesError`s,
//! and the two filters v1 never had — named versions *of one path*, and
//! resolving a *name* rather than an entity id.
//!
//! ## What a `VersionId` addresses here
//!
//! `VersionId` is a `Uuid` — 128 bits — and a version in the store is a
//! jj `CommitId`, 32 bytes on the media backend and 20 on a colocated
//! git one. Neither fits, so a `VersionId` carries the commit's
//! **leading 128 bits**, which is a 32-character hex prefix: unique past
//! any plausible collision, and prefix-addressable by the same resolver
//! every human-facing surface already uses (`task files chain` prints
//! twelve characters, jj is prefix-addressed throughout).
//!
//! Decoding is therefore one place — [`commit_ref`]. Minting a
//! `VersionId` from a commit is the reading lane's business (a chain
//! entry is where a caller gets one), so it is not duplicated here.

use files_proto::error::FilesFault;
use files_proto::id::{ProjectVersionId, RootId, VersionId};
use files_proto::model::{NamedVersion, ProjectVersion, RestartMode};
use files_proto::path::RootPath;
use files_proto::service::curation::CurationService;
use files_proto::service::legacy::{FilesError, FilesService as LegacyFiles};

use crate::backend::FilesBackend;

/// The commit reference a [`VersionId`] names — its 32 hex characters,
/// resolved as a prefix by the backend.
fn commit_ref(version: VersionId) -> String {
    version.commit_prefix()
}

/// Whether two hex ids name the same object, given either may be a
/// prefix of the other.
///
/// Prefixes do occur on both sides: a Vault page records whatever the
/// surface that wrote it printed, and a `VersionId` is a 32-character
/// prefix by construction. `NamedVersion::commit_id` is therefore only
/// ever compared, never trusted to be full-length.
fn same_object(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

/// v1's four prose variants onto the typed surface.
///
/// `NotFound` becomes [`FilesFault::Invalid`] rather than a
/// not-found variant because at this point the prose is all that is
/// left: the callers below have already turned the *identifiable*
/// absences — an unknown root, an unknown version — into their own
/// variants before the legacy method ever runs. Matching
/// `crate::error::Error`'s own mapping keeps one answer for one input.
fn fault(err: FilesError) -> FilesFault {
    match err {
        FilesError::NotFound(m) | FilesError::BadRequest(m) => FilesFault::Invalid(m),
        FilesError::AlreadyExists(m) => FilesFault::AlreadyRoot(m),
        FilesError::Io(m) => FilesFault::Io(m),
    }
}

impl FilesBackend {
    /// Reject an unknown root up front, so every method in this lane
    /// answers [`FilesFault::RootNotFound`] with the id it was given
    /// rather than whatever prose the legacy layer would have produced
    /// several calls deeper.
    fn curated_root(&self, root_id: RootId) -> Result<(), FilesFault> {
        self.registry_get(root_id.get())
            .map(|_| ())
            .ok_or(FilesFault::RootNotFound(root_id))
    }

    /// The root's Named Versions, newest-write-order as the vault holds
    /// them.
    async fn curated_names(&self, root_id: RootId) -> Result<Vec<NamedVersion>, FilesFault> {
        self.curated_root(root_id)?;
        LegacyFiles::list_named_versions(self, Some(root_id.get()))
            .await
            .map_err(fault)
    }

    /// The Named Version a [`VersionId`] picks out of `root_id`.
    ///
    /// Three handles are accepted, because a client holding a
    /// [`NamedVersion`] has all three and none of them is obviously the
    /// one to send: the entity's own id, the commit it names, and the
    /// change it names. They cannot collide — an entity id is minted,
    /// the other two are hashes — so accepting all three costs nothing
    /// and removes a round-trip the caller would otherwise make to find
    /// out which we meant.
    async fn named_by_version(
        &self,
        root_id: RootId,
        version: VersionId,
    ) -> Result<NamedVersion, FilesFault> {
        let handle = commit_ref(version);
        self.curated_names(root_id)
            .await?
            .into_iter()
            .find(|n| {
                n.id == version.get()
                    || same_object(&n.commit_id, &handle)
                    || same_object(&n.change_id, &handle)
            })
            .ok_or(FilesFault::VersionNotFound(version))
    }
}

impl CurationService for FilesBackend {
    // t[impl files.version.cadence] — "any version can be named after
    // the fact, and naming exempts it from retention collection". The
    // exemption is not applied here: the GC pass reads the Vault's
    // protect set live, so writing the entity *is* the exemption.
    async fn name_version(
        &self,
        root_id: RootId,
        version: VersionId,
        name: String,
    ) -> Result<NamedVersion, FilesFault> {
        self.curated_root(root_id)?;
        LegacyFiles::name_version(self, root_id.get(), commit_ref(version), name)
            .await
            .map_err(|e| match e {
                // The only thing this call can fail to find is the
                // commit, the root having been checked above.
                FilesError::NotFound(_) => FilesFault::VersionNotFound(version),
                other => fault(other),
            })
    }

    /// Returns the entity that was removed, so a subscriber can drop it
    /// by id without a refetch — which is also why it is read before it
    /// is deleted rather than after.
    async fn unname_version(
        &self,
        root_id: RootId,
        version: VersionId,
    ) -> Result<NamedVersion, FilesFault> {
        let named = self.named_by_version(root_id, version).await?;
        LegacyFiles::unname_version(self, named.id)
            .await
            .map_err(fault)?;
        Ok(named)
    }

    async fn named_versions(
        &self,
        root_id: RootId,
        path: Option<RootPath>,
    ) -> Result<Vec<NamedVersion>, FilesFault> {
        let all = self.curated_names(root_id).await?;
        let Some(path) = path else {
            return Ok(all);
        };
        // Re-validated because `Deserialize` bypasses `parse`, and this
        // value is about to address a file in the live tree.
        let path = path.validate()?;

        // A Named Version records a commit, not a path — the entity is
        // a version of the whole root. "Named versions of this path"
        // therefore means the names sitting on commits in *that path's*
        // chain, which is the same join `chain` already does in the
        // other direction, so the chain is the authority on membership
        // rather than a second traversal here.
        let chain = LegacyFiles::chain(self, root_id.get(), path.as_str().to_string())
            .await
            .map_err(fault)?;
        Ok(all
            .into_iter()
            .filter(|n| {
                chain
                    .iter()
                    .any(|entry| same_object(&entry.commit_id, &n.commit_id))
            })
            .collect())
    }

    /// Names are unique per root by construction — the vault page path
    /// is derived from the name, so a second claim on one name cannot
    /// be written — which is what makes a name a resolvable address.
    async fn resolve_name(
        &self,
        root_id: RootId,
        name: String,
    ) -> Result<NamedVersion, FilesFault> {
        let wanted = name.trim();
        let mut named = self
            .curated_names(root_id)
            .await?
            .into_iter()
            .find(|n| n.name.trim() == wanted)
            .ok_or_else(|| {
                FilesFault::Invalid(format!("no Named Version called {wanted:?} in {root_id}"))
            })?;

        // The entity records the ids as of the naming; a rewritten
        // change has moved since. Resolving through the root's index
        // gives the commit the name points at *now*, which is what a
        // share link must stream — the page itself stays as written.
        let at = LegacyFiles::resolve_named_version(self, named.id)
            .await
            .map_err(fault)?;
        named.change_id = at.change_id;
        named.commit_id = at.commit_id;
        Ok(named)
    }

    async fn start_project_version(
        &self,
        root_id: RootId,
        name: String,
    ) -> Result<ProjectVersion, FilesFault> {
        self.curated_root(root_id)?;
        // The number is the identity and the label is decoration, so an
        // empty label is a legitimate "just the next one" rather than a
        // bad request.
        let label = Some(name).filter(|l| !l.trim().is_empty());
        LegacyFiles::start_project_version(self, root_id.get(), label)
            .await
            .map_err(fault)
    }

    async fn project_versions(&self, root_id: RootId) -> Result<Vec<ProjectVersion>, FilesFault> {
        self.curated_root(root_id)?;
        LegacyFiles::list_project_versions(self, root_id.get())
            .await
            .map_err(fault)
    }

    async fn restart_project_version(
        &self,
        root_id: RootId,
        project_version: ProjectVersionId,
        mode: RestartMode,
    ) -> Result<ProjectVersion, FilesFault> {
        self.curated_root(root_id)?;
        let target = LegacyFiles::list_project_versions(self, root_id.get())
            .await
            .map_err(fault)?
            .into_iter()
            .find(|pv| pv.id == project_version.get())
            .ok_or(FilesFault::VersionNotFound(VersionId::new(
                project_version.get(),
            )))?;

        // Restarting mints the *next* iteration; the id argument says
        // which one is being restarted, and its label rides across
        // because "begin again" keeps the name of what began. The
        // number never does — numbers are per root, 1-based and never
        // reused.
        LegacyFiles::restart_project_version(self, root_id.get(), mode, target.label)
            .await
            .map_err(fault)
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn a_version_id_decodes_to_a_prefix_of_its_commit() {
        let commit = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let version = VersionId::new(Uuid::try_parse(&commit[..32]).expect("32 hex is a uuid"));
        assert_eq!(
            commit_ref(version),
            &commit[..32],
            "the reference sent to the resolver must be a prefix of the commit it came from"
        );
        assert!(same_object(commit, &commit_ref(version)));
    }

    #[test]
    fn ids_match_either_way_round() {
        assert!(same_object("abcdef0123", "abcd"));
        assert!(same_object("abcd", "abcdef0123"));
        assert!(!same_object("abcd", "abce"));
        assert!(
            !same_object("", "abcd"),
            "an absent id matches nothing — a page with no commit id names nothing at all"
        );
    }

    #[test]
    fn legacy_prose_lands_on_branchable_variants() {
        assert!(matches!(
            fault(FilesError::BadRequest("no".into())),
            FilesFault::Invalid(_)
        ));
        assert!(matches!(
            fault(FilesError::Io("disk".into())),
            FilesFault::Io(_)
        ));
    }
}
