//! Wire types for the Files RPC surface (issue #259). Vocabulary is the
//! Task glossary (`apps/task/CONTEXT.md`): a **File Root** is a folder
//! tree with its own identity; **browsing** walks a live tree; a **File
//! version chain** is a file's per-saved-state history; a **Session
//! checkpoint** is the durable, chain-visible version minted on demand
//! here (v1: only the explicit "checkpoint now" trigger — the
//! quiescence/debounce cadence engine is future work).

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A File Root's versioning mode, chosen at creation and immutable
/// afterwards (ADR 0001; flavor conversion, if ever wanted, is a
/// deliberate relocation-style operation).
///
/// - `Media` — the default. The root's history lives in Files' own
///   content-addressed store (FastCDC + BLAKE3), built for multi-GB
///   binaries that dedup across versions.
/// - `Software` — a colocated git repository (issue #273): the root
///   carries an ordinary `.git`, so GitHub, CI, and IDEs see a normal
///   checkout, while the Files RPC surface reads that same history.
///   Heavy stray files are kept out of it by the flavor's Ignore set
///   seed plus the tree's own `.gitignore`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum RootFlavor {
    Media,
    Software,
}

/// A File Root: a folder tree with a stable identity (ADR 0001 /
/// glossary "File Root").
///
/// **Identity and placement are separate.** `id` is the root, the same
/// on every host that knows it — it is the id in the folder's own
/// `.fts-root.json` marker, which travels with the folder. `path` is
/// only where that root's tree happens to sit *on this host*, and two
/// hosts holding the same root will disagree about it.
///
/// That separation is what `files.peering.replication` needs: a host
/// may hold an org's structure and none of its content, and such a host
/// knows the root without having anywhere to put it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct FileRootInfo {
    pub id: Uuid,
    pub name: String,
    /// This root's live tree **on this host**, absent when the host
    /// holds structure without content.
    ///
    /// `Option` rather than an empty string on purpose: every operation
    /// needing a tree must be made to handle its absence, and an empty
    /// path is worse than no path — `Path::new("").join("Stems")` is
    /// *relative*, so a missed site would silently resolve against the
    /// process's working directory and read or write the wrong files.
    #[facet(default)]
    pub path: Option<String>,
    pub flavor: RootFlavor,
    pub created_at: DateTime<Utc>,
    /// The root's CURRENT lineage: its highest-numbered
    /// [`ProjectVersion`] entity (issue #261), or `None` on a root that
    /// has never been restarted. Derived from the Vault on read — the
    /// entities stay the authority, this is the projection the explorer
    /// renders as the root's badge.
    ///
    /// `#[facet(default)]` so a NEW client decoding an OLD server's
    /// response (the client-first upgrade direction) defaults the field
    /// instead of failing the decode plan.
    #[facet(default)]
    pub project_version: Option<ProjectVersion>,
}

impl FileRootInfo {
    /// This root's tree on this host, if it has one.
    ///
    /// The single place `path` is turned into something a filesystem
    /// call can use. A `None` here is not an error — it is a host that
    /// hosts the org's structure — so callers that genuinely need bytes
    /// answer `FilesFault::Unavailable`, and callers that only need
    /// structure carry on without touching a disk.
    #[must_use]
    pub fn local_tree(&self) -> Option<&std::path::Path> {
        self.path.as_deref().map(std::path::Path::new)
    }

    /// Whether this host holds the root's tree.
    #[must_use]
    pub fn is_placed(&self) -> bool {
        self.path.is_some()
    }
}

