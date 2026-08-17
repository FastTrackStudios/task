//! [`Translation`] — edition metadata + licensing posture.
//!
//! The bundle-vs-fetch decision from the bible-study design lives here in
//! the type system: public-domain / open texts are
//! [`Availability::Bundled`] and ship with the app; copyright-restricted
//! texts (NIV, ESV, …) are [`Availability::Api`] and are fetched
//! per-passage with the user's key, never persisted as redistributable
//! files. Notes anchor to [`crate::VerseId`], not to translation text, so
//! the vault stays shareable regardless of which editions are installed.

use serde::{Deserialize, Serialize};

/// How a translation's text reaches the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Availability {
    /// Public-domain or openly licensed — shipped offline with the app.
    Bundled,
    /// Copyright-restricted — fetched per-passage via an external API
    /// with the user's key; cached within license limits, never bundled.
    Api,
}

/// Static metadata for one Bible edition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Translation {
    /// Short id / abbreviation, e.g. `WEB`, `NIV`.
    pub id: &'static str,
    /// Full edition name.
    pub name: &'static str,
    /// Short license summary.
    pub license: &'static str,
    pub availability: Availability,
}

impl Translation {
    /// Is this edition bundled offline?
    #[must_use]
    pub fn is_bundled(&self) -> bool {
        matches!(self.availability, Availability::Bundled)
    }

    /// Look up a known translation by id (case-insensitive).
    #[must_use]
    pub fn lookup(id: &str) -> Option<&'static Self> {
        KNOWN.iter().find(|t| t.id.eq_ignore_ascii_case(id.trim()))
    }
}

/// Editions the feature knows about. Bundled ones land first (slice 1
/// ships WEB; BSB/KJV follow); the API-only rows document the licensing
/// path for the copyright-restricted editions the user wants to read.
pub const KNOWN: &[Translation] = &[
    Translation {
        id: "WEB",
        name: "World English Bible",
        license: "Public domain",
        availability: Availability::Bundled,
    },
    Translation {
        id: "BSB",
        name: "Berean Standard Bible",
        license: "CC0 (public domain dedication)",
        availability: Availability::Bundled,
    },
    Translation {
        id: "KJV",
        name: "King James Version",
        license: "Public domain (UK: Crown copyright)",
        availability: Availability::Bundled,
    },
    Translation {
        id: "NIV",
        name: "New International Version",
        license: "© Biblica — fetched via API, never bundled",
        availability: Availability::Api,
    },
    Translation {
        id: "ESV",
        name: "English Standard Version",
        license: "© Crossway — fetched via ESV API, never bundled",
        availability: Availability::Api,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_is_bundled_niv_is_api() {
        assert!(Translation::lookup("web").unwrap().is_bundled());
        assert!(Translation::lookup("WEB").unwrap().is_bundled());
        let niv = Translation::lookup("NIV").unwrap();
        assert_eq!(niv.availability, Availability::Api);
        assert!(!niv.is_bundled());
    }

    #[test]
    fn unknown_translation_is_none() {
        assert!(Translation::lookup("XYZ").is_none());
    }
}
