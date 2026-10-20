//! `SyncService` — facets, ignoring, hydration and devices.
//!
//! The daemon lane. A device subscribes to a project's **facets**, not to
//! path globs, and everything unsubscribed stays present as a stub. The
//! decisions themselves are not made here: `files_domain::facet` resolves
//! a directory to a facet, `files_domain::ignore` decides what is junk,
//! and `files_domain::hydration` turns a subscription plus a facet map
//! into resident-or-stub. This module is the seam between those pure
//! decisions and the two things they need from the world — a listing of
//! the live tree, and the hydrate/dehydrate machinery that already lives
//! in `backend.rs`.
//!
//! ## What is durable, and the one half that still is not
//!
//! **Subscriptions, pins, device rows and project facet mappings are all
//! things a human chose**, so `storage.tier.authored` puts them in the
//! durable tier and `files.device.control` says in as many words that
//! pinning survives restart. They live in [`SYNC`], a
//! [`crate::durable::Scoped`] keyed by the backend's own data directory —
//! which is the org boundary, because `FilesBackend` is constructed once
//! per org. That keying is the point twice over: the state outlives the
//! process, and one org's `devices()` can no longer answer with another
//! org's rows, which a process-wide `static` did.
//!
//! ## Whose subscription a call is about
//!
//! A subscription belongs to the **device that asked**, and which device
//! asked is a property of the connection, never of this process
//! (`files.device.identity`). [`Holder`] is that answer, and there are
//! exactly two:
//!
//! - **A peer device** — the caller is `Principal::Host`, i.e. an
//!   endpoint the transport proved during the handshake. It is looked up
//!   by that endpoint in the device rows `enroll_device` wrote, so the
//!   same machine is the same device across its restarts (it reloads the
//!   same endpoint key) and across ours (the rows are durable). An
//!   endpoint with no row, a revoked row, or a row whose admission has
//!   since been withdrawn is refused: per-device revocation is enforced
//!   here, on every call, and not only at the gate.
//! - **This node** — the process itself, or a person signed into it. A
//!   session is not a device and carries no device identity, so a person
//!   at this node's UI is choosing what *this node* keeps. Its id is
//!   minted once per org and persisted with the rest of [`SyncState`],
//!   so this node, too, is one device across restarts rather than a new
//!   stranger per process.
//!
//! Only this node's subscription changes this node's disk. A peer's
//! subscription is a record of what that peer keeps — applying it here
//! would let a laptop that unticks "footage" release the host's copy.
//!
//! **A project's capabilities are derived from its `RootFlavor`.** Facets
//! come from capabilities, capabilities come from a project declaration,
//! and that declaration is not yet wired to roots — see
//! [`capability_of`]. A media root therefore takes *every* media
//! capability's layouts rather than a guess at which one it holds, which
//! is the widest correct answer while the narrow one is unavailable.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::error::FilesError;
use chrono::Utc;
use files_domain::facet::{Capability, Facet, FacetMap, Source};
use files_domain::hydration::{Subscription as Plan, decide};
use files_domain::ignore::{IgnoreSet as DomainIgnores, Layer};
use files_proto::error::FilesFault;
use files_proto::id::{DeviceId, RootId};
use files_proto::model::{FileRootInfo, RootFlavor};
use files_proto::path::RootPath;
use files_proto::service::access::Capability as Can;
use files_proto::service::sync::{
    DeviceInfo, FacetBinding, FacetName, FacetSource, IgnoreSet, Subscription, SyncService,
    TransferPolicy,
};
use files_proto::service::tree::Hydration;

use crate::backend::FilesBackend;
use crate::consts::{GIT_DIR, MARKER_FILE, STORE_DIR};

// ── State ──────────────────────────────────────────────────────────

/// One org's sync state: subscriptions, devices and project facet
/// mappings.
///
/// Persisted per org — see [`crate::durable`]. Every method that reads it
/// degrades to "nothing subscribed" rather than to a guess, which under
/// `files.sync.selective` is a tree of stubs and never missing content.
#[derive(Debug, Default, Clone, facet::Facet)]
#[repr(C)]
struct SyncState {
    /// Keyed by device *and* root: a subscription is one device's answer
    /// about one root, and a laptop takes sessions from one album while
    /// taking nothing from another.
    subscriptions: HashMap<(DeviceId, RootId), Subscription>,
    devices: BTreeMap<String, DeviceInfo>,
    /// Project facet mappings per root, keyed by lowercased path — the
    /// `files.facet.project-override` half that the capability does not
    /// know about.
    mappings: HashMap<RootId, BTreeMap<String, FacetName>>,
    /// The device row that is *this node*. Minted on first use and kept,
    /// so a restart is the same device with the same subscriptions — see
    /// the module note on [`Holder`].
    local: Option<DeviceId>,
}