/// One entry in a directory listing — either a root-scoped
/// [`crate::service::FilesService::browse`] or a rootless
/// [`crate::service::FilesService::drive_browse`] ("Drive" browsing
/// per the glossary: loose files outside any root).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct BrowseEntry {
    pub name: String,
    pub is_dir: bool,
    /// `None` for directories, and for a [`BrowseEntry::stub`] entry
    /// (a stub's logical size lives in its manifest, not on disk).
    pub size: Option<u64>,
    /// The entry is a **pointer stub**: known to the root's version
    /// store at the checkpoint head but not resident in the live tree
    /// (glossary "Pointer stub" — browsing a 240 GB project must not
    /// mean downloading it). Always `false` for
    /// [`crate::service::FilesService::drive_browse`], which has no
    /// root context. On-demand hydration is issue #263; v1 reports the
    /// state so the explorer can show resident-vs-stub honestly.
    #[facet(default)]
    pub stub: bool,
    /// The entry has **Divergent versions**: the root's version store
    /// holds more than one visible head and this path's content differs
    /// between them (glossary "Divergent versions" — concurrent saves
    /// survive side by side instead of clobbering). Resolution
    /// (pick A / pick B / keep both) is issue #267.
    #[facet(default)]
    pub divergent: bool,
}

/// One resolved node of the org tree (issue #304) — the unified
/// namespace the Files surface and the OS mount both browse:
/// `Projects/` (vault project folders joined with their File Roots),
/// `Vault/` and `Wiki/` (lensed: Folders + Tags), `Assets/` (loose
/// files outside any root).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub enum TreeNode {
    /// A listing at this tree path — virtual (areas, lenses, tag
    /// folders) or filesystem-backed (vault/wiki dirs, assets).
    Listing(Vec<BrowseEntry>),
    /// The subtree from here down IS a File Root's live tree — the
    /// client mounts its full root explorer (inspector, versions,
    /// review) instead of walking entries one RPC at a time.
    Root { id: Uuid, subpath: String },
}

/// A project-file save observed during a session (glossary
/// "Auto-snapshot": "a project-file save marks the nearest
/// auto-snapshot as a **save point** (display metadata, not a
/// version)"). A save point is never itself a version — it is a label
/// riding the capture that followed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct SavePoint {
    /// Root-relative path of the project file that was saved.
    pub path: String,
    /// When the save was observed.
    pub at: DateTime<Utc>,
}

/// An **auto-snapshot** (glossary): the ephemeral safety capture the
/// cadence engine takes during activity. Never a chain entry — snapshot
/// commits branch off the checkpoint line rather than sitting on it, so
/// [`crate::service::FilesService::chain`] walks straight past them —
/// and expirable, so a tracking day doesn't drown the history in noise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct SnapshotInfo {
    pub root_id: Uuid,
    /// Hex-encoded jj `CommitId` of the snapshot commit.
    pub snapshot_id: String,
    pub at: DateTime<Utc>,
    /// Root-relative paths written or removed by this snapshot, sorted.
    pub changed_paths: Vec<String>,
    /// Project-file saves this snapshot is the nearest capture for.
    pub save_points: Vec<SavePoint>,
}

/// One entry in a file's version chain (glossary "File version
/// chain"), newest first — the wire projection of
/// `files_store::version::chain::VersionEntry`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ChainEntry {
    /// Hex-encoded jj `CommitId` that produced this saved state.
    pub commit_id: String,
    /// The path the file lived at in that commit (chains follow
    /// recorded renames).
    pub path: String,
    /// Hex-encoded jj `FileId` (content address) of this saved state.
    pub file_id: String,
    /// Set when this entry is the commit where the file arrived at
    /// `path` via a recorded rename.
    pub renamed_from: Option<String>,
    /// Project-file saves recorded during the session this checkpoint
    /// closed — the save-point markers, surfaced as chain metadata
    /// (issue #260). Empty for a checkpoint minted with no observed
    /// project-file save (including every explicit "checkpoint now").
    pub save_points: Vec<SavePoint>,
    /// Names of every [`NamedVersion`] the Vault holds against this
    /// entry's commit — the curated metadata layered on top of the
    /// automatic chain (issue #261). Empty for an ordinary,
    /// un-curated Session checkpoint. The store itself knows nothing
    /// about names; this is resolved from the Vault on every read.
    pub names: Vec<String>,
}

