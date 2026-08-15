//! Folder aliases. The UI sees a stable name; the backend sees
//! whatever the server actually called the mailbox.
//!
//! The classic case is Gmail's `[Gmail]/Sent Mail` — the UI shows
//! `"Sent"` regardless of which backend serves the account, and
//! the alias map handles the translation at the wire edge.
//!
//! Lookups are case-insensitive on both directions: an alias
//! recorded as `"Sent"` resolves a query for `"sent"`, `"SENT"`,
//! or `"Sent"`. We persist the alias as the user typed it (the
//! canonical-case version) but match leniently.

use facet::Facet;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// Wire-facing folder name (what the UI shows + what
/// `EmailSync::list_folders` returns). Newtype so we can't mix
/// it up with raw backend names at the type level once the UI
/// fully adopts this.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Facet)]
pub struct FolderName(pub String);

impl FolderName {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Case-insensitive alias → backend-name map. Internally a
/// `BTreeMap<String, String>` keyed by the lower-cased alias so
/// lookups are O(log n) and stable on iteration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Facet)]
pub struct FolderAliases {
    /// keyed by lower-case alias; value is `(canonical_alias, backend_name)`.
    inner: BTreeMap<String, (String, String)>,
}

impl FolderAliases {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert/replace an alias. `alias` is stored both as a key
    /// (lower-cased) and a value (canonical case) so iteration
    /// returns the spelling the user chose.
    pub fn insert(&mut self, alias: impl Into<String>, backend_name: impl Into<String>) {
        let alias = alias.into();
        self.inner
            .insert(alias.to_lowercase(), (alias, backend_name.into()));
    }

    /// Look up a backend name by alias. Case-insensitive.
    #[must_use]
    pub fn backend_for<'a>(&'a self, alias: &str) -> Option<&'a str> {
        self.inner
            .get(&alias.to_lowercase())
            .map(|(_, b)| b.as_str())
    }

    /// Reverse lookup — find the alias for a backend folder name.
    /// O(n); only used on the way up to the UI so the cost is
    /// rare. Returns the canonical (user-typed) spelling.
    #[must_use]
    pub fn alias_for(&self, backend_name: &str) -> Option<&str> {
        self.inner
            .values()
            .find(|(_, b)| b.eq_ignore_ascii_case(backend_name))
            .map(|(a, _)| a.as_str())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterate `(canonical_alias, backend_name)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.inner.values().map(|(a, b)| (a.as_str(), b.as_str()))
    }

    /// Resolve a UI-side folder name through the alias map; if no
    /// alias is registered, pass the name through unchanged so
    /// callers don't have to branch.
    #[must_use]
    pub fn resolve<'a>(&'a self, ui_name: &'a str) -> &'a str {
        self.backend_for(ui_name).unwrap_or(ui_name)
    }
}

/// Serialize as a simple `{alias: backend}` map — TOML-friendly,
/// JSON-friendly. We don't expose the internal lower-cased key.
impl Serialize for FolderAliases {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut m = ser.serialize_map(Some(self.inner.len()))?;
        for (canonical, backend) in self.inner.values() {
            m.serialize_entry(canonical, backend)?;
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for FolderAliases {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw: BTreeMap<String, String> = BTreeMap::deserialize(de)?;
        let mut out = FolderAliases::new();
        for (alias, backend) in raw {
            out.insert(alias, backend);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_insensitive_lookup() {
        let mut a = FolderAliases::new();
        a.insert("Sent", "[Gmail]/Sent Mail");
        assert_eq!(a.backend_for("sent"), Some("[Gmail]/Sent Mail"));
        assert_eq!(a.backend_for("SENT"), Some("[Gmail]/Sent Mail"));
        assert_eq!(a.backend_for("Sent"), Some("[Gmail]/Sent Mail"));
    }

    #[test]
    fn passthrough_when_unaliased() {
        let a = FolderAliases::new();
        assert_eq!(a.resolve("INBOX"), "INBOX");
    }

    #[test]
    fn resolve_translates_when_aliased() {
        let mut a = FolderAliases::new();
        a.insert("Trash", "[Gmail]/Bin");
        assert_eq!(a.resolve("Trash"), "[Gmail]/Bin");
        assert_eq!(a.resolve("Inbox"), "Inbox"); // unaliased
    }

    #[test]
    fn reverse_lookup_finds_alias() {
        let mut a = FolderAliases::new();
        a.insert("Sent", "[Gmail]/Sent Mail");
        assert_eq!(a.alias_for("[Gmail]/Sent Mail"), Some("Sent"));
        assert_eq!(a.alias_for("[gmail]/sent mail"), Some("Sent"));
    }

    #[test]
    fn iter_returns_canonical_case() {
        let mut a = FolderAliases::new();
        a.insert("Sent", "X");
        a.insert("Drafts", "Y");
        let pairs: Vec<_> = a.iter().collect();
        assert!(pairs.contains(&("Sent", "X")));
        assert!(pairs.contains(&("Drafts", "Y")));
    }

    #[test]
    fn serde_roundtrips_through_btreemap() {
        let mut a = FolderAliases::new();
        a.insert("Sent", "[Gmail]/Sent Mail");
        a.insert("Trash", "[Gmail]/Bin");
        let json = serde_json::to_string(&a).unwrap();
        let b: FolderAliases = serde_json::from_str(&json).unwrap();
        assert_eq!(a.backend_for("sent"), b.backend_for("sent"));
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn later_insert_replaces_earlier() {
        let mut a = FolderAliases::new();
        a.insert("Sent", "first");
        a.insert("SENT", "second");
        assert_eq!(a.backend_for("sent"), Some("second"));
        // The canonical form follows the latest insert.
        assert_eq!(a.iter().next().unwrap().0, "SENT");
    }
}
