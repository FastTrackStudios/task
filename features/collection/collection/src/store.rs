//! [`Store`] — a file-backed [`CollectionService`] backend.
//!
//! Collections live as JSONL at a single path (`<org>/collections.jsonl`),
//! one [`Collection`] per line, loaded into memory on open and rewritten
//! on mutation — the same shape as the `links` store. Item ordering uses
//! `vault_live::lexorank` fractional-index keys, so an insert or a reorder
//! only recomputes the one moved item's rank (no re-indexing of siblings).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use collection_proto::{
    Collection, CollectionError, CollectionItem, CollectionKind, CollectionService, NodeRef,
    Placement,
};
use vault_live::lexorank;

#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    collections: Vec<Collection>,
}

impl Store {
    /// Open (or start) the store at `path`. A missing file is an empty
    /// store; the file is created on first write.
    #[must_use]
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let collections = std::fs::read_to_string(&path)
            .ok()
            .map(|text| {
                text.lines()
                    .filter(|l| !l.trim().is_empty())
                    .filter_map(|l| serde_json::from_str::<Collection>(l).ok())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            inner: Arc::new(Mutex::new(Inner { path, collections })),
        }
    }
}

impl Inner {
    fn persist(&self) -> Result<(), CollectionError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CollectionError::Io(e.to_string()))?;
        }
        let mut out = String::new();
        for c in &self.collections {
            if let Ok(line) = serde_json::to_string(c) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        std::fs::write(&self.path, out).map_err(|e| CollectionError::Io(e.to_string()))
    }

    fn find_mut(&mut self, id: &str) -> Result<&mut Collection, CollectionError> {
        self.collections
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| CollectionError::NotFound(id.to_string()))
    }
}

/// Compute a lexorank for an item placed after `after` in `items`.
///
/// `items` must be sorted ascending by rank and must NOT contain the item
/// being (re)placed. `after == Some(node)` yields a rank between that
/// node's item and its successor (or past the tail if it's last);
/// `after == None` appends to the tail. Falls back to appending when the
/// `after` node isn't present.
fn rank_after(items: &[CollectionItem], after: Option<&NodeRef>) -> String {
    match after {
        Some(node) => {
            if let Some(i) = items.iter().position(|it| &it.node == node) {
                let lo = &items[i].rank;
                match items.get(i + 1) {
                    Some(next) => {
                        lexorank::between(lo, &next.rank).unwrap_or_else(|| lexorank::after(lo))
                    }
                    None => lexorank::after(lo),
                }
            } else {
                // Unknown anchor — append to the tail.
                append_rank(items)
            }
        }
        None => append_rank(items),
    }
}

/// A rank strictly greater than every current item (or `first()` when empty).
fn append_rank(items: &[CollectionItem]) -> String {
    match items.last() {
        Some(last) => lexorank::after(&last.rank),
        None => lexorank::first(),
    }
}

impl CollectionService for Store {
    fn create(
        &self,
        org: String,
        title: String,
        kind: CollectionKind,
    ) -> Result<Collection, CollectionError> {
        // An unlabelled collection is not a thing a caller ever means.
        // `CollectionKind` stays total so that a legacy or hand-edited
        // row still loads (losing a collection's whole ordering over a
        // blank label would be the worse failure), so the refusal lives
        // here, on the way in, where there is somebody to tell.
        if kind.is_empty() {
            return Err(CollectionError::BadRequest(
                "collection kind is empty; a kind is the caller's own label \
                 and must be a non-blank word"
                    .to_string(),
            ));
        }
        let mut inner = self.inner.lock().expect("collection store poisoned");
        let mut c = Collection::new(org, title, kind);
        c.id = uuid::Uuid::new_v4().to_string();
        inner.collections.push(c.clone());
        inner.persist()?;
        Ok(c)
    }

    fn get(&self, id: &str) -> Result<Option<Collection>, CollectionError> {
        let inner = self.inner.lock().expect("collection store poisoned");
        Ok(inner.collections.iter().find(|c| c.id == id).cloned())
    }

