//! Walk a `vault::Vault` and collect every page that looks
//! like a pantry item.

use chrono::NaiveDate;
use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::PantryItems;
use crate::model::PantryItem;

/// Every pantry page, parse failures logged and skipped.
#[must_use]
pub fn scan_vault(vault: &Vault) -> Vec<PantryItem> {
    VaultEntityStore::<PantryItems>::scan(vault)
}

/// Convenience: every pantry item past its printed expiry as
/// of `today`.
#[must_use]
pub fn expired(vault: &Vault, today: NaiveDate) -> Vec<PantryItem> {
    scan_vault(vault)
        .into_iter()
        .filter(|i| i.is_expired(today))
        .collect()
}

/// Convenience: every pantry item at or below its
/// `minimum` reorder threshold.
#[must_use]
pub fn low_stock(vault: &Vault) -> Vec<PantryItem> {
    scan_vault(vault)
        .into_iter()
        .filter(super::model::PantryItem::is_low)
        .collect()
}

/// Stock entries (paired with their parent item) whose
/// `best_before` falls within `[today, today + days)`.
/// Drives the "expiring this week" surface — wires into
/// shopping-list auto-populate in phase 7.
#[must_use]
pub fn expiring_within(
    vault: &Vault,
    today: NaiveDate,
    days: u32,
) -> Vec<(PantryItem, crate::model::StockEntry)> {
    let horizon = today
        .checked_add_days(chrono::Days::new(u64::from(days)))
        .unwrap_or(NaiveDate::MAX);
    let mut out = Vec::new();
    for item in scan_vault(vault) {
        for entry in item.stock_entries.iter() {
            if let Some(bb) = entry.best_before {
                if bb >= today && bb < horizon {
                    out.push((item.clone(), entry.clone()));
                }
            }
        }
    }
    out
}
