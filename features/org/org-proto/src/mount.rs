//! Where an org's directories appear in the tree a person is shown.
//!
//! The file-sync agent composes every root it holds into one folder —
//! `~/Task/<org>/Projects/<name>`, `~/Task/<org>/Vault` — and each root
//! needs a *place* in that tree. The place used to be typed by hand,
//! once per root per machine, which is how the mount came to show
//! `Wiki/Knowledge` and nothing else long after the org had grown named
//! wikis, a resource library and subscribed copies.
//!
//! The org's layout is the authority on what each directory is, so the
//! org answers the question: given a root's tree, here is its place and
//! whether a person may write there. The sync engine carries the answer
//! to the agent, and the agent records it for any root nobody has placed
//! by hand. Nothing here touches a disk except [`OrgRoot::knowledge_trees`],
//! which lists what exists so the server can adopt it.
//!
//! # The layout, as shown
//!
//! ```text
//! <org>/
//!   Projects/<name>/              read-write   files/Projects/<name>
//!   Vault/                        read-write   vault/
//!   Wiki/                         read-write   wiki/        (Knowledge + LLM)
//!   Wiki/<slug>/                  read-write   wikis/<slug>/
//!   Subscribed/<domain>/<slug>/   read-only    subscribed/<domain>/<slug>/
//!   Resources/                    read-only    resources/
//! ```
//!
//! One canonical path per wiki. The default wiki is `Wiki/Knowledge`
//! because that is the directory it lives in and the name every page,
//! skill and lint already uses (`wiki/Knowledge`); its slug `knowledge`
//! is not surfaced as a second folder. `Wiki/LLM` rides along inside
//! the same root, exactly as it did before. Named wikis sit beside them
//! under their slug, so `Wiki/` is the one folder that holds everything
//! the org knows.
//!
//! Subscribed copies and the resource library are read-only because
//! neither is the org's to edit: a subscribed copy is somebody else's
//! wiki, refreshed from upstream, and an edit to it would be overwritten
//! or — worse — pulled back to the server as a local change.

use std::path::{Path, PathBuf};

use crate::root::OrgRoot;

/// A root's place in the composed tree, and whether it may be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreePlace {
    /// The shown path, `<org>/…`, with no leading or trailing slash.
    pub place: String,
    /// Whether the agent should refuse writes under it.
    pub read_only: bool,
}

impl TreePlace {
    fn writable(place: String) -> Self {
        Self {
            place,
            read_only: false,
        }
    }

    fn read_only(place: String) -> Self {
        Self {
            place,
            read_only: true,
        }
    }
}

/// A directory the server should hold as a File Root so the mount can
/// show it: its tree, the name to register it under, and its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeTree {
    pub name: String,
    pub tree: PathBuf,
    pub at: TreePlace,
}

/// The directory the tree shows as `Wiki/`.
pub const WIKI_FOLDER: &str = "Wiki";
/// The directory the tree shows as `Subscribed/`.
pub const SUBSCRIBED_FOLDER: &str = "Subscribed";
/// The directory the tree shows as `Resources/`.
pub const RESOURCES_FOLDER: &str = "Resources";
/// The directory the tree shows as `Projects/`.
pub const PROJECTS_FOLDER: &str = "Projects";
/// The directory the tree shows as `Vault/`.
pub const VAULT_FOLDER: &str = "Vault";

