//! `OrganiseService` — hand-organisation and accountability.
//!
//! Two rules live here. `files.organise.manual` is the layer no
//! extractor can infer: the tags somebody chose and the shortlist they
//! keep. `files.organise.activity` is the record of what was done to the
//! root, and the rule's teeth are in its last clause — the feed *derives
//! from* the events that drive live propagation, so an audit trail and a
//! change feed cannot disagree.
//!
//! ## A tag is a view, and the filesystem never hears about it
//!
//! [`set_tags`](OrganiseService::set_tags) touches no path, creates no
//! directory and issues no rename. A tagged file is exactly where it was,
//! byte for byte, and [`tagged`](OrganiseService::tagged) is a query over
//! marks rather than a directory listing. That is the whole of
//! `files.organise.manual`'s second sentence, and
//! `tests/organise_lane.rs` pins it against a real root on disk.
//!
//! ## What is not durable, and is not pretended to be
//!
//! **Tags and favourites have no store anywhere in this codebase.** They
//! are held in [`state`], a process-lifetime `Mutex<Organised>` — the
//! same shape, and the same gap, as [`crate::lane::sync`]'s
//! subscriptions. Restarting the server forgets every tag. Nothing
//! degrades into a *wrong* answer: a forgotten tag reads back as an
//! untagged file, and `tagged` returns fewer rows rather than other
//! people's.
//!
//! **A favourite is per principal, but no caller identity reaches this
//! trait.** No method carries one and `FilesBackend` holds no session, so
//! [`this_principal`] mints one id per process — the same placeholder
//! `crate::lane::version` uses for advisory holds, and for the same
//! reason: an attribution we cannot make is not one we should fake. The
//! consequence is exact — two clients of one server share a favourites
//! list, and every client of *different* servers has its own. The storage
//! is keyed by principal already, so threading a real one through is a
//! change to this function, not to the table.
//!
//! ## Why the activity feed is derived rather than logged
//!
//! The obvious implementation — append a row on every mutation — is the
//! one the rule forbids: a log maintained beside the propagation path
//! drifts from it the first time a code path forgets to write a row, and
//! then the audit trail and the change feed disagree. So nothing here
//! writes an activity row.
//!
//! [`activity`](OrganiseService::activity) reads, at call time, the
//! durable records that `FilesEvent` is published *from*: a root's
//! creation (`RootCreated`), and its cadence journal's snapshots
//! (`Snapshotted`) and checkpoints (`Checkpointed`). One capture writes
//! one journal record and publishes one event in the same call
//! (`backend::capture_inner`), so a row exists in this feed exactly when
//! an event went out — and unlike a subscriber-fed log, the derivation
//! survives a restart, because the journal does.
//!
//! Ideally the lane would attach a sink to the backend's `PubSub` hub and
//! fold the live stream. It cannot: `PubSub::attach` takes a `vox::Tx`,
//! whose sink is bound by the vox runtime when a client subscribes, and
//! there is no in-process sink to bind — the crate has no way to consume
//! its own hub without a server in front of it. Deriving from the records
//! the same code path writes is the closest honest thing, and arguably
//! the stronger one.
//!
//! ### Two gaps the derivation cannot close, stated plainly
//!
//! - **No actor.** Nothing in the recorded stream carries one: the
//!   journal has no actor field, and no method on any of these traits
//!   carries a caller. Rows report [`UNATTRIBUTED`] rather than stamping
//!   this process's principal onto history it did not make.
//! - **No access rows.** `files.organise.activity` wants reads of shared
//!   content recorded too, and no `FilesEvent` variant announces a read —
//!   there is nothing to derive [`Action::Viewed`] or
//!   [`Action::Downloaded`] from. Synthesising them here would be exactly
//!   the second log the rule forbids, so the feed carries structural
//!   change only. Closing this needs a published read event, not more
//!   code in this file.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::OnceLock;
use facet::Facet;

use chrono::{DateTime, Utc};
use files_domain::cadence::Journal;
use files_proto::error::FilesFault;
use files_proto::id::{ActivityId, PrincipalId, RootId};
use files_proto::path::RootPath;
use files_proto::service::organise::{Action, Activity, Marks, OrganiseService, Tag};
use uuid::Uuid;

use crate::backend::FilesBackend;

// ── In-memory state ────────────────────────────────────────────────

