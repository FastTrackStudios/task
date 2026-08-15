//! Walk the vault for body metrics.

use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::BodyMetrics;
use crate::model::BodyMetric;

/// Every body-metric page, parse failures logged and skipped.
pub fn scan_vault(vault: &Vault) -> Vec<BodyMetric> {
    VaultEntityStore::<BodyMetrics>::scan(vault)
}

/// Convenience: metric whose `kind` matches (case-insensitive).
/// First match wins; typically there's one page per kind.
pub fn by_kind(vault: &Vault, kind: &str) -> Option<BodyMetric> {
    let needle = kind.to_ascii_lowercase();
    scan_vault(vault)
        .into_iter()
        .find(|m| m.kind.eq_ignore_ascii_case(&needle))
}
