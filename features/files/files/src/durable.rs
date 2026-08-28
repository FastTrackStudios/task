//! Per-org lane state that survives a restart.
//!
//! The lanes each began with a module-level `OnceLock<Mutex<State>>`,
//! which has two faults and they are not equally bad.
//!
//! **The serious one is that a process-wide static is not per-org.**
//! `FilesBackend` is constructed once per org — `FilesBackend::new(
//! org_root.path().join("files"), …)` — so a static shared by every
//! backend in the process is shared by every org on the server. Anything
//! that enumerates without a root to scope it (`all_tags`, `devices`,
//! `pending`) was answering with other orgs' rows. [`Scoped`] keys state
//! by the backend's own data directory, which is the org boundary, so
//! that stops being possible rather than stopping by convention.
//!
//! **The second is that none of it was durable.** `files.device.control`
//! says pinning survives restart; it did not. Tags, favourites, grants
//! and subscriptions are all things a human chose, and
//! `storage.tier.authored` puts those in the vault as markdown — which is
//! where they should eventually live. This is not that. It is an
//! atomically-written JSON file per concern in the org's own Files data
//! directory, exactly as [`crate::registry::Registry`] already keeps
//! `roots.json`, and it is honest about being a step rather than the
//! destination.
//!
//! What deliberately does **not** live here:
//!
//! - **Advisory holds.** A signal that survived a restart would be the
//!   stranding lock `files.concurrency.advisory-lock` exists to refuse:
//!   the holder's process is gone, so the file is held by nobody and
//!   nothing can release it. Losing them on restart is the correct
//!   behaviour, not a gap.
//! - **The catalogue.** Derived, rebuildable by walking the tree, and
//!   `storage.tier.derived` says derived state is disposable. Losing the
//!   change log costs a client one resync — the safe direction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use facet::Facet;

use crate::backend::FilesBackend;

// t[impl storage.tier.observed] — machine-observed facts (device
// registrations, sync and hydration state, subscriptions) in a store
// rather than in markdown. The module docs are candid that the ones a
// *human* chose — tags, favourites, grants — are here too and belong in
// the vault; that is a step, not the destination
/// State partitioned by org, loaded on first touch and written on change.
///
/// Declared as a `static` in the lane that owns it:
///
/// ```ignore
/// static TAGS: Scoped<Organised> = Scoped::new("organise");
/// ```
pub(crate) struct Scoped<T> {
    /// The file stem under the org's Files data directory.
    name: &'static str,
    cells: OnceLock<Mutex<HashMap<PathBuf, T>>>,
}

/// The shape a lane's state takes on disk.
///
/// Usually the state itself, and [`durable_as_itself!`] says so in one
/// line. It is a *separate* type when the runtime shape has no JSON: the
/// sync lane keys subscriptions by `(device, root)` because that is the
/// question every lookup asks, and JSON has no composite map key.
/// Degrading the runtime type to fit the file format would put the cost
/// in every lookup rather than in the one place that writes the file.
///
/// serde expressed this as `#[serde(from = "Wire", into = "Wire")]`.
/// facet has no container conversion, so the seam is explicit — which is
/// arguably where it belonged: the fact that a type's stored shape
/// differs from its runtime shape is worth being able to find.
pub(crate) trait Durable: Sized {
    type Wire: Default + Facet<'static>;
    fn to_wire(&self) -> Self::Wire;
    fn from_wire(wire: Self::Wire) -> Self;
}

/// For state whose runtime shape *is* its stored shape.
macro_rules! durable_as_itself {
    ($($t:ty),* $(,)?) => {
        $(impl crate::durable::Durable for $t {
            type Wire = Self;
            fn to_wire(&self) -> Self { self.clone() }
            fn from_wire(wire: Self) -> Self { wire }
        })*
    };
}
pub(crate) use durable_as_itself;

