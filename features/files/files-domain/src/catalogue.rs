//! The replicated catalogue — `files.catalogue.*`.
//!
//! Structure replicates everywhere; content replicates selectively. This
//! is the structure half, and it is authoritative about structure and
//! **nothing else**. The moment it also caches content, its size stops
//! scaling with file count and it has quietly become a second storage
//! system.
//!
//! Three properties the type is built to hold:
//!
//! - **Browsable offline.** Every reachable path has an entry whatever
//!   its hydration, so a folder never appears empty because its server is
//!   down. An unreachable location changes entries' *hydration*, never
//!   their existence.
//! - **Staleness is visible.** Each entry records when its structure was
//!   last confirmed. The catalogue is allowed to be stale; it is not
//!   allowed to look current when it is not.
//! - **Convergence without re-listing.** Changes carry a monotonic
//!   sequence, so a client that has been away resumes from a cursor
//!   rather than re-reading the tree.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use files_proto::id::RootId;
use files_proto::path::RootPath;
use files_proto::service::tree::{CatalogueEntry, Cursor, Freshness, Hydration};

/// A change, as a client sees it.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Upserted(CatalogueEntry),
    Removed(RootPath),
}

/// One root's structure, plus the change log a client resumes from.
#[derive(Debug, Clone)]
// t[impl files.catalogue.complete]
pub struct Catalogue {
    root_id: RootId,
    entries: BTreeMap<RootPath, CatalogueEntry>,
    /// `(sequence, change)`, oldest first. Monotonic and never reordered.
    log: Vec<(u64, Change)>,
    seq: u64,
    /// Last time the holding location answered at all.
    confirmed_at: Option<DateTime<Utc>>,
    reachable: bool,
}

impl Catalogue {
    #[must_use]
    pub fn new(root_id: RootId) -> Self {
        Self {
            root_id,
            entries: BTreeMap::new(),
            log: Vec::new(),
            seq: 0,
            confirmed_at: None,
            reachable: true,
        }
    }

    #[must_use]
    pub fn root_id(&self) -> RootId {
        self.root_id
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn get(&self, path: &RootPath) -> Option<&CatalogueEntry> {
        self.entries.get(path)
    }

    /// Direct children of `path`. Serves a browse without touching the
    /// filesystem, which is what makes an offline listing possible.
    #[must_use]
    // t[impl files.catalogue.offline] — served without touching the filesystem
    pub fn children(&self, path: &RootPath) -> Vec<&CatalogueEntry> {
        let depth = if path.is_root() {
            1
        } else {
            path.components().count() + 1
        };
        self.entries
            .values()
            .filter(|e| e.path.is_within(path) && &e.path != path)
            .filter(|e| e.path.components().count() == depth)
            .collect()
    }

    /// Add or replace an entry.
    pub fn upsert(&mut self, entry: CatalogueEntry) {
        self.seq += 1;
        self.confirmed_at = Some(entry.confirmed_at);
        self.entries.insert(entry.path.clone(), entry.clone());
        self.log.push((self.seq, Change::Upserted(entry)));
    }

    /// Drop an entry and everything beneath it.
    pub fn remove(&mut self, path: &RootPath) {
        let doomed: Vec<RootPath> = self
            .entries
            .keys()
            .filter(|p| p.is_within(path))
            .cloned()
            .collect();
        for p in doomed {
            self.entries.remove(&p);
            self.seq += 1;
            self.log.push((self.seq, Change::Removed(p)));
        }
    }

    /// A location became reachable or stopped being so.
    ///
    /// Entries held there change hydration and **nothing else** — they
    /// keep their name, size, kind and content address, because a server
    /// being down is not a fact about the tree.
    // t[impl files.catalogue.offline] — hydration changes, structure does not
    pub fn set_location_reachable(&mut self, location: &str, reachable: bool, now: DateTime<Utc>) {
        let affected: Vec<RootPath> = self
            .entries
            .values()
            .filter(|e| e.locations.iter().any(|l| l == location))
            .map(|e| e.path.clone())
            .collect();

        for path in affected {
            let Some(entry) = self.entries.get_mut(&path) else {
                continue;
            };
            let next = if reachable {
                Hydration::Stub
            } else {
                Hydration::Unavailable
            };
            if entry.hydration == next {
                continue;
            }
            entry.hydration = next;
            let updated = entry.clone();
            self.seq += 1;
            self.log.push((self.seq, Change::Upserted(updated)));
        }

        self.reachable = reachable;
        if reachable {
            self.confirmed_at = Some(now);
        }
    }

    /// The current cursor. A client subscribes, takes this, then pulls
    /// state — so nothing is missed in between.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        Cursor(self.seq.to_string())
    }