/// The on-disk shape.
///
/// JSON has no representation for a composite map key, and the
/// subscription table is keyed by `(device, root)` because that is the
/// question every lookup here asks. Rather than degrade the runtime types
/// to fit the file format, the file format is lists of rows and the
/// conversion is here.
///
/// The rows are the wire types themselves rather than a second copy of
/// their fields: a [`Subscription`] already carries its own `device` and
/// `root_id`, and a [`DeviceInfo`] its own `id`, so the map keys are
/// recoverable from the values and storing them again would be a way for
/// a key and its row to disagree. Only `mappings`, whose inner map is
/// path-keyed and has no such field, needs a row struct.
#[derive(Default, facet::Facet)]
#[repr(C)]
struct Wire {
    subscriptions: Vec<Subscription>,
    devices: Vec<DeviceInfo>,
    mappings: Vec<MappingRow>,
    /// Absent in files written before this node had a stable id; such a
    /// file loads with none and mints one on the next write.
    #[facet(default)]
    local: Option<DeviceId>,
}

#[derive(facet::Facet)]
#[repr(C)]
struct MappingRow {
    root: RootId,
    /// Lowercased path to facet, exactly as `map_facet` recorded it.
    facets: BTreeMap<String, FacetName>,
}

impl From<Wire> for SyncState {
    fn from(w: Wire) -> Self {
        Self {
            subscriptions: w
                .subscriptions
                .into_iter()
                .map(|s| ((s.device, s.root_id), s))
                .collect(),
            devices: w
                .devices
                .into_iter()
                .map(|d| (d.id.to_string(), d))
                .collect(),
            mappings: w.mappings.into_iter().map(|m| (m.root, m.facets)).collect(),
            local: w.local,
        }
    }
}

impl From<SyncState> for Wire {
    fn from(s: SyncState) -> Self {
        Self {
            subscriptions: s.subscriptions.into_values().collect(),
            devices: s.devices.into_values().collect(),
            mappings: s
                .mappings
                .into_iter()
                .map(|(root, facets)| MappingRow { root, facets })
                .collect(),
            local: s.local,
        }
    }
}

impl crate::durable::Durable for SyncState {
    type Wire = Wire;

    fn to_wire(&self) -> Wire {
        Wire::from(self.clone())
    }

    fn from_wire(wire: Wire) -> Self {
        Self::from(wire)
    }
}

static SYNC: crate::durable::Scoped<SyncState> = crate::durable::Scoped::new("sync");

/// Whose subscription a sync-lane call reads or writes.
///
/// Resolved from the caller by [`holder`], never from the process: see
/// the module note. The `local` half is what decides whether a change
/// touches this node's disk, so it is carried rather than re-derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Holder {
    device: DeviceId,
    /// Whether `device` is this node — the only device whose
    /// subscription this node's live tree follows.
    local: bool,
}

/// Make sure this node has its device row, minting its id the first time.
///
/// The id is minted *once per org* and then read back from the durable
/// state, so this node is one device for as long as its data directory
/// exists. That is the difference from the per-process id this replaced,
/// under which every restart came back as a stranger with an empty
/// subscription and the devices table grew a row per process lifetime.
///
/// Stable is right here, where it would be wrong for a peer: nothing but
/// this node is ever resolved to this row. A peer is resolved by the
/// endpoint it proved ([`enrolled`]), so making this one stable cannot
/// make every client the same device — which is the failure a stable
/// server-side id used to risk when it was the answer for *every* caller.
fn ensure_local(s: &mut SyncState, backend: &FilesBackend) -> DeviceId {
    let local = *s.local.get_or_insert_with(DeviceId::generate);
    let endpoint = backend.endpoint_id();
    let row = s
        .devices
        .entry(local.to_string())
        .or_insert_with(|| DeviceInfo {
            id: local,
            name: "this device".to_string(),
            // The machine this backend is running on, which for a server
            // is the org's own endpoint. It is not enrolled and does not
            // need to be — a host does not admit itself — but a row with
            // no address at all would read as "a device that cannot
            // sync", and this one is the one every sync goes through.
            endpoint: endpoint.clone(),
            last_seen: Utc::now(),
            transfer: TransferPolicy {
                max_bytes_per_sec: None,
                paused: false,
            },
            revoked: false,
        });
    // An endpoint is bound after the backend is built (`with_remotes`),
    // and the row may predate it. Keep the row's address current rather
    // than frozen at whatever the first write happened to see.
    if endpoint.is_some() && row.endpoint != endpoint {
        row.endpoint = endpoint;
    }
    local
}

/// Mutate this org's state and persist it, registering this node first.
///
/// The registration is why announcing a device belongs on the write path
/// and not on the read one: a row saying "this device exists" is a fact
/// about the org, and a table that only remembered it for as long as the
/// process ran could not be revoked against.
fn with_state<T>(backend: &FilesBackend, f: impl FnOnce(&mut SyncState) -> T) -> T {
    SYNC.write(backend, |s| {
        ensure_local(s, backend);
        f(s)
    })
}

/// This node's own device id — the device a person or the process acts
/// for. Read-only once it exists; minted and persisted the first time.
fn local_device(backend: &FilesBackend) -> DeviceId {
    let known = read_state(backend, |s| {
        s.local.filter(|id| s.devices.contains_key(&id.to_string()))
    });
    match known {
        Some(id) => id,
        None => SYNC.write(backend, |s| ensure_local(s, backend)),
    }
}