impl<T> Scoped<T>
where
    T: Default + Durable + Send + 'static,
{
    pub(crate) const fn new(name: &'static str) -> Self {
        Self {
            name,
            cells: OnceLock::new(),
        }
    }

    fn cells(&self) -> MutexGuard<'_, HashMap<PathBuf, T>> {
        self.cells
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn file(&self, data_dir: &Path) -> PathBuf {
        data_dir.join(format!("{}.json", self.name))
    }

    /// Read this org's state.
    ///
    /// Loads from disk the first time an org is touched in this process.
    /// A file that will not parse is treated as absent and logged rather
    /// than failing the call: losing a tag is bad, and refusing to serve
    /// a root because its tag file is corrupt is worse.
    pub(crate) fn read<R>(&self, backend: &FilesBackend, f: impl FnOnce(&T) -> R) -> R {
        let dir = backend.data_dir().to_path_buf();
        let mut cells = self.cells();
        let state = cells
            .entry(dir.clone())
            .or_insert_with(|| self.load(&self.file(&dir)));
        f(state)
    }

    /// Mutate this org's state and persist the result.
    ///
    /// Persisting inside the lock is deliberate: two writers must not be
    /// able to interleave such that the later write is the one that
    /// lands on disk while the earlier one is the one in memory.
    pub(crate) fn write<R>(&self, backend: &FilesBackend, f: impl FnOnce(&mut T) -> R) -> R {
        let dir = backend.data_dir().to_path_buf();
        let path = self.file(&dir);
        let mut cells = self.cells();
        let state = cells.entry(dir).or_insert_with(|| self.load(&path));
        let out = f(state);
        save(&path, &state.to_wire());
        out
    }

    /// Drop this org's cached copy, so the next read comes from disk.
    ///
    /// Only a test wants this. A "survives a restart" test that keeps the
    /// same process holds the same cache, so without it the test reads
    /// back what it wrote to memory and proves nothing about the file —
    /// which is exactly what the first version of those tests did.
    #[cfg(test)]
    pub(crate) fn forget(&self, backend: &FilesBackend) {
        self.cells().remove(backend.data_dir());
    }

    fn load(&self, path: &Path) -> T {
        let Ok(bytes) = std::fs::read(path) else {
            return T::default();
        };
        facet_json::from_slice::<T::Wire>(&bytes)
            .map(T::from_wire)
            .unwrap_or_else(|err| {
                tracing::warn!(
                    path = %path.display(),
                    %err,
                    "files: lane state unreadable, continuing without it"
                );
                T::default()
            })
    }
}

/// Write the file atomically.
///
/// A bare `write` interrupted by a crash or a full disk leaves a
/// truncated file, and a truncated JSON object is every row in it — the
/// same reason `Registry::persist` renames into place rather than
/// writing over.
fn save<W: Facet<'static>>(path: &Path, state: &W) {
    let bytes = match facet_json::to_string(state) {
        Ok(text) => text.into_bytes(),
        Err(err) => {
            // Loud in development, survivable in production.
            //
            // Reads are served from the in-memory copy, so a state type
            // that will not serialise would keep working for the life of
            // the process and lose everything on restart — a failure that
            // passes every test and appears only in front of a user.
            debug_assert!(
                false,
                "lane state at {} would not serialise: {err}",
                path.display()
            );
            tracing::error!(path = %path.display(), %err, "files: lane state would not serialise");
            return;
        }
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if let Err(err) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
        // A lane whose state cannot be written still serves this process
        // correctly; it simply forgets on restart, which is where it was
        // before this module existed.
        tracing::error!(path = %path.display(), %err, "files: lane state not persisted");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default, Facet, PartialEq, Debug, Clone)]
    struct Counter {
        n: u32,
    }
    durable_as_itself!(Counter);

    static COUNTER: Scoped<Counter> = Scoped::new("test-counter");

    fn backend(dir: &Path) -> FilesBackend {
        FilesBackend::new(dir, dir.join("vault")).expect("backend")
    }

    #[test]
    fn state_survives_a_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let a = backend(tmp.path());
        COUNTER.write(&a, |c| c.n = 7);

        // A second backend over the same data dir is what a restart looks
        // like from this module's point of view.
        drop(a);
        COUNTER.forget(&backend(tmp.path()));
        let b = backend(tmp.path());
        assert_eq!(COUNTER.read(&b, |c| c.n), 7);
    }

    #[test]
    fn one_orgs_state_is_not_anothers() {
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let a = backend(one.path());
        let b = backend(two.path());

        COUNTER.write(&a, |c| c.n = 1);
        COUNTER.write(&b, |c| c.n = 2);

        assert_eq!(COUNTER.read(&a, |c| c.n), 1);
        assert_eq!(
            COUNTER.read(&b, |c| c.n),
            2,
            "a process-wide static would have made these one org"
        );
    }

    #[test]
    fn a_corrupt_file_reads_as_empty_rather_than_failing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("test-counter.json"), b"{ not json").unwrap();
        let b = backend(tmp.path());
        COUNTER.forget(&b);
        assert_eq!(
            COUNTER.read(&b, |c| c.n),
            0,
            "refusing to serve a root because its tag file is corrupt is worse \
             than losing the tags"
        );
    }
}
