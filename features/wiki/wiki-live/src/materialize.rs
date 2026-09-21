//! Bringing a subscribed source onto local disk, and keeping it there.
//!
//! This rides the vault sync engine rather than inventing a second one.
//! A wiki *is* a vault — same primitives, same tree — so
//! `vault_sync_client::{index_local, plan_sync, apply_one}` already
//! knows how to compare two markdown trees and move the difference,
//! over vox to another server just as readily as in process.
//!
//! # Why the plan is filtered rather than applied whole
//!
//! The engine's own policy resolves a conflict by mtime — newer side
//! wins. That is right for *your* vault on two of your machines, and
//! wrong for a subscribed copy, where the spec is explicit that a
//! conflict is "never resolved by recency" and that local work is
//! never overwritten by an upstream update
//! (`wiki.subscribe.refresh`).
//!
//! # Two sides are not enough
//!
//! `plan_sync` compares local against remote, so two differing SHAs
//! are a `Conflict` — it cannot tell "upstream moved and I did not"
//! from "we both moved". For a peer sync that is fine, because the
//! mtime tiebreak resolves either. For a subscription it is not: the
//! first case is ordinary news that must arrive, and the second must
//! never be decided by clock.
//!
//! So a copy records the **base**: the sha of every file as of the
//! last successful refresh. Three sides make the distinction exact.
//!
//! | local vs base | remote vs base | meaning | what happens |
//! |---|---|---|---|
//! | same | changed | upstream's news | pulled |
//! | changed | same | the subscriber's work | kept, never sent |
//! | changed | changed | genuine conflict | kept, reported, never resolved |
//!
//! The base lives outside the copy (`subscribed/.state/`), so the
//! mounted folder stays markdown a person can open
//! (`wiki.local.mount`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vault_sync_client::{SyncOp, index_local, plan_sync};

use crate::source::SourceVault;

/// Path → sha as of the last successful refresh.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Base(BTreeMap<String, String>);

#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    #[error("read upstream manifest for `{id}`: {source}")]
    Manifest {
        id: String,
        #[source]
        source: vault_proto::VaultSyncError,
    },
    #[error("sync `{id}`: {source}")]
    Sync {
        id: String,
        #[source]
        source: vault_sync_client::SyncError,
    },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("base state for `{id}`: {source}")]
    Base {
        id: String,
        #[source]
        source: serde_json::Error,
    },
}

/// What a refresh did, and what it deliberately did not do.
///
/// The divergence counts are the point: a subscriber who has edited
/// their copy needs to be told that, not have it quietly reconciled.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Refreshed {
    /// Files brought down from upstream.
    pub pulled: usize,
    /// Files already identical.
    pub in_sync: usize,
    /// Pages the subscriber has that upstream does not. Held, never
    /// pushed.
    pub local_only: Vec<String>,
    /// Pages both sides changed. Held, never resolved.
    pub conflicted: Vec<String>,
    /// Files upstream holds that this subscription's
    /// [`org_proto::Selection`] does not take (ADR 0004 decision 1a).
    ///
    /// Counted rather than ignored, because "the shelf has forty
    /// gigabytes of stems and you asked for the charts" and "the shelf
    /// is empty" look identical from the copy on disk. One is working
    /// as asked and the other is broken, and a person has to be able to
    /// tell which. Zero for a whole-shelf subscription, which is every
    /// subscription that existed before selections did.
    pub skipped: usize,
}

impl Refreshed {
    /// Whether the copy carries local work that upstream has not seen.
    ///
    /// What `wiki.subscribe.local-copy` needs before unsubscribing:
    /// dropping a copy with unpushed changes has to say so and take an
    /// answer, rather than discarding work to tidy up.
    #[must_use]
    pub fn has_local_work(&self) -> bool {
        !self.local_only.is_empty() || !self.conflicted.is_empty()
    }
}

/// Where an org keeps the sources it subscribes to.
///
/// One directory per source, addressed the way a reference addresses
/// it — `<org>/subscribed/<domain>/<slug>/` — so the path a file sits
/// at and the reference that names it cannot drift apart.
#[must_use]
pub fn local_copy_dir(org_root: &Path, domain: &str, slug: &str) -> PathBuf {
    org_root.join("subscribed").join(domain).join(slug)
}

/// Where the base snapshot for one copy lives.
///
/// Outside the copy itself: a subscribed wiki appears in the file sync
/// clients as plain markdown, and a state file sitting in it would be
/// one more thing a person has to know to ignore.
#[must_use]
pub fn base_path(org_root: &Path, domain: &str, slug: &str) -> PathBuf {
    org_root
        .join("subscribed")
        .join(".state")
        .join(domain)
        .join(format!("{slug}.json"))
}

/// Bring a subscribed source's local copy up to date with `upstream`.
///
/// `upstream_id` is the source's id on the far side — its wiki slug.
/// The same call serves a publisher on this disk and one on another
/// server, since both implement [`SourceVault`] — the local backend for
/// free, via its `VaultSync` impl, and the remote one by bridging two
/// read calls onto the wire.
///
/// t[impl wiki.subscribe.local-copy] — after this returns, the copy
/// resolves with the network down, because everything upstream had is
/// on disk.
/// t[impl wiki.subscribe.refresh] — local work is replayed rather than
/// overwritten: it is simply never touched, and a conflict is reported
/// for a person instead of being decided by clock.
/// t[impl wiki.subscribe.local-authority] — nothing here writes
/// upstream. There is no code path from a refresh to a `put_file`.
///
/// # Errors
///
/// A failure reading the upstream manifest, fetching a file, or
/// writing the local copy.
pub fn refresh<U: SourceVault + ?Sized>(
    upstream: &U,
    upstream_id: &str,
    local_root: &Path,
    base_at: &Path,
) -> Result<Refreshed, MaterializeError> {
    refresh_taking(
        upstream,
        upstream_id,
        local_root,
        base_at,
        &Take::everything(),
    )
}