/// The enrolled device behind a peer `endpoint`, or why it may not act
/// as one.
///
/// Three refusals, all `Denied` because each is the same answer to the
/// caller — "you are not a device of this org" — and the reason is for
/// the operator reading the log, not for the machine being turned away:
///
/// - **never enrolled**: a verified endpoint is still a stranger until a
///   member pairs it (`enroll_device`);
/// - **revoked**: the row says so, and the row is durable, so a
///   revocation outlives the process that issued it;
/// - **no longer admitted**: the admitted set is the other durable half
///   of a revocation (`revoke_device` dismisses the endpoint), and an
///   operator can also withdraw it directly. Either is enough.
///
/// This node's own row is excluded: it is reached as [`Holder::local`],
/// never by presenting an endpoint.
fn enrolled(backend: &FilesBackend, endpoint: &str) -> Result<DeviceId, FilesFault> {
    let row = read_state(backend, |s| {
        s.devices
            .values()
            .filter(|d| Some(d.id) != s.local)
            .find(|d| d.endpoint.as_deref() == Some(endpoint))
            .map(|d| (d.id, d.revoked))
    });
    let refuse = |why: &str| {
        tracing::warn!(
            device = endpoint,
            why,
            "files sync: a peer was refused as a device"
        );
        Err(FilesFault::denied(
            format!("sync as {why}"),
            RootPath::root(),
        ))
    };
    match row {
        None => refuse("a device this org never enrolled"),
        Some((_, true)) => refuse("a revoked device"),
        Some((id, false)) => {
            if backend
                .admits(&files_domain::HostId(endpoint.to_string()))
                .is_none()
            {
                return refuse("a device this org no longer admits");
            }
            Ok(id)
        }
    }
}

/// Who this call is about, authorised.
///
/// A peer device is authorised by being one — enrolled, not revoked,
/// admitted — and acts on its own subscription only. Everyone else is
/// authorised the way every lane authorises a person ([`caller`]), at
/// the root or at each named path, and acts for this node.
///
/// [`caller`]: crate::lane::caller
async fn holder(
    backend: &FilesBackend,
    root_id: RootId,
    paths: &[RootPath],
    can: Can,
) -> Result<Holder, FilesFault> {
    if let Some(endpoint) = crate::lane::caller::host_endpoint() {
        crate::lane::root_or_fault(backend, root_id)?;
        return Ok(Holder {
            device: enrolled(backend, &endpoint)?,
            local: false,
        });
    }
    if paths.is_empty() {
        crate::lane::caller::authorise_root(backend, root_id, can).await?;
    } else {
        for path in paths {
            crate::lane::caller::authorise(backend, root_id, path, can).await?;
        }
    }
    Ok(Holder {
        device: local_device(backend),
        local: true,
    })
}

/// Read this org's state without registering or persisting.
///
/// Separate from [`with_state`] because a read that writes the file back
/// is both wasted io and a lie in its mtime — "when did this device last
/// change what it takes" must not answer "when it last asked".
fn read_state<T>(backend: &FilesBackend, f: impl FnOnce(&SyncState) -> T) -> T {
    SYNC.read(backend, f)
}

// ── Capabilities, ignores, facet maps ──────────────────────────────

/// A root's capabilities.
///
/// Capabilities belong to a project declaration — "this is a music
/// project" — and that declaration is not yet attached to a File Root.
/// Until it is, the root's flavour is the only signal available, and a
/// media root is assumed to be music production.
///
/// FUTURE: read the project's declared capabilities once projects carry
/// them. This is wrong for a video project, which will see its `Proxies`
/// and `CacheClip` directories reported as unmapped rather than
/// recognised — reported, note, not guessed at, so the failure is
/// visible and recoverable by `map_facet` rather than silent.
/// The capabilities whose tool layouts apply to a root.
///
/// Both, for a media root. `project.capability` makes capability a
/// *set* precisely because a collaborative album is music production
/// and video production at once, and a root cannot know which half of
/// such a project it holds — flavour distinguishes media from source,
/// not audio from video.
///
/// Reading it as one capability meant Resolve's layouts never applied
/// anywhere, so `Proxies` was unclassifiable and a laptop that declined
/// footage was declining *unmapped* content instead. The two
/// vocabularies are disjoint — nothing in Pro Tools, REAPER or Logic
/// names a directory Resolve also names — so the union classifies
/// strictly more and reclassifies nothing.
///
/// The honest fix is upstream: capabilities belong to the project
/// declaration (`project.identity.declaration`), not to a root's
/// flavour. Until that seam exists this is the widest correct answer
/// rather than a guess at the narrow one.
fn capability_of(root: &FileRootInfo) -> Vec<Capability> {
    match root.flavor {
        RootFlavor::Media => vec![Capability::MusicProduction, Capability::VideoProduction],
        // A software root holds source, not sessions or footage. No
        // tool layout applies and nothing here classifies it.
        RootFlavor::Software => Vec::new(),
    }
}

