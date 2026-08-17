//! `mount` — per-machine project content mount facade.
//!
//! - [`Filesystem`] is the canonical `mount_proto::ContentBackend`
//!   impl. Wraps a root [`std::path::PathBuf`] and serves
//!   reads/writes from it using ordinary `std::fs`.
//! - [`MountRegistry`] reads/writes
//!   `$XDG_CONFIG_HOME/task/mounts.toml` (override with
//!   `TASK_MOUNTS_TOML`). Holds the per-machine `project_id →
//!   Mount` map.
//!
//! Federated-platform Phase 2.

mod filesystem;
mod registry;

pub use filesystem::Filesystem;
pub use mount_proto::{BackendKind, ContentBackend, Entry, Mount, MountError};
pub use registry::{MountRegistry, RegistryError, default_mounts_path};
