//! Live sessions (`live-proto`): setlists played together, kept open here.
//!
//! Task is the broker behind Session. A live set is a setlist every peer
//! (the desktop app, a browser, a phone) meets in: each song's shared doc
//! synced over the org's `DocSync`, one presence channel for the set (who
//! is on which song, cursors, the transport's position), and Task's clock,
//! which everyone's positions are stamped in.
//!
//! The docs live in [`LiveHost`]'s own `DocRegistry`, beside the vault's
//! (the org mounts one `DocSync` and one `DocPresence` dispatcher; the
//! [`DocSyncRouter`] and the presence router send a live doc's id here).
//! Only ids a join handed out are served ([`LiveHost::admits`]). A song's
//! doc starts empty and the first peer on it seeds it from the song it
//! opened. Docs nobody is syncing are let go after a while: a set nobody
//! is in starts again from its songs' files.
//!
//! **Epochs.** A set shared as a playground (the public demo: a live share
//! link with a reset interval) moves on every so often: its epoch bumps,
//! its docs start over under new ids, and every peer is told
//! ([`LiveSessions::epochs`]) so it re-opens its songs and joins again.
//!
//! Two lanes serve it: a member's ([`LiveLane`], any setlist of the org)
//! and a guest's ([`GuestLiveLane`], a live share link's one setlist, with
//! each song's files as a documents link).
//!
//! **Observers.** A peer that only watches (an assistant without edit
//! rights) calls `observe` instead of `join`. Its connection's
//! [`LiveAccess`] then marks the set's song docs read-only, and that
//! connection's `DocSync` serves them through [`LiveHost::watch`]: the doc
//! and every change to it go down, and every update it sends up is
//! refused (counted, logged, never applied or relayed). Presence is not
//! gated: an observer shows up in the set like anyone. Members and bots
//! with edit rights `join` and write live.
//!
//! **Agents over MCP.** An agent reaches the same docs as JSON through
//! [`crate::live_docs`]: `live_observe` a set, `doc_read` a song's doc,
//! `doc_apply` ops to it — applied to the served doc here, so every peer
//! syncing it sees the edit at once, attributed and undoable.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use architect_telemetry::wide;
use crdt::CrdtDoc;
use crdt::registry::DocRegistry;
use crdt::sync::{DocPresence, DocSync, SyncDown, SyncError};
use live_proto::{LiveEpoch, LiveError, LiveSet, LiveSong};
use uuid::Uuid;

/// The namespace live doc ids are made in (`live-sessn-task`).
const NAMESPACE: Uuid = Uuid::from_u128(0x6c69_7665_2d73_6573_736e_2d74_6173_6b00);
/// How long a peer's presence lives without an update.
const PRESENCE_TIMEOUT_MS: i64 = 30_000;
/// How long a doc nobody syncs stays open.
const IDLE: Duration = Duration::from_secs(15 * 60);
/// A reset interval is never shorter than this (a link's typo is not a
/// set that resets every second).
const SHORTEST_RESET: Duration = Duration::from_secs(30);
/// How often a watched doc is touched in the registry, so it is not let
/// go while only observers follow it (well inside [`IDLE`]).
const WATCH_KEEPALIVE: Duration = Duration::from_secs(60);
/// The library kind a setlist is.
const SETLIST_KIND: &str = "songlist";
/// A set for a service or a show — played live as a song list is.
const SET_KIND: &str = "setlist";

/// A song's (or a set's presence channel's) doc id in one epoch.
#[must_use]
pub fn doc_id(org: &str, setlist: &str, epoch: u64, part: &str) -> Uuid {
    Uuid::new_v5(
        &NAMESPACE,
        format!("{org}/{setlist}/{epoch}/{part}").as_bytes(),
    )
}

/// Where a set is.
struct SetState {
    epoch: u64,
    /// Its reset interval, once a playground link opened it.
    reset: Option<Duration>,
    /// When the current epoch began, on Task's clock (microseconds) — set
    /// when the resets start and at each one.
    started_at: f64,
    /// The ids its current epoch serves.
    ids: Vec<Uuid>,
}

struct Inner {
    org: String,
    registry: DocRegistry,
    /// Every id a join handed out, this epoch.
    admitted: Arc<Mutex<HashSet<Uuid>>>,
    sets: Mutex<HashMap<String, SetState>>,
    epochs: architect::PubSub<LiveEpoch>,
    collections: collection::Store,
    resources: resources::ResourcesBackend,
    /// What agents applied to the docs over MCP (`crate::live_docs`).
    edits: crate::live_docs::Edits,
}

