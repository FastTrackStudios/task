//! App documents per org: architect's `crdt::docs`, served here.
//!
//! An app (Studio first) keeps its data as Loro docs: a project, a
//! setlist, a rig. It registers each one in its org's catalog (the
//! personal org for the person's own things) and syncs it over the
//! org's one `DocSync` lane. The server stores and relays the bytes and
//! the catalog row and never reads a doc: `kind` is the app's word, not
//! one Task interprets.
//!
//! - Storage: `<org>/app_docs.sqlite` (`OrgRoot::app_docs_db`). It holds
//!   the catalog (`crdt_doc_meta`) and the bytes (`crdt_doc`,
//!   `crdt_update`), migrated by `crdt_seaorm::Migrator`.
//! - Access: the org's permits gate (`permits::APP_DOCS`): members read
//!   and write, and a personal org has only its owner. The host's
//!   per-doc authorizer is left at its default; sharing a doc beyond its
//!   org is a later grant table consulted there.
//! - Content: [`OrgDocSync`] sends a catalogued id to the host, and
//!   anything else to the vault's registry.
//! - Belonging: a project names the docs that are its own with a link,
//!   `project:<path>` → `doc:<uuid>` ([`links::NodeKind::AppDoc`],
//!   relation `Down`), in the org's links store. [`attach`],
//!   [`detach`] and [`project_docs`] are that, with the catalog checked
//!   on the way in and read on the way out. Nothing here looks at
//!   `kind`: a project holds whatever docs its apps made.

use std::sync::Arc;
use std::time::Duration;

use app_docs_proto::{DocCard, DocFace, read_face};
use crdt::Persistence as _;
use crdt::docs::{DocMeta, DocQuery, Docs as _, DocsError, DocsHost};
use crdt::loro;
use crdt::sync::{DocSync, SyncDown, SyncError};
use links::{
    Confidence, LinksService, NodeHomes, NodeKind, NodeRef, Reach, Relation, ResolvedNode,
    TypedLink,
};
use uuid::Uuid;

/// Fold a doc's update log into one snapshot every this many updates.
const COMPACT_EVERY: u32 = 64;
/// Close a doc nobody has attached to for this long.
const IDLE: Duration = Duration::from_secs(15 * 60);

// t[impl offline.app-docs.replica] — the catalog and bytes in the org's
// own database, compacted and evicted when idle
// t[impl offline.app-docs.tombstone] — `DocsHost` keeps deletes as
// tombstones and stops serving the content
/// The org's app-docs host over its database (already migrated).
pub async fn open(db: sea_orm::DatabaseConnection) -> eyre::Result<DocsHost> {
    let bytes = crdt_seaorm::SeaOrmPersistence::new(db.clone());
    let registry = crdt::DocRegistry::new(move |doc_id| {
        let bytes = bytes.clone();
        Box::pin(async move { crdt::CrdtDoc::open(doc_id, bytes).await })
    })
    .with_compaction(COMPACT_EVERY)
    .with_idle_eviction(IDLE);
    DocsHost::open(crdt_seaorm::SeaOrmDocsStore::new(db), registry)
        .await
        .map_err(|e| eyre::eyre!("open app docs: {e}"))
}

/// The org's `DocSync` below the live sessions: app docs to their host,
/// every other id to the vault's registry (its own admission refuses an
/// id it never registered).
#[derive(Clone)]
pub struct OrgDocSync {
    pub docs: DocsHost,
    pub vault: vault_collab::VaultCollab,
}

impl DocSync for OrgDocSync {
    // t[impl offline.app-docs.create] — a catalogued id syncs only once
    // registered; the host admits nothing else
    async fn sync(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        if self.docs.admits(doc_id) {
            architect_telemetry::wide::set("doc.lane", "app-docs");
            self.docs.sync(doc_id, from, up, down).await
        } else {
            self.vault.sync(doc_id, from, up, down).await
        }
    }
}

// ── A project's docs ─────────────────────────────────────────────────

/// The reference an app doc goes by: `doc:<uuid>`, local.
#[must_use]
pub fn doc_node(doc_id: Uuid) -> NodeRef {
    NodeRef::new(NodeKind::AppDoc, doc_id.to_string())
}

