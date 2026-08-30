// architect's rpc derives emit cfg-gated blocks; allow at crate scope
// (same convention as the sibling files crates).
#![allow(unexpected_cfgs)]

//! Files sync daemon (issue #265): the `files_sync` reconcile engine
//! shipped as a long-lived agent with a persistent **device identity**,
//! plus a local-socket control surface.
//!
//! - [`SyncDaemon`] — the core: device identity ([`identity`]), the set
//!   of roots chosen for sync (with selective-sync slices), a live
//!   per-file status store updated by the reconcile progress observer,
//!   pause/resume, and a `tick` that pulls every chosen root.
//! - [`DaemonControl`] — [`SyncDaemon`] as a
//!   [`service::DaemonControlService`] server: **status** (rich, with
//!   per-file progress), sync choices, pause/resume, hydrate,
//!   checkpoint-now, and a `status_events` stream. The desktop app and
//!   the CLI are both just clients of this — same surface either way.
//!
//! Device enrollment reuses the storage-agent protocol of issue #262:
//! the device id is the agent id, the secret is the enrollment token,
//! operator approval gates the device, and revocation (marking the
//! agent Rejected) cuts one device without touching others. See
//! [`identity`] for that mapping.
//!
//! The OS-socket binding and the desktop embed are thin adapters over
//! this crate — the tested core is the daemon + control service driven
//! over an in-process link, exactly the spec's RPC-seam testing rule.

mod control;
mod daemon;
mod error;
mod hub;
pub mod identity;
pub mod install;
pub mod mount;
pub mod peering;

// The wire contract lives in `files-daemon-proto` so a client — the
// desktop app, the CLI — can speak it without depending on the agent
// (jj-lib, a CAS store, an iroh endpoint) to hold the conversation.
// Re-exported under their old paths: one definition, and every caller
// here is unchanged.
pub use files_daemon_proto::{model, service};

/// Re-exported so the headless binary (and embedders) can name the
/// coordinator client type without a direct dependency.
pub use files_sync;

pub use control::DaemonControl;
pub use daemon::{EventHub, SyncDaemon};
pub use error::{DaemonError, Result};
pub use identity::DeviceIdentity;

// The architect-emitted vox client / layer / descriptor + the stream
// sibling — the app and CLI bind the client; a socket host mounts the
// layer.
pub use service::{
    DaemonControlService, DaemonControlServiceClient, DaemonControlServiceStream,
    DaemonControlServiceStreamClient, DaemonEvent,
};
