//! Typed paths.
//!
//! Paths were `String` everywhere, and each of the ~12 methods taking one
//! re-implemented its own `..`-escape check. A missed check is a path
//! traversal out of a root, so the check belongs in one place: the
//! constructor.
//!
//! ## What these types do and do not guarantee
//!
//! [`RootPath::parse`] and [`TreePath::parse`] normalise and reject —
//! absolute paths, `..` components, empty components, NUL bytes. Holding
//! one means the value passed that check.
//!
//! They are **transparent on the wire**, so a hostile peer can still send
//! any string and `Deserialize` will build the newtype without calling
//! `parse`. A backend must therefore re-validate what arrives from the
//! network, via [`RootPath::validate`]. What the type buys is that the
//! rule lives in one place and every in-process construction is checked;
//! it is not a substitute for validating untrusted input at the edge.

use std::fmt;

use facet::Facet;
use thiserror::Error;

/// Why a path was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(u8)]
pub enum PathError {
    #[error("path is absolute: {0}")]
    Absolute(String),
    #[error("path escapes its root: {0}")]
    Escapes(String),
    #[error("path has an empty component: {0}")]
    EmptyComponent(String),
    #[error("path contains a NUL byte")]
    Nul,
}

/// Normalise and validate, returning the cleaned relative path.
fn clean(raw: &str) -> Result<String, PathError> {
    if raw.contains('\0') {
        return Err(PathError::Nul);
    }
    let unified = raw.replace('\\', "/");
    let trimmed = unified.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if unified.starts_with('/') && raw.trim() != unified.trim() {
        return Err(PathError::Absolute(raw.to_string()));
    }

    let mut out: Vec<&str> = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "" => return Err(PathError::EmptyComponent(raw.to_string())),
            "." => continue,
            ".." => return Err(PathError::Escapes(raw.to_string())),
            other => out.push(other),
        }
    }
    Ok(out.join("/"))
}

macro_rules! rel_path {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Facet, )]
        // The same statement to facet, which is what vox actually
        // encodes with. Without it the two disagree: serde sees a
        // string, facet sees a one-field tuple.
        #[facet(transparent)]
        #[repr(C)]
        pub struct $name(String);

        impl $name {
            /// Normalise and validate. The only checked constructor.
            pub fn parse(raw: impl AsRef<str>) -> Result<Self, PathError> {
                clean(raw.as_ref()).map(Self)
            }

            /// The root itself — the empty path.
            #[must_use]
            pub fn root() -> Self {
                Self(String::new())
            }

            /// Re-check a value that arrived over the wire, where
            /// `Deserialize` bypassed [`Self::parse`].
            pub fn validate(&self) -> Result<Self, PathError> {
                Self::parse(&self.0)
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn is_root(&self) -> bool {
                self.0.is_empty()
            }

            /// Components, outermost first. Empty for the root.
            pub fn components(&self) -> impl DoubleEndedIterator<Item = &str> {
                self.0.split('/').filter(|c| !c.is_empty())
            }

            /// The final component, or `None` at the root.
            #[must_use]
            pub fn name(&self) -> Option<&str> {
                self.components().next_back()
            }

            /// The containing path, or `None` at the root.
            #[must_use]
            pub fn parent(&self) -> Option<Self> {
                let idx = self.0.rfind('/')?;
                Some(Self(self.0[..idx].to_string()))
            }

            /// Append a single component. Fails if it is not one.
            pub fn join(&self, component: impl AsRef<str>) -> Result<Self, PathError> {
                let child = Self::parse(component)?;
                if self.is_root() {
                    return Ok(child);
                }
                if child.is_root() {
                    return Ok(self.clone());
                }
                Ok(Self(format!("{}/{}", self.0, child.0)))
            }

            /// Whether `self` is `other` or lies beneath it.
            #[must_use]
            pub fn is_within(&self, other: &Self) -> bool {
                other.is_root()
                    || self.0 == other.0
                    || self.0.starts_with(&format!("{}/", other.0))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = PathError;
            fn try_from(raw: String) -> Result<Self, PathError> {
                Self::parse(raw)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = PathError;
            fn try_from(raw: &str) -> Result<Self, PathError> {
                Self::parse(raw)
            }
        }
    };
}

rel_path!(
    /// A path inside one File Root's live tree, relative to the root.
    ///
    /// Confinement is the point: a `RootPath` cannot name anything
    /// outside the root it is used with.
    RootPath
);

rel_path!(
    /// A path in the org tree — the unified namespace over Projects,
    /// Vault, Wiki and Assets that backs both the explorer and the
    /// WebDAV mount.
    ///
    /// The same resolver serves both, so the app and a mounted share
    /// always show one tree.
    TreePath
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal() {
        assert!(matches!(
            RootPath::parse("a/../../etc/passwd"),
            Err(PathError::Escapes(_))
        ));
        assert!(matches!(
            RootPath::parse("../x"),
            Err(PathError::Escapes(_))
        ));
    }

    #[test]
    fn rejects_nul() {
        assert!(matches!(RootPath::parse("a\0b"), Err(PathError::Nul)));
    }

    #[test]
    fn normalises() {
        assert_eq!(RootPath::parse("/a/b/").unwrap().as_str(), "a/b");
        assert_eq!(RootPath::parse("a/./b").unwrap().as_str(), "a/b");
        assert_eq!(RootPath::parse("").unwrap().as_str(), "");
        assert_eq!(RootPath::parse("a\\b").unwrap().as_str(), "a/b");
    }

    #[test]
    fn walks() {
        let p = RootPath::parse("Sessions/Song/Audio Files").unwrap();
        assert_eq!(p.name(), Some("Audio Files"));
        assert_eq!(p.parent().unwrap().as_str(), "Sessions/Song");
        assert!(p.is_within(&RootPath::parse("Sessions").unwrap()));
        assert!(p.is_within(&RootPath::root()));
        assert!(!p.is_within(&RootPath::parse("Renders").unwrap()));
    }

    #[test]
    fn joins_one_component() {
        let p = RootPath::parse("Sessions").unwrap();
        assert_eq!(p.join("Song").unwrap().as_str(), "Sessions/Song");
        assert!(p.join("../escape").is_err());
    }
}
