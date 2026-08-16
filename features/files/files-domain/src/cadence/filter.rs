//! What the engine needs to know about a path, and nothing more.
//!
//! The engine used to take a `&GitIgnoreFile` and a `RootFlavor` and call
//! `ignore::is_ignored` / `ignore::is_project_file` on them — two pure
//! predicates, reached through jj-lib and a glob table. That put jj's
//! gitignore machinery in the middle of a state machine about *time*.
//!
//! The engine asks two questions. This is those two questions. The
//! `files` crate answers them with the real Ignore set; a test answers
//! them with three lines.

/// The engine's view of a path.
pub trait ActivityFilter {
    /// Whether the Ignore set covers this path.
    ///
    /// Dropped before it can open a session, which is the whole point of
    /// the set: a `.rpp-bak` storm is not a working session.
    fn is_ignored(&self, rel_path: &str) -> bool;

    /// Whether this path is the project document itself — the `.rpp`,
    /// the `.ptx` — rather than something beside it. A surviving hint on
    /// one of these marks a save point.
    fn is_project_file(&self, rel_path: &str) -> bool;
}

/// Ignores nothing and recognises nothing. Every hint survives, no hint
/// marks a save point.
#[derive(Debug, Clone, Copy, Default)]
pub struct PassThrough;

impl ActivityFilter for PassThrough {
    fn is_ignored(&self, _rel_path: &str) -> bool {
        false
    }
    fn is_project_file(&self, _rel_path: &str) -> bool {
        false
    }
}

/// Suffix matching, for tests and for callers with no Ignore set to
/// hand.
#[derive(Debug, Clone, Default)]
pub struct SuffixFilter {
    pub ignored: Vec<String>,
    pub project: Vec<String>,
}

impl SuffixFilter {
    #[must_use]
    pub fn new(
        ignored: impl IntoIterator<Item = &'static str>,
        project: impl IntoIterator<Item = &'static str>,
    ) -> Self {
        Self {
            ignored: ignored.into_iter().map(str::to_ascii_lowercase).collect(),
            project: project.into_iter().map(str::to_ascii_lowercase).collect(),
        }
    }
}

impl ActivityFilter for SuffixFilter {
    fn is_ignored(&self, rel_path: &str) -> bool {
        let p = rel_path.to_ascii_lowercase();
        self.ignored.iter().any(|s| p.ends_with(s))
    }

    fn is_project_file(&self, rel_path: &str) -> bool {
        let p = rel_path.to_ascii_lowercase();
        self.project.iter().any(|s| p.ends_with(s))
    }
}

impl<T: ActivityFilter + ?Sized> ActivityFilter for &T {
    fn is_ignored(&self, rel_path: &str) -> bool {
        (**self).is_ignored(rel_path)
    }
    fn is_project_file(&self, rel_path: &str) -> bool {
        (**self).is_project_file(rel_path)
    }
}