    /// Everything that happened after `cursor`.
    ///
    /// An unparseable or future cursor yields everything, which is the
    /// safe direction: a client re-syncs rather than silently skipping
    /// changes it never saw.
    #[must_use]
    // t[impl files.catalogue.concurrent] — resume, never re-list
    pub fn changes_since(&self, cursor: &Cursor) -> Vec<Change> {
        let from: u64 = cursor.0.parse().unwrap_or(0);
        self.log
            .iter()
            .filter(|(seq, _)| *seq > from)
            .map(|(_, c)| c.clone())
            .collect()
    }

    /// Drop log entries a client can no longer be resuming from.
    ///
    /// Bounded growth is the requirement the log would otherwise break:
    /// entries scale with file count, but a change log scales with
    /// *edits*, forever.
    // t[impl files.catalogue.bounded]
    pub fn compact(&mut self, keep_after: &Cursor) {
        let from: u64 = keep_after.0.parse().unwrap_or(0);
        self.log.retain(|(seq, _)| *seq > from);
    }

    /// How current this view is, and of what.
    #[must_use]
    pub fn freshness(&self, now: DateTime<Utc>) -> Freshness {
        Freshness {
            root_id: self.root_id,
            confirmed_at: self.confirmed_at.unwrap_or(now),
            reachable: self.reachable,
            entries: self.entries.len() as u64,
        }
    }

    /// Whether this view should be presented as "as of" rather than as
    /// current.
    #[must_use]
    // t[impl files.catalogue.staleness]
    pub fn is_stale(&self, now: DateTime<Utc>, ttl: Duration) -> bool {
        match self.confirmed_at {
            None => true,
            Some(at) => !self.reachable || now.signed_duration_since(at) > ttl,
        }
    }

    /// Entries not confirmed within `ttl` — what a re-confirmation pass
    /// works through.
    #[must_use]
    pub fn stale_entries(&self, now: DateTime<Utc>, ttl: Duration) -> Vec<&CatalogueEntry> {
        self.entries
            .values()
            .filter(|e| now.signed_duration_since(e.confirmed_at) > ttl)
            .collect()
    }

    /// Entries published but not yet verified — adoption's tail.
    #[must_use]
    pub fn unverified(&self) -> Vec<&CatalogueEntry> {
        self.entries.values().filter(|e| e.content.is_none()).collect()
    }

