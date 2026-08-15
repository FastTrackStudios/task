//! Per-root WebDAV policy — the "a per-root policy can hide a root from
//! WebDAV" half of issue #274.
//!
//! Persisted as JSON beside the Files registry
//! (`<data_dir>/webdav-policy.json`, next to `roots.json`), for the same
//! reason the registry lives there: the WebDAV bridge is a *view* of an
//! org's roots, so its policy belongs with that org's Files data rather
//! than in a server-global config.
//!
//! **Why a file and not an RPC verb.** Hiding a root is an
//! operator/owner-tier decision on a compat surface, and
//! [`files_proto::FilesService`] is a shared wire contract that
//! concurrent tickets are also extending — adding a method here would
//! be a cross-ticket collision for a knob with exactly one production
//! caller. The file is the surface: an operator edits it, and the next
//! request picks it up ([`WebdavPolicy::reload_if_changed`] watches its
//! mtime), no restart. When the Vault-entity integration lands and a
//! root carries real per-root settings, this collapses into that.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use uuid::Uuid;

/// On-disk shape of `webdav-policy.json`. One key today; a struct
/// (rather than a bare array) so later per-root knobs — read-only,
/// per-root principal mapping — are additive rather than a format
/// break.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct PolicyFile {
    /// Roots that must not appear on, or be reachable through, the
    /// WebDAV bridge. Ids not matching any registered root are kept
    /// verbatim (a root may be re-registered later).
    #[serde(default)]
    hidden: BTreeSet<Uuid>,
}

/// What the last successful `stat` of the policy file saw. `Absent` is
/// a real, cacheable state — an org that has never hidden a root has no
/// policy file, and re-reading nothing on every request is pure waste —
/// so it is distinguished from `Present`, whose mtime drives reloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stamp {
    Absent,
    Present(SystemTime),
}

#[derive(Debug)]
struct Cached {
    file: PolicyFile,
    /// What the cache was populated from, or `None` before the first
    /// read (and after a read that failed, so the next call retries).
    stamp: Option<Stamp>,
}

/// Why the policy could not be consulted. Every variant means "keep
/// whatever was in force" — never "nothing is hidden".
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("webdav policy file is present but unreadable")]
    Unreadable,
    #[error("webdav policy file is malformed")]
    Malformed,
    #[error("webdav policy: {0}")]
    Io(#[source] std::io::Error),
}

/// Which of an org's File Roots the WebDAV bridge may expose.
#[derive(Debug)]
pub struct WebdavPolicy {
    path: PathBuf,
    cached: Mutex<Cached>,
}

impl WebdavPolicy {
    /// Open (or default-initialize) the policy for a Files data
    /// directory. A missing file means "nothing hidden" and is not
    /// written out — the empty policy has no on-disk representation to
    /// maintain.
    ///
    /// Infallible: a policy that cannot be read *at boot* must not stop
    /// the server coming up, and it does not have to — the cache is
    /// left unpopulated, so the first request retries and fails closed
    /// there if it still cannot read it.
    #[must_use]
    pub fn open(data_dir: impl Into<PathBuf>) -> Self {
        let path = data_dir.into().join("webdav-policy.json");
        let policy = Self {
            path,
            cached: Mutex::new(Cached {
                file: PolicyFile::default(),
                // `None` forces the first `reload_if_changed` to read.
                stamp: None,
            }),
        };
        let _ = policy.reload_if_changed();
        policy
    }

