// architect's Entity/rpc derives emit cfg-gated blocks; allow at crate
// scope (same convention as `files-proto` / `task-proto`).
#![allow(unexpected_cfgs)]

//! Wire contract for the Files **placement layer** (GitHub issue #262,
//! part of the Files spec #255; engine decisions in ADR 0001 —
//! `apps/task/docs/adr/0001-files-version-store-jj-cas.md`).
//!
//! Placement is the axis #259's RPC surface deliberately left out: a File
//! Root has to live *somewhere*, and "somewhere" is governed rather than
//! accidental. This crate owns the wasm-clean wire surface for that —
//! [`model`]'s types plus three `#[architect::rpc]` traits, one per lane
//! (operator / org / agent; see [`service`]). The sibling `files-storage`
//! crate owns the server-side implementation, exactly as `files` sits on
//! top of `files-proto`.
//!
//! The shape, in the glossary's words (`apps/task/CONTEXT.md`):
//!
//! - the operator registers **Storage Locations** — deployment-scoped,
//!   each declaring capability classes (live-trees and/or blobs) and
//!   spoken for by exactly one **Storage agent**;
//! - an org reaches a location only through a **Storage grant** — a
//!   capability subset, a logical-byte quota, and a path prefix that is
//!   the org's own subtree there;
//! - a root's **live tree** binds to one location, whose agent owns the
//!   authoritative repo, while its **blobs** may be replicated onto other
//!   locations — two independent placement axes.

pub mod error;
pub mod model;
pub mod service;

pub use error::StorageError;
pub use model::{
    AgentAnnouncement, AgentCredential, AgentDirective, AgentEnrollment, AgentHosting, AgentInfo,
    AgentStatus, AnnouncedVolume, BlobReplica, CapabilityClass, ConfinedPath, DirectiveKind,
    DirectiveOutcome, GrantSpec, GrantUsage, LiveTreeBinding, LocationHealth, LocationKind,
    PlacementStatus, RootPlacement, StorageGrantInfo, StorageLocationInfo, VolumeHealth,
};
pub use service::org::StorageEvent;
pub use service::{StorageAdminService, StorageAgentService, StorageService};

// architect-emitted vox bits, per lane. Mount sites stitch each
// descriptor + `serve` into the router it belongs on: the admin lane onto
// the server router, the org lane onto each org router, the agent lane
// wherever agents connect.
#[cfg(feature = "vox")]
pub use service::admin::{
    StorageAdminServiceClient, StorageAdminServiceRpcDispatcher as StorageAdminDispatcher,
    layer as storage_admin_layer, serve as serve_storage_admin,
    storage_admin_service_rpc_service_descriptor as storage_admin_descriptor,
};
#[cfg(feature = "vox")]
pub use service::agent::{
    StorageAgentServiceClient, StorageAgentServiceRpcDispatcher as StorageAgentDispatcher,
    layer as storage_agent_layer, serve as serve_storage_agent,
    storage_agent_service_rpc_service_descriptor as storage_agent_descriptor,
};
#[cfg(feature = "vox")]
pub use service::org::{
    StorageServiceClient, StorageServiceRpcDispatcher as StorageDispatcher,
    layer as storage_service_layer, serve as serve_storage_service,
    storage_service_rpc_service_descriptor as storage_service_descriptor,
};

// The `#[subscribe]` stream siblings: org-lane placement/grant events and
// the agent lane's directive stream.
#[cfg(feature = "vox")]
pub use service::agent::{
    StorageAgentServiceStream, StorageAgentServiceStreamClient, StorageAgentServiceStreamSource,
    storage_agent_service_stream_service_descriptor as storage_agent_stream_descriptor,
    stream_layer as storage_agent_stream_layer, stream_serve as serve_storage_agent_stream,
};
#[cfg(feature = "vox")]
pub use service::org::{
    StorageServiceStream, StorageServiceStreamClient, StorageServiceStreamSource,
    storage_service_stream_service_descriptor as storage_stream_descriptor,
    stream_layer as storage_service_stream_layer, stream_serve as serve_storage_service_stream,
};
