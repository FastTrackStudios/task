//! `RootsService` — root lifecycle and adoption.
//!
//! The first lane of the v2 surface to be implemented. It is the entry
//! point: every other lane addresses a `RootId`, so nothing else can be
//! reached until this one works.
//!
//! Adoption is the substantive part. `create_root` returned only once a
//! root existed and had been scanned; `adopt` returns as soon as the root
//! has an identity, and the caller follows
//! [`AdoptionProgress`](files_proto::service::roots::AdoptionProgress)
//! for the rest. That is `files.adopt.catalogue-first`, and it is what
//! makes taking on a 77 GB album feel instant rather than absent.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::Utc;
use files_domain::adopt::Adoption;
use files_proto::error::FilesFault;
use files_proto::id::RootId;
use files_proto::model::FileRootInfo;
use files_proto::service::roots::{AdoptRequest, AdoptionPhase, AdoptionProgress, RootsService};

use crate::backend::FilesBackend;

/// In-flight adoptions, keyed by root.
///
/// Held in memory rather than persisted: an adoption interrupted by a
/// process restart is resumed by re-walking, which is exactly what
/// `files.adopt.resumable` already describes — content already hashed is
/// still hashed, because the addresses live in the store rather than
/// here. What is lost across a restart is the *progress display*, not the
/// work.
#[derive(Debug, Default)]
pub struct Adoptions(Mutex<HashMap<RootId, Adoption>>);

impl Adoptions {
    fn with<T>(&self, id: RootId, f: impl FnOnce(&mut Adoption) -> T) -> Option<T> {
        let mut guard = self.0.lock().expect("adoptions lock poisoned");
        guard.get_mut(&id).map(f)
    }

    pub(crate) fn begin(&self, id: RootId, hash_content: bool) {
        let mut guard = self.0.lock().expect("adoptions lock poisoned");
        guard.insert(id, Adoption::begin(Utc::now(), hash_content));
    }

    fn snapshot(&self, id: RootId) -> Option<Adoption> {
        self.0
            .lock()
            .expect("adoptions lock poisoned")
            .get(&id)
            .cloned()
    }

    /// Whether this adoption still has work to do.
    ///
    /// A missing adoption reads as "stop" rather than "carry on": a root
    /// released while it was being adopted must not go on being walked.
    fn running(&self, id: RootId) -> bool {
        self.snapshot(id).is_some_and(|a| a.is_running())
    }
}

/// Translate the domain's state machine into the wire shape.
fn progress_of(root_id: RootId, a: &Adoption) -> AdoptionProgress {
    AdoptionProgress {
        root_id,
        phase: a.phase(),
        entries_seen: a.entries_seen(),
        entries_hashed: a.entries_hashed(),
        bytes_seen: a.bytes_seen(),
        bytes_hashed: a.bytes_hashed(),
        entries_total: a.entries_total(),
        started_at: a.started_at(),
        updated_at: a.updated_at(),
    }
}

impl FilesBackend {
    fn adoption_or_complete(&self, root_id: RootId) -> Result<AdoptionProgress, FilesFault> {
        // A root with no live adoption was adopted before this process
        // started, or by the legacy path. Reporting `Complete` is the
        // honest answer: its entries are in the store, and the state
        // machine that watched them arrive is simply gone.
        self.adoptions().snapshot(root_id).map_or_else(
            || {
                crate::lane::root_or_fault(self, root_id).map(|_| AdoptionProgress {
                    root_id,
                    phase: AdoptionPhase::Complete,
                    entries_seen: 0,
                    entries_hashed: 0,
                    bytes_seen: 0,
                    bytes_hashed: 0,
                    entries_total: None,
                    started_at: Utc::now(),
                    updated_at: Utc::now(),
                })
            },
            |a| Ok(progress_of(root_id, &a)),
        )
    }

