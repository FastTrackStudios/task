//! OS keyring resolver. Wraps the `keyring` crate so the rest
//! of the codebase stays platform-blind.

use crate::{SecretError, SecretValue};

pub fn get(service: &str, account: &str) -> Result<SecretValue, SecretError> {
    let entry =
        keyring::Entry::new(service, account).map_err(|e| SecretError::Keyring(e.to_string()))?;
    let secret = entry
        .get_password()
        .map_err(|e| SecretError::Keyring(e.to_string()))?;
    Ok(SecretValue::new(secret))
}
