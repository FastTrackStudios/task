// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! `task` — first-party task feature.
//!
//! The wasm-clean wire surface ([`TaskInfo`] and its enums, the
//! pure domain rules — [`relations`] / [`relevance`] / [`capture`]
//! — and the [`TaskService`] RPC trait with its `#[subscribe]
//! events` stream) lives in the sibling [`task_proto`] crate;
//! this crate sits on top of it and owns the vault-backed side
//! (parse / serialize / scan / write / [`TaskBackend`]). Every
//! proto item is re-exported here at its historical `task::…`
//! path.
//!
//! Tasks are plain markdown pages with YAML frontmatter living
//! inside a `vault::Vault`. The schema mirrors
//! [callumalpass/tasknotes](https://github.com/callumalpass/tasknotes)
//! so existing `TaskNotes` vaults round-trip into Task without
//! conversion.
//!
//! Surface:
//! - [`TaskInfo`] — the parsed task model.
//! - [`Status`] / [`Priority`] — configurable enums (default set
//!   mirrors `TaskNotes` defaults).
//! - [`parse_page`] — `vault_proto::VaultPage` → `TaskInfo`.
//! - [`serialize_task`] — `TaskInfo` → markdown bytes.
//! - [`scan_vault`] — collect every `type: task` (or
//!   `tags: [task]`) page from a `vault::Vault`.
//! - [`capture`] — minimal natural-language capture: parse
//!   `"Buy milk tomorrow #errands @shopping"` into a
//!   `TaskInfo`. Date keywords: today / tomorrow / `next-<day>`.
//!
//! Higher-level views (kanban, calendar) ride on `vault-live`'s
//! `.base` query DSL via formulas + filters; they live in
//! `task-ui` (future) and don't need anything from this crate
//! beyond `TaskInfo`.
//!
//! ## Wasm
//!
//! There is nothing to gate here any more. Browser consumers take
//! [`task_proto`] — the wire model, the pure domain rules and the
//! RPC client all live there — so this crate is unconditionally
//! the server side: it opens a `vault::Vault` and walks
//! `std::fs`. Mirrors `project`.

pub mod capture;
pub mod model;
pub mod parse;
pub mod relations;
pub mod relevance;
pub mod service;

/// Filing — what a task belongs to (see [`task_proto::filing`]).
pub use task_proto::filing;
pub use task_proto::filing::{Anchor, anchor, is_filed, is_unfiled};

/// The agent lane's triage vocabulary (see [`task_proto::agent_lane`]).
pub use task_proto::agent_lane;

pub use task_proto::agent_lane::{
    TriageLabel, has_triage_label, is_untriaged, triage_label, triage_labels,
};
/// Wayfinder map bodies (see [`task_proto::wayfinder`]).
pub use task_proto::wayfinder;
pub use task_proto::wayfinder::{MapBody, Section, map_body};

// FS-dependent modules (vault::Vault, std::fs walks).
pub mod backend;
pub mod scan;
pub mod write;

pub use capture::{capture, infer_project_id};
pub use model::{
    Priority, Relation, RelationKind, Status, TaskInfo, TimeEntry, close_open_time_entries,
    is_due_on_or_before, status_is_open, status_is_terminal, track_status_transition,
};
pub use relations::{ReverseRelation, arrange_families, cascade_status, click_transition};
pub use relevance::{
    RelevanceContext, condense_next_per_project, filter_relevant, is_relevant, relevance_rank,
};
// Workflow actor/audit types referenced by `WorkflowAttrs` — re-exported
// so UI consumers don't need their own workflows-proto dep.
pub use parse::{ParseError, parse_page, parse_str};
pub use service::{
    TaskError, TaskEvent, TaskListFilter, TaskReverseRelations, TaskService, TaskServiceRpc,
};
pub use workflows_proto;

pub use backend::TaskBackend;
pub use scan::scan_vault;
pub use write::{WriteError, serialize_task, write_task};

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