/// Which of a source's files a refresh will take.
///
/// A wiki takes all of them: every page is content and a page is a few
/// kilobytes. A shelf is where both halves of this matter — a
/// subscription may name part of one (ADR 0004 decision 1a), and a shelf
/// is "any file, at any size", which over a wire is a different
/// proposition from a shelf on this disk.
#[derive(Debug, Clone, Copy)]
pub struct Take<'a> {
    /// What the subscription asked for (`files.sync.selective`, at the
    /// organisation scope).
    pub selection: &'a org_proto::Selection,
    /// Who wins when both sides hold a file and they differ.
    pub divergence: Divergence,
    /// The largest single file this refresh will fetch.
    ///
    /// A bound rather than a preference, and the reason is ADR 0003's
    /// rule: **subscribing moves names, not gigabytes.** Every fetch
    /// here is one whole file in one message, so a source that holds a
    /// forty-gigabyte camera original would not be slow, it would be a
    /// server trying to put forty gigabytes in a frame. Bytes at that
    /// size cross as a File Root — published once, offered, accepted,
    /// pulled in chunks by the lane that owns resumption and renditions
    /// — and a file over this bound is reported rather than attempted so
    /// a person can see which route it needs.
    pub max_bytes: u64,
}

impl Take<'_> {
    /// The whole source, at any size, keeping whatever the subscriber
    /// wrote — a local refresh, and every wiki.
    #[must_use]
    pub const fn everything() -> Self {
        Self {
            selection: &org_proto::Selection::All,
            max_bytes: u64::MAX,
            divergence: Divergence::Keep,
        }
    }
}

/// What a refresh does about a file both sides hold and disagree about.
///
/// The one place the kinds of source genuinely differ, so it is said in
/// the type rather than by which function a caller happened to reach for.
/// It used to be the latter — a second copier existed largely to express
/// [`Self::Upstream`] — and a policy expressed as a choice of function is
/// a policy that drifts the moment somebody adds a third caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divergence {
    /// The subscriber's version stays, and the difference is reported —
    /// `local_only` when only they moved, `conflicted` when both did.
    ///
    /// For anything a subscriber may write in: a wiki's pages, a shelf's
    /// documents. `wiki.subscribe.refresh` is explicit that local work is
    /// never overwritten and a conflict is never decided by clock.
    Keep,
    /// Upstream wins and the local file is replaced.
    ///
    /// For a Resource only, and it follows from that rule rather than
    /// from convenience: nothing is ever written into one
    /// (`wiki.resource.no-annotations`), so a file that differs is not
    /// somebody's work — it is a stale or damaged copy, and keeping it
    /// would be keeping corruption and calling it a conflict.
    ///
    /// A file only the subscriber holds is still kept and reported
    /// either way: an edition installed here that the publisher does not
    /// carry is not a difference of opinion about one file.
    Upstream,
}

/// The largest single file a refresh **over the wire** will fetch.
///
/// Eight mebibytes: comfortably more than any document a shelf holds —
/// a chart, a song, a patch, a lighting cue are kilobytes — and far
/// less than a take, a stem or a camera original. So the bound falls
/// exactly where ADR 0003 puts the boundary between what a subscription
/// carries and what a File Root carries, rather than at a number chosen
/// for the transport.
pub const REMOTE_FILE_LIMIT: u64 = 8 << 20;

