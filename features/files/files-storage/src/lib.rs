// architect's HasDispatcher/rpc derives emit cfg-gated blocks; allow at
// crate scope (same convention as `files` / `task`).
#![allow(unexpected_cfgs)]

//! Server-side half of the Files **placement layer** (issue #262, part of
//! spec #255; engine decisions in ADR 0001 —
//! `apps/task/docs/adr/0001-files-version-store-jj-cas.md`). The
//! wasm-clean wire surface lives in the sibling `files-storage-proto`
//! crate; this crate is the coordinator plus the in-server agent hosting.
//!
//! # The shape
//!
//! ```text
//!            operator lane            org lane              agent lane
//!        StorageAdminBackend      StorageBackend       StorageAgentBackend
//!                   \                   |                   /
//!                    \                  |                  /
//!                     ──────────  StorageCore  ─────────────
//!                                  (registry)
//!                                       |
//!                                  directives
//!                                       |
//!                                 InServerAgent   ← the first of three
//!                                (live trees,        hostings; desktop
//!                                 blobs, bytes)      and standalone are
//!                                                    the same protocol
//!                                                    over vox
//! ```
//!
//! [`StorageCore`] is deployment-scoped: ONE registry of Storage
//! Locations serves every org, which is precisely why an org's reach into
//! it is a **Storage grant** — a capability subset, a logical-byte quota,
//! and a path prefix that is the org's own subtree. Nothing an org sends
//! bypasses that check; a location it holds no grant on does not exist as
//! far as its lane is concerned.
//!
//! Placement is two independent axes (ADR 0001): a root's **live tree**
//! binds to exactly one location, whose agent creates it and owns the
//! authoritative version-store repo, while its **blobs** may be
//! replicated onto any number of blob-capable locations — including ones
//! that could never host a live tree at all.
//!
//! # Wiring it up
//!
//! ```no_run
//! # use std::sync::Arc;
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use files_storage::{InServerAgent, StorageAdminBackend, StorageAgentBackend, StorageBackend,
//!     StorageCore, core::{in_server_announcement, registry_dir, server_volume}};
//! use uuid::Uuid;
//!
//! let data_root = std::path::Path::new("/var/lib/task");
//! let core = StorageCore::open(registry_dir(data_root))?;
//!
//! // The server speaks for its own volumes through an ordinary agent —
//! // enrolling like any other, and keeping the secret it is handed.
//! let agent_id = Uuid::new_v4();
//! core.register_local_agent(Arc::new(InServerAgent::new(agent_id)));
//! let enrollment = core.announce(in_server_announcement(
//!     agent_id,
//!     "task-server",
//!     None,
//!     vec![server_volume("primary", "Server primary", &data_root.join("files-volumes/primary"))],
//! ))?;
//! let _agent_token = enrollment.token.expect("first enrollment mints the secret");
//!
//! // Operator lane on the server router (authorized; `new_local_trusted`
//! // for the in-process transport); org lane on each org's.
//! let admin = StorageAdminBackend::new_local_trusted(core.clone());
//! let agents = StorageAgentBackend::new(core.clone());
//! let org = StorageBackend::new(core.clone(), "acme");
//! # let _ = (admin, agents, org);
//! # Ok(())
//! # }
//! ```

pub mod admin;
pub mod agent;
pub mod agent_lane;
pub mod core;
pub mod error;
pub mod org;
mod state;

pub use admin::{AuthorizeFuture, LocalTrusted, OperatorAuth, StorageAdminBackend};
pub use agent::{InServerAgent, LocalAgent, Measured};
pub use agent_lane::StorageAgentBackend;
pub use core::StorageCore;
pub use error::Result;
pub use org::StorageBackend;

pub use files_storage_proto as proto;
pub use files_storage_proto::{
    AgentAnnouncement, AgentCredential, AgentDirective, AgentEnrollment, AgentHosting, AgentInfo,
    AgentStatus, AnnouncedVolume, BlobReplica, CapabilityClass, ConfinedPath, DirectiveKind,
    DirectiveOutcome, GrantSpec, GrantUsage, LiveTreeBinding, LocationHealth, LocationKind,
    PlacementStatus, RootPlacement, StorageError, StorageEvent, StorageGrantInfo,
    StorageLocationInfo, VolumeHealth,
};

// architect-emitted vox bits, re-exported so mount sites need only this
// crate: the operator lane onto the server router, the org lane onto each
// org router, the agent lane wherever agents connect.
pub use files_storage_proto::{
    StorageAdminServiceClient, StorageAgentServiceClient, StorageServiceClient,
    serve_storage_admin, serve_storage_agent, serve_storage_service, storage_admin_descriptor,
    storage_admin_layer, storage_agent_descriptor, storage_agent_layer, storage_service_descriptor,
    storage_service_layer,
};
pub use files_storage_proto::{
    StorageAgentServiceStreamClient, StorageServiceStreamClient, serve_storage_agent_stream,
    serve_storage_service_stream, storage_agent_stream_descriptor, storage_agent_stream_layer,
    storage_service_stream_layer, storage_stream_descriptor,
};
