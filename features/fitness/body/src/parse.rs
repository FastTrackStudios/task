//! `vault::VaultPage` → `BodyMetric`.
//!
//! The field mapping lives in [`crate::entity`]; this module keeps the
//! historical `body::parse::*` paths working.

pub use vault_entity::ParseError;

use vault::VaultPage;
use vault_entity::store::VaultEntity;

use crate::entity::BodyMetrics;
use crate::model::BodyMetric;

/// True when `page` carries `type: body-metric` (or the tag).
pub fn looks_like_body_metric(page: &VaultPage) -> bool {
    BodyMetrics::matches(page)
}

/// Parse a body-metric page.
pub fn parse_page(page: &VaultPage) -> Result<BodyMetric, ParseError> {
    BodyMetrics::from_page(page)
}
