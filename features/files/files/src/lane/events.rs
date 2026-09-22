//! The v2 live stream — [`TreeService::events`](files_proto::service::tree::TreeService::events).
//!
//! One hub per backend ([`FilesBackend::lane_events`]), fed from two
//! places: every v2 lane mutation publishes its own nested
//! [`FilesEvent`], and every legacy event is translated on its way out
//! ([`from_legacy`]) — so a v2 subscriber hears about checkpoints the
//! cadence engine took and hydration a daemon asked for, not only about
//! what arrived through a v2 method.
//!
//! ## Only what the subscriber may read
//!
//! Each subscription is filtered for the caller who opened it, captured
//! when it attaches — the stream outlives the request task, and the
//! task-local caller with it. A caller whose role reads every root hears
//! everything (narrowed to one root if they asked). A caller holding only
//! grants hears an event only when every path it names is one they may
//! read; an event naming no path reaches them only for a root they hold
//! something in. An event whose root cannot be told — a review comment
//! carries no root — reaches role holders only.

use files_proto::id::RootId;
use files_proto::path::RootPath;
use files_proto::service::FilesEvent;
use files_proto::service::access::Capability;
use files_proto::service::legacy::FilesEvent as Legacy;
use files_proto::service::tree::TreeServiceStreamSource;
use files_proto::service::{curation, review, roots, sync, tree, upload, version, write};

use crate::backend::FilesBackend;
use crate::lane::caller::{self, Caller};

/// Publish to every v2 subscriber. Never blocks and never fails: a hub
/// with nobody listening is not an error.
pub fn publish(backend: &FilesBackend, event: FilesEvent) {
    let _ = backend.lane_events().send(event);
}

/// A legacy event in v2's vocabulary, when it has one.
#[must_use]
pub fn from_legacy(event: &Legacy) -> Option<FilesEvent> {
    Some(match event.clone() {
        Legacy::RootCreated(info) => FilesEvent::Root(roots::RootEvent::Created(info)),
        Legacy::Checkpointed(c) => FilesEvent::Version(version::VersionEvent::Checkpointed(c)),
        Legacy::Snapshotted(s) => FilesEvent::Version(version::VersionEvent::Snapshotted(s)),
        Legacy::VersionNamed(n) => FilesEvent::Curation(curation::CurationEvent::VersionNamed(n)),
        Legacy::VersionUnnamed(n) => {
            FilesEvent::Curation(curation::CurationEvent::VersionUnnamed(n))
        }
        Legacy::ProjectVersionStarted(p) => {
            FilesEvent::Curation(curation::CurationEvent::ProjectVersionStarted(p))
        }
        Legacy::HydrationChanged(h) => {
            let path = RootPath::parse(&h.path).ok()?;
            FilesEvent::Sync(sync::SyncEvent::HydrationChanged(path))
        }
        Legacy::ReviewCreated(r) => FilesEvent::Review(review::ReviewEvent::Created(r)),
        Legacy::ReviewCommentAdded(c) => FilesEvent::Review(review::ReviewEvent::CommentAdded(c)),
        Legacy::ReviewCommentDeleted(c) => {
            FilesEvent::Review(review::ReviewEvent::CommentDeleted(c))
        }
    })
}