/// Hand-organisation for one org: who tagged what, and whose shortlist a
/// path is on.
///
/// Tags are org-wide because a tag is a shared view ("everything for the
/// client edit"); favourites are keyed by principal because a shortlist
/// is one person's.
///
/// Persisted per org — see [`crate::durable`]. It is keyed by the
/// backend's data directory rather than held in a process-wide static,
/// which is what stops one org's `all_tags` answering with another's.
#[derive(Debug, Default, Clone, Facet)]
#[repr(C)]
struct Organised {
    /// Canonical (lowercased) tags per marked path.
    tags: BTreeMap<(RootId, RootPath), BTreeSet<String>>,
    favourites: BTreeSet<(PrincipalId, RootId, RootPath)>,
}

/// The on-disk shape.
///
/// JSON has no representation for a composite map key, and the in-memory
/// maps are keyed by `(root, path)` because that is what every lookup
/// here wants. Rather than degrade the runtime types to fit the file
/// format, the file format is a list of rows and the conversion is here.
#[derive(Default, Facet)]
#[repr(C)]
struct Wire {
    tags: Vec<TagRow>,
    favourites: Vec<FavouriteRow>,
}

#[derive(Facet)]
#[repr(C)]
struct TagRow {
    root: RootId,
    path: RootPath,
    tags: BTreeSet<String>,
}

#[derive(Facet)]
#[repr(C)]
struct FavouriteRow {
    principal: PrincipalId,
    root: RootId,
    path: RootPath,
}

impl From<Wire> for Organised {
    fn from(w: Wire) -> Self {
        Self {
            tags: w
                .tags
                .into_iter()
                .map(|r| ((r.root, r.path), r.tags))
                .collect(),
            favourites: w
                .favourites
                .into_iter()
                .map(|r| (r.principal, r.root, r.path))
                .collect(),
        }
    }
}

impl From<Organised> for Wire {
    fn from(o: Organised) -> Self {
        Self {
            tags: o
                .tags
                .into_iter()
                .map(|((root, path), tags)| TagRow { root, path, tags })
                .collect(),
            favourites: o
                .favourites
                .into_iter()
                .map(|(principal, root, path)| FavouriteRow {
                    principal,
                    root,
                    path,
                })
                .collect(),
        }
    }
}

impl crate::durable::Durable for Organised {
    type Wire = Wire;

    fn to_wire(&self) -> Wire {
        Wire::from(self.clone())
    }

    fn from_wire(wire: Wire) -> Self {
        Self::from(wire)
    }
}

static ORGANISED: crate::durable::Scoped<Organised> = crate::durable::Scoped::new("organise");

/// Mutate and persist.
fn with_state<T>(backend: &FilesBackend, f: impl FnOnce(&mut Organised) -> T) -> T {
    ORGANISED.write(backend, f)
}

/// Read without persisting.
///
/// Separate from [`with_state`] because a read that writes the state file
/// back is both wasted io and a lie in the file's mtime — "when did
/// someone last change a tag" should not answer "when someone last
/// listed them".
fn read_state<T>(backend: &FilesBackend, f: impl FnOnce(&Organised) -> T) -> T {
    ORGANISED.read(backend, f)
}

/// The principal this process speaks for. See the module doc for why it
/// is minted rather than received.
fn this_principal() -> PrincipalId {
    static ME: OnceLock<PrincipalId> = OnceLock::new();
    *ME.get_or_init(PrincipalId::generate)
}

/// The actor of a derived activity row: nobody, recorded as such.
///
/// The alternative is to stamp [`this_principal`] on a checkpoint taken
/// months ago by someone else, which would make the feed *look*
/// attributable while being wrong — the one failure mode an audit trail
/// may not have.
const UNATTRIBUTED: PrincipalId = PrincipalId::new(Uuid::nil());

// ── Tags ───────────────────────────────────────────────────────────

/// A tag's canonical form: trimmed and case-folded.
///
/// Case-insensitive on purpose. `Vocals` and `vocals` differing would
/// split one view in half and neither half would look wrong, which is a
/// worse failure than a label losing its capital — and the capital is a
/// display decision the UI already makes for tag labels.
fn canonical(tag: &Tag) -> Result<String, FilesFault> {
    let trimmed = tag.0.trim();
    if trimmed.is_empty() {
        return Err(FilesFault::invalid("a tag may not be empty"));
    }
    Ok(trimmed.to_lowercase())
}

