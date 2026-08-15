//! Walk a `vault::Vault` and collect every page that
//! looks like an exercise.

use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::Exercises;
use crate::model::Exercise;

/// Every exercise page, parse failures logged and skipped.
pub fn scan_vault(vault: &Vault) -> Vec<Exercise> {
    VaultEntityStore::<Exercises>::scan(vault)
}

/// Convenience: every exercise in a given category.
pub fn by_category(vault: &Vault, category: &str) -> Vec<Exercise> {
    let needle = category.to_ascii_lowercase();
    scan_vault(vault)
        .into_iter()
        .filter(|e| e.category.eq_ignore_ascii_case(&needle))
        .collect()
}

/// Convenience: every exercise that uses any of `equipment`.
/// Empty `equipment` returns the full list (no filter).
pub fn by_equipment(vault: &Vault, equipment: &[String]) -> Vec<Exercise> {
    if equipment.is_empty() {
        return scan_vault(vault);
    }
    let needles: Vec<String> = equipment.iter().map(|e| e.to_ascii_lowercase()).collect();
    scan_vault(vault)
        .into_iter()
        .filter(|ex| {
            ex.equipment.iter().any(|have| {
                let have = have.to_ascii_lowercase();
                needles.iter().any(|n| have.contains(n))
            })
        })
        .collect()
}
