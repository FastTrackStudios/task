//! The version this build of Task ships.
//!
//! A release used to write the tag's version into `[workspace.package]
//! version`. Cargo hashes a package's version into every unit it builds and
//! into every dependent's, so that changed the identity of all ~190
//! workspace crates and a release recompiled every one of them, however warm
//! its cache. Now the version reaches the build as an environment variable
//! instead (`release.sh stamp` exports `TASK_RELEASE_VERSION`), and Cargo
//! recompiles only the crates that read it, so only binaries read it:
//!
//! - each binary's `main` records it first thing:
//!   `task_release_version::set(task_release_version::of_this_build!())`,
//!   which expands inside the binary crate, so the variable is tracked there
//!   and nowhere else;
//! - everything else asks [`get`] at runtime.
//!
//! Without the variable (a dev build, `cargo test`, a Nix build) the version
//! is the committed `[workspace.package] version`, as it always was.
//!
//! `scripts/check-versions.sh` refuses `env!("CARGO_PKG_VERSION")` anywhere
//! but here, since a library reading it would report the committed version
//! in a release, and runs each release binary's `--version` against the tag.

use std::sync::OnceLock;

static VERSION: OnceLock<&'static str> = OnceLock::new();

/// The version this binary was built as: the release's (`TASK_RELEASE_VERSION`)
/// or else the committed workspace version. Expand it in a binary crate's
/// `main`, never in a library, which would recompile with every release.
#[macro_export]
macro_rules! of_this_build {
    () => {
        match ::core::option_env!("TASK_RELEASE_VERSION") {
            Some(version) => version,
            None => ::core::env!("CARGO_PKG_VERSION"),
        }
    };
}

/// Record the version this process is (from [`of_this_build!`]). The first
/// call wins; later ones are ignored.
pub fn set(version: &'static str) {
    let _ = VERSION.set(version);
}

/// The version this process is: what its binary recorded with [`set`], or
/// the committed workspace version when nothing did (tests, tools).
#[must_use]
pub fn get() -> &'static str {
    VERSION.get().copied().unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_recorded_version_is_the_one_reported() {
        assert_eq!(get(), env!("CARGO_PKG_VERSION"), "nothing recorded yet");
        set("26.10.9");
        set("1.0.0");
        assert_eq!(get(), "26.10.9");
    }
}