/// [`refresh`], taking only what `take` admits.
///
/// # Errors
///
/// As [`refresh`].
pub fn refresh_taking<U: SourceVault + ?Sized>(
    upstream: &U,
    upstream_id: &str,
    local_root: &Path,
    base_at: &Path,
    take: &Take<'_>,
) -> Result<Refreshed, MaterializeError> {
    std::fs::create_dir_all(local_root).map_err(|source| MaterializeError::Io {
        path: local_root.display().to_string(),
        source,
    })?;
    let base = load_base(base_at, upstream_id)?;

    let mut manifest =
        upstream
            .manifest(upstream_id)
            .map_err(|source| MaterializeError::Manifest {
                id: upstream_id.to_owned(),
                source,
            })?;
    // The source's own bookkeeping — its declaration, its Edit
    // Requests, its queues under `_state/` — is not content and is
    // never the subscriber's: a copy that carried the publisher's
    // Editor list or open requests would present someone else's state
    // as its own. Pages only.
    manifest.files.retain(|f| !is_source_bookkeeping(&f.path));
    // Then what this subscription asked for, and what it can carry.
    // Both are counted into `skipped` and neither is fetched: the
    // decision is made against the manifest, before a byte crosses,
    // which is the only place a size bound can be honoured.
    let mut skipped = 0usize;
    manifest.files.retain(|f| {
        let admitted =
            take.selection.admits(org_proto::facet_of(&f.path)) && f.size <= take.max_bytes;
        if !admitted {
            skipped += 1;
        }
        admitted
    });
    let local = index_local(local_root).map_err(|source| MaterializeError::Sync {
        id: upstream_id.to_owned(),
        source,
    })?;

    let mut out = Refreshed::default();
    let mut next = Base::default();
    for entry in &manifest.files {
        next.0.insert(entry.path.clone(), entry.sha256.clone());
    }

    for op in plan_sync(&local, &manifest) {
        match &op {
            SyncOp::Pull { path, remote_sha } => {
                pull(upstream, upstream_id, local_root, path)?;
                out.pulled += 1;
                next.0.insert(path.clone(), remote_sha.clone());
            }
            SyncOp::InSync { .. } => out.in_sync += 1,
            // A file we have and upstream does not. Either the
            // subscriber wrote it, or upstream deleted it — and
            // without a base those are the same picture. Kept either
            // way: a subscription never deletes the subscriber's
            // files, and `apply_one` would have pushed it upstream,
            // which is exactly what a subscription withholds.
            SyncOp::Push { path, .. } => {
                out.local_only.push(path.clone());
                // Keep whatever the base said, so a later upstream
                // return of this path is still judged against it.
                if let Some(sha) = base.0.get(path) {
                    next.0.insert(path.clone(), sha.clone());
                }
            }
            SyncOp::Conflict {
                path,
                local_sha,
                remote_sha,
                ..
            } => {
                // A source nobody may write into has no conflicts to
                // have: a file that differs is a stale or damaged copy,
                // and upstream is the only authority there is. Decided
                // before the base is consulted, because the base could
                // only say *when* the copy went wrong, and the answer
                // would be the same either way.
                if take.divergence == Divergence::Upstream {
                    pull(upstream, upstream_id, local_root, path)?;
                    out.pulled += 1;
                    next.0.insert(path.clone(), remote_sha.clone());
                    continue;
                }
                match base.0.get(path.as_str()) {
                    // Local is untouched since the last refresh, so
                    // the difference is upstream's news and arrives.
                    Some(based) if based == local_sha => {
                        pull(upstream, upstream_id, local_root, path)?;
                        out.pulled += 1;
                        next.0.insert(path.clone(), remote_sha.clone());
                    }
                    // Upstream is untouched: ours is a local edit,
                    // held and never sent.
                    Some(based) if based == remote_sha => {
                        out.local_only.push(path.clone());
                        next.0.insert(path.clone(), based.clone());
                    }
                    // Both moved, or we have no base to judge by.
                    // Never decided here.
                    _ => {
                        out.conflicted.push(path.clone());
                        if let Some(based) = base.0.get(path.as_str()) {
                            next.0.insert(path.clone(), based.clone());
                        }
                    }
                }
            }
        }
    }
    out.local_only.sort();
    out.conflicted.sort();
    out.skipped = skipped;
    save_base(base_at, &next, upstream_id)?;
    Ok(out)
}

/// Bring one file down from upstream into the local copy.
///
/// This used to be `vault_sync_client::apply_one`, which dispatches on
/// a whole [`SyncOp`] and can push as well as pull. Both call sites here
/// only ever passed it a `Pull`, and a refresh must never do anything
/// else — so it takes the path directly and the surface it needs is two
/// read methods ([`SourceVault`]) rather than the whole of `VaultSync`.
///
/// The `..` check that mattered stays where it was: `write_file` is
/// public for exactly this caller, so a manifest written by another
/// organisation is screened by the same code the push path uses.
fn pull<U: SourceVault + ?Sized>(
    upstream: &U,
    id: &str,
    root: &Path,
    path: &str,
) -> Result<(), MaterializeError> {
    let bytes = upstream
        .get_file(id, path)
        .map_err(|source| MaterializeError::Sync {
            id: id.to_owned(),
            source: vault_sync_client::SyncError::Remote(source),
        })?;
    vault_sync_client::write_file(root, path, &bytes.0).map_err(|source| MaterializeError::Sync {
        id: id.to_owned(),
        source,
    })
}

fn load_base(path: &Path, id: &str) -> Result<Base, MaterializeError> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.is_empty() => Ok(Base::default()),
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| MaterializeError::Base {
            id: id.to_owned(),
            source,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Base::default()),
        Err(source) => Err(MaterializeError::Io {
            path: path.display().to_string(),
            source,
        }),
    }
}

