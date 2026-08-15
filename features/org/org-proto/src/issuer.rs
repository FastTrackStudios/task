//! Billing identity for the org — the "From" block on
//! invoices and other outward financial documents.
//!
//! Stored as `<org>/issuer.toml` beside `org.toml` (same
//! portability/human-readability rationale; see the manifest
//! module). Kept out of [`crate::OrgManifest`] because the
//! manifest is the *federated identity record* — slug,
//! display name, federation URL — while the issuer block is
//! private billing detail (postal address, tax id) that
//! should never travel over federation.
//!
//! All fields are optional in the file; missing ones default
//! to empty strings so a minimal `name = "…"` file works.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// `<org>/issuer.toml` contents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IssuerProfile {
    /// Display name on the invoice "From" block — a person
    /// or business name.
    pub name: String,
    /// Postal address, newline-separated.
    pub address: String,
    pub email: String,
    pub phone: String,
    /// VAT / EIN / ABN — whatever the jurisdiction wants
    /// printed on invoices.
    pub tax_id: String,
}

/// Errors loading `issuer.toml`.
#[derive(Debug, thiserror::Error)]
pub enum IssuerError {
    #[error("read issuer.toml: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse issuer.toml: {0}")]
    Parse(#[from] toml::de::Error),
}

impl IssuerProfile {
    /// Load from `path`. `Ok(None)` when the file doesn't
    /// exist — callers fall back to env vars / placeholders.
    pub fn load(path: &Path) -> Result<Option<Self>, IssuerError> {
        match std::fs::read_to_string(path) {
            Ok(raw) => Ok(Some(toml::from_str(&raw)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Serialize and write to `path`.
    pub fn save(&self, path: &Path) -> Result<(), IssuerError> {
        let raw = toml::to_string_pretty(self).expect("issuer profile serializes");
        std::fs::write(path, raw)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_none() {
        let dir = std::env::temp_dir().join("issuer-test-none");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            IssuerProfile::load(&dir.join("issuer.toml"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join("issuer-test-rt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("issuer.toml");
        let p = IssuerProfile {
            name: "Cody Wright".into(),
            email: "cody@example.com".into(),
            ..Default::default()
        };
        p.save(&path).unwrap();
        assert_eq!(IssuerProfile::load(&path).unwrap(), Some(p));
    }

    #[test]
    fn partial_file_defaults_rest() {
        let p: IssuerProfile = toml::from_str("name = \"X\"").unwrap();
        assert_eq!(p.name, "X");
        assert_eq!(p.tax_id, "");
    }
}
