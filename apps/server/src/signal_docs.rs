//! Signal's rig in Task (spec §7 S5): Signal writes, everyone else reads.
//!
//! Signal keeps its library (rigs, patches, profiles, songs, setlists,
//! and the `library:settings` default-profile pointer) in its own SQLite
//! repos on the device: those stay the source of truth. Signal mirrors
//! them into the org here, **one Loro doc per entity** (ADR 0001 in the
//! studio repo, measured in S4), and agents and other devices read them
//! with `doc_read` — `signal:patch:<slug>`, `signal:song:<id>`, … — the
//! same JSON-plus-version shape a live song reads as.
//!
//! **The contract with Signal** (studio `signal_task_mirror`; both sides
//! derive the ids from the org slug and the key, so nothing is negotiated
//! on the wire):
//!
//! ```text
//! doc id   = uuid v5(NAMESPACE, "<org>/signal/<key>")
//! key      = "index" | "<kind>:<id>", kind ∈ rig patch profile song setlist library
//! index    : entities : Map  key → { kind: str, deleted: bool, at: i64 (ms) }
//!            meta     : Map  { schema: i64, writer: "signal:<device>" }
//! entity   : meta     : Map  { schema: i64 }
//!            <kind>   : Map  key → the entity (signal-proto's doc codec)
//! ```
//!
//! - **Admission.** Only the index's id is served from the start. An
//!   entity's doc is served once the index names it: Signal writes the
//!   index first, then the entity docs. Any other id is not this host's
//!   and falls through to the vault's registry.
//! - **Deletes are tombstones.** A deleted entity stays in the index with
//!   `deleted: true`, and its doc drops the entity (its history stays).
//!   `doc_read` says `deleted` instead of "no such doc", so an agent can
//!   tell a removed patch from a typo.
//! - **One writer.** Agents read; `doc_apply` refuses a `signal:` ref
//!   ([`crate::mcp`]): Signal's SQLite is the truth and a doc edit nobody
//!   applies to it would be lost on the next mirror. Two-way editing is
//!   S7's. Over `DocSync` the docs are served to the org's members like
//!   the vault's — Signal is the app that writes them.
//! - **Durable.** Unlike a live set's songs, these docs persist under the
//!   org's `signal-docs/` (one directory per doc), so a device that joins
//!   cold gets the rig even when no Signal is online.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crdt::CrdtDoc;
use crdt::fs::FilePersistence;
use crdt::loro::LoroValue;
use crdt::registry::DocRegistry;
use crdt::sync::{SyncDown, SyncError};
use serde_json::{Value, json};
use uuid::Uuid;

/// The namespace Signal's doc ids are made in (`signal-docs-task`). The
/// studio side holds the same constant; a test on each side pins it.
pub const NAMESPACE: Uuid = Uuid::from_u128(0x7369_676e_616c_2d64_6f63_732d_7461_736b);
/// The index doc's key.
pub const INDEX_KEY: &str = "index";
/// What a doc ref of a Signal doc starts with.
pub const REF_PREFIX: &str = "signal:";
/// The kinds of entity Signal mirrors — the root map each entity doc
/// keeps its entity in, and the prefix of its key.
pub const KINDS: [&str; 6] = ["rig", "patch", "profile", "song", "setlist", "library"];
/// How long a doc nobody syncs stays open (it is on disk either way).
const IDLE: Duration = Duration::from_secs(15 * 60);
/// Fold a doc's update log into a snapshot every this many updates.
const COMPACT_EVERY: u32 = 256;

/// A Signal doc's id in org `org`: `key` is [`INDEX_KEY`] or an entity
/// key (`patch:worship-lead`).
#[must_use]
pub fn doc_id(org: &str, key: &str) -> Uuid {
    Uuid::new_v5(&NAMESPACE, format!("{org}/signal/{key}").as_bytes())
}

/// The kind an entity key names (`patch:x` → `patch`), or `None` for
/// anything that is not one.
#[must_use]
pub fn kind_of(key: &str) -> Option<&'static str> {
    let (prefix, rest) = key.split_once(':')?;
    if rest.is_empty() {
        return None;
    }
    KINDS.into_iter().find(|k| *k == prefix)
}

/// A key's doc ref, `signal:<key>`.
#[must_use]
pub fn doc_ref(key: &str) -> String {
    format!("{REF_PREFIX}{key}")
}

/// Whether `raw` is a Signal doc ref.
#[must_use]
pub fn is_ref(raw: &str) -> bool {
    raw.trim().starts_with(REF_PREFIX)
}

/// One index entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: String,
    pub deleted: bool,
    /// When Signal last changed it, ms since the epoch (0 when unsaid).
    pub at: i64,
}

struct Inner {
    org: String,
    registry: DocRegistry,
    /// Every id served: the index's, and each entity's the index names.
    known: Arc<Mutex<HashMap<Uuid, String>>>,
}

