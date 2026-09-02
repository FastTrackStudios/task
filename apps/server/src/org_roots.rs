//! The org's knowledge directories as File Roots, and where each one
//! appears in a mounted tree.
//!
//! An org keeps more than a vault: named wikis (`wiki/Knowledge`,
//! `wikis/<slug>`), a resource library (`resources/`) and copies of the
//! sources it subscribes to (`subscribed/<domain>/<slug>`). A machine
//! syncing the org used to see the vault and whatever somebody had
//! registered by hand — which on the deployment this was written
//! against meant `Wiki/Knowledge` and nothing newer than it.
//!
//! Two halves, both driven by what is on disk so a wiki created after
//! boot needs no code change to appear:
//!
//! - [`adopt_knowledge_roots`] registers each directory
//!   [`org_proto::OrgRoot::knowledge_trees`] lists as a root, in place,
//!   the way the vault is adopted. Run at boot and on every device-sync
//!   sweep, so a new wiki is a root within a sweep.
//! - [`OrgPlacer`] answers the replica lane's `roots` call with each
//!   root's place — `<org>/Wiki/<slug>`, read-only for `Subscribed/` and
//!   `Resources/` — so the agent composes its mount from what the org
//!   says rather than from places typed per machine.

use std::sync::Arc;

use files_sync::{Placement, Placer};
use org_proto::OrgRoot;

/// Register every knowledge directory the org holds as a File Root.
///
/// Idempotent and per-directory: one that cannot be adopted — most
/// likely because a narrower root already covers it, `wiki/Knowledge`
/// registered before `wiki/` was — is logged and the rest proceed. A
/// narrower root still gets its place from [`OrgPlacer`], so the mount
/// shows the same folder either way.
///
/// Returns how many roots were adopted on this call (not how many the
/// org has), which is what a log line after a sweep wants to say.
pub async fn adopt_knowledge_roots(files: &files::FilesBackend, org_root: &OrgRoot) -> usize {
    use files::FilesService as _;
    let known: std::collections::HashSet<std::path::PathBuf> = files
        .list_roots()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| r.local_tree().map(std::path::Path::to_path_buf))
        .collect();
    let mut adopted = 0;
    for tree in org_root.knowledge_trees() {
        // Cheap check first: `adopt_tree` is idempotent, but it opens
        // the root's store to say so, and a sweep runs every minute.
        let canonical = tree.tree.canonicalize().unwrap_or(tree.tree.clone());
        if known.contains(&canonical) {
            continue;
        }
        // On the blocking pool like every synchronous entry into the
        // backend: adoption opens the root's store, which must not park
        // a runtime worker.
        let adopting = files.clone();
        let (path, name) = (tree.tree.clone(), tree.name.clone());
        let outcome = tokio::task::spawn_blocking(move || adopting.adopt_tree(&path, &name))
            .await
            .unwrap_or_else(|panic| {
                Err(files_proto::error::FilesFault::io(std::io::Error::other(
                    panic.to_string(),
                )))
            });
        match outcome {
            Ok(root_id) => {
                adopted += 1;
                tracing::info!(
                    org = %org_root.slug(),
                    root_id = %root_id.get(),
                    name = %tree.name,
                    place = %tree.at.place,
                    read_only = tree.at.read_only,
                    "files: knowledge tree adopted as a root"
                );
            }
            Err(err) => tracing::warn!(
                org = %org_root.slug(),
                name = %tree.name,
                tree = %tree.tree.display(),
                %err,
                "files: knowledge tree not adopted; it will not sync until it is"
            ),
        }
    }
    adopted
}

/// Where this org shows each of its roots — see [`OrgRoot::tree_place`].
#[derive(Debug, Clone)]
pub struct OrgPlacer {
    org_root: OrgRoot,
}

impl OrgPlacer {
    #[must_use]
    pub fn new(org_root: OrgRoot) -> Arc<Self> {
        Arc::new(Self { org_root })
    }
}