impl OrgRoot {
    /// Where a root whose tree is `tree` appears, if the org knows.
    ///
    /// `None` for a tree outside the org's layout — a project on a NAS
    /// placed through the storage layer, a folder somebody shared from
    /// their own disk — which the agent then places by name, as it
    /// always has.
    ///
    /// Compares paths lexically, not canonically: the caller passes a
    /// root's registered path and this org's own, and both come from the
    /// same registry on the same host, so a symlink resolved on one side
    /// and not the other would be the caller's inconsistency to fix.
    #[must_use]
    pub fn tree_place(&self, tree: &Path) -> Option<TreePlace> {
        let rel = tree.strip_prefix(self.path()).ok()?;
        let parts: Vec<&str> = rel.iter().map(|s| s.to_str().unwrap_or("")).collect();
        let org = self.slug();
        let shown = |segments: &[&str]| {
            let mut out = org.to_string();
            for s in segments {
                out.push('/');
                out.push_str(s);
            }
            out
        };
        Some(match parts.as_slice() {
            ["vault"] => TreePlace::writable(shown(&[VAULT_FOLDER])),
            // The whole tier: Knowledge and LLM ride inside one root.
            ["wiki"] => TreePlace::writable(shown(&[WIKI_FOLDER])),
            // Or either tier adopted on its own — same shown path, so a
            // server that registered `wiki/Knowledge` before the whole
            // directory was a root shows the same folder.
            ["wiki", tier] => TreePlace::writable(shown(&[WIKI_FOLDER, tier])),
            ["wikis", slug] => TreePlace::writable(shown(&[WIKI_FOLDER, slug])),
            ["resources", rest @ ..] => {
                let mut segs = vec![RESOURCES_FOLDER];
                segs.extend(rest);
                TreePlace::read_only(shown(&segs))
            }
            ["subscribed", domain, slug] if !domain.starts_with('.') => {
                TreePlace::read_only(shown(&[SUBSCRIBED_FOLDER, domain, slug]))
            }
            ["files", "Projects", name] | ["files", name] => {
                TreePlace::writable(shown(&[PROJECTS_FOLDER, name]))
            }
            _ => return None,
        })
    }

    /// Every knowledge directory this org holds that the mount should
    /// show — what the server adopts as File Roots so the agent can sync
    /// and place them.
    ///
    /// Read from disk, so a wiki created or a source subscribed since
    /// boot appears on the next call without a code change. The vault
    /// is not listed: it is adopted separately, because adopting it also
    /// binds the page-write sink. Projects are not listed either — they
    /// are roots by creation, not by discovery.
    ///
    /// Sorted by place, so two servers holding the same org register in
    /// the same order and a log of what was adopted reads the same way
    /// twice.
    #[must_use]
    pub fn knowledge_trees(&self) -> Vec<KnowledgeTree> {
        let mut out = Vec::new();
        let mut push = |name: String, tree: PathBuf| {
            if !tree.is_dir() {
                return;
            }
            if let Some(at) = self.tree_place(&tree) {
                out.push(KnowledgeTree { name, tree, at });
            }
        };

        push(WIKI_FOLDER.to_string(), self.wiki_dir());
        for (slug, tree) in self.named_wikis() {
            if slug == crate::DEFAULT_WIKI {
                // Inside `wiki/`, already covered by the tier root.
                continue;
            }
            push(format!("{WIKI_FOLDER} — {slug}"), tree);
        }
        push(RESOURCES_FOLDER.to_string(), self.resources_dir());
        for (domain, slug, tree) in subscribed_copies(&self.path().join("subscribed")) {
            push(format!("{SUBSCRIBED_FOLDER} — {domain} — {slug}"), tree);
        }

        out.sort_by(|a, b| a.at.place.cmp(&b.at.place));
        out
    }
}