/// A **Named Version** (glossary): a user-facing, deliberately labeled
/// version of a deliverable ("v3 for client"), curated on top of the
/// automatic chain. A Vault entity — a markdown page with frontmatter
/// under the org vault — referencing `(root id, change id)`; ADR 0001:
/// "the version store knows nothing about names".
///
/// Both ids are recorded: `change_id` is jj's stable, rewrite-surviving
/// identity (what the reference *is*), `commit_id` the exact content
/// pointer it resolved to when named (what GC protects and what a
/// share link streams). See
/// [`crate::service::FilesService::resolve_named_version`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct NamedVersion {
    pub id: Uuid,
    /// Vault-relative path of the entity's own page. Empty on input to
    /// [`crate::service::FilesService::name_version`]; the server fills
    /// it in.
    pub path: String,
    /// The curated label, as the producer typed it ("v3 for client").
    pub name: String,
    pub root_id: Uuid,
    /// Hex-encoded jj `ChangeId`.
    pub change_id: String,
    /// Hex-encoded jj `CommitId`.
    pub commit_id: String,
    /// The page body — free-form producer notes about this version.
    pub note: String,
    pub created_at: DateTime<Utc>,
}

/// A **Project Version** (glossary): a whole-project iteration of one
/// File Root — the same root with a new lineage, replacing the
/// "Project Title old" / "Project Title NEW final2" folder idiom. The
/// folder name never changes. Auto-numbered from 1 with an optional
/// label; a Vault entity like [`NamedVersion`], referencing the commit
/// the iteration starts from.
///
/// This ticket (#261) is the entity plus its numbering — the restart
/// flow that actually flips a root's live tree is #268.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ProjectVersion {
    pub id: Uuid,
    /// Vault-relative path of the entity's own page (server-filled).
    pub path: String,
    pub root_id: Uuid,
    /// Auto-assigned, 1-based, per root — never reused.
    pub number: u32,
    /// Optional producer label ("Client remix").
    pub label: Option<String>,
    /// Hex-encoded jj `ChangeId` of the commit this iteration starts
    /// from.
    pub change_id: String,
    /// Hex-encoded jj `CommitId` of that same commit.
    pub commit_id: String,
    pub started_at: DateTime<Utc>,
}

/// How a Project Version restart seeds the root's new live tree
/// (issue #268, glossary "Project Version": restarting replaces the
/// "Project Title NEW final2" folder idiom — same folder, new
/// lineage). Whatever the mode, the old iteration's terminal state is
/// checkpointed first and stays browsable read-only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum RestartMode {
    /// The new iteration starts with nothing: every tracked file is
    /// cleared from the live tree (ignored/untracked junk is left
    /// alone — a restart never deletes unversioned data).
    Empty,
    /// The new iteration starts from a template folder's contents,
    /// copied in after the clear. The path is org-confined server-side
    /// like every other path argument.
    Template { source_path: String },
    /// The new iteration carries chosen files forward from the old
    /// one; everything else is cleared. An empty list means the
    /// picker's default — carry **everything minus the Ignore set**
    /// (spec: "picker defaults to everything minus the Ignore set"),
    /// which makes the restart a pure lineage cut with no tree change.
    CarryForward { paths: Vec<String> },
}

/// What a Vault version reference resolves to in the store right now —
/// the answer a share link targeting a Named Version needs before it
/// can stream anything (glossary "Share link": target enum
/// `Note | Slice | Named Version | Review`).
///
/// `commit_id` is the exact change the reference names: resolved from
/// the entity's `change_id` through the root's index when possible, so
/// a rewritten change still lands on its current commit, and falling
/// back to the recorded `commit_id` otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct VersionRef {
    pub root_id: Uuid,
    /// Hex-encoded jj `ChangeId` — the stable half of the reference.
    pub change_id: String,
    /// Hex-encoded jj `CommitId` this reference resolves to now.
    pub commit_id: String,
}

/// Result of [`crate::service::FilesService::gc_root`] — one
/// mark-and-sweep pass over a root's version store, with the protect
/// set resolved from the Vault (ADR 0001: "protect set =
/// index-reachable ∪ Vault-referenced ... the Vault is the authority
/// on immortality").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct GcReport {
    /// Tree/commit/copy-history objects removed.
    pub objects_swept: u64,
    /// Chunk-store manifests removed. Their now-unreferenced chunks are
    /// reclaimed on the chunk store's own background schedule.
    pub manifests_swept: u64,
    /// How many commits the Vault protected this pass — the Named
    /// Version and Project Version entities pointing at this root.
    pub protected_commits: u32,
}

