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
/// The library kind a setlist is.
const SETLIST_KIND: &str = "songlist";

/// A song's (or a set's presence channel's) doc id in one epoch.
#[must_use]
pub fn doc_id(org: &str, setlist: &str, epoch: u64, part: &str) -> Uuid {
    Uuid::new_v5(&NAMESPACE, format!("{org}/{setlist}/{epoch}/{part}").as_bytes())
}

/// Where a set is.
struct SetState {
    epoch: u64,
    /// Its reset interval, once a playground link opened it.
    reset: Option<Duration>,
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
    pub fn new(org: String, collections: collection::Store, resources: resources::ResourcesBackend) -> Self {
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
            }),
        }
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
            .filter(|c| c.kind.as_str() == SETLIST_KIND)
            .ok_or_else(|| LiveError::NotFound(setlist.to_owned()))?;
        let slugs: Vec<String> = collection
            .items
            .iter()
            .filter(|i| i.node.kind == collection::NodeKind::Song)
            .map(|i| i.node.id.clone())
            .collect();
        let (epoch, start_resetting) = {
            let mut sets = lock(&self.inner.sets);
            let state = sets
                .entry(setlist.to_owned())
                .or_insert_with(|| SetState { epoch: 0, reset: None, ids: Vec::new() });
            let start = state.reset.is_none() && reset.is_some();
            if start {
                state.reset = reset.map(|r| r.max(SHORTEST_RESET));
            }
            let epoch = state.epoch;
            let mut ids: Vec<Uuid> = slugs.iter().map(|s| doc_id(&self.inner.org, setlist, epoch, s)).collect();
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
        let resets_every_secs = lock(&self.inner.sets)
            .get(setlist)
            .and_then(|s| s.reset)
            .and_then(|r| u32::try_from(r.as_secs()).ok());
        wide::set("live.setlist", setlist.to_owned());
        wide::set("live.epoch", i64::try_from(epoch).unwrap_or(i64::MAX));
        Ok(LiveSet {
            setlist: setlist.to_owned(),
            title: collection.title.clone(),
            epoch,
            presence_id: doc_id(&self.inner.org, setlist, epoch, "presence").to_string(),
            songs,
            resets_every_secs,
        })
    }

    fn title(&self, slug: &str) -> String {
        use resources_proto::ResourcesService as _;
        self.inner.resources.song(slug).map(|s| s.title).unwrap_or_else(|_| slug.to_owned())
    }

    /// Move `setlist` on to its next epoch: the old ids are no longer
    /// served, and every subscriber is told.
    pub fn next_epoch(&self, setlist: &str) -> Option<u64> {
        let epoch = {
            let mut sets = lock(&self.inner.sets);
            let state = sets.get_mut(setlist)?;
            let mut admitted = lock(&self.inner.admitted);
            for id in state.ids.drain(..) {
                admitted.remove(&id);
            }
            state.epoch += 1;
            state.epoch
        };
        tracing::info!(live.setlist = setlist, live.epoch = epoch, "live: the set starts over");
        self.inner.epochs.publish(LiveEpoch { setlist: setlist.to_owned(), epoch });
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

/// A member's live sessions: any setlist of the org.
#[derive(Clone)]
pub struct LiveLane(pub LiveHost);

impl live_proto::LiveSessions for LiveLane {
    async fn join(&self, setlist: String) -> Result<LiveSet, LiveError> {
        self.0.join(&setlist, None)
    }

    async fn now(&self) -> f64 {
        now_micros()
    }
}

impl live_proto::LiveSessionsStreamSource for LiveLane {
    fn epochs_hub(&self) -> &architect::PubSub<LiveEpoch> {
        self.0.epochs_hub()
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
}

impl live_proto::LiveSessions for GuestLiveLane {
    /// `setlist` empty joins the link's set — a guest need not know its id.
    async fn join(&self, setlist: String) -> Result<LiveSet, LiveError> {
        let setlist = if setlist.is_empty() { self.setlist.clone() } else { setlist };
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

    async fn now(&self) -> f64 {
        now_micros()
    }
}

impl live_proto::LiveSessionsStreamSource for GuestLiveLane {
    fn epochs_hub(&self) -> &architect::PubSub<LiveEpoch> {
        self.host.epochs_hub()
    }
}

/// The org's one `DocSync`: a live doc to the live host, anything else to
/// the vault's registry.
#[derive(Clone)]
pub struct DocSyncRouter {
    pub live: LiveHost,
    pub vault: DocRegistry,
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
            self.live.registry().sync(doc_id, from, up, down).await
        } else {
            self.vault.sync(doc_id, from, up, down).await
        }
    }
}

/// A guest's `DocSync` / `DocPresence`: live docs only.
#[derive(Clone)]
pub struct LiveOnly(pub LiveHost);

impl DocSync for LiveOnly {
    async fn sync(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        self.0.registry().sync(doc_id, from, up, down).await
    }
}

impl DocPresence for LiveOnly {
    async fn presence(&self, doc_id: Uuid, up: vox::Rx<Vec<u8>>, down: vox::Tx<Vec<u8>>) -> Result<(), SyncError> {
        self.0.registry().presence(doc_id, up, down).await
    }
}

#[cfg(test)]
mod tests {
    use super::doc_id;

    #[test]
    fn a_songs_doc_is_the_same_everywhere_and_new_each_epoch() {
        let a = doc_id("days-to-praise", "worship-set", 0, "washed");
        assert_eq!(a, doc_id("days-to-praise", "worship-set", 0, "washed"), "every peer meets in one doc");
        assert_ne!(a, doc_id("days-to-praise", "worship-set", 1, "washed"), "a reset starts it over");
        assert_ne!(a, doc_id("acme-audio", "worship-set", 0, "washed"), "orgs never share one");
    }
}