/// `<subscribed>/<domain>/<slug>/` directories, skipping the `.state`
/// bookkeeping beside them.
fn subscribed_copies(subscribed: &Path) -> Vec<(String, String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(domains) = std::fs::read_dir(subscribed) else {
        return out;
    };
    for domain in domains.flatten() {
        let Some(domain_name) = domain.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if domain_name.starts_with('.') || !domain.path().is_dir() {
            continue;
        }
        let Ok(slugs) = std::fs::read_dir(domain.path()) else {
            continue;
        };
        for slug in slugs.flatten() {
            let Some(slug_name) = slug.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if slug_name.starts_with('.') || !slug.path().is_dir() {
                continue;
            }
            out.push((domain_name.clone(), slug_name, slug.path()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::DataRoot;

    fn org() -> (tempfile::TempDir, OrgRoot) {
        let tmp = tempfile::tempdir().unwrap();
        let root = DataRoot::new(tmp.path().to_owned());
        let org = root.init_org("acme-audio", "ACME Audio", true).unwrap();
        (tmp, org)
    }

    #[test]
    fn the_layout_maps_onto_the_shown_tree() {
        let (_tmp, org) = org();
        let p = |rel: &str| org.tree_place(&org.path().join(rel));
        let rw = |place: &str| Some(TreePlace::writable(place.to_string()));
        let ro = |place: &str| Some(TreePlace::read_only(place.to_string()));

        assert_eq!(p("vault"), rw("acme-audio/Vault"));
        assert_eq!(p("wiki"), rw("acme-audio/Wiki"));
        assert_eq!(p("wiki/Knowledge"), rw("acme-audio/Wiki/Knowledge"));
        assert_eq!(p("wiki/LLM"), rw("acme-audio/Wiki/LLM"));
        assert_eq!(p("wikis/music-theory"), rw("acme-audio/Wiki/music-theory"));
        assert_eq!(p("resources"), ro("acme-audio/Resources"));
        assert_eq!(
            p("resources/bible/WEB"),
            ro("acme-audio/Resources/bible/WEB")
        );
        assert_eq!(
            p("subscribed/acme.test/music-theory"),
            ro("acme-audio/Subscribed/acme.test/music-theory")
        );
        assert_eq!(
            p("files/Projects/First Single"),
            rw("acme-audio/Projects/First Single")
        );
        assert_eq!(
            p("files/Laptop Project"),
            rw("acme-audio/Projects/Laptop Project")
        );
    }

    #[test]
    fn what_the_layout_does_not_name_has_no_place() {
        let (_tmp, org) = org();
        assert_eq!(org.tree_place(Path::new("/mnt/nas/Sessions/Ghosts")), None);
        assert_eq!(org.tree_place(&org.path().join("attachments")), None);
        assert_eq!(
            org.tree_place(&org.path().join("subscribed/.state/x")),
            None
        );
        assert_eq!(
            org.tree_place(&org.path().join("subscribed/acme.test")),
            None
        );
        assert_eq!(org.tree_place(org.path()), None);
    }

    /// The property the mount exists for: two named wikis, a subscribed
    /// copy and a resource library yield exactly the trees the tree
    /// should show, each at its place, with the read-only ones marked.
    #[test]
    fn knowledge_trees_are_what_is_on_disk() {
        let (_tmp, org) = org();
        for dir in [
            "wikis/music-theory",
            "wikis/audio-production",
            "wikis/Not A Slug",
            "resources/bible/WEB",
            "subscribed/acme.test/music-theory",
            "subscribed/.state/acme.test",
        ] {
            std::fs::create_dir_all(org.path().join(dir)).unwrap();
        }
        std::fs::write(
            org.path().join("subscribed/acme.test/stray.md"),
            "not a copy",
        )
        .unwrap();

        let trees = org.knowledge_trees();
        let places: Vec<(&str, bool)> = trees
            .iter()
            .map(|t| (t.at.place.as_str(), t.at.read_only))
            .collect();
        assert_eq!(
            places,
            [
                ("acme-audio/Resources", true),
                ("acme-audio/Subscribed/acme.test/music-theory", true),
                ("acme-audio/Wiki", false),
                ("acme-audio/Wiki/audio-production", false),
                ("acme-audio/Wiki/music-theory", false),
            ]
        );
        // The default wiki is inside `Wiki`, not a second entry.
        assert!(trees.iter().all(|t| !t.at.place.ends_with("/knowledge")));
        // Names are flat: a name is a directory an agent lands the tree
        // in, and a nested one would sit inside another root's tree.
        assert!(trees.iter().all(|t| !t.name.contains('/')), "{trees:?}");
        assert_eq!(
            trees
                .iter()
                .find(|t| t.name == "Wiki")
                .map(|t| t.tree.clone()),
            Some(org.wiki_dir())
        );
    }

    #[test]
    fn an_org_with_nothing_but_a_vault_has_no_knowledge_trees_to_adopt() {
        let tmp = tempfile::tempdir().unwrap();
        let org = DataRoot::new(tmp.path().to_owned()).org("bare");
        std::fs::create_dir_all(org.vault_dir()).unwrap();
        assert!(org.knowledge_trees().is_empty());
    }
}
