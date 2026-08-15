//! Walk the vault for intake logs.

use chrono::NaiveDate;
use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::IntakeLogs;
use crate::model::IntakeLog;

/// Every intake-log page, parse failures logged and skipped.
pub fn scan_vault(vault: &Vault) -> Vec<IntakeLog> {
    VaultEntityStore::<IntakeLogs>::scan(vault)
}

/// Convenience: log on a specific day. First match wins
/// (there's typically one log per day; multi-logs merge
/// at the next write).
pub fn for_day(vault: &Vault, day: NaiveDate) -> Option<IntakeLog> {
    scan_vault(vault).into_iter().find(|l| l.date == day)
}

/// Logs in `[start, end)`. Used by weekly + monthly
/// summary views and by (future) fitness goal tracking.
pub fn between(vault: &Vault, start: NaiveDate, end: NaiveDate) -> Vec<IntakeLog> {
    scan_vault(vault)
        .into_iter()
        .filter(|l| l.date >= start && l.date < end)
        .collect()
}