/// Which root an event is about, and which paths in it — what the
/// per-subscriber filter decides on. `None` for the root means it cannot
/// be told from the event.
fn scope(event: &FilesEvent) -> (Option<RootId>, Vec<RootPath>) {
    use FilesEvent as E;
    let id = |u: uuid::Uuid| Some(RootId::new(u));
    match event {
        E::Root(roots::RootEvent::Created(r) | roots::RootEvent::Renamed(r)) => {
            (id(r.id), Vec::new())
        }
        E::Root(roots::RootEvent::AdoptionProgressed(p)) => (Some(p.root_id), Vec::new()),
        E::Root(roots::RootEvent::Released(r)) => (Some(*r), Vec::new()),
        E::Tree(tree::TreeEvent::Changed(d)) => {
            let root = d.changed.first().map(|e| e.root_id);
            let mut paths: Vec<RootPath> = d.changed.iter().map(|e| e.path.clone()).collect();
            paths.extend(d.removed.iter().cloned());
            (root, paths)
        }
        E::Tree(tree::TreeEvent::FreshnessChanged(f)) => (Some(f.root_id), Vec::new()),
        E::Write(write::WriteEvent::Created(entries)) => (
            entries.first().map(|e| e.root_id),
            entries.iter().map(|e| e.path.clone()).collect(),
        ),
        E::Write(
            write::WriteEvent::Moved(r)
            | write::WriteEvent::Copied(r)
            | write::WriteEvent::Deleted(r),
        ) => {
            let mut paths: Vec<RootPath> = r.outcomes.iter().map(|o| o.path.clone()).collect();
            paths.extend(r.outcomes.iter().filter_map(|o| o.landed_at.clone()));
            (Some(r.root_id), paths)
        }
        E::Upload(upload::UploadEvent::Completed(e)) => (Some(e.root_id), vec![e.path.clone()]),
        E::Upload(_) => (None, Vec::new()),
        E::Version(version::VersionEvent::Checkpointed(c)) => (id(c.root_id), Vec::new()),
        E::Version(version::VersionEvent::Snapshotted(s)) => (id(s.root_id), Vec::new()),
        E::Version(version::VersionEvent::OccupancyChanged(o)) => {
            (Some(o.root_id), vec![o.path.clone()])
        }
        E::Version(_) => (None, Vec::new()),
        E::Curation(
            curation::CurationEvent::VersionNamed(n) | curation::CurationEvent::VersionUnnamed(n),
        ) => (id(n.root_id), Vec::new()),
        E::Curation(
            curation::CurationEvent::ProjectVersionStarted(p)
            | curation::CurationEvent::ProjectVersionRestarted(p),
        ) => (id(p.root_id), Vec::new()),
        E::Review(review::ReviewEvent::Created(r)) => (id(r.root_id), Vec::new()),
        E::Sync(sync::SyncEvent::FacetsChanged(r)) => (Some(*r), Vec::new()),
        _ => (None, Vec::new()),
    }
}

/// Whether `who` may hear `event`, given the root they narrowed to and
/// whether their role reads everywhere.
fn audible(
    backend: &FilesBackend,
    who: &Caller,
    reads_everywhere: bool,
    only: Option<RootId>,
    event: &FilesEvent,
) -> bool {
    let (root, paths) = scope(event);
    if let (Some(want), Some(got)) = (only, root)
        && want != got
    {
        return false;
    }
    if only.is_some() && root.is_none() {
        // Asked about one root; an event we cannot place is not about it.
        return false;
    }
    if reads_everywhere {
        return true;
    }
    let (Some(root), Some(me)) = (root, who.subject()) else {
        return false;
    };
    if paths.is_empty() {
        return backend.holds_anything_in(&me, root);
    }
    paths
        .iter()
        .all(|p| backend.authorise(&me, root, p, Capability::Read).is_ok())
}

impl TreeServiceStreamSource for FilesBackend {
    // t[impl files.live.propagation] — v2: one stream, every lane, filtered
    // to what the subscriber may read
    fn events_attach(&self, root_id: Option<RootId>, sink: architect::vox::Tx<FilesEvent>) {
        // Captured here, on the request task, while the gate's caller is
        // still in scope; the relay below runs after it has gone.
        let who = caller::current();
        let mut rx = self.lane_events().subscribe();
        let backend = self.clone();
        tokio::spawn(async move {
            if let Some(root) = root_id
                && !crate::lane::root_or_fault(&backend, root).is_ok()
            {
                return;
            }
            let reads_everywhere = caller::baseline_as(&backend, &who)
                .await
                .contains(&Capability::Read);
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if !audible(&backend, &who, reads_everywhere, root_id, &event) {
                            continue;
                        }
                        // Awaiting the send is the flow control: a
                        // subscriber that stops reading holds this relay,
                        // and the broadcast drops its oldest events rather
                        // than growing without bound.
                        if sink.send(event).await.is_err() {
                            return;
                        }
                    }
                    // Fell behind. What was missed is recoverable from
                    // `changes_since`; the stream carries on from now.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_legacy_event_with_a_v2_meaning_is_translated() {
        let root = files_proto::model::FileRootInfo {
            id: uuid::Uuid::nil(),
            name: "r".into(),
            path: None,
            flavor: files_proto::model::RootFlavor::Media,
            created_at: chrono::Utc::now(),
            project_version: None,
        };
        let v2 = from_legacy(&Legacy::RootCreated(root.clone())).expect("translated");
        assert_eq!(v2, FilesEvent::Root(roots::RootEvent::Created(root)));
    }
}
