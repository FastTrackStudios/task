//! The per-root **Ignore set** (glossary: "the per-root list of
//! patterns that are neither versioned nor synced (backup files, peak
//! caches). Seeded by root flavor, edited per root; versioning and
//! selective sync share it").
//!
//! Three layers, matched as one, all in gitignore syntax so a single
//! matcher serves them:
//!
//! 1. A **flavor seed** — patterns every root of that flavor starts
//!    with. [`RootFlavor::Software`]'s seed (issue #273) is build
//!    scaffolding plus the heavy media formats that belong in a media
//!    root rather than a git object database. [`RootFlavor::Media`]'s
//!    (issue #260) is the DAW backup storm and the peak/analysis caches
//!    every editor regenerates on demand — versioning those would fill
//!    the store with scaffolding that is, by construction, derivable
//!    from the work.
//! 2. The root's **stored patterns** — the editable half, round-tripped
//!    over `ignore_set` / `set_ignore_set` and persisted in the root's
//!    own store directory (see [`stored_patterns`]). They live *with the
//!    root* rather than in the server's registry precisely because
//!    selective sync must read the same list on a replica that never saw
//!    that registry — "shared verbatim" is a storage decision, not just
//!    an API one.
//! 3. The root's own **`.gitignore` files**, honored on software roots
//!    only ([`honors_gitignore`]). This is what keeps the flavor's
//!    promise that git tooling and Files agree on what is content: a
//!    repo's committed `.gitignore` is already the project's own
//!    statement of what is scaffolding, and Files versioning the files
//!    git deliberately ignores would make `git status` a lie.
//!
//! Matching is jj-lib's [`GitIgnoreFile`] — real gitignore semantics
//! (negation, anchoring, directory patterns, per-directory chaining),
//! the same matcher jj's own working-copy snapshotting uses.
//!
//! **Ignored never means deleted.** An ignored path that is *already
//! tracked* in the root's history stays tracked — git's own rule (ignore
//! applies to untracked files) and the only safe one here: a checkpoint
//! must never record a deletion just because a pattern started matching.
//! [`crate::checkpoint`] enforces that; see [`crate::scan::LiveFile`].

use std::path::Path;
use std::sync::{Arc, LazyLock};

use files_proto::RootFlavor;
use globset::{Glob, GlobSet, GlobSetBuilder};
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::repo_path::RepoPath;

use crate::error::{Error, Result};

/// Software roots: build scaffolding + the heavy formats ADR 0001 says
/// belong in a media root. Deliberately narrow — a repo's own
/// `.gitignore` (honored on top of this) is the authority on anything
/// project-specific, and a seed that guessed too much would silently drop
/// files a developer meant to commit.
const SOFTWARE_SEED: &str = "\
# Files' software-root seed (ADR 0001). A root's own .gitignore is layered
# on top of this and can re-include anything here with a `!` rule.
/.git/
target/
node_modules/
.DS_Store
# Heavy stray media: belongs in a media File Root, not a git object store.
*.wav
*.aif
*.aiff
*.flac
*.mov
*.mp4
*.mkv
*.mxf
*.r3d
*.braw
*.iso
*.dmg
";

/// Media roots: DAW backup files and the peak/analysis caches editors
/// rebuild from the audio itself (issue #260). The `.rpp-bak` storm is
/// the El Artisa fixture's shape and the reason this seed exists — a
/// tracking day's saves would otherwise multiply the session document
/// into the store once per save.
const MEDIA_SEED: &str = "\
# Files' media-root seed (issue #260). Editable per root: `task files
# ignore set …` replaces the stored layer, which is chained on top of
# this one and can re-include anything here with a `!` rule.
# REAPER: a .rpp-bak on every save, plus caches rebuilt from the audio.
*.rpp-bak
*.RPP-bak
*.reapeaks
*.reapindex
# Other DAWs' regenerable peak / analysis sidecars.
*.asd
*.pkf
*.sfk
*.wfm
*.peak
# Editors' scratch and OS junk.
*.tmp
*~
.DS_Store
Thumbs.db
# macOS AppleDouble sidecars. A Mac writing to a non-HFS volume — a NAS
# share, an exFAT drive, exactly how this material arrives — splits every
# file in two and leaves a `._name` beside `name` holding the resource
# fork and Finder metadata. Without this a browse shows every file twice,
# and a capture versions the metadata alongside the content. Regenerable
# and disposable: the audio, video and session data are all in the real
# file.
._*
.AppleDouble/
.AppleDB/
.AppleDesktop/
.Spotlight-V100/
.TemporaryItems/
.Trashes/
.fseventsd/
";

