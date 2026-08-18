// architect's `Entity` derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the project feature.
//!
//! Projects are plain markdown pages living under `Projects/*.md`
//! in the vault. Frontmatter carries the identity (stable `id:`
//! UUID), display fields (title, description, color), status, and
//! the billing knobs that downstream features (`timer`,
//! `finances`, `agent-dispatch`) need to know about a project
//! without opening a database.
//!
//! This proto owns the **wire surface**:
//! - [`ProjectInfo`] — the canonical project entity (+ [`Status`]).
//! - [`states`] — the per-project state registry ([`StateGroup`] /
//!   [`StateDef`] / [`StatesConfig`] + [`resolve_state_group`]).
//! - [`ProjectService`] — the RPC trait CLI / web UI bind to.
//!
//! It is wasm-clean so the browser UI can talk to the service
//! directly. The sibling `project` crate sits on top and owns the
//! parse / serialize / scan / write side plus the disk-backed
//! `ProjectBackend` — the same split `milestone` /
//! `milestone-proto` uses.

pub mod model;
pub mod parts;
pub mod service;
pub mod states;
pub mod verify;

pub use model::{ProjectInfo, Status, Tags};
pub use parts::{
    Audience, Capabilities, Capability, Deliverable, DeliverableItem, Deliverables, Medium, Part,
    Parts, Piece, Scope,
};
pub use service::{ProjectError, ProjectEvent, ProjectService, ProjectServiceRpc};
pub use states::{StateDef, StateGroup, StatesConfig, default_states, resolve_state_group};
pub use verify::{project_default as verify_project_default, resolve as resolve_verify_command};

// architect-emitted vox bits: the async client / dispatcher /
// descriptor / serve helpers. Mount sites stitch the descriptor
// + `serve` into the org router; the web UI binds the client.
#[cfg(feature = "vox")]
pub use service::{
    ProjectServiceClient, ProjectServiceRpcDispatcher as ProjectDispatcher,
    Service as ProjectServiceBridge, layer as project_service_layer,
    project_service_rpc_service_descriptor as project_service_descriptor,
    serve as serve_project_service,
};

// `#[subscribe] fn events` stream sibling — live project changes.
// Mount `project_service_stream_layer(backend)` next to the base
// service; subscribers drive a `ProjectServiceStreamClient`.
#[cfg(feature = "vox")]
pub use service::{
    ProjectServiceStream, ProjectServiceStreamClient, ProjectServiceStreamSource,
    project_service_stream_service_descriptor as project_stream_descriptor,
    stream_layer as project_service_stream_layer, stream_serve as serve_project_service_stream,
};