impl FilesBackend {
    /// The facet map in force for a root: the capability's tool layouts
    /// plus whatever the project has mapped.
    fn facet_map(&self, root_id: RootId, root: &FileRootInfo) -> FacetMap {
        let mut map = FacetMap::new(capability_of(root));
        read_state(self, |s| {
            if let Some(project) = s.mappings.get(&root_id) {
                for (path, facet) in project {
                    map.map(path, Facet::new(facet.0.clone()));
                }
            }
        });
        map
    }

    /// The compiled ignore rules for a root — platform and capability
    /// from the domain, project layer off disk via the legacy store.
    async fn sync_ignores(
        &self,
        root_id: RootId,
        root: &FileRootInfo,
    ) -> Result<DomainIgnores, FilesFault> {
        let project = self.ignore_patterns(root_id.get()).await.map_err(fault)?;
        let mut set = DomainIgnores::new(capability_of(root));
        set.with_project(project);
        Ok(set)
    }

    /// This node's subscription to a root, as the domain wants it.
    ///
    /// This node's, and never a peer's: it is what the live tree here is
    /// brought into line with.
    fn plan_for(&self, root_id: RootId) -> Plan {
        let stored = stored_subscription(self, local_device(self), root_id);
        let mut plan = Plan::new(stored.facets.into_iter().map(|f| Facet::new(f.0)));
        for pinned in stored.pinned {
            plan.pin(pinned);
        }
        plan
    }

    /// Bring the live tree into line with this node's subscription.
    ///
    /// Returns the paths whose residency this pass changed. Two refusals
    /// are deliberate and are *skipped* rather than propagated:
    ///
    /// - **Uncheckpointed content is never released.** Dehydration
    ///   replaces bytes with a pointer to what the store holds; if the
    ///   store holds nothing, releasing them destroys them. `backend`
    ///   already refuses, and a subscription change must not be a way to
    ///   lose an unsaved take.
    /// - **A file being written right now is left alone**, for the same
    ///   reason the checkpoint path does.
    ///
    /// A software root is skipped whole: its working tree belongs to its
    /// colocated git, which has its own opinion about what is on disk.
    async fn apply_subscription(&self, root_id: RootId) -> Result<Vec<RootPath>, FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
        if root.flavor != RootFlavor::Media {
            return Ok(Vec::new());
        }
        let ignores = self.sync_ignores(root_id, &root).await?;
        let listing = self.list_tree(&root, &ignores).await?;
        let map = self.facet_map(root_id, &root);
        let plan = self.plan_for(root_id);

        let mut changed = Vec::new();
        for path in listing.files {
            let wanted = decide(&path, &map, &plan).hydration;
            let outcome = match wanted {
                Hydration::Resident => self.hydrate_path(root_id.get(), path.to_string()).await,
                // `Unavailable` is a fact about the network, decided
                // elsewhere; nothing the domain returns here asks for it.
                _ => self.dehydrate_path(root_id.get(), path.to_string()).await,
            };
            match outcome {
                Ok(entry) => {
                    if entry.stub == (wanted != Hydration::Resident) {
                        changed.push(path);
                    }
                }
                // Refused, per the doc above. The file keeps its bytes.
                Err(FilesError::BadRequest(_) | FilesError::NotFound(_)) => {}
                Err(other) => return Err(fault(other)),
            }
        }
        Ok(changed)
    }

    /// Walk a root's live tree, skipping its internals and everything
    /// the ignore set covers.
    ///
    /// Bounded on purpose: a caller asking "what facets does this root
    /// have" must not be answered by a full walk of a 240 GB album, and
    /// the answer past a few levels of nesting is noise either way.
    async fn list_tree(
        &self,
        root: &FileRootInfo,
        ignores: &DomainIgnores,
    ) -> Result<Listing, FilesFault> {
        let base = crate::lane::lane_tree(&root)?.to_path_buf();
        let ignores = ignores.clone();
        crate::lane::blocking(move || {
            let mut out = Listing::default();
            walk(&base, &RootPath::root(), &ignores, 0, &mut out)?;
            Ok(out)
        })
        .await
    }
}

/// A root's directories and files, root-relative.
#[derive(Debug, Default)]
struct Listing {
    dirs: Vec<RootPath>,
    files: Vec<RootPath>,
}

/// Deep enough for `Album/01 Song/Audio Files/Vox/Comps`, shallow enough
/// that a pathological tree cannot turn a listing into an outage.
const MAX_DEPTH: usize = 8;
/// A hard ceiling on one listing, so an enormous root degrades to a
/// truncated answer rather than to unbounded memory.
const MAX_ENTRIES: usize = 50_000;

fn walk(
    dir: &Path,
    prefix: &RootPath,
    ignores: &DomainIgnores,
    depth: usize,
    out: &mut Listing,
) -> Result<(), crate::error::Error> {
    if depth >= MAX_DEPTH || out.dirs.len() + out.files.len() >= MAX_ENTRIES {
        return Ok(());
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // An unreadable subdirectory is not a reason to fail the whole
        // listing — the rest of the root is still an honest answer.
        Err(_) => return Ok(()),
    };
    // Sorted, so a facet listing is stable between calls rather than
    // whatever order the filesystem happened to hand back.
    let mut names: Vec<_> = entries
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, path) in names {
        if name == STORE_DIR || name == GIT_DIR || name == MARKER_FILE {
            continue;
        }
        let Ok(rel) = prefix.join(&name) else {
            continue;
        };
        if ignores.is_ignored(rel.as_str()) {
            continue;
        }
        if path.is_dir() {
            out.dirs.push(rel.clone());
            walk(&path, &rel, ignores, depth + 1, out)?;
        } else {
            out.files.push(rel);
        }
    }
    Ok(())
}