/// Filename globs whose write counts as a **project-file save** — the
/// event that marks a save point (glossary "Auto-snapshot"). A project
/// file is the DAW/NLE's own session document: the thing a producer
/// means by "I saved".
pub const MEDIA_PROJECT_FILES: &[&str] = &[
    "*.rpp",
    "*.RPP",
    "*.daw",  // .daw project format (issue #155)
    "*.als",  // Ableton Live
    "*.drp",  // DaVinci Resolve
    "*.ptx",  // Pro Tools
    "*.cpr",  // Cubase
    "*.flp",  // FL Studio
    "*.song", // Studio One / Reason
    "*.logicx",
    "*.dawproject",
];

/// [`MEDIA_PROJECT_FILES`], compiled once. [`is_project_file`] runs per
/// surviving hint path, and hints arrive per watcher event during
/// exactly the write storms the cadence engine exists for — recompiling
/// eleven globs on each one is work with no purpose (PR #283 review).
static MEDIA_PROJECT_GLOBS: LazyLock<GlobSet> = LazyLock::new(|| {
    let mut builder = GlobSetBuilder::new();
    for pattern in MEDIA_PROJECT_FILES {
        builder.add(Glob::new(pattern).expect("project-file patterns are valid globs"));
    }
    builder.build().expect("project-file glob set builds")
});

/// Filename of the root's stored (editable) patterns inside its store
/// directory.
pub const IGNORE_FILE: &str = "ignore.json";

/// The flavor's built-in patterns, as an ignore file rooted at the
/// root's top level.
pub fn seed(flavor: RootFlavor) -> Result<Arc<GitIgnoreFile>> {
    let (name, body) = match flavor {
        RootFlavor::Media => ("<media-root-seed>", MEDIA_SEED),
        RootFlavor::Software => ("<software-root-seed>", SOFTWARE_SEED),
    };
    GitIgnoreFile::empty()
        .chain(RepoPath::root(), Path::new(name), body.as_bytes())
        .map_err(|e| Error::Repo(format!("building the {name} ignore seed: {e}")))
}

/// Whether a root of this flavor layers the tree's own `.gitignore` files
/// on top of [`seed`].
#[must_use]
pub fn honors_gitignore(flavor: RootFlavor) -> bool {
    matches!(flavor, RootFlavor::Software)
}

