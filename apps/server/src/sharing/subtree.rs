//! What a grant on a project exposes — the visible set behind every
//! preview, and the digest an owner confirms it with.
//!
//! The set is computed by a [`Subtree`]. The one here,
//! [`ProjectTreeStub`], is deliberately small: the root project and its
//! sub-projects at any depth. The full subtree of
//! `t[sharing.grant.subtree]` — parts, tasks, adopted File Roots, vault
//! notes under the project's directory — replaces it behind the same
//! trait, and nothing else in the sharing module changes when it does.
//!
//! The digest is one function, [`digest`], over the set's `(kind, id)`
//! pairs. Labels are left out on purpose: renaming a song is not a
//! change in what is shared, and must not make a confirmed preview
//! stale.

use std::collections::{BTreeSet, HashMap};

use project::ProjectService as _;
use sha2::{Digest as _, Sha256};
use share_proto::VisibleItem;
use uuid::Uuid;

/// Computes the visible set of a grant rooted at a project.
pub trait Subtree: Send + Sync + 'static {
    /// Everything a grant on `root` would expose, or `None` when there is
    /// no such project — which the caller answers as absent. Called on
    /// a blocking thread: implementations may walk the disk.
    fn visible(&self, root: Uuid) -> Option<Vec<VisibleItem>>;
}

/// The stand-in subtree: the root project and its sub-projects.
#[derive(Clone)]
pub struct ProjectTreeStub {
    projects: project::ProjectBackend,
}

impl ProjectTreeStub {
    #[must_use]
    pub fn new(projects: project::ProjectBackend) -> Self {
        Self { projects }
    }
}

impl Subtree for ProjectTreeStub {
    fn visible(&self, root: Uuid) -> Option<Vec<VisibleItem>> {
        let all = self.projects.list().ok()?;
        let by_id: HashMap<Uuid, &project::ProjectInfo> = all.iter().map(|p| (p.id, p)).collect();
        let root_info = by_id.get(&root)?;
        let mut children: HashMap<Uuid, Vec<&project::ProjectInfo>> = HashMap::new();
        for p in &all {
            if let Some(parent) = p.parent_id {
                children.entry(parent).or_default().push(p);
            }
        }
        let mut out = vec![item(root_info)];
        let mut seen = BTreeSet::from([root]);
        let mut stack = vec![root];
        while let Some(at) = stack.pop() {
            for child in children.get(&at).into_iter().flatten() {
                // A parent cycle in hand-edited frontmatter must not loop.
                if seen.insert(child.id) {
                    out.push(item(child));
                    stack.push(child.id);
                }
            }
        }
        Some(sorted(out))
    }
}

fn item(p: &project::ProjectInfo) -> VisibleItem {
    VisibleItem {
        kind: "project".into(),
        id: p.id.to_string(),
        label: p.title.clone(),
    }
}

/// The set in its canonical order: by `(kind, id)`.
#[must_use]
pub fn sorted(mut items: Vec<VisibleItem>) -> Vec<VisibleItem> {
    items.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    items.dedup_by(|a, b| a.kind == b.kind && a.id == b.id);
    items
}

/// THE preview digest: lower-case hex SHA-256 over the root and the
/// visible set's `(kind, id)` pairs in canonical order. Order-independent
/// in its input, label-independent, and the only place the digest is
/// computed — the preview shows it and the create checks against it.
// t[impl sharing.grant.preview]
#[must_use]
pub fn digest(root: Uuid, visible: &[VisibleItem]) -> String {
    let mut pairs: Vec<(&str, &str)> = visible
        .iter()
        .map(|v| (v.kind.as_str(), v.id.as_str()))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    let mut h = Sha256::new();
    h.update(b"task.sharing.preview.v1\n");
    h.update(root.to_string().as_bytes());
    h.update(b"\n");
    for (kind, id) in pairs {
        h.update(kind.as_bytes());
        h.update(b"\t");
        h.update(id.as_bytes());
        h.update(b"\n");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(kind: &str, id: &str, label: &str) -> VisibleItem {
        VisibleItem {
            kind: kind.into(),
            id: id.into(),
            label: label.into(),
        }
    }

    #[test]
    fn the_digest_is_the_set_not_its_order_or_labels() {
        let root = Uuid::new_v4();
        let a = [v("project", "1", "Crescendum"), v("part", "2", "Song A")];
        let b = [
            v("part", "2", "Song A (renamed)"),
            v("project", "1", "Crescendum"),
        ];
        assert_eq!(digest(root, &a), digest(root, &b));
        assert_eq!(digest(root, &a).len(), 64);
        let more = [a[0].clone(), a[1].clone(), v("task", "3", "Mix")];
        assert_ne!(
            digest(root, &a),
            digest(root, &more),
            "a new object changes it"
        );
        assert_ne!(
            digest(root, &a),
            digest(Uuid::new_v4(), &a),
            "so does the root"
        );
    }

    #[test]
    fn the_stub_walks_sub_projects_at_any_depth() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = project::ProjectBackend::new(tmp.path());
        let mk = |title: &str, parent: Option<Uuid>| {
            let mut p = project::ProjectInfo {
                title: title.into(),
                parent_id: parent,
                ..Default::default()
            };
            p.id = Uuid::new_v4();
            backend.create(p).unwrap()
        };
        let root = mk("Crescendum", None);
        let song = mk("Song One", Some(root.id));
        let stem = mk("Stems", Some(song.id));
        let other = mk("Somebody Else's Album", None);

        let tree = ProjectTreeStub::new(backend.clone());
        let seen = tree.visible(root.id).expect("root exists");
        let ids: BTreeSet<String> = seen.iter().map(|i| i.id.clone()).collect();
        assert_eq!(
            ids,
            BTreeSet::from([
                root.id.to_string(),
                song.id.to_string(),
                stem.id.to_string()
            ])
        );
        assert!(
            !ids.contains(&other.id.to_string()),
            "other projects stay out"
        );
        assert!(tree.visible(Uuid::new_v4()).is_none(), "no root, no set");
    }
}