// ── Conversions ────────────────────────────────────────────────────

/// The legacy fault surface onto the v2 one, matching
/// `crate::error`'s own mapping so a caller sees one story whichever
/// path an operation took.
fn fault(err: FilesError) -> FilesFault {
    match err {
        FilesError::NotFound(m) | FilesError::BadRequest(m) => FilesFault::Invalid(m),
        FilesError::AlreadyExists(m) => FilesFault::AlreadyRoot(m),
        FilesError::Io(m) => FilesFault::Io(m),
    }
}

fn wire_source(source: Source) -> FacetSource {
    match source {
        Source::ToolLayout => FacetSource::ToolLayout,
        Source::Project => FacetSource::Project,
        Source::Unmapped => FacetSource::Unmapped,
    }
}

fn binding_for(path: RootPath, map: &FacetMap) -> FacetBinding {
    let binding = map.resolve(path.as_str());
    FacetBinding {
        path,
        facet: binding.facet.map(|f| FacetName(f.0)),
        source: wire_source(binding.source),
        atomic: binding.atomic,
    }
}

/// Whether an ancestor of `path` already carries a facet. A tool
/// directory classifies its whole subtree, so `Audio Files/Vox` is not a
/// decision anyone needs to make.
fn ancestor_is_classified(path: &RootPath, map: &FacetMap) -> bool {
    let mut probe = path.parent();
    while let Some(p) = probe {
        if map.resolve(p.as_str()).source != Source::Unmapped {
            return true;
        }
        probe = p.parent();
    }
    false
}

/// A device's stored subscription, or an empty one.
///
/// Empty rather than absent: a device that has said nothing takes
/// nothing, which under `files.sync.selective` means everything is a
/// stub — present, sized, hydrating on access. There is no state in
/// which content is missing.
fn stored_subscription(backend: &FilesBackend, device: DeviceId, root_id: RootId) -> Subscription {
    read_state(backend, |s| {
        s.subscriptions
            .get(&(device, root_id))
            .cloned()
            .unwrap_or(Subscription {
                device,
                root_id,
                facets: Vec::new(),
                pinned: Vec::new(),
            })
    })
}

/// Record a subscription under the device it names.
///
/// Also the moment a peer was last heard from: a device that changes
/// what it takes is a device that is alive, which is what `last_seen`
/// tells the person deciding whether to revoke it.
fn store_subscription(backend: &FilesBackend, sub: &Subscription) {
    with_state(backend, |s| {
        if let Some(row) = s.devices.get_mut(&sub.device.to_string()) {
            row.last_seen = Utc::now();
        }
        s.subscriptions
            .insert((sub.device, sub.root_id), sub.clone());
    });
}

/// Set `holder`'s facets in a root, and — only when the holder is this
/// node — bring this node's disk into line with them.
async fn set_facets(
    backend: &FilesBackend,
    holder: Holder,
    root_id: RootId,
    facets: Vec<FacetName>,
) -> Result<Subscription, FilesFault> {
    let mut sub = stored_subscription(backend, holder.device, root_id);
    sub.facets = facets;
    sub.facets.sort_by(|a, b| a.0.cmp(&b.0));
    sub.facets.dedup();
    store_subscription(backend, &sub);
    // Content leaving the subscription becomes a stub here rather
    // than at some later sweep: a user who unticks "footage" and
    // then unplugs must already have the space back.
    if holder.local {
        backend.apply_subscription(root_id).await?;
    }
    Ok(sub)
}

impl FilesBackend {
    /// This node's own device id in this org.
    ///
    /// Stable for the life of the org's data directory — minted the first
    /// time anything asks and persisted with the lane's state.
    #[must_use]
    pub fn local_device(&self) -> DeviceId {
        local_device(self)
    }

