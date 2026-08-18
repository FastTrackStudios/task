//! What a device originates — `files.device.ingest`.
//!
//! A phone's camera roll, a recorder's card, a scanner's output
//! directory. The rule:
//!
//! > A device uploads what it originates — camera roll, stills, voice
//! > memos — to a configured destination with no per-item action. Ingest
//! > is idempotent across restarts and re-registration: captured once,
//! > uploaded once.
//!
//! # "No per-item action" is what makes this a feature
//!
//! Uploading a photo is not hard and nobody needs help with it. What the
//! rule asks for is that the *decision* is made once — this folder goes
//! to that inbox — and never again. So the surface is a source you
//! declare and a sweep that runs on a timer, and there is deliberately
//! no verb that means "send this one".
//!
//! # Idempotency is content-addressed, not bookkeeping
//!
//! "Across restarts and re-registration" rules out remembering by
//! filename (a camera reuses `IMG_0001.JPG` every ten thousand shots),
//! by path (a card is remounted somewhere else), by mtime (a copy
//! changes it), and by device id (re-registration mints a new one — that
//! is what re-registration *is*).
//!
//! What survives all four is the content. A file is ingested when its
//! bytes are in the store, and [`FilesBackend::sync_ingest_path`] both
//! puts them there and returns their address — so the ledger below is a
//! set of addresses, and asking "have I sent this?" is asking "is this
//! content here?". A phone re-registered against a new server sends
//! everything again exactly once; re-registered against the same one
//! sends nothing.
//!
//! The ledger is still worth keeping, because the alternative is
//! re-hashing every file in the camera roll on every sweep. It is a
//! cache over a question the store can always answer, which is why a
//! ledger lost to a crash costs a re-hash and never a duplicate.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use files_proto::error::FilesFault;
use files_proto::id::RootId;
use files_proto::path::RootPath;
use uuid::Uuid;

use crate::backend::FilesBackend;
use crate::durable::Scoped;

/// A directory this device originates content in, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq, facet::Facet)]
#[repr(C)]
pub struct IngestSource {
    pub id: Uuid,
    /// Absolute path on this device — a camera roll, a card mount.
    pub source: String,
    /// The root that receives it.
    pub root_id: RootId,
    /// Where inside that root. An inbox, conventionally.
    pub dest: RootPath,
}

/// What one sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq, facet::Facet)]
#[repr(C)]
pub struct IngestReport {
    /// Files whose content was not already held, and now is.
    pub ingested: Vec<String>,
    /// Files already ingested — by content, so this counts a renamed
    /// copy of something already sent.
    pub already: usize,
    /// Files that could not be read. Reported rather than retried
    /// forever: a card pulled mid-sweep is ordinary, and the next sweep
    /// picks them up.
    pub failed: Vec<String>,
}

/// What a file looked like when it was ingested.
///
/// Length and mtime, which is what a stat costs. Not a content hash —
/// see [`Ingested`] on why this exists at all.
#[derive(Debug, Clone, PartialEq, Eq, facet::Facet)]
#[repr(C)]
pub struct Seen {
    /// Absolute path on this device.
    pub at: String,
    pub len: u64,
    /// Modification time, in nanoseconds since the epoch.
    pub mtime: u64,
    /// The address its bytes hashed to.
    pub address: String,
}

/// Declared sources, what has been sent, and what it looked like.
///
/// **Two keys, and they answer different questions.** `seen` is the set
/// of content addresses, and it is the one that decides: a file is
/// already ingested when its bytes are. `stat` is a cache from a path to
/// what was there last time, and it exists so that a sweep of a camera
/// roll does not re-hash ten thousand photos to learn what it learned
/// yesterday.
///
/// The cache can only ever cause a *hash*, never a duplicate: a stat
/// that matches skips, and a stat that misses falls through to hashing,
/// which then consults `seen`. So losing the cache costs time and losing
/// the set costs a duplicate — which is why the set is what gets written
/// down first.
#[derive(Debug, Default, Clone, facet::Facet)]
#[repr(C)]
pub struct Ingested {
    sources: Vec<IngestSource>,
    seen: BTreeSet<String>,
    stat: Vec<Seen>,
}