/// An org's live sessions.
#[derive(Clone)]
pub struct LiveHost {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl LiveHost {
    /// The live sessions of the org `org`, its setlists read from
    /// `collections` and their songs' titles from `resources`.
    #[must_use]
    pub fn new(
        org: String,
        collections: collection::Store,
        resources: resources::ResourcesBackend,
    ) -> Self {
        let admitted: Arc<Mutex<HashSet<Uuid>>> = Arc::default();
        let admit = Arc::clone(&admitted);
        // Every doc starts empty — the first peer on a song seeds it —
        // and the presence channel is only ever ephemeral state.
        let registry = DocRegistry::new(|_| Box::pin(async { Ok(CrdtDoc::ephemeral()) }))
            .with_presence_timeout(PRESENCE_TIMEOUT_MS)
            .with_admission(move |doc_id| lock(&admit).contains(&doc_id))
            .with_idle_eviction(IDLE);
        Self {
            inner: Arc::new(Inner {
                org,
                registry,
                admitted,
                sets: Mutex::default(),
                epochs: architect::PubSub::sliding(16),
                collections,
                resources,
                edits: crate::live_docs::Edits::default(),
            }),
        }
    }

    /// The applies agents made to this host's docs, and their undo.
    #[must_use]
    pub fn edits(&self) -> &crate::live_docs::Edits {
        &self.inner.edits
    }

    /// The set and song a served song doc belongs to (its current
    /// epoch's), or `None` for an id no set serves now — a presence
    /// channel, an old epoch's doc, a stranger's id.
    #[must_use]
    pub fn locate(&self, doc_id: Uuid) -> Option<(String, String)> {
        let setlist = lock(&self.inner.sets)
            .iter()
            .find(|(_, state)| state.ids.contains(&doc_id))
            .map(|(setlist, _)| setlist.clone())?;
        let set = self.join(&setlist, None).ok()?;
        let wanted = doc_id.to_string();
        let song = set.songs.into_iter().find(|s| s.doc_id == wanted)?;
        Some((setlist, song.slug))
    }

    /// Whether `doc_id` is a live doc this host serves now.
    #[must_use]
    pub fn admits(&self, doc_id: Uuid) -> bool {
        lock(&self.inner.admitted).contains(&doc_id)
    }

    /// The docs' registry (`DocSync` + `DocPresence`).
    #[must_use]
    pub fn registry(&self) -> &DocRegistry {
        &self.inner.registry
    }

    /// Every epoch as it moves.
    #[must_use]
    pub fn epochs_hub(&self) -> &architect::PubSub<LiveEpoch> {
        &self.inner.epochs
    }

    /// Join `setlist`: its current epoch's ids, admitted. A `reset` makes
    /// it a playground from now on (the first link to say so wins).
    ///
    /// # Errors
    ///
    /// No such setlist.
    pub fn join(&self, setlist: &str, reset: Option<Duration>) -> Result<LiveSet, LiveError> {
        use collection::CollectionService as _;
        let collection = self
            .inner
            .collections
            .get(setlist)
            .map_err(|e| LiveError::Failed(e.to_string()))?
            .filter(|c| matches!(c.kind.as_str(), SETLIST_KIND | SET_KIND))
            .ok_or_else(|| LiveError::NotFound(setlist.to_owned()))?;
        let slugs: Vec<String> = collection
            .items
            .iter()
            .filter(|i| i.node.kind == collection::NodeKind::Song)
            .map(|i| i.node.id.clone())
            .collect();
        let (epoch, start_resetting) = {
            let mut sets = lock(&self.inner.sets);
            let state = sets.entry(setlist.to_owned()).or_insert_with(|| SetState {
                epoch: 0,
                reset: None,
                started_at: now_micros(),
                ids: Vec::new(),
            });
            let start = state.reset.is_none() && reset.is_some();
            if start {
                state.reset = reset.map(|r| r.max(SHORTEST_RESET));
                state.started_at = now_micros();
            }
            let epoch = state.epoch;
            let mut ids: Vec<Uuid> = slugs
                .iter()
                .map(|s| doc_id(&self.inner.org, setlist, epoch, s))
                .collect();
            ids.push(doc_id(&self.inner.org, setlist, epoch, "presence"));
            lock(&self.inner.admitted).extend(ids.iter().copied());
            state.ids = ids;
            (epoch, start.then_some(state.reset).flatten())
        };
        if let Some(every) = start_resetting {
            self.reset_every(setlist.to_owned(), every);
        }
        let songs = slugs
            .iter()
            .map(|slug| LiveSong {
                slug: slug.clone(),
                title: self.title(slug),
                doc_id: doc_id(&self.inner.org, setlist, epoch, slug).to_string(),
                files: None,
            })
            .collect();
        let (resets_every_secs, resets_at) = lock(&self.inner.sets)
            .get(setlist)
            .and_then(|s| Some((s.reset?, s.started_at)))
            .map_or((None, None), |(every, started)| {
                (
                    u32::try_from(every.as_secs()).ok(),
                    Some(started + every.as_secs_f64() * 1e6),
                )
            });
        wide::set("live.setlist", setlist.to_owned());
        wide::set("live.epoch", i64::try_from(epoch).unwrap_or(i64::MAX));
        Ok(LiveSet {
            setlist: setlist.to_owned(),
            title: collection.title,
            epoch,
            presence_id: doc_id(&self.inner.org, setlist, epoch, "presence").to_string(),
            songs,
            resets_every_secs,
            resets_at,
            read_only: false,
        })
    }

