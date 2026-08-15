// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the workstream feature.
//!
//! A workstream is the parent-with-swarm construct — a
//! project-scoped stream of work with a lead (the orchestrator),
//! members (the swarm), lifecycle status, and start/target
//! dates. It replaces the old `epic` tag convention. Tasks
//! attach via `task::WorkflowAttrs::workstream`; progress is a
//! derived rollup, never stored.
//!
//! This proto owns the **wire surface**: the [`Workstream`]
//! model (+ canonical [`Status`] enum), the derived
//! [`WorkstreamRollup`] / [`WorkstreamWithRollup`] shapes, the
//! [`WorkstreamEvent`] stream payload, and the
//! [`WorkstreamService`] RPC trait the CLI / web UIs bind to.
//! It is wasm-clean so the web UI can talk to the service
//! directly.
//!
//! The sibling `workstream` crate sits on top of this proto and
//! owns the parse / serialize / rollup-compute side plus the
//! disk-backed `WorkstreamBackend` — exactly like `milestone`
//! sits on top of `milestone-proto`.

pub mod model;
pub mod service;

pub use model::{AgentRefList, Links, Status, Workstream};
pub use service::{
    StateGroupCounts, WorkstreamError, WorkstreamEvent, WorkstreamRollup, WorkstreamService,
    WorkstreamServiceRpc, WorkstreamWithRollup,
};

// architect-emitted vox bits: the async client / dispatcher /
// descriptor / serve helpers. Mount sites stitch the descriptor
// + `serve` into the org router; the web UI binds the client.
#[cfg(feature = "vox")]
pub use service::{
    Service as WorkstreamServiceBridge, WorkstreamServiceClient,
    WorkstreamServiceRpcDispatcher as WorkstreamDispatcher, layer as workstream_service_layer,
    serve as serve_workstream_service,
    workstream_service_rpc_service_descriptor as workstream_service_descriptor,
};

// `#[subscribe] fn events` stream sibling — live workstream
// changes. Mount `workstream_service_stream_layer(backend)` next
// to the base service; subscribers drive a
// `WorkstreamServiceStreamClient`.
#[cfg(feature = "vox")]
pub use service::{
    WorkstreamServiceStream, WorkstreamServiceStreamClient, WorkstreamServiceStreamSource,
    stream_layer as workstream_service_stream_layer,
    stream_serve as serve_workstream_service_stream,
    workstream_service_stream_service_descriptor as workstream_stream_descriptor,
};
