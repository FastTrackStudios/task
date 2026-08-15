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

use links_proto::{Confidence, LinksError, LinksService, NodeRef, TypedLink, Visibility};

#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    inner: Arc<Mutex<Inner>>,
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
        }
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