/// The reference a project goes by: `project:<path>`, its path on the
/// Projects tier (`example-album`, `example-album/track-two`). Slashes
/// at either end are dropped; an empty segment, `.` or `..` is refused,
/// since the path is the project's address and those name nothing.
pub fn project_node(project: &str) -> Result<NodeRef, String> {
    let path = project.trim().trim_matches('/');
    if path.is_empty()
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(format!(
            "`{project}` is not a project path (its path on the Projects tier, \
             e.g. `example-album` or `example-album/track-two`)"
        ));
    }
    Ok(NodeRef::new(NodeKind::Project, path))
}

/// The catalog's live rows, oldest change first, at most `limit`;
/// `kind` narrows to one app's word. Tombstones are left out.
pub async fn list(
    docs: &DocsHost,
    kind: Option<String>,
    limit: usize,
) -> Result<Vec<DocMeta>, DocsError> {
    let mut out = Vec::new();
    let mut since_rev = 0;
    loop {
        let page = docs
            .list(DocQuery {
                kind: kind.clone(),
                since_rev,
                include_deleted: false,
                limit: 256,
            })
            .await?;
        out.extend(page.docs.into_iter().filter(|d| !d.deleted));
        if !page.more || out.len() >= limit {
            break;
        }
        since_rev = page.cursor;
    }
    out.truncate(limit);
    Ok(out)
}

/// Every link by which `project` holds an app doc.
fn attached(links: &impl LinksService, project: &NodeRef) -> Result<Vec<TypedLink>, String> {
    let held = links
        .links_for(project.clone())
        .map_err(|e| format!("couldn't read the links of `{}`: {e}", project.id))?;
    Ok(held
        .into_iter()
        .filter(|l| {
            l.source == *project
                && l.relation == Relation::Down
                && l.target.kind == NodeKind::AppDoc
                && l.target.is_local()
        })
        .collect())
}

/// Attach a catalogued doc to `project`: the `Down` link, made once. An
/// identical link already held is returned rather than duplicated. A
/// doc the catalog does not hold, or holds as deleted, is refused: a
/// link to nothing would list as a broken entry from the moment it was
/// made. `by` is who asked, for the link's provenance.
pub async fn attach(
    docs: &DocsHost,
    links: &impl LinksService,
    project: &NodeRef,
    doc_id: Uuid,
    by: &str,
) -> Result<(TypedLink, DocMeta), String> {
    let meta = match docs.get(doc_id).await {
        Ok(m) if !m.deleted => m,
        Ok(_) | Err(DocsError::Deleted) => {
            return Err(format!(
                "app doc `{doc_id}` was deleted; a deleted doc cannot be attached"
            ));
        }
        Err(DocsError::NotFound) => {
            return Err(format!(
                "no app doc `{doc_id}` in this org's catalog; list_app_docs shows the docs it holds"
            ));
        }
        Err(e) => return Err(format!("couldn't read app doc `{doc_id}`: {e}")),
    };
    let doc = doc_node(doc_id);
    if let Some(held) = attached(links, project)?
        .into_iter()
        .find(|l| l.target == doc)
    {
        return Ok((held, meta));
    }
    let mut link = TypedLink::new(project.clone(), doc, Relation::Down, Confidence::Certain);
    link.provenance.created_by = by.to_owned();
    let made = links
        .create(link)
        .map_err(|e| format!("couldn't attach `{doc_id}` to `{}`: {e}", project.id))?;
    Ok((made, meta))
}

/// Detach a doc from `project`: every link by which the project holds
/// it. Works whether or not the doc is still in the catalog, so a link
/// to a deleted doc can be cleared. Returns how many links went; none is
/// an error, naming what was asked.
pub fn detach(links: &impl LinksService, project: &NodeRef, doc_id: Uuid) -> Result<usize, String> {
    let doc = doc_node(doc_id);
    let held: Vec<TypedLink> = attached(links, project)?
        .into_iter()
        .filter(|l| l.target == doc)
        .collect();
    if held.is_empty() {
        return Err(format!(
            "app doc `{doc_id}` is not attached to project `{}`",
            project.id
        ));
    }
    for l in &held {
        links
            .delete(&l.id)
            .map_err(|e| format!("couldn't detach `{doc_id}`: {e}"))?;
    }
    Ok(held.len())
}