    /// Wait for an adoption to stop running.
    ///
    /// `adopt` returns before the work starts — that is the point of it
    /// — so anything that needs the *result* of adoption rather than the
    /// root's identity has to wait for it. In an application that is a
    /// progress view; here it is one call, because a test that races the
    /// driver fails somewhere unrelated and blames the wrong code.
    ///
    /// Polling rather than a notification: the state machine is a plain
    /// value behind a mutex with no waker attached, and giving it one to
    /// serve this would put a channel in the domain for the benefit of
    /// the caller that cares least about latency.
    ///
    /// Returns the final progress, or what it had reached when the wait
    /// ran out. It never fails on a slow adoption, because "not finished
    /// yet" is a legitimate state and this is not the thing that should
    /// decide otherwise.
    pub async fn settled(&self, root_id: RootId) -> Option<AdoptionProgress> {
        for _ in 0..600 {
            let snapshot = self.adoptions().snapshot(root_id)?;
            if !snapshot.is_running() {
                return Some(progress_of(root_id, &snapshot));
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        self.adoptions()
            .snapshot(root_id)
            .map(|a| progress_of(root_id, &a))
    }

    /// Walk the tree, then read it — the work behind an `adopt` that has
    /// already returned.
    ///
    /// Two passes, because that is what `files.adopt.catalogue-first`
    /// describes rather than an implementation convenience:
    ///
    /// 1. **Enumerate.** Build the catalogue from what the filesystem
    ///    already knows. Every entry is published here, unverified, and
    ///    the tree is browsable from this point on whatever its size.
    /// 2. **Hash.** Read the bytes into the store and pin them, which
    ///    gives every file a content address; write those addresses back
    ///    onto the entries the walk published.
    ///
    /// The second pass is one capture rather than a file at a time, and
    /// the progress counters therefore advance in one step at the end of
    /// it rather than smoothly. That is a real limitation and not a
    /// hidden one: the store's snapshot is whole-tree, and the honest
    /// options were a jumping counter or a second full read of every
    /// byte purely to animate a bar.
    ///
    /// Nothing here blocks the applications that own the tree, and
    /// nothing here is required for the root to be usable — that is the
    /// point of it running behind `adopt` rather than inside it.
    async fn drive_adoption(self, root_id: RootId) {
        let this = self.clone();
        let walked = crate::lane::blocking(move || {
            Ok(crate::lane::tree::enumerate_files(&this, root_id))
        })
        .await
        .and_then(|r| r);
        let Ok(files) = walked else {
            // The tree would not walk. The adoption stays where it is
            // rather than claiming to be complete: `adoption_progress`
            // reporting `Enumerating` forever is a state someone can see
            // and resume, and `Complete` over an empty catalogue is not.
            return;
        };

        for (_path, size) in &files {
            if !self.adoptions().running(root_id) {
                return;
            }
            self.adoptions().with(root_id, |a| a.saw(*size, Utc::now()));
        }
        self.adoptions().with(root_id, |a| a.enumerated(Utc::now()));

        // `enumerated` decides whether there is a hashing phase at all:
        // a survey (`hash_content: false`) and an empty tree both land
        // on `Complete` here, and neither should read a byte.
        if !self.adoptions().running(root_id) {
            return;
        }

        let this = self.clone();
        let captured = crate::lane::blocking(move || {
            this.checkpoint_now_inner(root_id.get(), Some("adopted".to_string()))
        })
        .await;
        if captured.is_err() {
            // Same reasoning as the failed walk: leave it visibly
            // unfinished. `files.adopt.resumable` is what recovers it.
            return;
        }

        let this = self.clone();
        let verified = crate::lane::blocking(move || {
            let addressed = this.head_addresses(root_id.get())?;
            Ok(crate::lane::tree::verify_addresses(&this, root_id, &addressed))
        })
        .await
        .unwrap_or_default();

        for (_path, size) in verified {
            self.adoptions()
                .with(root_id, |a| a.hashed(size, Utc::now()));
        }
    }
}

impl RootsService for FilesBackend {
    // t[impl files.adopt.in-place] — nothing is moved, copied or renamed
    // t[impl files.adopt.catalogue-first] — returns before the walk finishes
    async fn adopt(&self, request: AdoptRequest) -> Result<FileRootInfo, FilesFault> {
        let this = self.clone();
        let AdoptRequest {
            path,
            name,
            flavor,
            hash_content,
        } = request;

        let root =
            crate::lane::blocking(move || this.create_root_in_place(path, name, flavor)).await?;

        // The work happens behind the return, which is the whole of
        // `files.adopt.catalogue-first`: a 77 GB album is a root the
        // moment someone asks for it to be one, and reading it is then
        // something that happens to a root that already exists.
        let root_id = RootId::new(root.id);
        self.adoptions().begin(root_id, hash_content);
        let driver = self.clone();
        tokio::spawn(async move { driver.drive_adoption(root_id).await });

        Ok(root)
    }

    // t[impl files.adopt.resumable]
    async fn resume_adoption(&self, root_id: RootId) -> Result<AdoptionProgress, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        self.adoptions()
            .with(root_id, |a| {
                a.resume(Utc::now());
                progress_of(root_id, a)
            })
            .map_or_else(|| self.adoption_or_complete(root_id), Ok)
    }

    async fn pause_adoption(&self, root_id: RootId) -> Result<AdoptionProgress, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        self.adoptions()
            .with(root_id, |a| {
                a.pause(Utc::now());
                progress_of(root_id, a)
            })
            .map_or_else(|| self.adoption_or_complete(root_id), Ok)
    }