/// The root's stored (editable) patterns — layer 2. An absent file means
/// no stored layer, which is the normal state of a root that has never
/// been edited: the flavor seed is what it runs on.
pub fn stored_patterns(store_dir: &Path) -> Result<Vec<String>> {
    let path = store_dir.join(IGNORE_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(&path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// Replace the root's stored patterns, returning them normalized
/// (trimmed, deduplicated) exactly as persisted.
pub fn save_patterns(store_dir: &Path, patterns: Vec<String>) -> Result<Vec<String>> {
    let normalized = normalize(patterns)?;
    std::fs::create_dir_all(store_dir)?;
    let path = store_dir.join(IGNORE_FILE);
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(&normalized)?)?;
    std::fs::rename(&temp, &path)?;
    Ok(normalized)
}

/// Trim, drop blanks, deduplicate — and reject anything that would
/// smuggle in more than the one rule it claims to be. Gitignore syntax
/// is line-oriented, so an embedded newline in a "pattern" is really
/// several patterns (a `!` re-include among them could quietly undo the
/// flavor seed), and a comment marker would make the rule inert rather
/// than failing loudly.
///
/// **Order is preserved, never sorted.** In gitignore the *last*
/// matching rule wins, so `["*.wav", "!keep.wav"]` and its sorted
/// permutation mean opposite things — sorting would silently disarm
/// every `!` re-include a user wrote.
pub(crate) fn normalize(patterns: Vec<String>) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for pattern in patterns {
        let trimmed = pattern.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.contains(['\n', '\r', '\0']) {
            return Err(Error::BadRequest(format!(
                "ignore pattern {pattern:?}: one pattern per entry — no line breaks"
            )));
        }
        if trimmed.starts_with('#') {
            return Err(Error::BadRequest(format!(
                "ignore pattern {pattern:?}: a comment ignores nothing"
            )));
        }
        if !out.contains(&trimmed) {
            out.push(trimmed);
        }
    }
    Ok(out)
}

/// The root's whole Ignore set as one matcher: the flavor seed with the
/// stored patterns chained on top (so a stored `!` rule can re-include
/// something the seed excluded). The tree's own `.gitignore` files, on
/// the flavors that honor them, are chained per directory as the scan
/// descends — see [`crate::scan`].
pub fn for_root(store_dir: &Path, flavor: RootFlavor) -> Result<Arc<GitIgnoreFile>> {
    let base = seed(flavor)?;
    let stored = stored_patterns(store_dir)?;
    if stored.is_empty() {
        return Ok(base);
    }
    let body = stored.join("\n") + "\n";
    base.chain(
        RepoPath::root(),
        Path::new("<root-ignore-set>"),
        body.as_bytes(),
    )
    .map_err(|e| Error::Repo(format!("building the root's ignore set: {e}")))
}

/// Does `rel_path` (root-relative, `/`-separated) name a project file
/// for `flavor` — i.e. does writing it mark a save point?
#[must_use]
pub fn is_project_file(rel_path: &str, flavor: RootFlavor) -> bool {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    match flavor {
        RootFlavor::Media => MEDIA_PROJECT_GLOBS.is_match(name),
        // Software roots have no single session document.
        RootFlavor::Software => false,
    }
}

/// Is `rel_path` ignored by `ignores`? The cadence engine's question
/// (an ignored write is not activity), asked of the same matcher the
/// scan walks with. A path that isn't a valid repo path can't be in the
/// store either, so it is treated as ignored rather than erroring a hint.
#[must_use]
pub fn is_ignored(ignores: &GitIgnoreFile, rel_path: &str) -> bool {
    match jj_lib::repo_path::RepoPathBuf::from_internal_string(rel_path) {
        Ok(path) => ignores.matches_file(&path),
        Err(_) => true,
    }
}

/// The real Ignore set, as the cadence engine sees it.
///
/// The engine asks two questions about a path — is it ignored, and is it
/// the project document — and used to be handed a `&GitIgnoreFile` and a
/// `RootFlavor` to answer them itself. That put jj's gitignore machinery
/// inside a state machine about time. It now takes a
/// [`files_domain::cadence::ActivityFilter`]; this is the implementation
/// backed by the genuine set.
pub struct RootFilter {
    ignores: Arc<GitIgnoreFile>,
    flavor: RootFlavor,
}

impl RootFilter {
    #[must_use]
    pub fn new(ignores: Arc<GitIgnoreFile>, flavor: RootFlavor) -> Self {
        Self { ignores, flavor }
    }
}

impl files_domain::cadence::ActivityFilter for RootFilter {
    fn is_ignored(&self, rel_path: &str) -> bool {
        is_ignored(&self.ignores, rel_path)
    }

    fn is_project_file(&self, rel_path: &str) -> bool {
        is_project_file(rel_path, self.flavor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(flavor: RootFlavor, path: &str) -> bool {
        is_ignored(&seed(flavor).unwrap(), path)
    }

    fn matches_dir(flavor: RootFlavor, path: &str) -> bool {
        seed(flavor)
            .unwrap()
            .matches_dir(&jj_lib::repo_path::RepoPathBuf::from_internal_string(path).unwrap())
    }

    /// Real listing from the production archive: a Mac wrote this
    /// material to a NAS share, so every file has a `._name` twin
    /// holding its resource fork. Browsing a root showed each file
    /// twice before these rules existed.
    #[test]
    fn the_media_seed_hides_macos_appledouble_sidecars() {
        assert!(matches(RootFlavor::Media, "._12 UNITY 1.4 SommaPrep.ptx"));
        assert!(matches(RootFlavor::Media, "Audio Files/._Unity.wav"));
        assert!(matches(RootFlavor::Media, "._.DS_Store"));
        assert!(matches_dir(RootFlavor::Media, ".Spotlight-V100"));

        // The real files must survive: the sidecar is a prefix, never a
        // reason to drop the content beside it.
        assert!(!matches(RootFlavor::Media, "12 UNITY 1.4 SommaPrep.ptx"));
        assert!(!matches(RootFlavor::Media, "Audio Files/Unity.wav"));
    }

    #[test]
    fn the_media_seed_catches_the_reaper_backup_storm_at_any_depth() {
        assert!(matches(RootFlavor::Media, "El Artisa.rpp-bak"));
        assert!(matches(RootFlavor::Media, "sessions/El Artisa.rpp-bak"));
        assert!(matches(RootFlavor::Media, "Audio Files/kick.reapeaks"));
        assert!(!matches(RootFlavor::Media, "El Artisa.rpp"));
        assert!(!matches(RootFlavor::Media, "Audio Files/kick.wav"));
    }

    #[test]
    fn the_software_seed_keeps_heavy_media_out_of_a_git_object_store() {
        // `target/` is a directory rule — the scan prunes at the
        // directory, which is what `matches_dir` answers.
        assert!(matches_dir(RootFlavor::Software, "target"));
        assert!(matches(RootFlavor::Software, "assets/theme.wav"));
        assert!(!matches(RootFlavor::Software, "src/main.rs"));
        // …and a media root versions its audio, obviously.
        assert!(!matches(RootFlavor::Media, "assets/theme.wav"));
    }

    #[test]
    fn stored_patterns_layer_over_the_seed_and_can_re_include() {
        let dir = tempfile::tempdir().unwrap();
        let stored = save_patterns(
            dir.path(),
            vec!["  *.wav ".into(), "*.wav".into(), "!keep.wav".into()],
        )
        .unwrap();
        assert_eq!(
            stored,
            ["*.wav", "!keep.wav"],
            "trimmed and deduplicated, but never reordered — in gitignore \
             the last matching rule wins"
        );

        let set = for_root(dir.path(), RootFlavor::Media).unwrap();
        assert!(is_ignored(&set, "Audio Files/gtr.wav"));
        assert!(!is_ignored(&set, "keep.wav"), "a `!` rule re-includes");
        assert!(
            is_ignored(&set, "El Artisa.rpp-bak"),
            "the flavor seed still applies underneath"
        );
        assert_eq!(stored_patterns(dir.path()).unwrap(), stored);
    }

    #[test]
    fn a_pattern_may_not_smuggle_extra_rules() {
        assert!(save_patterns(Path::new("/nonexistent"), vec!["a\n!b".into()]).is_err());
        assert!(save_patterns(Path::new("/nonexistent"), vec!["# nope".into()]).is_err());
    }

    #[test]
    fn project_files_are_flavor_scoped() {
        assert!(is_project_file("El Artisa.rpp", RootFlavor::Media));
        assert!(is_project_file("sessions/Cut 3.drp", RootFlavor::Media));
        assert!(!is_project_file("mix.wav", RootFlavor::Media));
        assert!(!is_project_file("El Artisa.rpp", RootFlavor::Software));
    }
}