    /// `stat` the policy file. `Err` means the file is *there* but its
    /// metadata could not be read — never conflated with `Absent`.
    fn stamp(&self) -> Result<Stamp, std::io::Error> {
        match std::fs::metadata(&self.path) {
            Ok(m) => Ok(Stamp::Present(m.modified()?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Stamp::Absent),
            Err(e) => Err(e),
        }
    }

    /// Re-read the policy file when its mtime moved since the last read
    /// — how an operator's edit reaches a running server without a
    /// restart.
    ///
    /// **Fails closed.** Anything that leaves us unable to read the
    /// *current* policy — malformed JSON, EACCES, EIO, EMFILE, a
    /// remounted read-restricted volume — keeps the last policy that
    /// did load, logs, and does **not** cache the failure, so the next
    /// request retries. Only a file that is genuinely *absent* means
    /// "nothing hidden". The earlier code collapsed every `read` error
    /// to the empty policy AND cached that state, so one permissions
    /// blip published a deliberately hidden client root to every org
    /// member, silently, until the mtime next changed (PR #287 review).
    ///
    /// Erroring is reserved for the one case with no safe answer: we
    /// have *never* loaded a policy and cannot load one now, so we
    /// genuinely do not know what is hidden. Falling back to the last
    /// known policy the rest of the time is what keeps a blip from
    /// taking the whole mount down — which would be its own version of
    /// finding 7, a transient fault presented as a catastrophe.
    ///
    /// The file read happens *outside* the lock. The steady state is
    /// "nothing changed", where this costs one `stat` and one short
    /// lock, never a read while holding it.
    fn reload_if_changed(&self) -> Result<(), Error> {
        let stamp = match self.stamp() {
            Ok(stamp) => stamp,
            Err(e) => {
                tracing::warn!(
                    target: "files_webdav::policy",
                    path = %self.path.display(),
                    error = %e,
                    "webdav policy file could not be stat'd — keeping the previous policy",
                );
                return self.keep_previous(Error::Unreadable);
            }
        };
        {
            let cached = self.cached.lock().expect("webdav policy lock poisoned");
            if cached.stamp == Some(stamp) {
                return Ok(());
            }
        }
        // Absent is a state, not a failure: no file, nothing hidden.
        // Caching it is what keeps the common case (an org that never
        // hid anything) down to a single `stat` per request.
        let file = if stamp == Stamp::Absent {
            PolicyFile::default()
        } else {
            match std::fs::read(&self.path) {
                Ok(bytes) => match serde_json::from_slice::<PolicyFile>(&bytes) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        tracing::warn!(
                            target: "files_webdav::policy",
                            path = %self.path.display(),
                            error = %e,
                            "webdav policy file is malformed — keeping the previous policy",
                        );
                        return self.keep_previous(Error::Malformed);
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        target: "files_webdav::policy",
                        path = %self.path.display(),
                        error = %e,
                        "webdav policy file is present but unreadable — keeping the previous policy",
                    );
                    return self.keep_previous(Error::Unreadable);
                }
            }
        };
        let mut cached = self.cached.lock().expect("webdav policy lock poisoned");
        cached.file = file;
        cached.stamp = Some(stamp);
        Ok(())
    }

    /// `Ok` when a previously-loaded policy is still in hand (keep
    /// serving it), `Err(why)` when there has never been one — the only
    /// state in which "what is hidden?" has no safe answer.
    fn keep_previous(&self, why: Error) -> Result<(), Error> {
        let cached = self.cached.lock().expect("webdav policy lock poisoned");
        if cached.stamp.is_some() {
            Ok(())
        } else {
            Err(why)
        }
    }

    fn persist(&self, file: &PolicyFile) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(file)?;
        std::fs::write(&self.path, bytes)
    }

    /// Every hidden root id, as one snapshot. This is the shape callers
    /// want: filtering a root list needs *one* policy read, not one per
    /// root — a mount with twenty roots would otherwise `stat` the
    /// policy file twenty times per request.
    ///
    /// Fallible on purpose. There is no honest empty set to return when
    /// the policy cannot be read: "nothing is hidden" and "we do not
    /// know what is hidden" must not look the same to a caller that is
    /// about to publish a root list over the network.
    pub fn hidden_set(&self) -> Result<BTreeSet<Uuid>, Error> {
        self.reload_if_changed()?;
        Ok(self
            .cached
            .lock()
            .expect("webdav policy lock poisoned")
            .file
            .hidden
            .clone())
    }

    /// May the bridge expose `root_id`? Picks up an operator's edit to
    /// the policy file first.
    pub fn is_visible(&self, root_id: Uuid) -> Result<bool, Error> {
        Ok(!self.hidden_set()?.contains(&root_id))
    }

    /// Hide (or un-hide) a root from the WebDAV bridge. Idempotent.
    ///
    /// Refuses when the current policy cannot be read: writing a fresh
    /// file over one we failed to parse would silently drop every other
    /// root's hidden state.
    pub fn set_hidden(&self, root_id: Uuid, hidden: bool) -> Result<(), Error> {
        self.reload_if_changed()?;
        let mut cached = self.cached.lock().expect("webdav policy lock poisoned");
        let changed = if hidden {
            cached.file.hidden.insert(root_id)
        } else {
            cached.file.hidden.remove(&root_id)
        };
        if !changed {
            return Ok(());
        }
        self.persist(&cached.file).map_err(Error::Io)?;
        cached.stamp = self.stamp().ok();
        Ok(())
    }

    /// Every root id this policy hides, sorted.
    pub fn hidden(&self) -> Result<Vec<Uuid>, Error> {
        Ok(self.hidden_set()?.into_iter().collect())
    }

    /// The policy file this instance reads and writes — the operator's
    /// editing surface.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