/// One doc a project holds, as the catalog has it now.
pub struct ProjectDoc {
    /// The link that attaches it.
    pub link: TypedLink,
    /// The catalog row: `None` when the catalog has no such id (or the
    /// link's id is not a UUID), a row with `deleted` set when the doc
    /// was deleted after it was attached.
    pub meta: Option<DocMeta>,
}

/// Every doc `project` holds, each with its catalog row. A link whose
/// doc is gone is kept and says so (see [`ProjectDoc::meta`]): dropping
/// it would hide a broken entry its owner can clear with [`detach`].
pub async fn project_docs(
    docs: &DocsHost,
    links: &impl LinksService,
    project: &NodeRef,
) -> Result<Vec<ProjectDoc>, String> {
    let mut out = Vec::new();
    for link in attached(links, project)? {
        let meta = match Uuid::parse_str(&link.target.id) {
            Ok(id) => match docs.get(id).await {
                Ok(m) => Some(m),
                Err(DocsError::NotFound) => None,
                Err(e) => return Err(format!("couldn't read app doc `{id}`: {e}")),
            },
            Err(_) => None,
        };
        out.push(ProjectDoc { link, meta });
    }
    Ok(out)
}

// ── Faces ────────────────────────────────────────────────────────────

/// What each doc shows of itself: its `face` map
/// ([`app_docs_proto::read_face`]), the one part of an app's content
/// Task reads. A doc the host has open is read where it is; any other
/// is read from its stored bytes into a throwaway `LoroDoc`, so listing
/// an org's docs does not open (and keep resident) every one of them.
#[derive(Clone)]
pub struct Faces {
    pub docs: DocsHost,
    bytes: crdt_seaorm::SeaOrmPersistence,
}

impl Faces {
    /// The faces of `docs`, whose bytes are in `db` (the connection
    /// [`open`] was given).
    #[must_use]
    pub const fn new(docs: DocsHost, db: sea_orm::DatabaseConnection) -> Self {
        Self {
            docs,
            bytes: crdt_seaorm::SeaOrmPersistence::new(db),
        }
    }

    /// `doc_id`'s face, `None` when it keeps none. Errors only when the
    /// stored bytes cannot be read or imported; what the app wrote in
    /// the face never fails the read.
    pub async fn read(&self, doc_id: Uuid) -> Result<Option<DocFace>, String> {
        let registry = self.docs.registry();
        if registry.is_open(doc_id).await
            && let Ok(doc) = registry.doc(doc_id).await
        {
            return Ok(read_face(doc.loro()));
        }
        let doc = loro::LoroDoc::new();
        let snapshot = self
            .bytes
            .load_snapshot(doc_id)
            .await
            .map_err(|e| e.to_string())?;
        let updates = self
            .bytes
            .load_updates(doc_id)
            .await
            .map_err(|e| e.to_string())?;
        for bytes in snapshot.iter().chain(&updates) {
            doc.import(bytes).map_err(|e| format!("import: {e}"))?;
        }
        Ok(read_face(&doc))
    }

    /// `meta` with its face. A face that cannot be read leaves the card
    /// faceless (titled by the catalog's name) rather than failing a
    /// list over one bad doc; `Err` carries why, for the caller's span.
    pub async fn card(&self, meta: DocMeta) -> (DocCard, Result<(), String>) {
        let (face, read) = match self.read(meta.doc_id).await {
            Ok(face) => (face, Ok(())),
            Err(e) => (None, Err(e)),
        };
        (
            DocCard::new(meta.doc_id, meta.kind, meta.name, meta.updated_us, face),
            read,
        )
    }
}