    /// Serve `doc_id` to a peer that only watches it: the doc's history
    /// it lacks (after `from`, its version vector) and every change after
    /// that go down `down`; every update it sends up `up` is refused —
    /// never applied, never relayed. Runs until the peer's up channel
    /// closes (or the doc stops being served: a new epoch), like a
    /// `DocSync` session.
    ///
    /// The registry does not count this as a session, so the doc is
    /// touched every [`WATCH_KEEPALIVE`] to keep it from being let go
    /// while only observers follow it.
    ///
    /// # Errors
    ///
    /// The doc is not served here, or `from` is not a version vector.
    pub async fn watch(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        mut up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        use crdt::loro::{ExportMode, VersionVector};
        let registry = self.registry();
        let doc = registry.doc(doc_id).await?;
        let from = if from.is_empty() {
            VersionVector::new()
        } else {
            VersionVector::decode(&from).map_err(|e| SyncError::BadVersion(e.to_string()))?
        };
        // Subscribe before the first export, so a change landing between
        // the two is not missed (sending it twice is harmless: imports
        // merge). The callback holds no doc handle — see the deadlock
        // note on `CrdtDoc::downgrade`.
        let changed = Arc::new(tokio::sync::Notify::new());
        let wake = Arc::clone(&changed);
        let subscription =
            doc.loro()
                .subscribe_root(Arc::new(move |_: crdt::loro::event::DiffEvent<'_>| {
                    wake.notify_one()
                }));
        // The frontier first, then everything after `from`: the export can
        // only run ahead of what `sent` says, never behind it.
        let mut sent = doc.loro().oplog_vv();
        let backlog = doc
            .loro()
            .export(ExportMode::updates(&from))
            .map_err(|e| SyncError::Internal(format!("export updates: {e}")))?;
        down.send(SyncDown::Attached {
            server_vv: sent.encode(),
            backlog,
        })
        .await
        .map_err(|e| SyncError::Internal(format!("send attach: {e}")))?;
        wide::set("live.sync", "observer");

        let mut refused: u64 = 0;
        let mut keepalive = tokio::time::interval(WATCH_KEEPALIVE);
        keepalive.tick().await; // the first tick is now
        loop {
            tokio::select! {
                () = changed.notified() => {
                    let now = doc.loro().oplog_vv();
                    if now == sent {
                        continue;
                    }
                    let update = match doc.loro().export(ExportMode::updates(&sent)) {
                        Ok(update) => update,
                        Err(e) => {
                            tracing::warn!(%doc_id, "live: an observer's update could not be exported: {e}");
                            continue;
                        }
                    };
                    sent = now;
                    if down.send(SyncDown::Update(update)).await.is_err() {
                        break;
                    }
                }
                frame = up.recv() => {
                    if !matches!(frame, Ok(Some(_))) {
                        break;
                    }
                    refused += 1;
                    if refused == 1 {
                        tracing::warn!(%doc_id, "live: an observer sent an edit; refused (read-only)");
                    }
                }
                _ = keepalive.tick() => {
                    // Gone from the host (a new epoch): the session ends,
                    // and the peer re-observes the next epoch's docs.
                    if registry.doc(doc_id).await.is_err() {
                        break;
                    }
                }
            }
        }
        wide::set(
            "live.writes_refused",
            i64::try_from(refused).unwrap_or(i64::MAX),
        );
        // The subscription goes before the doc handle.
        drop(subscription);
        drop(doc);
        Ok(())
    }

