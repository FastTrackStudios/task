//! Walk a `vault::Vault` and collect every page that looks
//! like an inventory item.

use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::Items;
use crate::model::Item;

/// Every item page, parse failures logged and skipped.
#[must_use]
pub fn scan_vault(vault: &Vault) -> Vec<Item> {
    VaultEntityStore::<Items>::scan(vault)
}

/// Convenience: every item whose `location_id` matches.
#[must_use]
pub fn items_at(vault: &Vault, location_id: uuid::Uuid) -> Vec<Item> {
    scan_vault(vault)
        .into_iter()
        .filter(|i| i.location_id == Some(location_id))
        .collect()
}

/// Convenience: every item flagged for attention (poor /
/// broken / missing / in-repair).
#[must_use]
pub fn items_needing_attention(vault: &Vault) -> Vec<Item> {
    scan_vault(vault)
        .into_iter()
        .filter(|i| {
            let cond = crate::model::Condition::from_str(&i.condition);
            let stat = crate::model::Status::from_str(&i.status);
            cond.is_some_and(crate::model::Condition::needs_attention)
                || matches!(
                    stat,
                    Some(crate::model::Status::Missing | crate::model::Status::InRepair)
                )
        })
        .collect()
}