    fn list(
        &self,
        org: String,
        kind: Option<CollectionKind>,
    ) -> Result<Vec<Collection>, CollectionError> {
        let inner = self.inner.lock().expect("collection store poisoned");
        Ok(inner
            .collections
            .iter()
            .filter(|c| c.org == org)
            .filter(|c| kind.as_ref().is_none_or(|k| &c.kind == k))
            .cloned()
            .collect())
    }

    fn add_item(&self, placement: Placement) -> Result<Collection, CollectionError> {
        let mut inner = self.inner.lock().expect("collection store poisoned");
        let c = inner.find_mut(&placement.collection_id)?;
        if c.items.iter().any(|it| it.node == placement.node) {
            return Err(CollectionError::BadRequest(format!(
                "item already present: {}",
                placement.node.to_token()
            )));
        }
        let rank = rank_after(&c.items, placement.after.as_ref());
        c.items.push(CollectionItem::new(placement.node, rank));
        c.sort_items();
        inner.persist()?;
        inner.find_mut(&placement.collection_id).cloned()
    }

    fn remove_item(
        &self,
        collection_id: &str,
        node: NodeRef,
    ) -> Result<Collection, CollectionError> {
        let mut inner = self.inner.lock().expect("collection store poisoned");
        let c = inner.find_mut(collection_id)?;
        let before = c.items.len();
        c.items.retain(|it| it.node != node);
        if c.items.len() == before {
            return Err(CollectionError::NotFound(node.to_token()));
        }
        inner.persist()?;
        inner.find_mut(collection_id).cloned()
    }

