//! Federated org-root resolver.
//!
//! An [`OrgRoot`] is one self-contained federated node on
//! disk: `<data_root>/orgs/<slug>/`. Inside it sit the org's
//! `org.toml` manifest, its `auth.sqlite` / `timer.sqlite` /
//! `finance.sqlite` databases, the vault, and any other
//! org-scoped state. See the federated-platform design
//! for the full federation model.
//!
//! This crate owns the **layout** — path resolvers, manifest
//! read/write, directory discovery — and nothing else. No
//! database, no server, no CLI. Downstream crates (server,
//! CLI) use [`OrgRoot`] to find where their files belong.

pub mod issuer;
pub mod manifest;
pub mod root;
#[cfg(feature = "vox")]
pub mod schema_stamp;
pub mod service;
pub mod snapshot;

pub use issuer::{IssuerError, IssuerProfile};
pub use manifest::{OrgManifest, ParseError};
pub use root::{
    DEFAULT_WIKI, DataRoot, OrgRoot, RootError, default_client_vault_root, wiki_slug,
};
pub use service::{
    CreateOrgRequest, OrgManagementError, OrgManagementService, OrgManagementServiceRpc,
};

#[cfg(feature = "vox")]
pub use service::{
    OrgManagementServiceClient, OrgManagementServiceRpcDispatcher as OrgManagementDispatcher,
    Service as OrgManagementServiceBridge, layer as org_management_layer,
    org_management_service_rpc_service_descriptor as org_management_descriptor,
    serve as serve_org_management,
};
pub use snapshot::{
    BranchResult, RepoResult, RestoreReport, SnapshotError, SnapshotLogEntry, SnapshotReport,
    SnapshotService,
};

#[cfg(feature = "vox")]
pub use snapshot::{
    SnapshotServiceClient, SnapshotServiceRpcDispatcher as SnapshotDispatcher,
    layer as snapshot_layer, serve as serve_snapshot,
    snapshot_service_rpc_service_descriptor as snapshot_descriptor,
};