    /// Serve `doc_id` over `DocSync` as `access` allows: read-only to a
    /// connection that observes it ([`Self::watch`]), else as the
    /// registry does.
    async fn sync_as(
        &self,
        access: &LiveAccess,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        if access.is_read_only(doc_id) {
            self.watch(doc_id, from, up, down).await
        } else {
            self.registry().sync(doc_id, from, up, down).await
        }
    }

    fn title(&self, slug: &str) -> String {
        use resources_proto::ResourcesService as _;
        self.inner
            .resources
            .song(slug)
            .map(|s| s.title)
            .unwrap_or_else(|_| slug.to_owned())
    }

    /// Move `setlist` on to its next epoch: the old ids are no longer
    /// served, and every subscriber is told.
    pub fn next_epoch(&self, setlist: &str) -> Option<u64> {
        let epoch = {
            let mut sets = lock(&self.inner.sets);
            let state = sets.get_mut(setlist)?;
            let mut admitted = lock(&self.inner.admitted);
            let old: Vec<Uuid> = std::mem::take(&mut state.ids);
            for id in &old {
                admitted.remove(id);
            }
            self.inner.edits.forget(&old);
            state.epoch += 1;
            state.started_at = now_micros();
            state.epoch
        };
        tracing::info!(
            live.setlist = setlist,
            live.epoch = epoch,
            "live: the set starts over"
        );
        self.inner.epochs.publish(LiveEpoch {
            setlist: setlist.to_owned(),
            epoch,
        });
        Some(epoch)
    }

    /// Start `setlist` over every `every`, for as long as the host lives.
    fn reset_every(&self, setlist: String, every: Duration) {
        let host: Weak<Inner> = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(inner) = host.upgrade() else { return };
                LiveHost { inner }.next_epoch(&setlist);
            }
        });
    }
}

/// Task's clock, microseconds on a monotonic base.
fn now_micros() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(std::time::Instant::now);
    start.elapsed().as_secs_f64() * 1e6
}

/// What one connection may do in the live sets it reached: the song docs
/// it observes (read-only) rather than joined. Shared by that
/// connection's `LiveSessions` lane and its `DocSync`, so an `observe`
/// makes its `DocSync` refuse the set's writes, and a `join` lifts it.
/// A fresh connection observes nothing.
#[derive(Clone, Default)]
pub struct LiveAccess {
    read_only: Arc<Mutex<HashSet<Uuid>>>,
}

impl LiveAccess {
    /// `set`'s songs are read-only on this connection from now on.
    fn observe(&self, set: &LiveSet) {
        lock(&self.read_only).extend(song_ids(set));
    }

    /// `set`'s songs are writable on this connection again.
    fn join(&self, set: &LiveSet) {
        let mut read_only = lock(&self.read_only);
        for id in song_ids(set) {
            read_only.remove(&id);
        }
    }

    /// Whether this connection only watches `doc_id`.
    #[must_use]
    pub fn is_read_only(&self, doc_id: Uuid) -> bool {
        lock(&self.read_only).contains(&doc_id)
    }
}

/// The ids of `set`'s song docs (not its presence channel: an observer's
/// presence counts like anyone's).
fn song_ids(set: &LiveSet) -> impl Iterator<Item = Uuid> + '_ {
    set.songs.iter().filter_map(|s| s.doc_id.parse().ok())
}

/// `set`, as an observer gets it — `access` marking its songs read-only.
fn observed(access: &LiveAccess, mut set: LiveSet) -> LiveSet {
    access.observe(&set);
    set.read_only = true;
    wide::set("live.role", "observer");
    set
}

/// A member's live sessions: any setlist of the org.
#[derive(Clone)]
pub struct LiveLane {
    pub host: LiveHost,
    /// This connection's access, shared with its [`DocSyncRouter`].
    pub access: LiveAccess,
}

impl LiveLane {
    /// A lane for one connection, observing nothing yet.
    #[must_use]
    pub fn new(host: LiveHost, access: LiveAccess) -> Self {
        Self { host, access }
    }
}

impl live_proto::LiveSessions for LiveLane {
    async fn join(&self, setlist: String) -> Result<LiveSet, LiveError> {
        let set = self.host.join(&setlist, None)?;
        self.access.join(&set);
        Ok(set)
    }

    async fn now(&self) -> f64 {
        now_micros()
    }

    async fn observe(&self, setlist: String) -> Result<LiveSet, LiveError> {
        Ok(observed(&self.access, self.host.join(&setlist, None)?))
    }
}