/// The on-disk shape.
#[derive(Default, facet::Facet)]
#[repr(C)]
pub struct IngestedWire {
    sources: Vec<IngestSource>,
    seen: Vec<String>,
    stat: Vec<Seen>,
}

impl crate::durable::Durable for Ingested {
    type Wire = IngestedWire;

    fn to_wire(&self) -> IngestedWire {
        IngestedWire {
            sources: self.sources.clone(),
            seen: self.seen.iter().cloned().collect(),
            stat: self.stat.clone(),
        }
    }

    fn from_wire(wire: IngestedWire) -> Self {
        Self {
            sources: wire.sources,
            seen: wire.seen.into_iter().collect(),
            stat: wire.stat,
        }
    }
}

/// A file's content address, computed here.
///
/// Deliberately not taken from [`FilesBackend::sync_ingest_path`]'s
/// reply, which would mean handing the store every file on every sweep
/// to find out whether it already has it. Two reasons, and the second
/// is the serious one:
///
/// - it is wasteful — the store re-chunks content it already holds, to
///   tell us a thing our own ledger knows;
/// - and it is a *sync bridge*. `sync_ingest_path` blocks on an async
///   store from a synchronous caller, and repeating it inside one
///   runtime stalls: a sweep that hands the store four already-held
///   files deadlocks on the third. That is a bug in the bridge and it is
///   filed as one — but a sweep on a timer has no business calling into
///   it for files it already sent, whether or not the bridge is fixed.
///
/// So the ledger is keyed by a hash this lane computes, and the store is
/// touched exactly once per genuinely new file.
fn address_of(file: &Path) -> Option<String> {
    let mut hasher = blake3::Hasher::new();
    let mut handle = std::fs::File::open(file).ok()?;
    std::io::copy(&mut handle, &mut hasher).ok()?;
    Some(hasher.finalize().to_hex().to_string())
}

/// What a file looks like right now, for the cheap comparison.
fn stat_of(file: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(file).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((meta.len(), u64::try_from(mtime).unwrap_or(u64::MAX)))
}

static INGEST: Scoped<Ingested> = Scoped::new("ingest");

/// Every file directly in `dir`, sorted, ignoring what an OS leaves.
///
/// Shallow on purpose. A camera roll is flat, and a recursive sweep of
/// somebody's home directory because they pointed this at the wrong
/// folder is a mistake worth making impossible rather than recoverable.
fn originated(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                files_domain::ignore::IgnoreSet::new([])
                    .ignored(n)
                    .is_none()
            })
        })
        .collect();
    out.sort();
    out
}

impl FilesBackend {
    // t[impl files.device.ingest] — a source is declared once and swept
    // on a timer, so there is no per-item action anywhere in the surface
    /// Declare a directory this device originates content in.
    ///
    /// Idempotent by `(source, root, dest)`: declaring the same
    /// arrangement twice is one source, because a device that re-runs
    /// its setup should not end up sweeping the same card into the same
    /// inbox twice per pass.
    pub fn watch_source(
        &self,
        source: impl Into<String>,
        root_id: RootId,
        dest: RootPath,
    ) -> IngestSource {
        let source = source.into();
        INGEST.write(self, |state| {
            if let Some(existing) = state
                .sources
                .iter()
                .find(|s| s.source == source && s.root_id == root_id && s.dest == dest)
            {
                return existing.clone();
            }
            let declared = IngestSource {
                id: Uuid::new_v4(),
                source: source.clone(),
                root_id,
                dest: dest.clone(),
            };
            state.sources.push(declared.clone());
            declared
        })
    }

    /// Stop sweeping a source. The content already sent stays sent.
    pub fn forget_source(&self, id: Uuid) {
        INGEST.write(self, |state| {
            state.sources.retain(|s| s.id != id);
        });
    }

    /// Every source this device sweeps.
    #[must_use]
    pub fn ingest_sources(&self) -> Vec<IngestSource> {
        INGEST.read(self, |state| state.sources.clone())
    }