/// Result of [`crate::service::FilesService::checkpoint_now`] (glossary
/// "Session checkpoint").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct CheckpointInfo {
    pub root_id: Uuid,
    /// Hex-encoded jj `CommitId` of the new checkpoint commit.
    pub commit_id: String,
    pub description: String,
    pub at: DateTime<Utc>,
    /// Root-relative paths written or removed by this checkpoint,
    /// sorted. Empty when the live tree was already identical to the
    /// previous checkpoint (still a valid, if uneventful, checkpoint).
    pub changed_paths: Vec<String>,
    /// Project-file saves observed during the session this checkpoint
    /// closed (issue #260). Empty for an explicit "checkpoint now" on
    /// a root with no observed saves.
    pub save_points: Vec<SavePoint>,
    /// Paths the certifying scan found still being written — the file
    /// changed between the stat taken before hashing it and the one
    /// taken after, on every attempt. They keep their previous
    /// versioned state in this checkpoint and ride into the next
    /// capture rather than being committed torn (issue #260).
    pub requeued_paths: Vec<String>,
}

/// One file's hydration state changing (issue #263): `Dehydrated` — its
/// live-tree content replaced by a pointer stub — or `Hydrated` — exact
/// content restored over the stub, verified by `FileId`. Carried by
/// [`crate::service::FilesEvent`] so explorers refresh the one row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct HydrationChange {
    pub root_id: Uuid,
    /// Root-relative path of the file.
    pub path: String,
    /// `true` when the path is now a stub (dehydrated), `false` when it
    /// is now resident (hydrated).
    pub stub: bool,
}

/// Result of [`crate::service::FilesService::apply_hydration_policy`]:
/// what one policy pass changed, root-relative paths, sorted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct HydrationReport {
    /// Stubs the policy matched and restored to resident content.
    pub hydrated: Vec<String>,
    /// Resident files the policy left unmatched and dehydrated.
    pub dehydrated: Vec<String>,
    /// Resident files the policy would have dehydrated but left alone
    /// because their content differs from the checkpoint head —
    /// dehydration never destroys unversioned work. Checkpoint, then
    /// re-apply, to complete the pass.
    pub skipped_dirty: Vec<String>,
    /// Paths whose hydrate/dehydrate errored (details in the server
    /// log). The pass is per-file fault tolerant: one unhydratable stub
    /// — normal on a partial replica missing chunks — never aborts the
    /// pass or discards this report.
    #[facet(default)]
    pub failed: Vec<String>,
}

/// One side of a file's Divergent versions (glossary): the commit
/// (a view head) holding this side and the content identity it has
/// there — `None` when this side deleted the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct DivergenceSide {
    /// Hex jj `CommitId` of the head carrying this side.
    pub commit_id: String,
    /// Hex `FileId` of this side's content, `None` for a deletion.
    pub file_id: Option<String>,
}

/// A path whose state differs between the root's visible heads —
/// concurrent saves that both survived (ADR 0001: nothing is lost, no
/// on-disk conflict markers; the UI resolves). Produced by
/// [`crate::service::FilesService::divergences`], settled by
/// [`crate::service::FilesService::resolve_divergence`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct DivergenceInfo {
    pub root_id: Uuid,
    /// Root-relative path.
    pub path: String,
    /// Every visible head's take on the path, in head order (the
    /// journal's own line first, then the others).
    pub sides: Vec<DivergenceSide>,
}

/// The resolution verdict for one divergent path (glossary "Divergent
/// versions": pick A / pick B / keep both).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum DivergenceChoice {
    /// Keep the named head's side of this path (pick by commit id —
    /// unambiguous where A/B would depend on listing order).
    Pick { commit_id: String },
    /// Keep every side: the first (journal-line) side keeps the path,
    /// each other side lands beside it as `<stem> (divergent <n>).<ext>`.
    KeepBoth,
}