fn save_base(path: &Path, base: &Base, id: &str) -> Result<(), MaterializeError> {
    let io = |source| MaterializeError::Io {
        path: path.display().to_string(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let body = serde_json::to_vec_pretty(base).map_err(|source| MaterializeError::Base {
        id: id.to_owned(),
        source,
    })?;
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, &body).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

/// Whether a path in a source is the source's private bookkeeping
/// rather than a page: anything under `_state/`, at the root or nested.
fn is_source_bookkeeping(path: &str) -> bool {
    let state = wiki_proto::paths::STATE_DIR;
    path == state || path.starts_with(&format!("{state}/")) || path.contains(&format!("/{state}/"))
}

/// Refresh one held subscription into its place under `org_root`.
///
/// Wraps [`refresh`] with the two paths a subscription implies, so a
/// caller never has to know where a copy or its base lives — the two
/// must agree, and a caller that computed one and forgot the other
/// would resync from scratch every time.
///
/// # Errors
///
/// As [`refresh`].
pub fn refresh_subscription<U: SourceVault + ?Sized>(
    upstream: &U,
    org_root: &Path,
    subscription: &wiki_proto::Subscription,
) -> Result<Refreshed, MaterializeError> {
    let copy = local_copy_dir(org_root, &subscription.domain, &subscription.slug);
    let base = base_path(org_root, &subscription.domain, &subscription.slug);
    refresh(upstream, &subscription.slug, &copy, &base)
}

/// Refresh a subscribed shelf whose publisher is **on this disk**.
///
/// [`refresh_shelf`] over the publisher's directory, with no bound: a
/// copy between two directories has no message to fit in. Here rather
/// than at each caller because opening the engine over a path is the
/// answer to "which door does a local shelf use", and a caller that
/// answered it for itself is a caller that could answer it differently —
/// which is how the two routes came to disagree in the first place.
///
/// # Errors
///
/// As [`refresh`], plus a failure opening the publisher's directory.
pub fn refresh_local_shelf(
    upstream_root: &Path,
    org_root: &Path,
    subscription: &wiki_proto::Subscription,
) -> Result<Refreshed, MaterializeError> {
    let upstream = vault_live::Backend::single(&subscription.slug, upstream_root.to_path_buf())
        .map_err(|source| MaterializeError::Io {
            path: upstream_root.display().to_string(),
            source: std::io::Error::other(source),
        })?;
    refresh_shelf(&upstream, org_root, subscription, u64::MAX)
}

/// Refresh a subscribed **asset shelf** — the one route, wherever the
/// publisher is.
///
/// The same engine a wiki uses, which is the finding this rests on: a
/// vault manifest is content-agnostic — `vault_live`'s walk hashes every
/// file it meets, markdown or not — so a shelf's documents cross without
/// the engine knowing they are documents.
///
/// `max_bytes` is the whole of the difference between a publisher on this
/// disk and one on another server. Locally it is `u64::MAX`: a copy
/// between two directories has no message to fit in. Over the wire it is
/// [`REMOTE_FILE_LIMIT`], because every fetch is one whole file in one
/// message and ADR 0003's rule is that subscribing moves names and not
/// gigabytes. What the bound leaves behind is counted into
/// [`Refreshed::skipped`], never attempted.
///
/// # Why one route rather than two
///
/// There were two, and they disagreed about the thing a subscriber cares
/// most about: the byte walker overwrote a file the subscriber had
/// edited, and this one keeps it and reports it. A subscriber cannot see
/// which route they are on — the publisher being on the same disk is not
/// a fact about their copy — so the two answers were one behaviour that
/// varied by accident. Now the promise is the same for every shelf:
/// `wiki.subscribe.refresh`, local work kept and named.
///
/// t[impl wiki.subscribe.local-copy] — after this returns the shelf
/// resolves with the network down.
/// t[impl wiki.subscribe.federated] — for an asset shelf: same
/// subscription, same copy directory, same base snapshot, same report.
///
/// # Errors
///
/// As [`refresh`].
pub fn refresh_shelf<U: SourceVault + ?Sized>(
    upstream: &U,
    org_root: &Path,
    subscription: &wiki_proto::Subscription,
    max_bytes: u64,
) -> Result<Refreshed, MaterializeError> {
    let copy = assets_copy_dir(org_root, &subscription.domain, &subscription.slug);
    let base = base_path(org_root, &subscription.domain, &subscription.slug);
    refresh_taking(
        upstream,
        &subscription.slug,
        &copy,
        &base,
        &Take {
            selection: &subscription.selection,
            max_bytes,
            // A shelf is a thing people type into, so their typing stays.
            divergence: Divergence::Keep,
        },
    )
}

/// Where an org keeps a subscribed **Resource**:
/// `<org>/subscribed/<domain>/<slug>/`, beside every other subscribed
/// source.
///
/// It used to be `<org>/resources/<slug>/` — the org's own corpus
/// library, the same directory `admin bible install` writes into — on the
/// argument that a reader opens a Resource by canonical address and would
/// not find a copy anywhere else. That argument was about scripture and
/// it does not survive the tier's other occupants: `resources/patches/`
/// and `resources/samples/` are app libraries, and merging a publisher's
/// into this org's own would put two organisations' slugs in one
/// directory, where [`Divergence::Upstream`] would let theirs overwrite
/// yours. One copy directory per publishing domain is what keeps a
/// subscription from editing the subscriber's own library.
///
/// What a reader loses is found again by looking in both places:
/// `scripture::Store::load_resource_roots` takes the installed corpus
/// *and* the subscribed copies, with the installed one winning.
#[must_use]
pub fn resource_copy_dir(org_root: &Path, domain: &str, slug: &str) -> PathBuf {
    local_copy_dir(org_root, domain, slug)
}

/// Bring a subscribed Resource up to date — the corpus *or* the library
/// of manifests, because the tier holds both.
///
/// t[impl wiki.resource.subscribe] — a Resource is subscribed to on the
/// same terms as a wiki: it has a local presence, it refreshes, and what
/// arrives is what the publisher holds.
///
/// [`Divergence::Upstream`], which is the whole of what makes this
/// different from a shelf: nothing is ever written into a Resource
/// (`wiki.resource.no-annotations`), so a file that differs is a stale or
/// damaged copy and upstream replaces it. A file only the subscriber
/// holds — an edition installed here that the publisher does not carry —
/// is still kept and reported.
///
/// # Two things live on this tier and only one of them is a corpus
///
/// `resources/bible/` is an edition somebody installs. `resources/patches/`
/// and `resources/samples/` are libraries of **manifests** — what a patch
/// *is*, what a sample *is*, kilobytes each, with the bytes they name
/// living in a File Root. ADR 0003 says so in as many words: small enough
/// that a subscription carries every one of them across an org boundary.
///
/// That distinction was missed once, and it mattered: a blanket refusal
/// of remote Resources meant Signal could not share a patch library with
/// another organisation, for a reason that was only ever true of
/// scripture. There is no distinction in the code because there does not
/// need to be one — a corpus and a library refresh identically, and
/// `max_bytes` is what keeps either honest over a wire.
///
/// # Errors
///
/// As [`refresh`].
pub fn refresh_resource<U: SourceVault + ?Sized>(
    upstream: &U,
    org_root: &Path,
    subscription: &wiki_proto::Subscription,
    max_bytes: u64,
) -> Result<Refreshed, MaterializeError> {
    let copy = resource_copy_dir(org_root, &subscription.domain, &subscription.slug);
    let base = base_path(org_root, &subscription.domain, &subscription.slug);
    refresh_taking(
        upstream,
        &subscription.slug,
        &copy,
        &base,
        &Take {
            selection: &subscription.selection,
            max_bytes,
            divergence: Divergence::Upstream,
        },
    )
}

/// [`refresh_resource`] from a publisher on this disk.
///
/// The local door, exactly as [`refresh_local_shelf`] is for a shelf, and
/// here for the same reason: opening the engine over a path is one
/// answer, given once.
///
/// # Errors
///
/// As [`refresh`], plus a failure opening the publisher's directory.
pub fn refresh_local_resource(
    upstream_root: &Path,
    org_root: &Path,
    subscription: &wiki_proto::Subscription,
) -> Result<Refreshed, MaterializeError> {
    let upstream = vault_live::Backend::single(&subscription.slug, upstream_root.to_path_buf())
        .map_err(|source| MaterializeError::Io {
            path: upstream_root.display().to_string(),
            source: std::io::Error::other(source),
        })?;
    refresh_resource(&upstream, org_root, subscription, u64::MAX)
}

/// Where an org keeps a subscribed **asset shelf**:
/// `<org>/subscribed/<domain>/<kind>/`, beside its subscribed wikis.
///
/// A wiki's address and not a Resource's, and the two choices are made
/// for the same reason rather than opposite ones. A Resource goes to
/// `resources/<slug>/` because a reader opens it by canonical address
/// and would not find it anywhere else. An asset shelf is *mounted*:
/// a person browses it, a file-sync client shows it, and a reference
/// resolves into it by `domain/kind`. `subscribed/<domain>/<kind>/` is
/// where the reference already looks.
#[must_use]
pub fn assets_copy_dir(org_root: &Path, domain: &str, kind: &str) -> PathBuf {
    local_copy_dir(org_root, domain, kind)
}

/// Bring a subscribed **project** up to date from `upstream_root`, the
/// publishing org's `projects/<name>/` directory.
///
/// t[impl wiki.subscribe.local-copy] — after this returns the project
/// resolves with the network down, because everything upstream had is on
/// disk.
///
/// # The one subscribed tree that is not the vault engine, and why
///
/// A shelf used to come through here too, and that was the source of a
/// disagreement worth remembering: this walker **overwrites** a file that
/// differs, while the engine keeps the subscriber's version and reports
/// it. Which one a subscriber got depended on whether the publisher
/// happened to be on the same disk — a distinction nobody who edited a
/// file can see. So a shelf goes through [`refresh_shelf`] now, local or
/// remote, and gets a wiki's promise about local work either way.
///
/// A project does not, and the reason is subtree boundaries rather than
/// cost. `vault_live`'s walk prunes at a **nested shelf** — a directory
/// carrying a shelf marker, which for the Projects tier is precisely a
/// sub-project's `project.md`. Routing a project through the engine would
/// therefore stop copying sub-projects, which is arguably the intended
/// model (`Depth::Surface`: a subscriber who took the album holds a
/// *reference* to the song, not its bytes) and is emphatically a
/// different change from fixing an overwrite. It is named here rather
/// than made silently.
///
/// The copy is still registered for CRDT on **both** sides, which is the
/// part that surprises: collaboration follows registration, and each org
/// registers its own copy. What crosses is bytes; what does not cross is
/// a Loro document, which was never true of wikis either.
///
/// # Errors
///
/// Any failure reading upstream or writing the copy.
pub fn refresh_project(
    upstream_root: &Path,
    org_root: &Path,
    subscription: &wiki_proto::Subscription,
) -> Result<Refreshed, MaterializeError> {
    let copy = assets_copy_dir(org_root, &subscription.domain, &subscription.slug);
    refresh_tree(upstream_root, &copy, &subscription.selection)
}

/// Copy every file under `upstream_root` that `selection` admits into
/// `copy`, byte for byte.
///
/// Shared by Resources and asset shelves because it is one act: bring a
/// tree of arbitrary bytes across, keep what only the subscriber has,
/// and report rather than delete. A file that differs is overwritten —
/// for a Resource because nothing is ever written into one
/// (`wiki.resource.no-annotations`), and for an asset shelf because a
/// subscriber's edit to a shelf they do not own is upstream's content
/// with local work on top, which the base-snapshot path
/// (`refresh_subscription`) is what handles.
///
/// t[impl files.sync.selective] — at the organisation scope. What is
/// outside the selection is not fetched, and `skipped` counts it, so a
/// person can see that a partial subscription is partial on purpose
/// rather than broken.
fn refresh_tree(
    upstream_root: &Path,
    copy: &Path,
    selection: &org_proto::Selection,
) -> Result<Refreshed, MaterializeError> {
    let io = |path: &Path, source: std::io::Error| MaterializeError::Io {
        path: path.display().to_string(),
        source,
    };
    let mut out = Refreshed::default();
    let mut upstream_files = std::collections::BTreeSet::new();
    for rel in walk_files(upstream_root).map_err(|(p, e)| io(&p, e))? {
        if !selection.admits(org_proto::facet_of(&rel)) {
            out.skipped += 1;
            continue;
        }
        upstream_files.insert(rel.clone());
        let src = upstream_root.join(&rel);
        let dst = copy.join(&rel);
        let theirs = std::fs::read(&src).map_err(|e| io(&src, e))?;
        if let Ok(mine) = std::fs::read(&dst)
            && mine == theirs
        {
            out.in_sync += 1;
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        }
        std::fs::write(&dst, &theirs).map_err(|e| io(&dst, e))?;
        out.pulled += 1;
    }
    if copy.is_dir() {
        for rel in walk_files(copy).map_err(|(p, e)| io(&p, e))? {
            if !upstream_files.contains(&rel) && selection.admits(org_proto::facet_of(&rel)) {
                out.local_only.push(rel);
            }
        }
    }
    Ok(out)
}

/// Every regular file under `root`, as `/`-separated paths relative to
/// it, sorted.
fn walk_files(root: &Path) -> Result<Vec<String>, (PathBuf, std::io::Error)> {
    fn go(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), (PathBuf, std::io::Error)> {
        let entries = std::fs::read_dir(dir).map_err(|e| (dir.to_path_buf(), e))?;
        for entry in entries {
            let entry = entry.map_err(|e| (dir.to_path_buf(), e))?;
            let path = entry.path();
            if path.is_dir() {
                go(root, &path, out)?;
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    go(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// t[verify wiki.resource.subscribe] — a Resource refreshes into the
    /// subscriber's copy directory whole, a second refresh finds it in
    /// sync, and what this org installed for *itself* is untouched.
    ///
    /// That last clause is why the copy moved out of `resources/<slug>/`:
    /// an installed edition and a subscribed one are two different
    /// people's decisions, and a refresh that could reach the first would
    /// be a subscription editing its subscriber's own library.
    #[test]
    fn a_resource_lands_beside_the_other_copies_and_spares_what_was_installed() {
        let up = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(up.path().join("WEB")).unwrap();
        std::fs::write(up.path().join("WEB/JHN.usfm"), "\\id JHN\n").unwrap();
        let org = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(org.path().join("resources/bible/BSB")).unwrap();
        std::fs::write(
            org.path().join("resources/bible/BSB/JHN.usfm"),
            "\\id JHN bsb\n",
        )
        .unwrap();
        let sub = wiki_proto::Subscription {
            domain: "acme.test".into(),
            slug: "bible".into(),
            kind: wiki_proto::subscription::SourceKind::Resource,
            title: "Bible".into(),
            core: true,
            declined: false,
            selection: Default::default(),
        };

        let first = refresh_local_resource(up.path(), org.path(), &sub).unwrap();
        assert_eq!(first.pulled, 1);
        assert_eq!(first.in_sync, 0);
        assert!(
            first.local_only.is_empty(),
            "the installed edition is in another directory entirely, so it \
             is not this subscription's business: {first:?}"
        );
        assert_eq!(
            std::fs::read_to_string(org.path().join("subscribed/acme.test/bible/WEB/JHN.usfm"))
                .unwrap(),
            "\\id JHN\n"
        );

        let again = refresh_local_resource(up.path(), org.path(), &sub).unwrap();
        assert_eq!(again.pulled, 0);
        assert_eq!(again.in_sync, 1);
        assert_eq!(
            std::fs::read_to_string(org.path().join("resources/bible/BSB/JHN.usfm")).unwrap(),
            "\\id JHN bsb\n",
            "a refresh reached into what this org installed for itself"
        );
    }

    /// The other half of the Resource policy: a file that differs is
    /// **replaced**, because nothing may write into a Resource — while a
    /// file only the subscriber holds is still kept and named.
    ///
    /// Both halves in one test on purpose. "Upstream wins" and "your
    /// files are never deleted" sound like they disagree, and the shape
    /// that makes both true is the shape a corpus is actually in.
    ///
    /// t[verify wiki.resource.no-annotations] — a difference in a
    /// Resource is a damaged copy rather than somebody's work.
    #[test]
    fn a_resource_that_differs_is_replaced_and_a_local_edition_is_kept() {
        let up = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(up.path().join("WEB")).unwrap();
        std::fs::write(up.path().join("WEB/JHN.usfm"), "\\id JHN good\n").unwrap();
        let org = tempfile::tempdir().unwrap();
        let sub = wiki_proto::Subscription {
            domain: "acme.test".into(),
            slug: "bible".into(),
            kind: wiki_proto::subscription::SourceKind::Resource,
            title: "Bible".into(),
            core: true,
            declined: false,
            selection: Default::default(),
        };
        refresh_local_resource(up.path(), org.path(), &sub).unwrap();

        let copy = resource_copy_dir(org.path(), &sub.domain, &sub.slug);
        std::fs::write(copy.join("WEB/JHN.usfm"), "\\id JHN TRUNCATED").unwrap();
        std::fs::create_dir_all(copy.join("BSB")).unwrap();
        std::fs::write(copy.join("BSB/JHN.usfm"), "\\id JHN bsb\n").unwrap();

        let out = refresh_local_resource(up.path(), org.path(), &sub).unwrap();
        assert_eq!(
            std::fs::read_to_string(copy.join("WEB/JHN.usfm")).unwrap(),
            "\\id JHN good\n",
            "a damaged corpus file was kept as if it were somebody's work"
        );
        assert_eq!(out.pulled, 1);
        assert!(
            out.conflicted.is_empty(),
            "a Resource has no conflicts to have: {out:?}"
        );
        assert_eq!(
            out.local_only,
            vec!["BSB/JHN.usfm".to_owned()],
            "an edition the publisher does not carry was not kept: {out:?}"
        );
    }

    /// An upstream wiki, served by the real vault backend over a real
    /// directory — the same type the server mounts, so this exercises
    /// the actual engine rather than a stand-in.
    fn upstream_with(files: &[(&str, &str)]) -> (tempfile::TempDir, vault_live::Backend) {
        let tmp = tempfile::tempdir().unwrap();
        for (path, body) in files {
            let full = tmp.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, body).unwrap();
        }
        let backend = vault_live::Backend::single("music-theory", tmp.path().to_owned()).unwrap();
        (tmp, backend)
    }

    #[test]
    fn a_fresh_subscription_lands_the_whole_source() {
        let (_up, backend) = upstream_with(&[
            ("purpose.md", "# Purpose\n"),
            ("Concepts/Ionian.md", "# Ionian\n"),
        ]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();

        let out = refresh(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
        )
        .unwrap();
        assert_eq!(out.pulled, 2);
        assert!(!out.has_local_work());
        assert!(local.path().join("Concepts/Ionian.md").is_file());

        // Idempotent: a second refresh pulls nothing.
        let again = refresh(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
        )
        .unwrap();
        assert_eq!(again.pulled, 0);
        assert_eq!(again.in_sync, 2);
    }

    /// A subscriber's edit to a shelf document survives a refresh, and
    /// is reported — the same promise a wiki's copy makes, and the same
    /// one `wiki.subscribe.refresh` makes for every subscribed source.
    ///
    /// This is the claim that a shelf's two refresh routes disagreed
    /// about: the manifest path kept a divergent file and named it, and
    /// the byte walker overwrote it. Which one a subscriber got depended
    /// on whether the publisher happened to be on the same disk, which is
    /// not a distinction a person who edited a file can see.
    ///
    /// t[verify wiki.subscribe.refresh] — for an asset shelf.
    #[test]
    fn a_subscribers_edit_to_a_shelf_document_is_kept_and_reported() {
        let up = tempfile::tempdir().unwrap();
        std::fs::write(up.path().join("track-one.md"), "# Track One\nupstream\n").unwrap();
        let org = tempfile::tempdir().unwrap();
        let sub = wiki_proto::Subscription {
            domain: "acme.test".into(),
            slug: "songs".into(),
            kind: wiki_proto::subscription::SourceKind::Assets,
            title: "songs".into(),
            core: false,
            declined: false,
            selection: Default::default(),
        };

        // The same call the backend makes for a publisher on this disk.
        let first = refresh_local_shelf(up.path(), org.path(), &sub).unwrap();
        assert_eq!(first.pulled, 1);

        // The subscriber annotates their copy — a key correction, a note
        // in the margin. Ordinary use of a shelf you subscribe to.
        let copy = assets_copy_dir(org.path(), &sub.domain, &sub.slug).join("track-one.md");
        std::fs::write(&copy, "# Track One\nupstream\n\nWe play this in D.\n").unwrap();

        let again = refresh_local_shelf(up.path(), org.path(), &sub).unwrap();
        assert_eq!(
            std::fs::read_to_string(&copy).unwrap(),
            "# Track One\nupstream\n\nWe play this in D.\n",
            "the subscriber's edit was overwritten by a refresh"
        );
        assert_eq!(
            again.local_only,
            vec!["track-one.md".to_owned()],
            "and it has to be *reported*, or the subscriber cannot tell \
             their copy has diverged: {again:?}"
        );
    }

    /// The finding the remote-shelf path rests on: the vault engine's
    /// manifest is content-agnostic. A `.kf`, a `.wav`, a file with no
    /// extension at all — all of it crosses, and a shelf arriving without
    /// its documents was never the risk the old comment claimed.
    #[test]
    fn the_vault_engine_carries_every_file_and_not_only_markdown() {
        let (_up, backend) = upstream_with(&[
            ("track-one.md", "# Track One\n"),
            ("track-one.kf", "| C | G |\n"),
            ("stem.wav", "RIFF....not really\n"),
            ("LICENCE", "all rights reserved\n"),
        ]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let out = refresh(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
        )
        .unwrap();
        assert_eq!(out.pulled, 4, "a file was dropped by kind: {out:?}");
        for name in ["track-one.md", "track-one.kf", "stem.wav", "LICENCE"] {
            assert!(local.path().join(name).is_file(), "{name} did not arrive");
        }
    }

    /// A file too big to put in one message is **reported**, not
    /// attempted — and nothing else in the shelf is held up by it.
    ///
    /// t[verify wiki.subscribe.federated] — for a shelf: what crosses is
    /// the documents, and the bound is where ADR 0003's "names, not
    /// gigabytes" rule is actually enforced rather than hoped for.
    #[test]
    fn a_file_over_the_bound_is_skipped_and_the_rest_of_the_shelf_arrives() {
        let big = "x".repeat(4096);
        let (_up, backend) = upstream_with(&[
            ("track-one.md", "# Track One\n"),
            ("Stems/lead vocal.wav", big.as_str()),
        ]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let out = refresh_taking(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
            &Take {
                selection: &org_proto::Selection::All,
                max_bytes: 1024,
                divergence: Divergence::Keep,
            },
        )
        .unwrap();
        assert_eq!(out.pulled, 1, "the document should have come down: {out:?}");
        assert_eq!(out.skipped, 1, "the oversized take should be counted");
        assert!(local.path().join("track-one.md").is_file());
        assert!(
            !local.path().join("Stems/lead vocal.wav").exists(),
            "an oversized file was fetched anyway"
        );
        // And it is not mistaken for the subscriber's own work on the
        // next pass, which is what would happen if it had been skipped
        // *after* being planned.
        let again = refresh_taking(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
            &Take {
                selection: &org_proto::Selection::All,
                max_bytes: 1024,
                divergence: Divergence::Keep,
            },
        )
        .unwrap();
        assert_eq!(again.in_sync, 1);
        assert_eq!(again.skipped, 1);
        assert!(!again.has_local_work(), "{again:?}");
    }

    /// t[verify wiki.subscribe.local-copy] — after a refresh the copy
    /// answers on its own. Standing in for "the network is down" by
    /// reading the files directly: nothing about rendering a page
    /// needs upstream once it is here.
    #[test]
    fn the_copy_reads_without_upstream() {
        let (up, backend) = upstream_with(&[("Concepts/Modes.md", "# Modes\nSeven.\n")]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        refresh(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
        )
        .unwrap();

        drop(backend);
        drop(up);

        let body = std::fs::read_to_string(local.path().join("Concepts/Modes.md")).unwrap();
        assert!(body.contains("Seven."));
    }

    /// t[verify wiki.subscribe.local-authority] — the subscriber's own
    /// page is kept and is never sent upstream.
    #[test]
    fn a_local_only_page_is_kept_and_never_pushed() {
        let (up, backend) = upstream_with(&[("purpose.md", "# Purpose\n")]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        refresh(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
        )
        .unwrap();

        std::fs::write(local.path().join("My Notes.md"), "# Mine\n").unwrap();

        let out = refresh(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
        )
        .unwrap();
        assert_eq!(out.local_only, vec!["My Notes.md".to_owned()]);
        assert!(out.has_local_work());
        // Still ours...
        assert!(local.path().join("My Notes.md").is_file());
        // ...and upstream never heard about it.
        assert!(!up.path().join("My Notes.md").exists());
    }

    /// t[verify wiki.subscribe.refresh] — both sides changed one page.
    /// Neither is overwritten and neither wins by clock; the conflict
    /// is reported and the copy stays readable.
    #[test]
    fn a_conflict_is_reported_rather_than_decided() {
        let (up, backend) = upstream_with(&[("Concepts/Ionian.md", "# Ionian\noriginal\n")]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        refresh(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
        )
        .unwrap();

        // The subscriber edits; upstream also moves on.
        std::fs::write(
            local.path().join("Concepts/Ionian.md"),
            "# Ionian\nmy edit\n",
        )
        .unwrap();
        std::fs::write(
            up.path().join("Concepts/Ionian.md"),
            "# Ionian\ntheir edit\n",
        )
        .unwrap();

        let out = refresh(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
        )
        .unwrap();
        assert_eq!(out.conflicted, vec!["Concepts/Ionian.md".to_owned()]);
        assert_eq!(
            out.pulled, 0,
            "an upstream change never overwrites local work"
        );

        // Local work survives verbatim.
        let body = std::fs::read_to_string(local.path().join("Concepts/Ionian.md")).unwrap();
        assert!(body.contains("my edit"));
        // And upstream was not reverted by our stale copy either.
        let theirs = std::fs::read_to_string(up.path().join("Concepts/Ionian.md")).unwrap();
        assert!(theirs.contains("their edit"));
    }

    /// An upstream page that changed with no local edit is ordinary
    /// news and is taken.
    #[test]
    fn an_uncontested_upstream_change_arrives() {
        let (up, backend) = upstream_with(&[("Concepts/Modes.md", "# Modes\nv1\n")]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        refresh(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
        )
        .unwrap();

        std::fs::write(up.path().join("Concepts/Modes.md"), "# Modes\nv2\n").unwrap();
        let out = refresh(
            &backend,
            "music-theory",
            local.path(),
            &base.path().join("b.json"),
        )
        .unwrap();

        assert_eq!(out.pulled, 1);
        let body = std::fs::read_to_string(local.path().join("Concepts/Modes.md")).unwrap();
        assert!(body.contains("v2"));
    }

    #[test]
    fn the_copy_sits_where_a_reference_says_it_does() {
        let org = Path::new("/data/orgs/alice-personal");
        let dir = local_copy_dir(org, "acme.test", "music-theory");
        assert!(dir.ends_with("subscribed/acme.test/music-theory"));
    }
}
