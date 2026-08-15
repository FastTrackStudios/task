//! Walk a `vault::Vault` and collect every page that looks
//! like a location. Pages without the discriminator are
//! skipped silently; parse failures are logged + skipped.

use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::Locations;
use crate::model::Location;

#[must_use]
pub fn scan_vault(vault: &Vault) -> Vec<Location> {
    VaultEntityStore::<Locations>::scan(vault)
}
