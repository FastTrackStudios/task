//! [`Store`] — a file-backed [`LinksService`] backend.
//!
//! Links live as JSONL at a single path (`<org>/links.jsonl`), loaded
//! into memory on open and rewritten on mutation. Simple, durable, and
//! human-inspectable; the link count for a personal knowledge base is
//! modest. (Authoritative bulk link data — Bible cross-references etc. —
//! belongs in the read-only resource library, queried separately; this
//! store is for user-asserted links. A CRDT/collab migration can come
//! later.)

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use links_proto::{
    Confidence, LinksError, LinksService, NodeRef, Reach, ResolvedNode, TypedLink, Visibility,
};

/// Who can say where another org's node lives, and whether this reader
/// may see it (ADR 0003).
///
/// A hook rather than something this crate does, because answering it
/// needs three things a link store has no business holding: the map from
/// federation domain to org, the reader's membership rows, and the
/// subscription set. All three are the server's, so the server supplies
/// them — the same shape as `EditsBackend::with_lander` and
/// `ResourcesBackend::with_wikis`.
pub trait NodeHomes: Send + Sync + 'static {
    /// Resolve one reference that names another org. Called only for
    /// references carrying a domain; a local one never reaches here.
    fn resolve(&self, node: &NodeRef) -> ResolvedNode;
}

/// The default: this deployment knows of no org but the caller's own, so
/// every qualified reference is an unknown domain.
///
/// Not a refusal and not an error — a single-org server that has never
/// federated should say "I do not know that name", which is exactly
/// true, rather than pretend the node is missing.
pub struct NoFederation;

impl NodeHomes for NoFederation {
    fn resolve(&self, node: &NodeRef) -> ResolvedNode {
        ResolvedNode::refused(node.clone(), Reach::UnknownDomain)
    }
}

#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    inner: Arc<Mutex<Inner>>,
    homes: Arc<dyn NodeHomes>,
}

struct Inner {
    path: PathBuf,
    links: Vec<TypedLink>,
}

impl Store {
    /// Open (or start) the store at `path`. A missing file is an empty
    /// store; the file is created on first write.
    #[must_use]
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let links = std::fs::read_to_string(&path)
            .ok()
            .map(|text| {
                text.lines()
                    .filter(|l| !l.trim().is_empty())
                    .filter_map(|l| serde_json::from_str::<TypedLink>(l).ok())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            inner: Arc::new(Mutex::new(Inner { path, links })),
            homes: Arc::new(NoFederation),
        }
    }

    /// Attach the resolver that answers for other orgs.
    #[must_use]
    pub fn with_homes(mut self, homes: Arc<dyn NodeHomes>) -> Self {
        self.homes = homes;
        self
    }
}

impl Inner {
    fn persist(&self) -> Result<(), LinksError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| LinksError::Io(e.to_string()))?;
        }
        let mut out = String::new();
        for l in &self.links {
            if let Ok(line) = serde_json::to_string(l) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        std::fs::write(&self.path, out).map_err(|e| LinksError::Io(e.to_string()))
    }
}

impl LinksService for Store {
    fn create(&self, mut link: TypedLink) -> Result<TypedLink, LinksError> {
        let mut inner = self.inner.lock().expect("links store poisoned");
        if link.id.trim().is_empty() {
            link.id = uuid::Uuid::new_v4().to_string();
        }
        if link.provenance.created_at.trim().is_empty() {
            link.provenance.created_at = chrono::Utc::now().to_rfc3339();
        }
        inner.links.push(link.clone());
        inner.persist()?;
        Ok(link)
    }

    fn delete(&self, id: &str) -> Result<(), LinksError> {
        let mut inner = self.inner.lock().expect("links store poisoned");
        let before = inner.links.len();
        inner.links.retain(|l| l.id != id);
        if inner.links.len() == before {
            return Err(LinksError::NotFound(id.to_string()));
        }
        inner.persist()
    }

    fn get(&self, id: &str) -> Result<TypedLink, LinksError> {
        let inner = self.inner.lock().expect("links store poisoned");
        inner
            .links
            .iter()
            .find(|l| l.id == id)
            .cloned()
            .ok_or_else(|| LinksError::NotFound(id.to_string()))
    }

