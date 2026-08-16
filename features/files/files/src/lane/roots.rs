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
        self.adoptions()
            .snapshot(root_id)
            .map_or_else(
                || {
                    self.root_or_fault(root_id).map(|_| AdoptionProgress {
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

    fn root_or_fault(&self, root_id: RootId) -> Result<FileRootInfo, FilesFault> {
        self.registry_get(root_id.get())
            .ok_or(FilesFault::RootNotFound(root_id))
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

        let root = crate::lane::blocking(move || this.create_root_in_place(path, name, flavor))
            .await?;

        self.adoptions().begin(RootId::new(root.id), hash_content);
        Ok(root)
    }

    // t[impl files.adopt.resumable]
    async fn resume_adoption(&self, root_id: RootId) -> Result<AdoptionProgress, FilesFault> {
        self.root_or_fault(root_id)?;
        self.adoptions()
            .with(root_id, |a| {
                a.resume(Utc::now());
                progress_of(root_id, a)
            })
            .map_or_else(|| self.adoption_or_complete(root_id), Ok)
    }

    async fn pause_adoption(&self, root_id: RootId) -> Result<AdoptionProgress, FilesFault> {
        self.root_or_fault(root_id)?;
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

    async fn list(&self) -> Result<Vec<FileRootInfo>, FilesFault> {
        let this = self.clone();
        crate::lane::blocking(move || Ok(this.with_project_version(this.registry_list()))).await
    }

    async fn get(&self, root_id: RootId) -> Result<FileRootInfo, FilesFault> {
        let root = self.root_or_fault(root_id)?;
        let this = self.clone();
        crate::lane::blocking(move || {
            Ok(this
                .with_project_version(vec![root])
                .pop()
                .expect("one root in, one root out"))
        })
        .await
    }

    async fn rename(&self, root_id: RootId, name: String) -> Result<FileRootInfo, FilesFault> {
        if name.trim().is_empty() {
            return Err(FilesFault::invalid("a root's name may not be empty"));
        }
        let mut root = self.root_or_fault(root_id)?;
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
        self.root_or_fault(root_id)?;
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
        assert!(store.snapshot(b).is_none(), "one root's adoption is its own");
    }
}
