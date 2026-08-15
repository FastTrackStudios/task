// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the task feature.
//!
//! Tasks are plain markdown pages with YAML frontmatter. The
//! schema mirrors
//! [callumalpass/tasknotes](https://github.com/callumalpass/tasknotes)
//! so existing `TaskNotes` vaults round-trip into Task without
//! conversion.
//!
//! This proto owns the **wire surface** plus the pure domain
//! rules over it — everything that needs no filesystem, so the
//! browser UI can hold authoritative rows and classify them with
//! the same code the server runs:
//!
//! - [`TaskInfo`] — the task model (+ [`Status`] / [`Priority`] /
//!   [`TimeEntry`] / [`WorkflowAttrs`] / [`Estimate`] /
//!   [`Relation`]).
//! - [`status_is_open`] / [`status_is_terminal`] /
//!   [`is_due_on_or_before`] — the authoritative classification
//!   predicates every frontend routes through.
//! - [`relations`] — the merged (typed + legacy) task-graph edge
//!   set, reverse index and status cascade.
//! - [`relevance`] — contextual relevance, applied server-side in
//!   `TaskService::query` and client-side against the optimistic
//!   store.
//! - [`capture`] — minimal natural-language capture:
//!   `"Buy milk tomorrow #errands @shopping"` → [`TaskInfo`].
//! - [`TaskService`] — the RPC trait (CRUD + `try_claim` + the
//!   `#[subscribe] events` stream carrying [`TaskEvent`]).
//!
//! The sibling `task` crate sits on top of this proto and owns
//! the vault-backed side: `parse` / `write` / `scan` and the
//! disk-backed `TaskBackend`. Same split as `milestone` /
//! `milestone-proto`.

pub mod agent_lane;
pub mod capture;
pub mod filing;
pub mod model;
pub mod relations;
pub mod relevance;
pub mod service;
pub mod wayfinder;

pub use agent_lane::{
    TriageLabel, has_triage_label, is_untriaged, triage_label, triage_labels,
};
pub use capture::{capture, infer_project_id};
pub use filing::{Anchor, anchor, is_filed, is_unfiled};
pub use model::{
    Priority, Relation, RelationKind, Status, TaskInfo, TimeEntry, close_open_time_entries,
    is_due_on_or_before, status_is_open, status_is_terminal, track_status_transition,
};
pub use relations::{ReverseRelation, arrange_families, cascade_status, click_transition};
pub use relevance::{
    RelevanceContext, condense_next_per_anchor, condense_next_per_project, filter_relevant,
    is_relevant, next_action_key, partition_triage, relevance_rank,
};
pub use service::{
    TaskError, TaskEvent, TaskListFilter, TaskReverseRelations, TaskService, TaskServiceRpc,
};
pub use wayfinder::{MapBody, Section, map_body};
// Workflow actor/audit types referenced by `WorkflowAttrs` — re-exported
// so UI consumers don't need their own workflows-proto dep.
pub use workflows_proto;

// architect-emitted vox bits: the async client / dispatcher /
// descriptor / serve helpers. Mount sites stitch the descriptor
// + `serve` into the org router; the web UI binds the client.
#[cfg(feature = "vox")]
pub use service::{
    Service as TaskServiceBridge, TaskServiceClient, TaskServiceRpcDispatcher as TaskDispatcher,
    layer as task_service_layer, serve as serve_task_service,
    task_service_rpc_service_descriptor as task_service_descriptor,
};

// `#[subscribe] fn events` stream sibling — live task changes.
// Mount `task_service_stream_layer(backend)` next to the base
// service; subscribers drive a `TaskServiceStreamClient`.
#[cfg(feature = "vox")]
pub use service::{
    TaskServiceStream, TaskServiceStreamClient, TaskServiceStreamSource,
    stream_layer as task_service_stream_layer, stream_serve as serve_task_service_stream,
    task_service_stream_service_descriptor as task_stream_descriptor,
};