/// Which derived rendition of a media file (issue #269) — a proxy for
/// streaming, an audio rendition, waveform peaks, a filmstrip. The wire
/// tag mirrors `files_transcode::RenditionKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum RenditionKind {
    Proxy1080,
    Proxy720,
    Audio,
    Peaks,
    Filmstrip,
}

impl RenditionKind {
    /// The stable rendition tag — the `{kind}` path segment of the
    /// streaming route (issue #270). Must mirror
    /// `files_transcode::RenditionKind::tag`, which the server parses
    /// the segment with.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            RenditionKind::Proxy1080 => "proxy-1080",
            RenditionKind::Proxy720 => "proxy-720",
            RenditionKind::Audio => "audio-aac",
            RenditionKind::Peaks => "peaks",
            RenditionKind::Filmstrip => "filmstrip",
        }
    }
}

/// A **Review** (issue #270): the persistent review conversation for
/// one media file in a root — a Vault entity, like the curated version
/// entities (spec: "Review pages present a file via slice with
/// timecoded comments and frame annotations"). One review per
/// `(root, file path)`; it survives new versions of its file — the
/// comments record the version they were made on, the review itself
/// records only the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Review {
    pub id: Uuid,
    /// Vault-relative path of the entity's own page; server-filled.
    pub path: String,
    pub root_id: Uuid,
    /// Root-relative path of the media file under review.
    pub file_path: String,
    /// Display title — defaults to the file's name.
    pub title: String,
    /// The page body — free-form notes about the review.
    pub note: String,
    pub created_at: DateTime<Utc>,
}

/// One point of an annotation stroke, in coordinates normalized to the
/// video frame (`0..=1` on both axes) — what makes a drawing re-anchor
/// across renditions and window sizes (issue #270 AC 3).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct AnnotationPoint {
    pub x: f32,
    pub y: f32,
}

/// One freehand stroke of a frame annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct AnnotationStroke {
    pub points: Vec<AnnotationPoint>,
    /// CSS color of the stroke.
    pub color: String,
    /// Stroke width as a fraction of the frame width.
    pub width: f32,
}

/// One timecoded comment on a [`Review`] — a Vault entity page whose
/// body is the comment text. Records the file version (`commit_id`)
/// the commenter was watching, so a new version of the file keeps old
/// feedback attributed to the frames it was actually about (AC 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ReviewComment {
    pub id: Uuid,
    /// Vault-relative path of the entity's own page; server-filled.
    pub path: String,
    pub review_id: Uuid,
    /// Position in the media, in seconds (fractional for frame
    /// accuracy).
    pub timecode_secs: f64,
    /// Display name of the commenter; empty renders as a guest.
    pub author: String,
    /// The comment text (the page body).
    pub body: String,
    /// Hex commit id of the file version the comment was made on.
    pub commit_id: String,
    /// Frame drawing anchored to `(review, commit_id, timecode)` in
    /// normalized coordinates (AC 3). Empty = no drawing.
    pub annotation: Vec<AnnotationStroke>,
    /// Share-link attribution (issue #272 AC 1): the label/token of the
    /// guest link this comment arrived through. Empty for org members.
    /// Stamped by the guest lane server-side — a client never sets it.
    #[serde(default)]
    pub via_link: String,
    pub created_at: DateTime<Utc>,
}

/// Input half of [`ReviewComment`] for
/// [`crate::service::FilesService::add_review_comment`] (bundled — RPC
/// methods carry at most 4 params).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct NewReviewComment {
    pub timecode_secs: f64,
    pub author: String,
    pub body: String,
    /// The file version the commenter is watching — the client knows
    /// (it may be pinned to an older version via the switcher), the
    /// server validates it against the root's store.
    pub commit_id: String,
    pub annotation: Vec<AnnotationStroke>,
}

/// A generated (or cached) rendition, ready to stream: its CAS content
/// id, byte length, and MIME. The Review page (issue #270) resolves a
/// media file to this, then streams the bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct RenditionInfo {
    /// Hex CAS `FileId` of the rendition's bytes.
    pub file_id: String,
    pub len: u64,
    /// MIME type for the served stream.
    pub mime: String,
    pub kind: RenditionKind,
}