    // t[impl files.device.ingest] — "captured once, uploaded once":
    // idempotent on the content address, which is the one key that
    // survives a rename, a remount, a restart and a re-registration
    /// Sweep one source: send what is new, skip what is already held.
    ///
    /// Safe to call on a timer and safe to call twice — the second call
    /// ingests nothing, which is the property the rule is about.
    ///
    /// # Errors
    ///
    /// `NotFound` when the source was never declared. A source
    /// directory that is not there is *not* an error: a card that is
    /// unplugged is the ordinary state of a card.
    pub fn ingest_now(&self, id: Uuid) -> Result<IngestReport, FilesFault> {
        let source = INGEST
            .read(self, |state| {
                state.sources.iter().find(|s| s.id == id).cloned()
            })
            .ok_or_else(|| FilesFault::invalid(format!("no ingest source {id}")))?;

        let mut report = IngestReport::default();
        for file in originated(Path::new(&source.source)) {
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            let at = file.to_string_lossy().into_owned();

            // The cheap question first: is this the same file, at the
            // same path, that was ingested last time? A stat, against a
            // remembered stat. Ten thousand photos on a card are ten
            // thousand stats rather than ten thousand hashes, and that
            // is the difference between a sweep on a timer and a sweep
            // nobody leaves running.
            let now = stat_of(&file);
            let unchanged = INGEST.read(self, |state| {
                now.is_some_and(|(len, mtime)| {
                    state
                        .stat
                        .iter()
                        .any(|s| s.at == at && s.len == len && s.mtime == mtime)
                })
            });
            if unchanged {
                report.already += 1;
                continue;
            }

            // The expensive question, and the one that decides: what is
            // in this file? Hashed here rather than by handing it to the
            // store — see `address_of`.
            let Some(address) = address_of(&file) else {
                report.failed.push(name);
                continue;
            };

            let fresh = INGEST.write(self, |state| {
                let fresh = state.seen.insert(address.clone());
                if let Some((len, mtime)) = now {
                    state.stat.retain(|s| s.at != at);
                    state.stat.push(Seen {
                        at: at.clone(),
                        len,
                        mtime,
                        address: address.clone(),
                    });
                }
                fresh
            });
            if !fresh {
                report.already += 1;
                continue;
            }

            // Genuinely new, so the store gets it — once. Chunking here
            // is what makes it dedup against everything the root already
            // holds: an identical chunk costs nothing to add.
            if let Err(e) = self.sync_ingest_path(source.root_id.get(), &file) {
                INGEST.write(self, |state| state.seen.remove(&address));
                tracing::warn!(%name, error = %e, "ingest: storing the content failed");
                report.failed.push(name);
                continue;
            }

            // And it lands where a person will look for it. Written into
            // the live tree, because that is what a root *is* — the
            // store holds the bytes and the tree is how anyone sees them.
            if let Err(e) = self.place_ingested(&source, &file, &name) {
                // Put the address back: the content is in the store but
                // nobody can see it, so the next sweep should try again.
                INGEST.write(self, |state| state.seen.remove(&address));
                tracing::warn!(%name, error = %e, "ingest: placing the file failed");
                report.failed.push(name);
                continue;
            }
            report.ingested.push(name);
        }
        Ok(report)
    }

    /// Copy an originated file into the root's live tree.
    fn place_ingested(
        &self,
        source: &IngestSource,
        file: &Path,
        name: &str,
    ) -> Result<(), FilesFault> {
        let root = crate::lane::root_or_fault(self, source.root_id)?;
        let tree = root
            .local_tree()
            .ok_or(FilesFault::Unavailable {
                path: source.dest.clone(),
            })?
            .to_path_buf();
        let into = tree.join(source.dest.as_str());
        std::fs::create_dir_all(&into).map_err(|e| FilesFault::Io(e.to_string()))?;
        // A name already taken means two cards holding different photos
        // called `IMG_0001.JPG`. Both are wanted, so the second is
        // placed beside the first rather than over it.
        let mut at = into.join(name);
        let mut n = 1;
        while at.exists() {
            let stem = Path::new(name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(name);
            let ext = Path::new(name)
                .extension()
                .and_then(|s| s.to_str())
                .map(|e| format!(".{e}"))
                .unwrap_or_default();
            at = into.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        std::fs::copy(file, &at).map_err(|e| FilesFault::Io(e.to_string()))?;
        Ok(())
    }
}
