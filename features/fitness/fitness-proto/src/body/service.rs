//! `BodyService` — CRUD over metrics + a `log_entry`
//! convenience for the "weigh-in" flow.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::model::{BodyEntry, BodyMetric};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum BodyError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

#[architect::rpc]
pub trait BodyService {
    fn list(&self) -> Result<Vec<BodyMetric>, BodyError>;

    fn get(&self, id: &str) -> Result<BodyMetric, BodyError>;

    /// Look up a metric by its `kind` (e.g. `"weight"`,
    /// `"body-fat"`). Convenience for "log this morning's
    /// weight" flows.
    fn find_by_kind(&self, kind: &str) -> Result<BodyMetric, BodyError>;

    fn create(&self, metric: BodyMetric) -> Result<BodyMetric, BodyError>;

    fn update(&self, metric: BodyMetric) -> Result<BodyMetric, BodyError>;

    fn delete(&self, id: &str) -> Result<(), BodyError>;

    /// Append `entry` to the named metric's time series.
    /// Assigns `entry.id` if nil. Returns the updated
    /// metric.
    fn log_entry(&self, metric_id: &str, entry: BodyEntry) -> Result<BodyMetric, BodyError>;
}