    async fn adoption_progress(&self, root_id: RootId) -> Result<AdoptionProgress, FilesFault> {
        self.adoption_or_complete(root_id)
    }

    // t[impl files.peering.replication] — structure here, content elsewhere
    async fn host_structure(
        &self,
        root_id: RootId,
        name: String,
        flavor: files_proto::model::RootFlavor,
    ) -> Result<FileRootInfo, FilesFault> {
        // Idempotent, so a peer can re-run reconciliation without
        // asking first. Returning what is already here rather than
        // overwriting also means a host that *does* hold the tree does
        // not lose its placement to a peer's structure push.
        if let Some(known) = self.registry_get(root_id.get()) {
            return Ok(known);
        }
        let root = FileRootInfo {
            id: root_id.get(),
            name,
            // The whole point: known, and nowhere here.
            path: None,
            flavor,
            created_at: chrono::Utc::now(),
            project_version: None,
        };
        self.registry_insert(root.clone())
            .map_err(|e| FilesFault::Io(e.to_string()))?;
        Ok(root)
    }

    async fn list(&self) -> Result<Vec<FileRootInfo>, FilesFault> {
        let this = self.clone();
        crate::lane::blocking(move || Ok(this.with_project_version(this.registry_list()))).await
    }

    async fn get(&self, root_id: RootId) -> Result<FileRootInfo, FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
        let this = self.clone();
        crate::lane::blocking(move || {
            Ok(this
                .with_project_version(vec![root])
                .pop()
                .expect("one root in, one root out"))
        })
        .await
    }

    async fn rename_root(&self, root_id: RootId, name: String) -> Result<FileRootInfo, FilesFault> {
        if name.trim().is_empty() {
            return Err(FilesFault::invalid("a root's name may not be empty"));
        }
        let mut root = crate::lane::root_or_fault(self, root_id)?;
        root.name = name;
        let this = self.clone();
        let updated = root.clone();
        crate::lane::blocking(move || {
            this.registry_insert(updated)?;
            Ok(root)
        })
        .await
    }

    /// Stop tracking the root. Its bytes are untouched.
    async fn release(&self, root_id: RootId) -> Result<(), FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let this = self.clone();
        crate::lane::blocking(move || {
            this.registry_remove(root_id.get())?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // t[verify files.adopt.catalogue-first]
    #[test]
    fn a_fresh_adoption_is_browsable_and_unhashed() {
        let a = Adoption::begin(Utc::now(), true);
        let p = progress_of(RootId::new(uuid::Uuid::nil()), &a);
        assert_eq!(p.phase, AdoptionPhase::Enumerating);
        assert_eq!(p.entries_hashed, 0);
        assert_eq!(
            p.entries_total, None,
            "no denominator until the walk finishes"
        );
    }

    #[test]
    fn progress_carries_the_domain_state_verbatim() {
        let mut a = Adoption::begin(Utc::now(), true);
        a.saw(4_000, Utc::now());
        a.saw(6_000, Utc::now());
        a.enumerated(Utc::now());
        a.hashed(4_000, Utc::now());

        let p = progress_of(RootId::new(uuid::Uuid::nil()), &a);
        assert_eq!(p.entries_seen, 2);
        assert_eq!(p.entries_hashed, 1);
        assert_eq!(p.bytes_seen, 10_000);
        assert_eq!(p.bytes_hashed, 4_000);
        assert_eq!(p.entries_total, Some(2));
        assert_eq!(p.phase, AdoptionPhase::Hashing);
    }

    #[test]
    fn adoptions_track_per_root() {
        let store = Adoptions::default();
        let a = RootId::new(uuid::Uuid::from_bytes([1; 16]));
        let b = RootId::new(uuid::Uuid::from_bytes([2; 16]));
        store.begin(a, true);
        assert!(store.snapshot(a).is_some());
        assert!(
            store.snapshot(b).is_none(),
            "one root's adoption is its own"
        );
    }
}