fn canonical_all(tags: &[Tag]) -> Result<Vec<String>, FilesFault> {
    tags.iter().map(canonical).collect()
}

/// The marks for one path as the wire wants them.
fn marks_of(s: &Organised, root_id: RootId, path: &RootPath, who: PrincipalId) -> Marks {
    let key = (root_id, path.clone());
    Marks {
        root_id,
        path: path.clone(),
        tags: s
            .tags
            .get(&key)
            .into_iter()
            .flatten()
            .map(|t| Tag(t.clone()))
            .collect(),
        favourite: s.favourites.contains(&(who, root_id, path.clone())),
    }
}

// ── The derived activity feed ──────────────────────────────────────

/// A deterministic id for a derived row.
///
/// Derived rows are recomputed on every call, so a random id would give
/// the same event a new identity each time a client refreshed — no client
/// could key a list on it. Hashing the row's own facts keeps it stable
/// for as long as the facts are. It is a within-process identity, not a
/// durable one: nothing persists it, and nothing resolves it back.
fn activity_id(root_id: RootId, path: &RootPath, action: Action, at: DateTime<Utc>) -> ActivityId {
    let mut bytes = [0u8; 16];
    for (half, salt) in [(0usize, 0x5eed_0001_u64), (8, 0x5eed_0002_u64)] {
        let mut hasher = DefaultHasher::new();
        salt.hash(&mut hasher);
        root_id.hash(&mut hasher);
        path.as_str().hash(&mut hasher);
        (action as u8).hash(&mut hasher);
        at.timestamp_nanos_opt()
            .unwrap_or_default()
            .hash(&mut hasher);
        bytes[half..half + 8].copy_from_slice(&hasher.finish().to_le_bytes());
    }
    ActivityId::new(Uuid::from_bytes(bytes))
}

fn row(root_id: RootId, path: RootPath, action: Action, at: DateTime<Utc>) -> Activity {
    Activity {
        id: activity_id(root_id, &path, action, at),
        root_id,
        path,
        action,
        actor: UNATTRIBUTED,
        actor_name: String::new(),
        // Renames are recorded in the commit graph, not in the journal;
        // `from` is left unset rather than guessed at.
        from: None,
        at,
    }
}

/// Rows for a capture's paths. A path the journal cannot parse is
/// skipped: a feed missing one malformed row still reports the rest
/// truthfully, where failing the call reports nothing at all.
fn path_rows(
    root_id: RootId,
    paths: impl IntoIterator<Item = impl AsRef<str>>,
    at: DateTime<Utc>,
) -> Vec<Activity> {
    paths
        .into_iter()
        .filter_map(|p| RootPath::parse(p.as_ref()).ok())
        // `changed_paths` means "written or removed" and the journal does
        // not say which. `Modified` is the claim both cases support;
        // `Deleted` would need a diff this derivation does not do.
        .map(|p| row(root_id, p, Action::Modified, at))
        .collect()
}

// t[impl files.organise.activity] — derived from the records the
// published events are published from, so the two cannot disagree
fn feed_of(root_id: RootId, created_at: DateTime<Utc>, journal: &Journal) -> Vec<Activity> {
    // The root's own creation — the fact `FilesEvent::RootCreated`
    // announced, read back off the registry rather than remembered.
    let mut rows = vec![row(root_id, RootPath::root(), Action::Created, created_at)];

    for snapshot in &journal.snapshots {
        rows.extend(path_rows(root_id, &snapshot.changed_paths, snapshot.at));
    }
    for checkpoint in &journal.checkpoints {
        // A checkpoint is a change to the root as a whole, and its
        // per-file detail is not durable: `CheckpointInfo::changed_paths`
        // is computed during the capture and never written down. The save
        // points that *are* written give back the files a session
        // touched, at the times they were touched.
        rows.push(row(
            root_id,
            RootPath::root(),
            Action::Modified,
            checkpoint.at,
        ));
        for save in &checkpoint.save_points {
            rows.extend(path_rows(root_id, [&save.path], save.at));
        }
    }

    rows.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.path.cmp(&b.path)));
    // A save point and a snapshot can describe the same write; one write
    // is one row.
    rows.dedup_by(|a, b| a.at == b.at && a.path == b.path && a.action == b.action);
    rows
}