impl Placer for OrgPlacer {
    fn place(&self, root: &files_proto::model::FileRootInfo) -> Option<Placement> {
        let tree = root.local_tree()?;
        // The registry holds canonical paths; the org root may not be
        // canonical (a symlinked data root). Compare like with like.
        let org_canonical = self.org_root.path().canonicalize().ok();
        let at = self.org_root.tree_place(tree).or_else(|| {
            let canonical = org_canonical?;
            let rel = tree.strip_prefix(&canonical).ok()?;
            self.org_root.tree_place(&self.org_root.path().join(rel))
        })?;
        Some(Placement {
            place: at.place,
            read_only: at.read_only,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use files::FilesService as _;
    use files_sync::{SyncHost, SyncService as _};

    fn org() -> (tempfile::TempDir, OrgRoot, files::FilesBackend) {
        let tmp = tempfile::tempdir().unwrap();
        let data = org_proto::DataRoot::new(tmp.path().to_owned());
        let org = data.init_org("acme-audio", "ACME Audio", true).unwrap();
        let files = files::FilesBackend::new(org.path().join("files"), org.vault_dir()).unwrap();
        (tmp, org, files)
    }

    /// The mount's contract: two named wikis, a subscribed copy and a
    /// resource library become roots, and the replica lane offers each
    /// at its place — read-only where the org is not the author.
    #[tokio::test(flavor = "multi_thread")]
    async fn knowledge_trees_become_placed_roots() {
        let (_tmp, org, files) = org();
        for dir in [
            "wikis/music-theory",
            "wikis/audio-production",
            "resources/bible/WEB",
            "subscribed/acme.test/music-theory",
        ] {
            std::fs::create_dir_all(org.path().join(dir)).unwrap();
        }
        files.adopt_vault().unwrap();

        let adopted = adopt_knowledge_roots(&files, &org).await;
        assert_eq!(adopted, 5, "Wiki, two named wikis, Resources, one copy");

        let host = SyncHost::new(files.clone()).placing(OrgPlacer::new(org.clone()));
        let mut offered: Vec<(String, bool)> = host
            .roots()
            .await
            .unwrap()
            .into_iter()
            .map(|r| {
                (
                    r.place.unwrap_or_else(|| format!("(unplaced) {}", r.name)),
                    r.read_only,
                )
            })
            .collect();
        offered.sort();
        assert_eq!(
            offered,
            [
                ("acme-audio/Resources".to_string(), true),
                (
                    "acme-audio/Subscribed/acme.test/music-theory".to_string(),
                    true
                ),
                ("acme-audio/Vault".to_string(), false),
                ("acme-audio/Wiki".to_string(), false),
                ("acme-audio/Wiki/audio-production".to_string(), false),
                ("acme-audio/Wiki/music-theory".to_string(), false),
            ]
        );

        // The default wiki is inside the `Wiki` root, at `Wiki/Knowledge`
        // — one canonical path, not a second root under its slug.
        let wiki = files
            .list_roots()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.name == "Wiki")
            .expect("the wiki tier is a root");
        assert_eq!(
            wiki.local_tree(),
            Some(org.wiki_dir().canonicalize().unwrap().as_path())
        );
        assert!(org.wiki_knowledge_dir().is_dir());
    }

    /// A second sweep adopts nothing and a wiki created between sweeps
    /// is adopted on the next — the property that makes the mount
    /// data-driven rather than boot-driven.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_wiki_created_later_is_adopted_on_the_next_sweep() {
        let (_tmp, org, files) = org();
        assert_eq!(
            adopt_knowledge_roots(&files, &org).await,
            1,
            "the Wiki tier"
        );
        assert_eq!(adopt_knowledge_roots(&files, &org).await, 0, "idempotent");

        std::fs::create_dir_all(org.named_wiki_dir("bible-study")).unwrap();
        assert_eq!(adopt_knowledge_roots(&files, &org).await, 1);
        let names: Vec<String> = files
            .list_roots()
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert!(names.iter().any(|n| n == "Wiki — bible-study"), "{names:?}");
    }

    /// A root outside the org's layout — a project on a NAS — is offered
    /// by name, as before, rather than refused or misfiled.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_root_outside_the_layout_has_no_place() {
        let (tmp, org, files) = org();
        let elsewhere = tmp.path().join("nas").join("Ghosts");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let root = files
            .adopt_tree(&elsewhere, "Ghosts")
            .expect("registered outside the files boundary, like the vault");
        let placer = OrgPlacer::new(org);
        let info = files.get_root(root.get()).await.unwrap();
        assert_eq!(placer.place(&info), None);
    }
}
