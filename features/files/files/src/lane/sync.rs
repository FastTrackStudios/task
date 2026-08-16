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
//! **The half that is still missing is device *identity*.**
//! [`this_device`] mints an id per process, so a restart comes back as a
//! stranger: its subscriptions are on disk under the old id and it reads
//! back an empty one, and the devices table grows a row per process
//! lifetime. Nothing degrades into a *wrong* answer — an unrecognised
//! device takes nothing, and under `files.sync.selective` taking nothing
//! means stubs, never absent content — but a revocation does not yet
//! outlive the process it was issued in, which is the security-relevant
//! half. Closing that needs a device registry to mint a stable id from;
//! it is not a change to this file's storage.
//!
//! **A project's capabilities are derived from its `RootFlavor`.** Facets
//! come from capabilities, capabilities come from a project declaration,
//! and that declaration is not yet wired to roots — see
//! [`capability_of`]. A media root therefore takes *every* media
//! capability's layouts rather than a guess at which one it holds, which
//! is the widest correct answer while the narrow one is unavailable.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::OnceLock;

use chrono::Utc;
use files_domain::facet::{Capability, Facet, FacetMap, Source};
use files_domain::hydration::{Subscription as Plan, decide};
use files_domain::ignore::{IgnoreSet as DomainIgnores, Layer};
use files_proto::error::FilesFault;
use files_proto::id::{DeviceId, RootId};
use files_proto::model::{FileRootInfo, RootFlavor};
use files_proto::path::RootPath;
use files_proto::service::legacy::{FilesError, FilesService};
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
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(from = "Wire", into = "Wire")]
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
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Wire {
    subscriptions: Vec<Subscription>,
    devices: Vec<DeviceInfo>,
    mappings: Vec<MappingRow>,
}

#[derive(serde::Serialize, serde::Deserialize)]
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
        }
    }
}

static SYNC: crate::durable::Scoped<SyncState> = crate::durable::Scoped::new("sync");

/// The device this process speaks for.
///
/// **Deliberately not persisted, and persisting it would be worse than
/// leaving it.** `files.device.identity` says a device presents an
/// identity it holds itself and the server records it rather than
/// minting one — and that machinery already exists: `files-daemon`'s
/// `DeviceIdentity` is a per-machine `(device id, secret)` on disk,
/// surviving restart and session expiry, mapped onto storage-agent
/// enrollment so an operator can revoke one device without touching
/// others.
///
/// A server-side id is not that. The server is ONE machine serving many
/// devices, so making this stable would make it stably wrong: every
/// client would present as the same device forever, one subscription
/// would belong to all of them, and revoking it would cut everyone.
/// Minting it per process at least keeps the error obvious.
///
/// The fix is to thread the caller's identity from the connection, which
/// no method on this trait carries and `FilesBackend` holds no session
/// for. Until then, revocation is only as precise as identity is — which
/// is not very.
// FUTURE: take the device id from the connection, not from here.
fn this_device() -> DeviceId {
    static ID: OnceLock<DeviceId> = OnceLock::new();
    *ID.get_or_init(DeviceId::generate)
}