/// An org's Signal docs.
#[derive(Clone)]
pub struct SignalHost {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl SignalHost {
    /// The Signal docs of org `org`, persisted under `root` (one
    /// directory per doc). Must be made inside a tokio runtime (the idle
    /// sweeper is a task).
    #[must_use]
    pub fn new(org: String, root: PathBuf) -> Self {
        let known: Arc<Mutex<HashMap<Uuid, String>>> = Arc::new(Mutex::new(HashMap::from([(
            doc_id(&org, INDEX_KEY),
            INDEX_KEY.to_owned(),
        )])));
        let admit = Arc::clone(&known);
        let registry = DocRegistry::new(move |id| {
            let root = root.clone();
            Box::pin(async move { CrdtDoc::open(id, FilePersistence::new(root)).await })
        })
        .with_admission(move |id| lock(&admit).contains_key(&id))
        .with_compaction(COMPACT_EVERY)
        .with_idle_eviction(IDLE);
        Self {
            inner: Arc::new(Inner {
                org,
                registry,
                known,
            }),
        }
    }

    /// The org these docs are of.
    #[must_use]
    pub fn org(&self) -> &str {
        &self.inner.org
    }

    /// The docs' registry.
    #[must_use]
    pub fn registry(&self) -> &DocRegistry {
        &self.inner.registry
    }

    /// The index's id.
    #[must_use]
    pub fn index_id(&self) -> Uuid {
        doc_id(&self.inner.org, INDEX_KEY)
    }

    /// The key `doc_id` is served under, if it is one of these docs —
    /// the index read again when it is not known yet (Signal named a new
    /// entity since).
    pub async fn admits(&self, doc_id: Uuid) -> Option<String> {
        let hit = lock(&self.inner.known).get(&doc_id).cloned();
        if hit.is_some() {
            return hit;
        }
        self.refresh().await;
        lock(&self.inner.known).get(&doc_id).cloned()
    }

    /// The index as it is now, by key.
    ///
    /// # Errors
    ///
    /// The index could not be opened.
    pub async fn index(&self) -> Result<BTreeMap<String, Entry>, SyncError> {
        let doc = self.inner.registry.doc(self.index_id()).await?;
        Ok(read_index(&doc.loro().get_map("entities").get_deep_value()))
    }

    /// Learn every entity the index names.
    async fn refresh(&self) {
        let Ok(index) = self.index().await else {
            return;
        };
        let mut known = lock(&self.inner.known);
        for key in index.keys() {
            known.insert(doc_id(&self.inner.org, key), key.clone());
        }
    }

