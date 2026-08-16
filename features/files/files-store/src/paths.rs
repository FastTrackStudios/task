//! Filesystem-path confinement — the Files platform's one implementation
//! of "a caller-supplied path may not leave the boundary it was given".
//!
//! Every Files surface that accepts a path from outside applies this:
//! `files`' root browsing and root creation (confined to the org's own
//! area), and `files-storage`'s placement (confined to a Storage grant's
//! path prefix). It lives here because it was written three times
//! otherwise, and a hardening fix applied to one copy would leave the
//! others escapable (PR #284 review).
//!
//! Three layers, each closing a hole the others cannot:
//!
//! 1. [`safe_relative`] — a *textual* gate on the requested path, run
//!    before anything is created. Rejects absolute paths (`PathBuf::join`
//!    with an absolute argument silently replaces the base), `..`, and
//!    empty results.
//! 2. [`create_confined`] — creates the path one component at a time,
//!    **refusing to traverse an existing symlink**. This is the layer
//!    that has to run *before* any write: a check performed after
//!    creation can only report an escape that already happened.
//! 3. [`confine`] — a canonicalize-then-prefix check on an existing
//!    path, for read paths (where nothing is created) and as a
//!    post-condition assertion.

use std::io;
use std::path::{Component, Path, PathBuf};

/// Why a path was refused. Callers map this onto their own error type;
/// the `Display` text is safe to surface.
#[derive(Debug)]
pub enum PathError {
    /// The requested path is malformed or escapes textually.
    Rejected(String),
    /// The resolved path is outside the boundary.
    Escapes { path: PathBuf, boundary: PathBuf },
    /// Underlying filesystem failure.
    Io(io::Error),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(m) => write!(f, "{m}"),
            Self::Escapes { path, boundary } => write!(
                f,
                "{}: outside the permitted boundary ({})",
                path.display(),
                boundary.display()
            ),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PathError {}

impl From<io::Error> for PathError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, PathError>;

/// Validate a caller-supplied relative path and return it normalized.
/// Accepts ordinary nested paths (`clients/acme/mix-session`); rejects
/// anything that could resolve outside the boundary it will be joined
/// to.
pub fn safe_relative(requested: &str) -> Result<PathBuf> {
    let trimmed = requested.trim();
    if trimmed.is_empty() {
        return Err(PathError::Rejected("path is empty".into()));
    }
    let mut normalized = PathBuf::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(PathError::Rejected(format!(
                    "{requested}: `..` may not appear in a confined path"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(PathError::Rejected(format!(
                    "{requested}: must be relative to the boundary"
                )));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(PathError::Rejected(format!(
            "{requested}: resolves to the boundary itself"
        )));
    }
    Ok(normalized)
}

/// Canonicalize `target` and confirm it resolves inside `boundary`.
/// Both must already exist — this is the read-path check, and the
/// post-condition assertion for a path something else created.
pub fn confine(target: &Path, boundary: &Path) -> Result<PathBuf> {
    let boundary = boundary.canonicalize()?;
    let canonical = target.canonicalize()?;
    if canonical != boundary && !canonical.starts_with(&boundary) {
        return Err(PathError::Escapes {
            path: target.to_path_buf(),
            boundary,
        });
    }
    Ok(canonical)
}

/// Create `boundary/relative` — creating `boundary` itself if needed —
/// component by component, refusing to traverse anything that is not a
/// real directory.
///
/// This is the layer that makes confinement a *precondition* rather than
/// a postmortem. `create_dir_all` follows symlinks, so a link planted
/// inside the boundary (by an earlier legitimate write, or by another
/// tenant on a shared volume) is enough to place a whole directory tree
/// outside it; by the time a canonicalize-based check notices, the
/// directories — and whatever was initialized in them — already exist.
/// Here every intermediate component is checked with
/// [`std::fs::symlink_metadata`] (which does NOT follow links) before
/// being descended into, so an escape is refused before the first
/// `mkdir`.
///
/// Returns the canonicalized created path, which is re-checked against
/// the boundary as a belt-and-braces post-condition.
pub fn create_confined(boundary: &Path, relative: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(boundary)?;
    let mut current = boundary.canonicalize()?;

    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(PathError::Rejected(format!(
                "{}: only plain path components may be created under a boundary",
                relative.display()
            )));
        };
        let next = current.join(part);
        match std::fs::symlink_metadata(&next) {
            Ok(meta) if meta.is_dir() => {}
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(PathError::Rejected(format!(
                    "{}: refusing to follow a symlink inside the boundary",
                    next.display()
                )));
            }
            Ok(_) => {
                return Err(PathError::Rejected(format!(
                    "{}: exists and is not a directory",
                    next.display()
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => std::fs::create_dir(&next)?,
            Err(e) => return Err(PathError::Io(e)),
        }
        current = next;
    }
    // Post-condition: whatever we just built must still resolve inside.
    confine(&current, boundary)
}

/// A path as a UTF-8 string — every Files wire type carries paths as
/// `String`.
pub fn to_utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| PathError::Rejected(format!("{}: path is not valid UTF-8", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textual_gate_refuses_escapes() {
        assert!(safe_relative("").is_err());
        assert!(safe_relative("   ").is_err());
        assert!(safe_relative("/etc").is_err());
        assert!(safe_relative("../up").is_err());
        assert!(safe_relative("a/../../up").is_err());
        assert!(safe_relative(".").is_err());
        assert_eq!(safe_relative("a/./b").unwrap(), PathBuf::from("a/b"));
    }

    /// The regression this module exists for: a symlink inside the
    /// boundary must be refused BEFORE anything is created through it.
    #[test]
    #[cfg(unix)]
    fn create_confined_refuses_a_symlink_before_creating_anything() {
        let dir = tempfile::tempdir().unwrap();
        let boundary = dir.path().join("boundary");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&boundary).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, boundary.join("link")).unwrap();

        let err = create_confined(&boundary, Path::new("link/newroot")).unwrap_err();
        assert!(matches!(err, PathError::Rejected(_)), "{err:?}");
        assert!(
            !outside.join("newroot").exists(),
            "nothing may be created through the symlink"
        );
    }

    #[test]
    fn create_confined_builds_nested_paths_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let boundary = dir.path().join("boundary");
        let made = create_confined(&boundary, Path::new("a/b/c")).unwrap();
        assert!(made.is_dir());
        assert_eq!(
            made,
            create_confined(&boundary, Path::new("a/b/c")).unwrap()
        );
        assert!(made.starts_with(boundary.canonicalize().unwrap()));
    }

    #[test]
    fn create_confined_refuses_a_file_in_the_way() {
        let dir = tempfile::tempdir().unwrap();
        let boundary = dir.path().join("boundary");
        std::fs::create_dir_all(&boundary).unwrap();
        std::fs::write(boundary.join("a"), b"not a dir").unwrap();
        assert!(create_confined(&boundary, Path::new("a/b")).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn confine_catches_a_symlink_that_already_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let boundary = dir.path().join("boundary");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&boundary).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, boundary.join("link")).unwrap();
        assert!(confine(&boundary.join("link"), &boundary).is_err());
        assert!(confine(&boundary, &boundary).is_ok());
    }
}