    /// What `device` takes from `root_id`, as this org recorded it.
    ///
    /// The operator's view, in-process only. Over the wire a device reads
    /// its own subscription and nobody else's — [`SyncService::subscription`]
    /// answers for whoever is calling — so there is deliberately no wire
    /// method taking a device id.
    ///
    /// # Errors
    ///
    /// [`FilesFault::DeviceNotFound`] for a device this org has no row
    /// for; [`FilesFault::RootNotFound`] for an unknown root.
    pub fn device_subscription(
        &self,
        device: DeviceId,
        root_id: RootId,
    ) -> Result<Subscription, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        if !read_state(self, |s| s.devices.contains_key(&device.to_string())) {
            return Err(FilesFault::DeviceNotFound(device));
        }
        Ok(stored_subscription(self, device, root_id))
    }

    /// Record what an enrolled `device` takes from `root_id`, on its
    /// behalf.
    ///
    /// In-process only, for the places with no device on the other end of
    /// a connection to ask: the example seed, and an operator setting up a
    /// headless machine before it first dials in. Over the wire a device
    /// sets its own subscription by calling
    /// [`SyncService::subscribe`] as itself.
    ///
    /// For this node's own device it is exactly `subscribe` — the disk
    /// follows. For any other device it is a record and nothing more.
    ///
    /// # Errors
    ///
    /// As [`Self::device_subscription`], and whatever bringing this node's
    /// disk into line refuses.
    pub async fn record_device_subscription(
        &self,
        device: DeviceId,
        root_id: RootId,
        facets: Vec<FacetName>,
    ) -> Result<Subscription, FilesFault> {
        self.device_subscription(device, root_id)?;
        let holder = Holder {
            device,
            local: device == local_device(self),
        };
        set_facets(self, holder, root_id, facets).await
    }
}

// ── The lane ───────────────────────────────────────────────────────

impl SyncService for FilesBackend {
    // t[impl files.facet.vocabulary] — one vocabulary, from the capability
    // t[impl files.facet.tool-layout] — layouts arrive with the capability
    // t[impl files.facet.project-override] — unmapped is reported, never guessed
    async fn facets(&self, root_id: RootId) -> Result<Vec<FacetBinding>, FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
        crate::lane::caller::authorise_root(self, root_id, Can::Read).await?;
        let ignores = self.sync_ignores(root_id, &root).await?;
        let listing = self.list_tree(&root, &ignores).await?;
        let map = self.facet_map(root_id, &root);