impl live_proto::LiveSessionsStreamSource for LiveLane {
    fn epochs_hub(&self) -> &architect::PubSub<LiveEpoch> {
        self.host.epochs_hub()
    }
}

/// A guest's live session: a live share link's one setlist, played by
/// someone with no account — its songs' files as documents links.
#[derive(Clone)]
pub struct GuestLiveLane {
    pub host: LiveHost,
    pub setlist: String,
    pub reset: Option<Duration>,
    /// Each song's files link, by slug.
    pub files: Arc<HashMap<String, String>>,
    /// This connection's access, shared with its [`LiveOnly`].
    pub access: LiveAccess,
}

impl GuestLiveLane {
    /// The link's set, joined: `setlist` empty is the link's set — a guest
    /// need not know its id — and any other set is refused.
    fn open(&self, setlist: String) -> Result<LiveSet, LiveError> {
        let setlist = if setlist.is_empty() {
            self.setlist.clone()
        } else {
            setlist
        };
        if setlist != self.setlist {
            tracing::warn!(live.setlist = %setlist, "live: a guest asked for a set its link does not share");
            return Err(LiveError::NotAllowed("this link shares another set".into()));
        }
        let mut set = self.host.join(&setlist, self.reset)?;
        for song in &mut set.songs {
            song.files = self.files.get(&song.slug).cloned();
        }
        Ok(set)
    }
}

impl live_proto::LiveSessions for GuestLiveLane {
    /// `setlist` empty joins the link's set — a guest need not know its id.
    async fn join(&self, setlist: String) -> Result<LiveSet, LiveError> {
        let set = self.open(setlist)?;
        self.access.join(&set);
        Ok(set)
    }

    async fn now(&self) -> f64 {
        now_micros()
    }

    /// `setlist` empty observes the link's set.
    async fn observe(&self, setlist: String) -> Result<LiveSet, LiveError> {
        Ok(observed(&self.access, self.open(setlist)?))
    }
}

impl live_proto::LiveSessionsStreamSource for GuestLiveLane {
    fn epochs_hub(&self) -> &architect::PubSub<LiveEpoch> {
        self.host.epochs_hub()
    }
}

/// The org's one `DocSync`: a live doc to the live host (read-only where
/// this connection observes it), anything else to the org's app docs or
/// its vault ([`crate::app_docs::OrgDocSync`]).
#[derive(Clone)]
pub struct DocSyncRouter {
    pub live: LiveHost,
    pub rest: crate::app_docs::OrgDocSync,
    /// This connection's access, shared with its [`LiveLane`].
    pub access: LiveAccess,
}

impl DocSync for DocSyncRouter {
    async fn sync(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        if self.live.admits(doc_id) {
            self.live
                .sync_as(&self.access, doc_id, from, up, down)
                .await
        } else {
            self.rest.sync(doc_id, from, up, down).await
        }
    }
}

/// A guest's `DocSync` / `DocPresence`: live docs only (read-only where
/// this connection observes them).
#[derive(Clone)]
pub struct LiveOnly {
    pub host: LiveHost,
    /// This connection's access, shared with its [`GuestLiveLane`].
    pub access: LiveAccess,
}

impl LiveOnly {
    /// The live docs for one connection, observing nothing yet.
    #[must_use]
    pub fn new(host: LiveHost, access: LiveAccess) -> Self {
        Self { host, access }
    }
}

impl DocSync for LiveOnly {
    async fn sync(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        self.host
            .sync_as(&self.access, doc_id, from, up, down)
            .await
    }
}

impl DocPresence for LiveOnly {
    async fn presence(
        &self,
        doc_id: Uuid,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<Vec<u8>>,
    ) -> Result<(), SyncError> {
        self.host.registry().presence(doc_id, up, down).await
    }
}

#[cfg(test)]
mod tests {
    use super::doc_id;

    #[test]
    fn a_songs_doc_is_the_same_everywhere_and_new_each_epoch() {
        let a = doc_id("days-to-praise", "worship-set", 0, "washed");
        assert_eq!(
            a,
            doc_id("days-to-praise", "worship-set", 0, "washed"),
            "every peer meets in one doc"
        );
        assert_ne!(
            a,
            doc_id("days-to-praise", "worship-set", 1, "washed"),
            "a reset starts it over"
        );
        assert_ne!(
            a,
            doc_id("acme-audio", "worship-set", 0, "washed"),
            "orgs never share one"
        );
    }
}
