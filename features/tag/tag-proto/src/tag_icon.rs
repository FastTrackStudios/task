//! [`TagIcon`] — the icon a [`crate::Tag`] carries.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// The icon a [`crate::Tag`] carries — chosen once by the user when they
/// define the tag, not derived per use.
///
/// Open by design: pick a curated library icon by key, or paste raw SVG.
/// A free-form key (rather than a closed enum of variants) is what lets
/// the set grow and lets users bring their own icon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum TagIcon {
    /// A key into the supported icon set — a Lucide icon name the UI's
    /// render helper knows (e.g. `"utensils"`, `"dumbbell"`). Unknown
    /// keys fall back to a neutral icon. Expand the set by adding the key
    /// to the picker's curated list + an arm in the render helper; no
    /// proto change needed (the key is just a string).
    Named(String),
    /// Raw inline SVG markup — the "paste your own" path. Rendered inline
    /// by the UI, so any icon works without a library entry.
    Svg(String),
}

impl TagIcon {
    /// A named library icon by key.
    #[must_use]
    pub fn named(key: impl Into<String>) -> Self {
        Self::Named(key.into())
    }

    /// The key, if this is a [`TagIcon::Named`].
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        match self {
            Self::Named(k) => Some(k),
            Self::Svg(_) => None,
        }
    }
}

impl Default for TagIcon {
    /// The generic tag icon — a safe fallback before the user picks one.
    fn default() -> Self {
        Self::Named("tag".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::TagIcon;

    #[test]
    fn named_and_key_roundtrip() {
        let i = TagIcon::named("utensils");
        assert_eq!(i.key(), Some("utensils"));
        assert!(matches!(i, TagIcon::Named(_)));
    }

    #[test]
    fn svg_has_no_key() {
        assert_eq!(TagIcon::Svg("<svg/>".into()).key(), None);
    }

    #[test]
    fn json_roundtrip() {
        for icon in [TagIcon::named("heart"), TagIcon::Svg("<svg></svg>".into())] {
            let s = serde_json::to_string(&icon).unwrap();
            let back: TagIcon = serde_json::from_str(&s).unwrap();
            assert_eq!(icon, back);
        }
    }
}