        Ok(listing
            .dirs
            .into_iter()
            .filter(|d| {
                // Report a directory when it is itself a decision: it
                // carries a facet, or it carries none and no ancestor
                // does either — the mapping that is missing.
                map.resolve(d.as_str()).source != Source::Unmapped
                    || !ancestor_is_classified(d, &map)
            })
            .map(|d| binding_for(d, &map))
            .collect())
    }

    // t[impl files.facet.project-override] — the project maps what no tool made
    async fn map_facet(
        &self,
        root_id: RootId,
        path: RootPath,
        facet: FacetName,
    ) -> Result<FacetBinding, FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
        let path = path.validate()?;
        crate::lane::caller::authorise(self, root_id, &path, Can::Write).await?;
        if facet.0.trim().is_empty() {
            return Err(FilesFault::invalid("a facet's name may not be empty"));
        }
        with_state(self, |s| {
            s.mappings
                .entry(root_id)
                .or_default()
                .insert(path.as_str().to_ascii_lowercase(), facet);
        });
        Ok(binding_for(path, &self.facet_map(root_id, &root)))
    }

    // t[impl files.ignore.layers] — three layers, none able to defeat another
    async fn ignore_set(&self, root_id: RootId) -> Result<IgnoreSet, FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
        crate::lane::caller::authorise_root(self, root_id, Can::Read).await?;
        let project = self.ignore_patterns(root_id.get()).await.map_err(fault)?;
        // The domain owns both the matching and the lists; this is a
        // projection of them, not a second copy.
        let domain = DomainIgnores::new(capability_of(&root));
        Ok(IgnoreSet {
            platform: domain.patterns(Layer::Platform),
            capability: domain.patterns(Layer::Capability),
            project,
        })
    }

    // t[impl files.ignore.retained] — this edits what is shown, never what exists
    async fn set_project_ignores(
        &self,
        root_id: RootId,
        patterns: Vec<String>,
    ) -> Result<IgnoreSet, FilesFault> {
        crate::lane::caller::authorise_root(self, root_id, Can::Write).await?;
        // Only the project layer is settable: the other two are
        // knowledge about an OS and about an application, and a root
        // that could edit them could hide a `.DS_Store` policy bug from
        // every other root on the machine.
        self.set_ignore_patterns(root_id.get(), patterns)
            .await
            .map_err(fault)?;
        SyncService::ignore_set(self, root_id).await
    }

    // t[impl files.device.identity] — the subscription read is the
    // caller's own: a peer device by the endpoint it proved, otherwise
    // this node
    async fn subscription(&self, root_id: RootId) -> Result<Subscription, FilesFault> {
        let holder = holder(self, root_id, &[], Can::Read).await?;
        Ok(stored_subscription(self, holder.device, root_id))
    }

    // t[impl files.sync.selective] — facets, not globs; unsubscribed is a stub
    // t[impl files.device.identity] — and they are the calling device's
    // facets, under the identity it holds, not this process's
    async fn subscribe(
        &self,
        root_id: RootId,
        facets: Vec<FacetName>,
    ) -> Result<Subscription, FilesFault> {
        let holder = holder(self, root_id, &[], Can::Read).await?;
        set_facets(self, holder, root_id, facets).await
    }

    // t[impl files.device.control] — a pin beats the facets, and is not a cache hint
    async fn pin(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
        pinned: bool,
    ) -> Result<Subscription, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let paths: Vec<RootPath> = paths
            .into_iter()
            .map(|p| p.validate())
            .collect::<Result<_, _>>()?;
        let holder = holder(self, root_id, &paths, Can::Read).await?;

        let mut sub = stored_subscription(self, holder.device, root_id);
        if pinned {
            sub.pinned.extend(paths);
            sub.pinned.sort();
            sub.pinned.dedup();
        } else {
            sub.pinned.retain(|p| !paths.contains(p));
        }
        store_subscription(self, &sub);
        if holder.local {
            self.apply_subscription(root_id).await?;
        }
        Ok(sub)
    }

    async fn hydrate(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
        resident: bool,
    ) -> Result<Vec<RootPath>, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let mut done = Vec::with_capacity(paths.len());
        for path in paths {
            let path = path.validate()?;
            crate::lane::caller::authorise(self, root_id, &path, Can::Read).await?;
            // Explicit, so unlike the subscription sweep a refusal is
            // reported: the caller named this path and is owed the
            // reason it could not be released.
            let entry = if resident {
                self.hydrate_path(root_id.get(), path.to_string()).await
            } else {
                self.dehydrate_path(root_id.get(), path.to_string()).await
            }
            .map_err(fault)?;
            debug_assert_eq!(entry.stub, !resident);
            done.push(path);
        }
        Ok(done)
    }

    async fn residency(&self, root_id: RootId) -> Result<Vec<String>, FilesFault> {
        crate::lane::caller::authorise_root(self, root_id, Can::Read).await?;
        self.hydration_policy(root_id.get()).await.map_err(fault)
    }

    async fn set_residency(
        &self,
        root_id: RootId,
        patterns: Vec<String>,
    ) -> Result<Vec<String>, FilesFault> {
        crate::lane::caller::authorise_root(self, root_id, Can::Write).await?;
        self.set_hydration_policy(root_id.get(), patterns)
            .await
            .map_err(fault)
    }

    async fn apply_residency(
        &self,
        root_id: RootId,
    ) -> Result<files_proto::model::HydrationReport, FilesFault> {
        crate::lane::caller::authorise_root(self, root_id, Can::Read).await?;
        self.apply_hydration_policy(root_id.get())
            .await
            .map_err(fault)
    }

    async fn devices(&self) -> Result<Vec<DeviceInfo>, FilesFault> {
        // The write path, not the read one: listing is how this device
        // announces itself, and a caller asking "what talks to this org"
        // must see the machine it is asking from.
        Ok(with_state(self, |s| s.devices.values().cloned().collect()))
    }

    // t[impl files.device.control] — throttle and pause, losing no progress
    async fn set_transfer_policy(
        &self,
        device: DeviceId,
        policy: TransferPolicy,
    ) -> Result<DeviceInfo, FilesFault> {
        with_state(self, |s| {
            let info = s
                .devices
                .get_mut(&device.to_string())
                .ok_or(FilesFault::DeviceNotFound(device))?;
            // Pausing only sets a flag: transfers resume from where they
            // stopped because progress lives in the chunk store, not in
            // this policy.
            info.transfer = policy;
            Ok(info.clone())
        })
    }

    // t[impl files.device.control] — revoked, refused sync, wipes on next contact
    async fn revoke_device(&self, device: DeviceId) -> Result<DeviceInfo, FilesFault> {
        let revoked = with_state(self, |s| {
            let info = s
                .devices
                .get_mut(&device.to_string())
                .ok_or(FilesFault::DeviceNotFound(device))?;
            // Revocation is a flag rather than a deletion: a deleted row
            // is a device that reconnects as a stranger and is let back
            // in. The wipe happens on the device, on next contact.
            info.revoked = true;
            info.transfer.paused = true;
            Ok::<_, FilesFault>(info.clone())
        })?;
        // And it stops being admitted, which is the half that actually
        // refuses it. A flag on a row the sync path never consults is a
        // revocation the revoked machine does not notice: what gates a
        // pull is the admitted set, so revoking has to reach it.
        if let Some(endpoint) = &revoked.endpoint {
            self.dismiss_host(&files_domain::HostId(endpoint.clone()));
        }
        Ok(revoked)
    }

    // t[impl files.device.identity] — the device presents the identity it
    // holds (its endpoint key) and the org records it; nothing here mints
    // one for it
    async fn enroll_device(
        &self,
        endpoint: String,
        name: String,
    ) -> Result<DeviceInfo, FilesFault> {
        if endpoint.trim().is_empty() {
            return Err(FilesFault::Io(
                "a device must present an endpoint id".into(),
            ));
        }
        let info = with_state(self, |s| {
            // Keyed by endpoint, not by row id: a laptop that reinstalls
            // mints the same endpoint from the same key file and must not
            // become a second device — the org would then hold two rows
            // for one machine and revoking either would cut it.
            if let Some(existing) = s
                .devices
                .values_mut()
                .find(|d| d.endpoint.as_deref() == Some(endpoint.as_str()))
            {
                existing.name = name;
                existing.last_seen = Utc::now();
                // Re-enrolling is how a revoked machine is let back in,
                // by the same authority that admitted it first.
                existing.revoked = false;
                return existing.clone();
            }
            let info = DeviceInfo {
                id: DeviceId::generate(),
                name,
                endpoint: Some(endpoint.clone()),
                last_seen: Utc::now(),
                transfer: TransferPolicy {
                    max_bytes_per_sec: None,
                    paused: false,
                },
                revoked: false,
            };
            s.devices.insert(info.id.to_string(), info.clone());
            info
        });
        // The admission itself. `Hosting::working()` because a device
        // holds the content it subscribes to, not merely the structure.
        self.admit_host(
            files_domain::HostId(endpoint),
            files_domain::Hosting::working(),
        );
        Ok(info)
    }

    async fn coordinator(&self) -> Result<String, FilesFault> {
        self.endpoint_id().ok_or_else(|| {
            // Not "not found": the org exists and has no address, which
            // is a deployment that did not bind an endpoint (or has iroh
            // disabled). A device cannot sync with it and retrying will
            // not change that until an operator does something.
            FilesFault::Io(
                "this org has no endpoint — the server is not reachable by id (iroh disabled?)"
                    .into(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RootPath {
        RootPath::parse(s).expect("path")
    }

    // t[verify files.ignore.layers]
    #[test]
    fn the_reported_layers_come_from_the_domain() {
        // These lists used to be mirrored here, which is two sources of
        // truth for a rule that decides what a user can see. They are now
        // read from the domain; this pins the projection, so a layer the
        // lane reports is a layer the domain actually enforces.
        let domain = DomainIgnores::new([Capability::MusicProduction]);
        for layer in [Layer::Platform, Layer::Capability] {
            for pattern in domain.patterns(layer) {
                let sample = pattern.replace('*', "sample");
                assert!(
                    domain.is_ignored(&sample),
                    "reported {layer:?} pattern {pattern} is not enforced"
                );
            }
        }
    }

    // t[verify files.ignore.layers]
    #[test]
    fn a_root_with_no_capability_reports_no_capability_layer() {
        assert!(
            DomainIgnores::default()
                .patterns(Layer::Capability)
                .is_empty(),
            "a software root must not be told its DAW droppings are hidden"
        );
    }

    // t[verify files.facet.project-override]
    #[test]
    fn only_the_undecided_directories_are_worth_reporting() {
        let map = FacetMap::new([Capability::MusicProduction]);
        // A tool directory is a decision already made; its children are
        // not decisions at all.
        assert!(!ancestor_is_classified(&p("Audio Files"), &map));
        assert!(ancestor_is_classified(&p("Audio Files/Vox"), &map));
        assert!(!ancestor_is_classified(&p("Project Assembly"), &map));
    }

    // t[verify files.device.identity] — this node is one device across
    // restarts, read back from disk rather than minted per process
    #[test]
    fn this_node_is_the_same_device_after_a_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).expect("backend");
        let before = local_device(&backend);
        assert_eq!(local_device(&backend), before, "stable within a process");

        // Drop the in-memory state, so the next read is the disk's answer
        // — which is what a fresh process gets.
        SYNC.forget(&backend);
        let after = local_device(&backend);
        assert_eq!(after, before, "a restart minted a new device for this node");
        assert_eq!(
            read_state(&backend, |s| s.devices.len()),
            1,
            "a restart grew the devices table"
        );
    }

    // t[verify files.device.identity] — a peer is the row its endpoint
    // was enrolled under, and nothing else
    #[test]
    fn a_peer_is_resolved_only_by_an_enrolled_unrevoked_admitted_endpoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).expect("backend");
        let endpoint = "3333333333333333333333333333333333333333333333333333333333333333";

        assert!(enrolled(&backend, endpoint).is_err(), "never enrolled");

        let id = DeviceId::generate();
        with_state(&backend, |s| {
            s.devices.insert(
                id.to_string(),
                DeviceInfo {
                    id,
                    name: "laptop".into(),
                    endpoint: Some(endpoint.into()),
                    last_seen: Utc::now(),
                    transfer: TransferPolicy {
                        max_bytes_per_sec: None,
                        paused: false,
                    },
                    revoked: false,
                },
            );
        });
        assert!(
            enrolled(&backend, endpoint).is_err(),
            "a row without admission is not a device that may sync"
        );
        backend.admit_host(
            files_domain::HostId(endpoint.into()),
            files_domain::Hosting::working(),
        );
        assert_eq!(enrolled(&backend, endpoint).expect("enrolled"), id);
        assert_ne!(local_device(&backend), id, "a peer is never this node");

        with_state(&backend, |s| {
            s.devices.get_mut(&id.to_string()).unwrap().revoked = true;
        });
        assert!(enrolled(&backend, endpoint).is_err(), "revoked");
    }

    #[test]
    fn a_software_root_holds_no_facet_capability() {
        let root = FileRootInfo {
            id: uuid::Uuid::nil(),
            name: "src".into(),
            path: Some("/nowhere".into()),
            flavor: RootFlavor::Software,
            created_at: Utc::now(),
            project_version: None,
        };
        assert!(
            capability_of(&root).is_empty(),
            "source is neither sessions nor footage"
        );
    }
}
