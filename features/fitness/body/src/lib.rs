// architect's `Entity` derive emits cfg-gated blocks the
// host crate doesn't always re-export (`server-seaorm`,
// other internal markers). Allow at crate scope to keep
// the migration uniform with other architect-derived
// features.
#![allow(unexpected_cfgs)]

//! `body` — body-metric time series in a `vault::Vault`.
//!
//! One page per metric
//! (`Projects/Fitness/body/weight.md`, `.../bodyfat.md`,
//! `.../waist.md`) carrying the entry log. Modeled on
//! wger's `Measurement` + `Category` tables, flattened so a
//! year of daily weigh-ins doesn't explode into 365 files.
//!
//! Surface:
//! - [`BodyMetric`] / [`BodyEntry`] / [`MetricKind`]
//! - [`parse_page`] / [`looks_like_body_metric`]
//! - [`serialize_metric`] / [`write_metric`]
//! - [`scan_vault`] / [`by_kind`]
//! - [`BodyService`] — `#[architect::rpc]` trait
//! - [`Store`] — file-backed impl

#![cfg(not(target_arch = "wasm32"))]

pub mod entity;
pub mod model;
pub mod parse;
pub mod scan;
pub mod service;
pub mod store;
pub mod write;

pub use entity::BodyMetrics;
pub use model::{BodyEntry, BodyMetric, MetricKind};
pub use parse::{ParseError, looks_like_body_metric, parse_page};
pub use scan::{by_kind, scan_vault};
pub use service::{BodyError, BodyService};

#[cfg(feature = "vox")]
pub use fitness_proto::body::{
    BodyDispatcher, BodyServiceBridge, BodyServiceClient, body_service_descriptor,
    body_service_layer, serve_body_service,
};
#[cfg(feature = "vox")]
pub use service::{BodyServiceRpc, BodyServiceRpcDispatcher};
pub use store::Store;
pub use write::{WriteError, default_metric_path, serialize_metric, write_metric};