/// Answers a local `doc:<uuid>` from the org's catalog and hands every
/// other reference to `rest` (the federated resolver, or none).
///
/// A live doc is [`Reach::Local`], like any reference in the reader's
/// own org; one the catalog does not hold, or holds as deleted, is
/// [`Reach::NotFound`]. The title stays empty: the catalog's name is an
/// async read and [`NodeHomes`] is sync, so this asks only what the
/// host knows in memory (`DocsHost::admits`). A caller that wants the
/// name reads the row ([`project_docs`] does).
pub struct CatalogHomes {
    pub docs: DocsHost,
    pub rest: Arc<dyn NodeHomes>,
}

impl NodeHomes for CatalogHomes {
    fn resolve(&self, node: &NodeRef) -> ResolvedNode {
        self.rest.resolve(node)
    }

    fn resolve_local(&self, node: &NodeRef) -> ResolvedNode {
        if node.kind != NodeKind::AppDoc {
            return self.rest.resolve_local(node);
        }
        let live = Uuid::parse_str(&node.id).is_ok_and(|id| self.docs.admits(id));
        ResolvedNode::refused(
            node.clone(),
            if live { Reach::Local } else { Reach::NotFound },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_docs_proto::{FaceProp, FacePropType, FaceValue, write_face};
    use crdt::docs::{CreateDoc, Lineage};
    use sea_orm_migration::MigratorTrait as _;

    async fn host(dir: &std::path::Path) -> (DocsHost, sea_orm::DatabaseConnection) {
        let url = format!(
            "sqlite://{}?mode=rwc",
            dir.join("app_docs.sqlite").display()
        );
        let db = sea_orm::Database::connect(url).await.unwrap();
        crdt_seaorm::Migrator::up(&db, None).await.unwrap();
        (open(db.clone()).await.unwrap(), db)
    }

    fn face() -> DocFace {
        DocFace {
            title: "Sunday 10/26".into(),
            subtitle: Some("The Barn".into()),
            colour: None,
            props: vec![FaceProp {
                key: "entries".into(),
                label: "Entries".into(),
                ty: FacePropType::Number,
                value: FaceValue::Integer(12),
            }],
            links: vec![],
        }
    }

    async fn register(docs: &DocsHost, kind: &str, name: &str) -> Uuid {
        let doc_id = Uuid::new_v4();
        docs.create(CreateDoc {
            doc_id,
            kind: kind.into(),
            name: name.into(),
            parent: None,
            lineage: Lineage::Strict,
        })
        .await
        .unwrap();
        doc_id
    }

    #[tokio::test]
    async fn a_face_reads_from_an_open_doc_and_from_stored_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let (docs, db) = host(tmp.path()).await;
        let id = register(&docs, "app.list", "untitled").await;
        let doc = docs.registry().doc(id).await.unwrap();
        write_face(doc.loro(), &face()).unwrap();
        doc.loro().commit();
        doc.compact(id).await.unwrap();

        let faces = Faces::new(docs, db.clone());
        assert_eq!(faces.read(id).await.unwrap(), Some(face()), "open");

        // A second host over the same file has nothing open: the read
        // goes to the stored bytes and leaves the doc closed.
        let cold = Faces::new(open(db.clone()).await.unwrap(), db);
        assert_eq!(cold.read(id).await.unwrap(), Some(face()), "stored");
        assert!(!cold.docs.registry().is_open(id).await, "not kept resident");
    }

    #[tokio::test]
    async fn a_doc_without_a_face_is_titled_by_its_name() {
        let tmp = tempfile::tempdir().unwrap();
        let (docs, db) = host(tmp.path()).await;
        let id = register(&docs, "app.list", "Bare").await;
        let faces = Faces::new(docs.clone(), db);
        let meta = docs.get(id).await.unwrap();
        let (card, read) = faces.card(meta).await;
        assert!(read.is_ok());
        assert_eq!(card.face, None);
        assert_eq!(card.title, "Bare");
        assert_eq!(card.kind, "app.list");
    }

    #[test]
    fn a_project_path_is_trimmed_and_must_name_something() {
        assert_eq!(
            project_node("/example-album/track-two/").unwrap().id,
            "example-album/track-two"
        );
        for bad in ["", "/", "a//b", "../x", "a/./b"] {
            assert!(project_node(bad).is_err(), "{bad:?}");
        }
    }
}