    fn links_for(&self, node: NodeRef) -> Result<Vec<TypedLink>, LinksError> {
        let inner = self.inner.lock().expect("links store poisoned");
        Ok(inner
            .links
            .iter()
            .filter(|l| l.touches(&node))
            .cloned()
            .collect())
    }

    fn graph(
        &self,
        min_confidence: Confidence,
        include_private: bool,
    ) -> Result<Vec<TypedLink>, LinksError> {
        let inner = self.inner.lock().expect("links store poisoned");
        Ok(inner
            .links
            .iter()
            .filter(|l| l.confidence >= min_confidence)
            .filter(|l| include_private || l.visibility != Visibility::Private)
            .cloned()
            .collect())
    }

    fn resolve_nodes(&self, nodes: Vec<NodeRef>) -> Result<Vec<ResolvedNode>, LinksError> {
        // Local references never leave this process: the caller is
        // already inside the org that holds them, so the answer is the
        // reference itself. Only a domain sends the question outward.
        Ok(nodes
            .into_iter()
            .map(|node| {
                if node.is_local() {
                    ResolvedNode::refused(node, Reach::Local)
                } else {
                    self.homes.resolve(&node)
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use links_proto::{NodeKind, Relation};

    fn link(from: &str, to: &str, conf: Confidence, vis: Visibility) -> TypedLink {
        let mut l = TypedLink::new(
            NodeRef::verse(from),
            NodeRef::verse(to),
            Relation::CrossRef,
            conf,
        );
        l.visibility = vis;
        l
    }

    #[test]
    fn create_persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("links.jsonl");
        let store = Store::open(&path);
        let saved = store
            .create(link(
                "John.3.16",
                "Romans.5.8",
                Confidence::Likely,
                Visibility::Public,
            ))
            .unwrap();
        assert!(!saved.id.is_empty(), "id assigned");
        assert!(!saved.provenance.created_at.is_empty(), "timestamp stamped");

        // Reopen — persisted to disk.
        let reopened = Store::open(&path);
        assert_eq!(reopened.get(&saved.id).unwrap(), saved);
    }

    #[test]
    fn links_for_finds_either_endpoint() {
        let store =
            Store::open(std::env::temp_dir().join(format!("lf-{}.jsonl", uuid::Uuid::new_v4())));
        store
            .create(link(
                "John.3.16",
                "Romans.5.8",
                Confidence::Likely,
                Visibility::Public,
            ))
            .unwrap();
        let got = store
            .links_for(NodeRef::new(NodeKind::Verse, "Romans.5.8"))
            .unwrap();
        assert_eq!(got.len(), 1);
        assert!(
            store
                .links_for(NodeRef::verse("Genesis.1.1"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn graph_filters_by_confidence_and_visibility() {
        let store =
            Store::open(std::env::temp_dir().join(format!("g-{}.jsonl", uuid::Uuid::new_v4())));
        store
            .create(link(
                "A.1.1",
                "B.1.1",
                Confidence::Certain,
                Visibility::Public,
            ))
            .unwrap();
        store
            .create(link(
                "A.1.1",
                "C.1.1",
                Confidence::Speculative,
                Visibility::Public,
            ))
            .unwrap();
        store
            .create(link(
                "A.1.1",
                "D.1.1",
                Confidence::Certain,
                Visibility::Private,
            ))
            .unwrap();

        // Publishable graph at >= Likely: only the Certain+Public one.
        let pub_graph = store.graph(Confidence::Likely, false).unwrap();
        assert_eq!(pub_graph.len(), 1);
        // Private view includes the private Certain one.
        assert_eq!(store.graph(Confidence::Likely, true).unwrap().len(), 2);
    }

    #[test]
    fn delete_removes() {
        let store =
            Store::open(std::env::temp_dir().join(format!("d-{}.jsonl", uuid::Uuid::new_v4())));
        let l = store
            .create(link(
                "A.1.1",
                "B.1.1",
                Confidence::Likely,
                Visibility::Public,
            ))
            .unwrap();
        store.delete(&l.id).unwrap();
        assert!(store.get(&l.id).is_err());
        assert!(matches!(store.delete(&l.id), Err(LinksError::NotFound(_))));
    }
}