    /// Serve `doc_id` over `DocSync` (see the module docs: the org's
    /// members sync these docs; agents read them over MCP).
    ///
    /// # Errors
    ///
    /// Not one of these docs, or the sync session failed.
    pub async fn sync(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        architect_telemetry::wide::set("signal.sync", "member");
        self.inner.registry.sync(doc_id, from, up, down).await
    }
}

/// The index's `entities` map, read leniently: an entry that is not a
/// map, or a key that is not an entity's, is skipped.
fn read_index(entities: &LoroValue) -> BTreeMap<String, Entry> {
    let LoroValue::Map(map) = entities else {
        return BTreeMap::new();
    };
    map.iter()
        .filter_map(|(key, v)| {
            let kind = kind_of(key)?;
            let LoroValue::Map(entry) = v else {
                return None;
            };
            let deleted = matches!(entry.get("deleted"), Some(LoroValue::Bool(true)));
            let at = match entry.get("at") {
                Some(LoroValue::I64(n)) => *n,
                #[allow(clippy::cast_possible_truncation)]
                Some(LoroValue::Double(f)) => *f as i64,
                _ => 0,
            };
            Some((
                key.clone(),
                Entry {
                    kind: kind.to_owned(),
                    deleted,
                    at,
                },
            ))
        })
        .collect()
}

/// A served doc, on the blocking pool (MCP tools run there).
fn served(host: &SignalHost, id: Uuid) -> Result<CrdtDoc, String> {
    tokio::runtime::Handle::current()
        .block_on(host.registry().doc(id))
        .map_err(|e| format!("the doc is not served: {e}"))
}

fn index_now(host: &SignalHost) -> Result<BTreeMap<String, Entry>, String> {
    tokio::runtime::Handle::current()
        .block_on(host.index())
        .map_err(|e| format!("Signal's index could not be read: {e}"))
}

/// The key a `signal:` ref names (`signal:patch:x` → `patch:x`).
///
/// # Errors
///
/// Not a Signal doc ref, or not one of its keys.
pub fn key_of(raw: &str) -> Result<String, String> {
    let key = raw
        .trim()
        .strip_prefix(REF_PREFIX)
        .ok_or_else(|| format!("`{raw}` is not a Signal doc ref: write signal:<kind>:<id>"))?;
    if key == INDEX_KEY || kind_of(key).is_some() {
        Ok(key.to_owned())
    } else {
        Err(format!(
            "`{raw}` is not a Signal doc ref: write signal:<kind>:<id> with kind one of {} \
             (signal:index lists them)",
            KINDS.join(", ")
        ))
    }
}

// t[impl mcp.signal.read]
/// `doc_read` of a `signal:` ref: an entity as JSON with its version and
/// whether it was deleted, or (`signal:index`) every entity Signal
/// mirrors, each with its doc ref.
///
/// # Errors
///
/// A bad ref, an entity the index does not name, or a doc not served.
pub fn read(host: &SignalHost, raw_ref: &str) -> Result<Value, String> {
    let key = key_of(raw_ref)?;
    let index = index_now(host)?;
    if key == INDEX_KEY {
        let doc = served(host, host.index_id())?;
        let writer = doc
            .loro()
            .get_map("meta")
            .get("writer")
            .map_or(Value::Null, |v| crate::live_docs::to_json(&v.get_deep_value()));
        let entities: Vec<Value> = index
            .iter()
            .map(|(key, e)| {
                json!({
                    "key": key,
                    "kind": e.kind,
                    "doc_ref": doc_ref(key),
                    "deleted": e.deleted,
                    "at": e.at,
                })
            })
            .collect();
        return Ok(json!({
            "doc_ref": doc_ref(INDEX_KEY),
            "doc_id": host.index_id().to_string(),
            "version": crate::live_docs::version(doc.loro()),
            "writer": writer,
            "read_only": true,
            "entities": entities,
            "note": "Signal's library, mirrored from its device (Signal is the one writer). \
                     doc_read an entity's doc_ref for its JSON; a deleted one reads as deleted.",
        }));
    }
    let Some(entry) = index.get(&key) else {
        return Err(format!(
            "Signal has no `{key}` in this org; doc_read signal:index lists what it mirrors"
        ));
    };
    let id = doc_id(host.org(), &key);
    // The index names it, so it is admitted (learnt now if it is new).
    lock(&host.inner.known).insert(id, key.clone());
    let doc = served(host, id)?;
    let loro = doc.loro();
    let value = loro
        .get_map(entry.kind.as_str())
        .get(&key)
        .map_or(Value::Null, |v| crate::live_docs::to_json(&v.get_deep_value()));
    let schema = loro
        .get_map("meta")
        .get("schema")
        .map_or(Value::Null, |v| crate::live_docs::to_json(&v.get_deep_value()));
    // A tombstone, or an entity whose doc has not arrived yet.
    let deleted = entry.deleted;
    Ok(json!({
        "doc_ref": doc_ref(&key),
        "doc_id": id.to_string(),
        "key": key,
        "kind": entry.kind,
        "version": crate::live_docs::version(loro),
        "schema": schema,
        "deleted": deleted,
        "at": entry.at,
        "read_only": true,
        "value": if deleted { Value::Null } else { value },
        "note": if deleted {
            "Deleted in Signal: a tombstone. Its history stays in the doc."
        } else {
            "Mirrored from Signal (its SQLite library is the truth; Signal is the one writer)."
        },
    }))
}

/// Why `doc_apply` refuses a `signal:` ref.
pub const ONE_WRITER: &str = "Signal's docs have one writer, Signal itself (spec §7 S5): \
     its library on the device is the truth, and an edit made here would be lost on its \
     next mirror. doc_read them; change a patch or song in Signal.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_the_contract_with_signal() {
        // Pinned: the studio side (`signal_task_mirror::doc_id`) asserts
        // the same values.
        assert_eq!(
            NAMESPACE.to_string(),
            "7369676e-616c-2d64-6f63-732d7461736b"
        );
        assert_eq!(
            doc_id("acme", "patch:worship-lead"),
            Uuid::new_v5(&NAMESPACE, b"acme/signal/patch:worship-lead")
        );
        assert_ne!(doc_id("acme", "index"), doc_id("other", "index"));
    }

    #[test]
    fn keys_name_a_kind() {
        assert_eq!(kind_of("patch:worship-lead"), Some("patch"));
        assert_eq!(kind_of("library:settings"), Some("library"));
        assert_eq!(kind_of("patch:"), None);
        assert_eq!(kind_of("index"), None);
        assert_eq!(kind_of("chart:x"), None);
        assert_eq!(key_of("signal:index").unwrap(), "index");
        assert_eq!(key_of(" signal:song:abc ").unwrap(), "song:abc");
        assert!(key_of("live:set/song").is_err());
        assert!(key_of("signal:nope:x").is_err());
    }

    #[test]
    fn the_index_reads_tombstones_and_skips_junk() {
        let doc = crdt::loro::LoroDoc::new();
        let entities = doc.get_map("entities");
        let live = entities
            .insert_container("patch:a", crdt::loro::LoroMap::new())
            .unwrap();
        live.insert("kind", "patch").unwrap();
        live.insert("deleted", false).unwrap();
        live.insert("at", 5_i64).unwrap();
        let gone = entities
            .insert_container("song:b", crdt::loro::LoroMap::new())
            .unwrap();
        gone.insert("kind", "song").unwrap();
        gone.insert("deleted", true).unwrap();
        entities.insert("junk", 1).unwrap();
        entities.insert("patch:c", 1).unwrap();
        doc.commit();
        let index = read_index(&entities.get_deep_value());
        assert_eq!(index.len(), 2, "{index:?}");
        assert!(!index["patch:a"].deleted);
        assert_eq!(index["patch:a"].at, 5);
        assert!(index["song:b"].deleted);
    }
}
