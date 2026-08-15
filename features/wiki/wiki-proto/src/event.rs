//! Live change events. Mirrors `vault_proto::VaultChange` —
//! subscribers get a stream of typed updates and can build
//! reactive UI on top.

use chrono::{DateTime, Utc};
use facet::Facet;

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub enum WikiEvent {
    /// A wiki page was created or replaced.
    PageWritten { path: String, at: DateTime<Utc> },
    /// A wiki page was deleted.
    PageDeleted { path: String, at: DateTime<Utc> },
    /// A new ingest task hit the queue.
    IngestEnqueued {
        task_id: String,
        source_path: String,
    },
    /// An ingest task transitioned state.
    IngestStateChanged {
        task_id: String,
        /// `"Pending" | "Analyzing" | "Generating" | ...`
        /// matches [`crate::ingest::IngestStatus`].
        new_status: String,
    },
    /// A lint pass produced findings.
    LintCompleted {
        finding_count: u32,
        at: DateTime<Utc>,
    },
    /// A new review item is awaiting curator attention.
    ReviewEnqueued { item_id: String },
    /// A peer pull completed.
    PeerPulled { peer_id: String, changed: u32 },
    /// Broadcast lag — subscriber missed events. Re-pull
    /// state explicitly.
    Resync,
}

/// One wiki change, broadcast to every subscriber of the
/// [`crate::Events`] `changes` stream.
///
/// ## Why the wrapper
///
/// `#[subscribe]` streams take no filter params, so the scope
/// travels with the event: a backend can serve several wiki ids
/// (`Layout::UnderParent`), and every subscriber sees all of them.
/// Clients keep the id they browse — server-side filtering by
/// `wiki_id` (the shape the old `subscribe(wiki_id, tx)` rpc had)
/// is now a client-side `==`.
///
/// ## Subscriber contract (changes only, no snapshot variant)
///
/// The stream carries *changes only*. A subscriber fetches state
/// once — `Pages::list_pages`, `Graph::build_graph`,
/// `Ingest::list_ingest`, whichever it renders, after subscribing
/// so nothing is missed in between — then folds:
///
/// - [`WikiEvent::PageWritten`] / [`WikiEvent::PageDeleted`] —
///   `path` is now different / gone.
/// - [`WikiEvent::IngestEnqueued`] /
///   [`WikiEvent::IngestStateChanged`] — that task moved.
/// - [`WikiEvent::ReviewEnqueued`] / [`WikiEvent::LintCompleted`] /
///   [`WikiEvent::PeerPulled`] — that queue grew.
/// - [`WikiEvent::Resync`] — re-pull; state was skipped.
///
/// Events name *what* changed, not the new value: the derived
/// views (page index, relevance graph, queue rows) are server-built
/// from parsed content the event can't carry, so a client re-fetches
/// the view the event touched. The event is the trigger, the rpc
/// stays the source of truth.
#[derive(Debug, Clone, PartialEq, Facet)]
pub struct WikiChange {
    /// Which wiki changed — subscribers filter on this.
    pub wiki_id: String,
    /// What happened.
    pub event: WikiEvent,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{WikiChange, WikiEvent};
    unsafe impl vox_types::Reborrow for WikiEvent {
        type Ref<'a> = WikiEvent;
    }
    unsafe impl vox_types::Reborrow for WikiChange {
        type Ref<'a> = WikiChange;
    }
}