    fn reorder(&self, placement: Placement) -> Result<Collection, CollectionError> {
        let mut inner = self.inner.lock().expect("collection store poisoned");
        let c = inner.find_mut(&placement.collection_id)?;
        if !c.items.iter().any(|it| it.node == placement.node) {
            return Err(CollectionError::NotFound(placement.node.to_token()));
        }
        // Compute the new rank against the *other* items (the moving item
        // excluded), then reassign just that one rank.
        let others: Vec<CollectionItem> = c
            .items
            .iter()
            .filter(|it| it.node != placement.node)
            .cloned()
            .collect();
        let rank = rank_after(&others, placement.after.as_ref());
        for it in &mut c.items {
            if it.node == placement.node {
                it.rank = rank;
                break;
            }
        }
        c.sort_items();
        inner.persist()?;
        inner.find_mut(&placement.collection_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use collection_proto::NodeRef;

    fn store() -> Store {
        Store::open(std::env::temp_dir().join(format!("col-{}.jsonl", uuid::Uuid::new_v4())))
    }

    fn nodes(c: &Collection) -> Vec<String> {
        c.items.iter().map(|it| it.node.id.clone()).collect()
    }

    /// Two lines lifted verbatim out of a real planted vault
    /// (`~/.local/share/task-demo/acme/orgs/acme-audio/collections.jsonl`),
    /// written by the enum-era code, plus a third row in the shape the
    /// enum's `Other(String)` escape hatch produced.
    ///
    /// Copied rather than generated on purpose: a fixture this code
    /// wrote would only prove that this code agrees with itself, and the
    /// question the migration turns on is what is *already on somebody's
    /// disk*.
    const LEGACY_JSONL: &str = concat!(
        r#"{"id":"24aacad5-f90f-4546-a896-ac60ed94a511","org":"acme-audio","title":"Chart Library","kind":"Library","items":[{"node":{"domain":"","kind":"Chart","id":"track-one","anchor":""},"rank":"m"},{"node":{"domain":"","kind":"Chart","id":"track-two","anchor":""},"rank":"mm"}]}"#,
        "\n",
        r#"{"id":"e96d2138-0cd2-4545-958b-d127f28f74e1","org":"acme-audio","title":"Album Launch Set","kind":"Setlist","items":[{"node":{"domain":"","kind":"Song","id":"track-one","anchor":""},"rank":"m"}]}"#,
        "\n",
        r#"{"id":"aaaaaaaa-0000-0000-0000-000000000000","org":"acme-audio","title":"Rehearsal Pool","kind":{"Other":"Rehearsal Pool"},"items":[]}"#,
        "\n",
    );

    /// The claim ADR 0004 made about this migration — "existing rows
    /// carry their variant's `as_str()` value, which is already what is
    /// persisted" — was **wrong**, and this test is where that is
    /// established rather than asserted.
    ///
    /// serde never called `as_str()`. An externally-tagged unit variant
    /// writes its Rust variant name, so the bytes on disk say `"Library"`
    /// with a capital L, and `Other(label)` writes the map
    /// `{"Other":"<label>"}` rather than a string. Neither reads back as
    /// a plain `String`.
    ///
    /// What makes the migration free *anyway* is normalisation: the kind
    /// label is ASCII-lowercased on the way in, so the legacy `"Library"`
    /// lands on exactly the `library` that the post-ADR writer produces
    /// for the same collection. No rewrite pass, no version field, no
    /// dual-read window — but a hand-written `Deserialize`, and this
    /// test to keep it honest.
    ///
    /// The stakes are why it is pinned: `Store::open` drops lines it
    /// cannot parse (`filter_map(… .ok())`), so a failure here would not
    /// have surfaced as an error. It would have surfaced as a demo user
    /// opening a library that had silently become empty.
    #[test]
    fn legacy_rows_load_with_their_kinds_folded() {
        let path = std::env::temp_dir().join(format!("col-legacy-{}.jsonl", uuid::Uuid::new_v4()));
        std::fs::write(&path, LEGACY_JSONL).unwrap();

        let s = Store::open(&path);
        let held = s.list("acme-audio".into(), None).unwrap();
        assert_eq!(
            held.len(),
            3,
            "a legacy row failed to parse and was dropped"
        );

        // The capitalised unit variants fold onto the lowercase label the
        // new writer uses — which is what makes the filter keep working
        // across the change.
        let by_title = |t: &str| held.iter().find(|c| c.title == t).unwrap().clone();
        assert_eq!(by_title("Chart Library").kind.as_str(), "library");
        assert_eq!(by_title("Album Launch Set").kind.as_str(), "setlist");
        // And the `Other` map arm, whose free-text label is normalised
        // like any other: identity has to be stable under a shift key.
        assert_eq!(by_title("Rehearsal Pool").kind.as_str(), "rehearsal pool");

        // Filtering — the operation that would have split someone's data
        // — finds the legacy row from the new spelling.
        assert_eq!(
            s.list("acme-audio".into(), Some(CollectionKind::new("library")))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            s.list("acme-audio".into(), Some(CollectionKind::new("LIBRARY")))
                .unwrap()
                .len(),
            1,
            "a kind must not depend on how the caller shifted their keys"
        );

        // Items and their ranks survive intact: the migration touches the
        // label and nothing else about the row.
        assert_eq!(
            nodes(&by_title("Chart Library")),
            ["track-one", "track-two"]
        );

        // Rewriting the file (any mutation persists all rows) emits the
        // normalised string form, so the legacy shapes are gone for good
        // once a store has been written once.
        s.add_item(Placement {
            collection_id: by_title("Album Launch Set").id,
            node: NodeRef::song("track-two"),
            after: None,
        })
        .unwrap();
        let rewritten = std::fs::read_to_string(&path).unwrap();
        assert!(rewritten.contains(r#""kind":"library""#));
        assert!(!rewritten.contains(r#""kind":"Library""#));
        assert!(!rewritten.contains(r#""Other""#));

        // …and a store reopened on the rewritten bytes reads the same
        // kinds, which is the round trip the migration promises.
        let re = Store::open(&path);
        assert_eq!(
            re.list("acme-audio".into(), Some(CollectionKind::new("setlist")))
                .unwrap()
                .len(),
            1
        );
    }

    /// A collection with no kind is refused, because the label is the
    /// only thing a caller tells this store about its own domain and a
    /// blank one carries no information at all.
    #[test]
    fn an_empty_kind_is_refused_on_create() {
        let s = store();
        assert!(matches!(
            s.create("acme".into(), "Nameless".into(), CollectionKind::new("   ")),
            Err(CollectionError::BadRequest(_))
        ));
    }

    #[test]
    fn create_get_list_and_persist() {
        let path = std::env::temp_dir().join(format!("col-{}.jsonl", uuid::Uuid::new_v4()));
        let s = Store::open(&path);
        let lib = s
            .create(
                "acme".into(),
                "Songs".into(),
                CollectionKind::new("library"),
            )
            .unwrap();
        assert!(!lib.id.is_empty());
        s.create(
            "acme".into(),
            "Set A".into(),
            CollectionKind::new("setlist"),
        )
        .unwrap();
        s.create("other".into(), "X".into(), CollectionKind::new("library"))
            .unwrap();

        // Reopen — persisted.
        let re = Store::open(&path);
        assert_eq!(re.get(&lib.id).unwrap().unwrap().title, "Songs");
        assert_eq!(re.list("acme".into(), None).unwrap().len(), 2);
        assert_eq!(
            re.list("acme".into(), Some(CollectionKind::new("setlist")))
                .unwrap()
                .len(),
            1
        );
        assert!(re.get("nope").unwrap().is_none());
    }

    #[test]
    fn add_appends_and_inserts_after() {
        let s = store();
        let c = s
            .create("acme".into(), "Set".into(), CollectionKind::new("setlist"))
            .unwrap();
        let id = c.id;
        let add = |slug: &str, after: Option<NodeRef>| {
            s.add_item(Placement {
                collection_id: id.clone(),
                node: NodeRef::song(slug),
                after,
            })
            .unwrap()
        };
        add("a", None);
        add("b", None);
        add("c", None);
        // Insert "x" after "a".
        let c = add("x", Some(NodeRef::song("a")));
        assert_eq!(nodes(&c), ["a", "x", "b", "c"]);
        // Ranks are strictly ascending and sorted.
        let ranks: Vec<_> = c.items.iter().map(|it| it.rank.clone()).collect();
        let mut sorted = ranks.clone();
        sorted.sort();
        assert_eq!(ranks, sorted);
    }

    #[test]
    fn duplicate_add_rejected() {
        let s = store();
        let c = s
            .create("acme".into(), "L".into(), CollectionKind::new("library"))
            .unwrap();
        let p = Placement {
            collection_id: c.id.clone(),
            node: NodeRef::song("a"),
            after: None,
        };
        s.add_item(p.clone()).unwrap();
        assert!(matches!(s.add_item(p), Err(CollectionError::BadRequest(_))));
    }

    #[test]
    fn reorder_moves_one_item() {
        let s = store();
        let c = s
            .create("acme".into(), "Set".into(), CollectionKind::new("setlist"))
            .unwrap();
        let id = c.id;
        for slug in ["a", "b", "c", "d"] {
            s.add_item(Placement {
                collection_id: id.clone(),
                node: NodeRef::song(slug),
                after: None,
            })
            .unwrap();
        }
        // Move "d" to right after "a".
        let c = s
            .reorder(Placement {
                collection_id: id.clone(),
                node: NodeRef::song("d"),
                after: Some(NodeRef::song("a")),
            })
            .unwrap();
        assert_eq!(nodes(&c), ["a", "d", "b", "c"]);

        // Move "a" to the tail (after = None).
        let c = s
            .reorder(Placement {
                collection_id: id,
                node: NodeRef::song("a"),
                after: None,
            })
            .unwrap();
        assert_eq!(nodes(&c), ["d", "b", "c", "a"]);
    }

    #[test]
    fn remove_and_missing() {
        let s = store();
        let c = s
            .create("acme".into(), "L".into(), CollectionKind::new("library"))
            .unwrap();
        s.add_item(Placement {
            collection_id: c.id.clone(),
            node: NodeRef::song("a"),
            after: None,
        })
        .unwrap();
        let c = s.remove_item(&c.id, NodeRef::song("a")).unwrap();
        assert!(c.items.is_empty());
        assert!(matches!(
            s.remove_item(&c.id, NodeRef::song("a")),
            Err(CollectionError::NotFound(_))
        ));
    }
}