    /// Total logical bytes. Sums entry metadata, never content, because
    /// content is not here.
    #[must_use]
    pub fn logical_bytes(&self) -> u64 {
        self.entries.values().map(|e| e.size).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use files_proto::service::tree::EntryKind;
    use uuid::Uuid;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn entry(path: &str, size: u64, loc: &str, now: DateTime<Utc>) -> CatalogueEntry {
        CatalogueEntry {
            root_id: uuid(),
            path: RootPath::parse(path).unwrap(),
            kind: EntryKind::File,
            size,
            content: None,
            hydration: Hydration::Stub,
            locations: vec![loc.to_string()],
            modified_at: now,
            confirmed_at: now,
        }
    }

    fn uuid() -> RootId {
        RootId::new(Uuid::from_bytes([7u8; 16]))
    }

    fn cat() -> Catalogue {
        Catalogue::new(uuid())
    }

    #[test]
    // t[verify files.catalogue.complete]
    fn children_are_direct_only() {
        let mut c = cat();
        for p in [
            "Sessions",
            "Sessions/Song",
            "Sessions/Song/Audio Files",
            "Renders",
        ] {
            c.upsert(entry(p, 10, "studio", at(0)));
        }
        let top = c.children(&RootPath::root());
        assert_eq!(top.len(), 2, "Sessions and Renders, not their descendants");
    }

    #[test]
    // t[verify files.catalogue.offline]
    fn an_unreachable_location_hides_content_not_structure() {
        let mut c = cat();
        c.upsert(entry("Footage/A.mov", 400, "post", at(0)));
        c.upsert(entry("Sessions/A.rpp", 4, "studio", at(0)));

        c.set_location_reachable("post", false, at(10));

        assert_eq!(c.len(), 2, "the tree did not shrink");
        let e = c.get(&RootPath::parse("Footage/A.mov").unwrap()).unwrap();
        assert_eq!(e.hydration, Hydration::Unavailable);
        assert_eq!(e.size, 400, "size survives");
        assert!(!c.children(&RootPath::parse("Footage").unwrap()).is_empty());
    }

    #[test]
    // t[verify files.catalogue.concurrent]
    fn a_client_resumes_from_a_cursor_without_relisting() {
        let mut c = cat();
        c.upsert(entry("a", 1, "studio", at(0)));
        let seen = c.cursor();
        c.upsert(entry("b", 1, "studio", at(1)));
        c.upsert(entry("c", 1, "studio", at(2)));

        let delta = c.changes_since(&seen);
        assert_eq!(delta.len(), 2, "only what it missed");
    }

    #[test]
    // t[verify files.catalogue.concurrent]
    fn an_unknown_cursor_resyncs_rather_than_skipping() {
        let mut c = cat();
        c.upsert(entry("a", 1, "studio", at(0)));
        let all = c.changes_since(&Cursor("not-a-number".into()));
        assert_eq!(all.len(), 1, "safe direction is to send everything");
    }

    #[test]
    fn removing_takes_the_subtree() {
        let mut c = cat();
        for p in ["Sessions", "Sessions/Song", "Sessions/Song/Audio Files"] {
            c.upsert(entry(p, 1, "studio", at(0)));
        }
        c.remove(&RootPath::parse("Sessions").unwrap());
        assert!(c.is_empty());
    }

    #[test]
    // t[verify files.catalogue.bounded]
    fn compaction_bounds_the_log() {
        let mut c = cat();
        for i in 0..50 {
            c.upsert(entry(&format!("f{i}"), 1, "studio", at(i)));
        }
        let keep = c.cursor();
        c.compact(&keep);
        assert!(c.changes_since(&keep).is_empty());
        assert_eq!(c.len(), 50, "compaction drops history, never entries");
    }

    #[test]
    // t[verify files.catalogue.staleness]
    fn staleness_is_visible() {
        let mut c = cat();
        c.upsert(entry("a", 1, "studio", at(0)));
        assert!(!c.is_stale(at(10), Duration::seconds(60)));
        assert!(c.is_stale(at(120), Duration::seconds(60)));

        c.set_location_reachable("studio", false, at(15));
        assert!(
            c.is_stale(at(16), Duration::seconds(60)),
            "unreachable is stale however recently we heard"
        );
    }

    #[test]
    fn unverified_is_adoptions_tail() {
        let mut c = cat();
        c.upsert(entry("a", 1, "studio", at(0)));
        assert_eq!(c.unverified().len(), 1);
    }

    #[test]
    fn size_sums_metadata_not_content() {
        let mut c = cat();
        c.upsert(entry("a", 244_000_000_000, "post", at(0)));
        c.upsert(entry("b", 77_000_000_000, "studio", at(0)));
        assert_eq!(c.logical_bytes(), 321_000_000_000);
    }
}
