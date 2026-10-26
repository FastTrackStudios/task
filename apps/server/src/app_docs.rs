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
//! - A project holds collections the same way: `project:<path>` →
//!   `collection:<id>` ([`NodeKind::Collection`], `Down`), through
//!   [`attach_collection`] and [`detach_collection`]. One link shape,
//!   one code path ([`hold`], [`release`]); only the target's kind and
//!   the store that vouches for it differ. A series project holds one
//!   setlist a week this way, an album its running order.

use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "plugin-fasttrackstudio")]
use collection::{Collection, CollectionService};
use crdt::docs::{DocMeta, DocQuery, Docs as _, DocsError, DocsHost};
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

/// The reference a collection goes by: `collection:<id>`, local.
#[must_use]
pub fn collection_node(id: &str) -> NodeRef {
    NodeRef::new(NodeKind::Collection, id)
}

/// The reference a project goes by: `project:<path>`, its path on the
/// Projects tier (`example-album`, `example-album/track-two`). Slashes
/// at either end are dropped; an empty segment, `.` or `..` is refused,
/// since the path is the project's address and those name nothing.
///
/// The project's page path (`example-album/project.md`, what
/// `list_projects` reports) is accepted too and names its directory:
/// handing the page back is the obvious thing to do with a path a tool
/// just printed, and a link to `project:example-album/project.md` would
/// be a link no project page ever lists.
pub fn project_node(project: &str) -> Result<NodeRef, String> {
    let path = project.trim().trim_matches('/');
    let path = if path == org_proto::PROJECT_PAGE {
        ""
    } else {
        path.strip_suffix(org_proto::PROJECT_PAGE)
            .and_then(|dir| dir.strip_suffix('/'))
            .unwrap_or(path)
    };
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

/// Every `Down` link by which `project` holds a local node of `kind`.
fn held(
    links: &impl LinksService,
    project: &NodeRef,
    kind: NodeKind,
) -> Result<Vec<TypedLink>, String> {
    let all = links
        .links_for(project.clone())
        .map_err(|e| format!("couldn't read the links of `{}`: {e}", project.id))?;
    Ok(all
        .into_iter()
        .filter(|l| {
            l.source == *project
                && l.relation == Relation::Down
                && l.target.kind == kind
                && l.target.is_local()
        })
        .collect())
}

/// Every link by which `project` holds an app doc.
fn attached(links: &impl LinksService, project: &NodeRef) -> Result<Vec<TypedLink>, String> {
    held(links, project, NodeKind::AppDoc)
}

/// `project` holds `target`: the `Down` link, made once. An identical
/// link already held is returned rather than duplicated. The caller has
/// already checked that `target` names something; `by` is who asked,
/// for the link's provenance.
fn hold(
    links: &impl LinksService,
    project: &NodeRef,
    target: NodeRef,
    by: &str,
) -> Result<TypedLink, String> {
    if let Some(link) = held(links, project, target.kind)?
        .into_iter()
        .find(|l| l.target == target)
    {
        return Ok(link);
    }
    let what = target.to_token();
    let mut link = TypedLink::new(project.clone(), target, Relation::Down, Confidence::Certain);
    link.provenance.created_by = by.to_owned();
    links
        .create(link)
        .map_err(|e| format!("couldn't attach `{what}` to `{}`: {e}", project.id))
}

/// `project` no longer holds `target`: every link by which it did goes.
/// Returns how many; none is an error, naming what was asked.
fn release(
    links: &impl LinksService,
    project: &NodeRef,
    target: &NodeRef,
) -> Result<usize, String> {
    let gone: Vec<TypedLink> = held(links, project, target.kind)?
        .into_iter()
        .filter(|l| l.target == *target)
        .collect();
    if gone.is_empty() {
        return Err(format!(
            "`{}` is not attached to project `{}`",
            target.to_token(),
            project.id
        ));
    }
    for l in &gone {
        links
            .delete(&l.id)
            .map_err(|e| format!("couldn't detach `{}`: {e}", target.to_token()))?;
    }
    Ok(gone.len())
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
    let link = hold(links, project, doc_node(doc_id), by)?;
    Ok((link, meta))
}

/// Detach a doc from `project`: every link by which the project holds
/// it. Works whether or not the doc is still in the catalog, so a link
/// to a deleted doc can be cleared. Returns how many links went; none is
/// an error, naming what was asked.
pub fn detach(links: &impl LinksService, project: &NodeRef, doc_id: Uuid) -> Result<usize, String> {
    release(links, project, &doc_node(doc_id))
}

/// One of `org`'s collections, by id or, failing that, by exact title.
/// A title two collections share is refused rather than guessed at.
#[cfg(feature = "plugin-fasttrackstudio")]
fn find_collection(
    collections: &impl CollectionService,
    org: &str,
    target: &str,
) -> Result<Option<Collection>, String> {
    let target = target.trim();
    if let Some(c) = collections
        .get(target)
        .map_err(|e| format!("couldn't read collection `{target}`: {e}"))?
    {
        return Ok(Some(c));
    }
    let mut named: Vec<Collection> = collections
        .list(org.to_owned(), None)
        .map_err(|e| format!("couldn't list collections: {e}"))?
        .into_iter()
        .filter(|c| c.title == target)
        .collect();
    match named.len() {
        0 | 1 => Ok(named.pop()),
        n => Err(format!(
            "{n} collections are titled `{target}`; pass one's id instead"
        )),
    }
}

/// Attach one of `org`'s collections to `project` — a set list to the
/// show, the series or the album it belongs to. `target` is the
/// collection's id or its exact title. Idempotent, like [`attach`]; a
/// collection the store does not hold is refused. Any kind of
/// collection may be held: the kind is the app's word, not Task's.
#[cfg(feature = "plugin-fasttrackstudio")]
pub fn attach_collection(
    collections: &impl CollectionService,
    links: &impl LinksService,
    project: &NodeRef,
    org: &str,
    target: &str,
    by: &str,
) -> Result<(TypedLink, Collection), String> {
    let Some(collection) = find_collection(collections, org, target)? else {
        return Err(format!(
            "no collection `{target}` in this org (by id or title)"
        ));
    };
    let link = hold(links, project, collection_node(&collection.id), by)?;
    Ok((link, collection))
}

/// Detach a collection from `project`. `target` is its id or title;
/// an id the store no longer holds still clears its link, so a deleted
/// collection's entry can be removed. Returns the collection's id and
/// how many links went.
#[cfg(feature = "plugin-fasttrackstudio")]
pub fn detach_collection(
    collections: &impl CollectionService,
    links: &impl LinksService,
    project: &NodeRef,
    org: &str,
    target: &str,
) -> Result<(String, usize), String> {
    let id = find_collection(collections, org, target)?
        .map_or_else(|| target.trim().to_owned(), |c| c.id);
    let gone = release(links, project, &collection_node(&id))?;
    Ok((id, gone))
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

    #[test]
    fn a_project_path_is_trimmed_and_must_name_something() {
        assert_eq!(
            project_node("/example-album/track-two/").unwrap().id,
            "example-album/track-two"
        );
        for bad in ["", "/", "a//b", "../x", "a/./b", "project.md"] {
            assert!(project_node(bad).is_err(), "{bad:?}");
        }
    }

    /// The page path `list_projects` prints names the same project as
    /// its directory, so a tool's output can be handed straight back.
    #[test]
    fn a_project_page_path_names_its_directory() {
        assert_eq!(
            project_node("crescendum-album/project.md").unwrap(),
            project_node("crescendum-album").unwrap()
        );
        assert_eq!(
            project_node("example-album/track-two/project.md")
                .unwrap()
                .id,
            "example-album/track-two"
        );
        // Only a whole `project.md` segment is the page.
        assert_eq!(project_node("myproject.md").unwrap().id, "myproject.md");
    }

    /// A project holds a collection by the one link shape app docs use,
    /// found by id or title, made once, and cleared by either.
    #[cfg(feature = "plugin-fasttrackstudio")]
    #[test]
    fn a_project_holds_a_collection_once_and_lets_it_go() {
        use collection::{CollectionKind, CollectionService as _};

        let dir = tempfile::tempdir().unwrap();
        let links = links::Store::open(dir.path().join("links.jsonl"));
        let collections = collection::Store::open(dir.path().join("collections.jsonl"));
        let set = collections
            .create(
                "acme".into(),
                "Album Launch Set".into(),
                CollectionKind::new("setlist"),
            )
            .unwrap();
        let album = project_node("example-album").unwrap();

        let (first, c) = attach_collection(
            &collections,
            &links,
            &album,
            "acme",
            "Album Launch Set",
            "t",
        )
        .unwrap();
        assert_eq!(c.id, set.id);
        assert_eq!(first.target, collection_node(&set.id));
        assert_eq!(first.relation, Relation::Down);
        let (again, _) =
            attach_collection(&collections, &links, &album, "acme", &set.id, "t").unwrap();
        assert_eq!(again.id, first.id, "attaching twice keeps one link");

        assert!(
            attach_collection(&collections, &links, &album, "acme", "Nope", "t").is_err(),
            "a collection the store does not hold is refused"
        );

        let (id, gone) =
            detach_collection(&collections, &links, &album, "acme", "Album Launch Set").unwrap();
        assert_eq!((id.as_str(), gone), (set.id.as_str(), 1));
        assert!(detach_collection(&collections, &links, &album, "acme", &set.id).is_err());
    }
}
