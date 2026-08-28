//! The sync agent's control surface, as a **wire contract** — what a
//! client needs and nothing else.
//!
//! Split out of `files-daemon` for the reason every `*-proto` in this
//! repository exists: the daemon is the whole Files backend (jj-lib, a
//! content-addressed store, an iroh endpoint), and the desktop app wants
//! to ask it a question. Depending on the implementation to hold a
//! conversation with it would put that entire tree into an application
//! that renders a status panel — and into `crates/ui`, which also builds
//! for wasm, where none of it can go at all.
//!
//! So the types here are serde/facet and `#[architect::rpc]` and nothing
//! more. `files-daemon` re-exports every one of them, so the daemon side
//! is unchanged and there is one definition rather than two.

pub mod model;
pub mod service;

pub use model::{DaemonStatus, FileProgress, RootStatus, RootSyncState};
pub use service::{
    DaemonControlService, DaemonControlServiceClient, DaemonControlServiceStream, DaemonError,
    DaemonEvent, Pulled,
};