// ── The lane ───────────────────────────────────────────────────────

impl OrganiseService for FilesBackend {
    // t[impl files.organise.manual] — reads the marks, touches no path
    async fn marks(&self, root_id: RootId, path: RootPath) -> Result<Marks, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let path = path.validate()?;
        Ok(read_state(self, |s| {
            marks_of(s, root_id, &path, this_principal())
        }))
    }

    // t[impl files.organise.manual] — a tag is a view: no move, no rename,
    // no directory, and the path comes back the one that went in
    async fn set_tags(
        &self,
        root_id: RootId,
        path: RootPath,
        tags: Vec<Tag>,
    ) -> Result<Marks, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let path = path.validate()?;
        let canonical = canonical_all(&tags)?;

        // Deliberately *not* checked against the live tree: a stub is a
        // real file with no bytes resident, and a path can be marked
        // before its content has synced. Existence is the tree's question,
        // and answering it here would put a stat on every tag edit while
        // making an offline device unable to organise what it can see.
        Ok(with_state(self, |s| {
            let key = (root_id, path.clone());
            if canonical.is_empty() {
                s.tags.remove(&key);
            } else {
                s.tags.insert(key, canonical.into_iter().collect());
            }
            marks_of(s, root_id, &path, this_principal())
        }))
    }

    // t[impl files.organise.manual] — per principal, never global
    async fn set_favourite(
        &self,
        root_id: RootId,
        path: RootPath,
        favourite: bool,
    ) -> Result<Marks, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let path = path.validate()?;
        let who = this_principal();
        Ok(with_state(self, |s| {
            let key = (who, root_id, path.clone());
            if favourite {
                s.favourites.insert(key);
            } else {
                s.favourites.remove(&key);
            }
            marks_of(s, root_id, &path, who)
        }))
    }

    // t[impl files.organise.manual] — the view a tag produces
    async fn tagged(
        &self,
        tags: Vec<Tag>,
        root_id: Option<RootId>,
    ) -> Result<Vec<Marks>, FilesFault> {
        if let Some(id) = root_id {
            crate::lane::root_or_fault(self, id)?;
        }
        let wanted = canonical_all(&tags)?;
        let who = this_principal();

        Ok(read_state(self, |s| {
            s.tags
                .iter()
                .filter(|((root, _), _)| root_id.is_none_or(|id| id == *root))
                // Every named tag must be present: two tags narrow a view
                // rather than widening it, which is what a query with more
                // than one term is for. No tags names every marked path.
                .filter(|(_, held)| wanted.iter().all(|t| held.contains(t)))
                .map(|((root, path), _)| marks_of(s, *root, path, who))
                .collect()
        }))
    }

    // t[impl files.organise.manual] — what exists, for completion
    async fn all_tags(&self, root_id: Option<RootId>) -> Result<Vec<Tag>, FilesFault> {
        if let Some(id) = root_id {
            crate::lane::root_or_fault(self, id)?;
        }
        Ok(read_state(self, |s| {
            s.tags
                .iter()
                .filter(|((root, _), _)| root_id.is_none_or(|id| id == *root))
                .flat_map(|(_, held)| held.iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(Tag)
                .collect()
        }))
    }

    // t[impl files.organise.activity] — one feed, scoped to a file, a
    // folder or the root, newest first
    async fn activity(
        &self,
        root_id: RootId,
        under: Option<RootPath>,
        limit: Option<u32>,
    ) -> Result<Vec<Activity>, FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
        let under = under.map(|u| u.validate()).transpose()?;
        let store_dir = self.store_of(&root);
        let created_at = root.created_at;

        let mut rows = crate::lane::blocking(move || {
            // An unreadable journal is an empty journal (its own rule):
            // losing the labels must not cost the caller the root's
            // creation row too.
            Ok(feed_of(
                root_id,
                created_at,
                &Journal::load(&store_dir).unwrap_or_default(),
            ))
        })
        .await?;

        if let Some(under) = under.filter(|u| !u.is_root()) {
            rows.retain(|a| a.path.is_within(&under));
        }
        if let Some(limit) = limit {
            // Newest first already, so truncating keeps the newest —
            // the half a feed shows.
            rows.truncate(limit as usize);
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;
    use files_domain::cadence::journal::{CheckpointRecord, SnapshotRecord};
    use files_proto::model::SavePoint;

    use super::*;

    fn at(offset: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + offset, 0).expect("timestamp")
    }

    fn journal() -> Journal {
        Journal {
            checkpoint_head: None,
            snapshot_head: None,
            snapshots: vec![SnapshotRecord {
                snapshot_id: "aa".into(),
                at: at(10),
                changed_paths: vec!["stems/kick.wav".into(), "mix.wav".into()],
                save_points: Vec::new(),
            }],
            checkpoints: vec![CheckpointRecord {
                commit_id: "bb".into(),
                at: at(20),
                save_points: vec![SavePoint {
                    path: "song.rpp".into(),
                    at: at(15),
                }],
                requeued_paths: Vec::new(),
            }],
        }
    }

    fn root() -> RootId {
        RootId::new(Uuid::from_bytes([7; 16]))
    }

    // t[verify files.organise.activity]
    #[test]
    fn the_feed_is_every_recorded_change_newest_first() {
        let rows = feed_of(root(), at(0), &journal());
        let seen: Vec<_> = rows
            .iter()
            .map(|a| (a.path.as_str(), a.action, a.at))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("", Action::Modified, at(20)),
                ("song.rpp", Action::Modified, at(15)),
                ("mix.wav", Action::Modified, at(10)),
                ("stems/kick.wav", Action::Modified, at(10)),
                ("", Action::Created, at(0)),
            ],
            "every journal record and the root's creation, newest first"
        );
    }

    // t[verify files.organise.activity]
    #[test]
    fn a_rows_identity_is_its_facts() {
        // Recomputed on every call, so a client keying a list on the id
        // must see the same id for the same event.
        let first = feed_of(root(), at(0), &journal());
        let again = feed_of(root(), at(0), &journal());
        assert_eq!(
            first.iter().map(|a| a.id).collect::<Vec<_>>(),
            again.iter().map(|a| a.id).collect::<Vec<_>>()
        );
        // And a different root's identical history is not the same event.
        let elsewhere = feed_of(RootId::new(Uuid::from_bytes([8; 16])), at(0), &journal());
        assert_ne!(first[0].id, elsewhere[0].id);
    }

    /// The gap the module doc names, pinned so it reads as a decision
    /// rather than an oversight: nothing in the recorded stream carries
    /// an actor, and a derived row says so instead of guessing.
    // t[verify files.organise.activity]
    #[test]
    fn a_derived_row_claims_no_actor() {
        for a in feed_of(root(), at(0), &journal()) {
            assert_eq!(a.actor, UNATTRIBUTED);
            assert!(a.actor_name.is_empty());
        }
    }

    /// The other named gap: no `FilesEvent` announces a read, so no row
    /// may claim one.
    // t[verify files.organise.activity]
    #[test]
    fn no_row_claims_an_access_nothing_published() {
        assert!(
            feed_of(root(), at(0), &journal())
                .iter()
                .all(|a| !matches!(a.action, Action::Viewed | Action::Downloaded)),
            "an access row would be invented, not derived"
        );
    }

    // t[verify files.organise.activity]
    #[test]
    fn one_write_is_one_row_however_many_records_mention_it() {
        let moment = at(30);
        let mut j = Journal::default();
        j.snapshots.push(SnapshotRecord {
            snapshot_id: "cc".into(),
            at: moment,
            changed_paths: vec!["song.rpp".into()],
            save_points: Vec::new(),
        });
        j.checkpoints.push(CheckpointRecord {
            commit_id: "dd".into(),
            at: moment + TimeDelta::seconds(1),
            save_points: vec![SavePoint {
                path: "song.rpp".into(),
                at: moment,
            }],
            requeued_paths: Vec::new(),
        });
        assert_eq!(
            feed_of(root(), at(0), &j)
                .iter()
                .filter(|a| a.path.as_str() == "song.rpp")
                .count(),
            1
        );
    }

    // t[verify files.organise.manual]
    #[test]
    fn a_tag_is_case_folded_and_trimmed() {
        assert_eq!(canonical(&Tag("  Vocals ".into())).unwrap(), "vocals");
        assert!(
            canonical(&Tag("   ".into())).is_err(),
            "whitespace is not a label"
        );
    }
}