/// Mutate this org's state and persist it, registering this device first.
///
/// The registration is why announcing a device belongs on the write path
/// and not on the read one: a row saying "this device exists" is a fact
/// about the org, and a table that only remembered it for as long as the
/// process ran could not be revoked against.
fn with_state<T>(backend: &FilesBackend, f: impl FnOnce(&mut SyncState) -> T) -> T {
    SYNC.write(backend, |s| {
        let local = this_device();
        s.devices.entry(local.to_string()).or_insert(DeviceInfo {
            id: local,
            name: "this device".to_string(),
            last_seen: Utc::now(),
            transfer: TransferPolicy {
                max_bytes_per_sec: None,
                paused: false,
            },
            revoked: false,
        });
        f(s)
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
        let project = FilesService::ignore_set(self, root_id.get())
            .await
            .map_err(fault)?;
        let mut set = DomainIgnores::new(capability_of(root));
        set.with_project(project);
        Ok(set)
    }

    /// This device's subscription to a root, as the domain wants it.
    fn plan_for(&self, root_id: RootId) -> Plan {
        let stored = stored_subscription(self, root_id);
        let mut plan = Plan::new(stored.facets.into_iter().map(|f| Facet::new(f.0)));
        for pinned in stored.pinned {
            plan.pin(pinned);
        }
        plan
    }

    /// Bring the live tree into line with this device's subscription.
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
                Hydration::Resident => {
                    FilesService::hydrate(self, root_id.get(), path.to_string()).await
                }
                // `Unavailable` is a fact about the network, decided
                // elsewhere; nothing the domain returns here asks for it.
                _ => FilesService::dehydrate(self, root_id.get(), path.to_string()).await,
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

/// This device's stored subscription, or an empty one.
///
/// Empty rather than absent: a device that has said nothing takes
/// nothing, which under `files.sync.selective` means everything is a
/// stub — present, sized, hydrating on access. There is no state in
/// which content is missing.
fn stored_subscription(backend: &FilesBackend, root_id: RootId) -> Subscription {
    let device = this_device();
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

fn store_subscription(backend: &FilesBackend, sub: &Subscription) {
    with_state(backend, |s| {
        s.subscriptions
            .insert((sub.device, sub.root_id), sub.clone());
    });
}

// ── The lane ───────────────────────────────────────────────────────

impl SyncService for FilesBackend {
    // t[impl files.facet.vocabulary] — one vocabulary, from the capability
    // t[impl files.facet.tool-layout] — layouts arrive with the capability
    // t[impl files.facet.project-override] — unmapped is reported, never guessed
    async fn facets(&self, root_id: RootId) -> Result<Vec<FacetBinding>, FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
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
        let project = FilesService::ignore_set(self, root_id.get())
            .await
            .map_err(fault)?;
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
        crate::lane::root_or_fault(self, root_id)?;
        // Only the project layer is settable: the other two are
        // knowledge about an OS and about an application, and a root
        // that could edit them could hide a `.DS_Store` policy bug from
        // every other root on the machine.
        FilesService::set_ignore_set(self, root_id.get(), patterns)
            .await
            .map_err(fault)?;
        SyncService::ignore_set(self, root_id).await
    }

    async fn subscription(&self, root_id: RootId) -> Result<Subscription, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        Ok(stored_subscription(self, root_id))
    }

    // t[impl files.sync.selective] — facets, not globs; unsubscribed is a stub
    async fn subscribe(
        &self,
        root_id: RootId,
        facets: Vec<FacetName>,
    ) -> Result<Subscription, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let mut sub = stored_subscription(self, root_id);
        sub.facets = facets;
        sub.facets.sort_by(|a, b| a.0.cmp(&b.0));
        sub.facets.dedup();
        store_subscription(self, &sub);
        // Content leaving the subscription becomes a stub here rather
        // than at some later sweep: a user who unticks "footage" and
        // then unplugs must already have the space back.
        self.apply_subscription(root_id).await?;
        Ok(sub)
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

        let mut sub = stored_subscription(self, root_id);
        if pinned {
            sub.pinned.extend(paths);
            sub.pinned.sort();
            sub.pinned.dedup();
        } else {
            sub.pinned.retain(|p| !paths.contains(p));
        }
        store_subscription(self, &sub);
        self.apply_subscription(root_id).await?;
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
            // Explicit, so unlike the subscription sweep a refusal is
            // reported: the caller named this path and is owed the
            // reason it could not be released.
            let entry = if resident {
                FilesService::hydrate(self, root_id.get(), path.to_string()).await
            } else {
                FilesService::dehydrate(self, root_id.get(), path.to_string()).await
            }
            .map_err(fault)?;
            debug_assert_eq!(entry.stub, !resident);
            done.push(path);
        }
        Ok(done)
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
        with_state(self, |s| {
            let info = s
                .devices
                .get_mut(&device.to_string())
                .ok_or(FilesFault::DeviceNotFound(device))?;
            // Revocation is a flag rather than a deletion: a deleted row
            // is a device that reconnects as a stranger and is let back
            // in. The wipe happens on the device, on next contact.
            info.revoked = true;
            info.transfer.paused = true;
            Ok(info.clone())
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
            DomainIgnores::default().patterns(Layer::Capability).is_empty(),
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
