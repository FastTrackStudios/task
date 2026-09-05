//! Reading a vault off disk.
//!
//! The only part of this crate that touches the filesystem, kept in one
//! module so everything else stays pure and testable against literals.

use std::io;
use std::path::{Path, PathBuf};

use crate::frontmatter::Frontmatter;
use crate::render::Renderer;
use crate::{Page, Vault};

/// One markdown note as read from disk, before rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// File stem — the URL segment and the wikilink target.
    pub slug: String,
    /// Where it came from. Reported in errors, and emitted as
    /// `cargo:rerun-if-changed` by `ssg-build`.
    pub path: PathBuf,
    /// The file verbatim, frontmatter included.
    pub source: String,
}

/// What can go wrong reading a vault.
#[derive(Debug)]
pub enum ScanError {
    /// The vault directory could not be read.
    Directory { path: PathBuf, source: io::Error },
    /// A note could not be read.
    Note { path: PathBuf, source: io::Error },
    /// The directory held no markdown at all.
    ///
    /// Its own variant because it is the failure that would otherwise
    /// pass silently: a site whose guide directory moved builds fine and
    /// ships an empty guide, and nobody notices until a reader does.
    Empty { path: PathBuf },
    /// Two notes in the vault share a file name.
    ///
    /// Slugs are flat whatever the folder layout, so this would be two
    /// pages at one URL and one of them would win by directory order.
    DuplicateSlug {
        slug: String,
        first: PathBuf,
        second: PathBuf,
    },
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Directory { path, source } => {
                write!(f, "cannot read the vault at {}: {source}", path.display())
            }
            Self::Note { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            Self::DuplicateSlug {
                slug,
                first,
                second,
            } => write!(
                f,
                "two notes are both `{slug}`: {} and {}. Slugs are flat whatever \
                 the folder layout, so these are one URL — rename one.",
                first.display(),
                second.display()
            ),
            Self::Empty { path } => write!(
                f,
                "no markdown notes under {} — the site would ship an empty vault",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Directory { source, .. } | Self::Note { source, .. } => Some(source),
            Self::Empty { .. } | Self::DuplicateSlug { .. } => None,
        }
    }
}

/// Read every `.md` note under `dir`, at any depth.
///
/// **Slugs stay flat.** A slug is also a wikilink target and a URL
/// segment, so `chords/root.md` is the page `root`, not `chords/root`.
/// Folders are for whoever browses the repository; they change nothing
/// a reader sees, which is what lets a vault be reorganised without
/// breaking a single link.
///
/// The cost is that two notes cannot share a file name. That is a real
/// constraint, so it is an error rather than a silent last-writer-wins
/// — see [`ScanError::DuplicateSlug`].
///
/// Notes come back sorted by slug; ordering for display is the
/// frontmatter's job and happens in [`scan_with`].
pub fn scan(dir: impl AsRef<Path>) -> Result<Vec<Note>, ScanError> {
    let dir = dir.as_ref();
    let mut notes = Vec::new();
    collect(dir, &mut notes)?;

    // t[impl ssg.build.non-empty]
    if notes.is_empty() {
        return Err(ScanError::Empty {
            path: dir.to_owned(),
        });
    }

    // Directory order is whatever the filesystem says, which differs
    // between machines. Sorting here is what makes a build reproducible.
    notes.sort_by(|a, b| a.slug.cmp(&b.slug));

    // Two notes with one slug would be two pages at one URL. Whichever
    // won would depend on directory order, so neither does.
    if let Some(pair) = notes.windows(2).find(|w| w[0].slug == w[1].slug) {
        return Err(ScanError::DuplicateSlug {
            slug: pair[0].slug.clone(),
            first: pair[0].path.clone(),
            second: pair[1].path.clone(),
        });
    }

    Ok(notes)
}

/// Walk `dir`, appending every `.md` note found at any depth.
fn collect(dir: &Path, notes: &mut Vec<Note>) -> Result<(), ScanError> {
    let entries = std::fs::read_dir(dir).map_err(|source| ScanError::Directory {
        path: dir.to_owned(),
        source,
    })?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, notes)?;
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let Some(slug) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        let source = std::fs::read_to_string(&path).map_err(|source| ScanError::Note {
            path: path.clone(),
            source,
        })?;
        notes.push(Note { slug, path, source });
    }
    Ok(())
}

/// Read `dir` and render it into a [`Vault`].
///
/// The renderer is built by `build`, which receives the vault's slugs —
/// it needs them to resolve wikilinks, and they are not known until the
/// directory has been read.
pub fn scan_with<'a>(
    dir: impl AsRef<Path>,
    build: impl FnOnce(Vec<String>) -> Renderer<'a>,
) -> Result<Vault, ScanError> {
    let notes = scan(dir)?;

    // Titles are registered as link aliases, so `[[Recording]]` finds
    // `recording.md`. That is how a person writes a cross-reference —
    // by the page's name, not its filename — and it is what Obsidian
    // resolves, so a vault authored there is full of them.
    let titles: Vec<(String, String)> = notes
        .iter()
        .map(|note| (title_of(note), note.slug.clone()))
        .collect();

    let renderer = build(notes.iter().map(|n| n.slug.clone()).collect()).aliases(titles);

    let mut pages: Vec<Page> = notes
        .iter()
        .map(|note| {
            let (fm, body) = Frontmatter::split(&note.source);
            let rendered = renderer.render(note);
            Page {
                slug: note.slug.clone(),
                title: title_of(note),
                summary: fm
                    .any(&["summary", "blurb", "description"])
                    .unwrap_or_default()
                    .to_owned(),
                // A note with no `order:` sorts last rather than first:
                // an unordered note is usually one somebody has not
                // placed yet, and the front door is never the accident.
                order: fm.number("order").unwrap_or(u32::MAX),
                stage: fm.get("stage").unwrap_or_default().to_owned(),
                kind: fm.any(&["type", "kind"]).unwrap_or("other").to_lowercase(),
                source: note.source.clone(),
                body: body.to_owned(),
                html: rendered.html,
                headings: rendered.headings,
                // Tags are a vault-wide axis, so they are lowercased
                // here: `#Rig` and `#rig` are one tag, and a tag page
                // that split them would be lying about the vault.
                tags: fm
                    .list("tags")
                    .into_iter()
                    .map(|t| t.trim_start_matches('#').to_lowercase())
                    .filter(|t| !t.is_empty())
                    .collect(),
                words: rendered.words,
                // Left empty here: this crate does not read git, and a
                // caller that can is the one to fill it in.
                updated: String::new(),
                links: rendered.links,
                broken_links: rendered.broken_links,
            }
        })
        .collect();

    // t[impl ssg.order.reading] — declared order, slug breaking ties.
    // `scan` already sorted the notes, so this is stable across machines.
    pages.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.slug.cmp(&b.slug)));
    Ok(Vault { pages })
}

/// A note's display title: frontmatter `title:`, else its first `#`
/// heading, else the de-slugged file name.
///
/// One function because the title is used twice and the two must agree:
/// it names the page in a table of contents, and it is what a
/// `[[wikilink]]` written by a person is most likely to say.
fn title_of(note: &Note) -> String {
    let (fm, body) = Frontmatter::split(&note.source);
    fm.get("title")
        .map(str::to_owned)
        .or_else(|| first_heading(body))
        .unwrap_or_else(|| note.slug.replace('-', " "))
}

/// The first ATX heading in a body, as a title fallback.
fn first_heading(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        line.strip_prefix("# ")
            .map(|heading| heading.trim().to_owned())
    })
}

#[cfg(test)]
mod recursive_tests {
    use super::*;

    fn write(dir: &std::path::Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }

    #[test]
    fn notes_are_found_at_any_depth_with_flat_slugs() {
        // Folders are for whoever browses the repository. A slug is a
        // URL segment and a wikilink target, so it stays the file name —
        // which is what lets a vault be reorganised without breaking a
        // single link.
        let dir = std::env::temp_dir().join(format!("ssg-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir, "top.md", "# Top");
        write(&dir, "chords/root.md", "# Root");
        write(&dir, "chords/deeper/quality.md", "# Quality");

        let notes = scan(&dir).expect("scan");
        let slugs: Vec<&str> = notes.iter().map(|n| n.slug.as_str()).collect();
        assert_eq!(slugs, ["quality", "root", "top"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_notes_with_one_name_are_an_error() {
        let dir = std::env::temp_dir().join(format!("ssg-dupe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir, "a/root.md", "# One");
        write(&dir, "b/root.md", "# Two");

        let err = scan(&dir).expect_err("a duplicate slug is an error");
        assert!(matches!(err, ScanError::DuplicateSlug { .. }), "{err}");
        assert!(err.to_string().contains("root"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Renderer;

    /// A vault on disk, in a directory that cleans itself up.
    struct TempVault(PathBuf);

    impl TempVault {
        fn new(name: &str, notes: &[(&str, &str)]) -> Self {
            let dir = std::env::temp_dir().join(format!("ssg-vault-test-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp vault");
            for (slug, source) in notes {
                std::fs::write(dir.join(format!("{slug}.md")), source).expect("write note");
            }
            Self(dir)
        }
    }

    impl Drop for TempVault {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn render(slugs: Vec<String>) -> Renderer<'static> {
        Renderer::new("/guide", slugs)
    }

    #[test]
    fn reads_notes_and_ignores_everything_else() {
        let vault = TempVault::new("reads", &[("chords", "# Chords"), ("rhythm", "# Rhythm")]);
        std::fs::write(vault.0.join("notes.txt"), "not a note").expect("write");

        let notes = scan(&vault.0).expect("scan");
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].slug, "chords");
    }

    // t[verify ssg.build.non-empty]
    #[test]
    fn an_empty_directory_is_an_error_not_an_empty_vault() {
        let vault = TempVault::new("empty", &[]);
        assert!(matches!(scan(&vault.0), Err(ScanError::Empty { .. })));
    }

    #[test]
    fn a_missing_directory_reports_its_path() {
        let err = scan("/nonexistent/vault/path").expect_err("should fail");
        assert!(matches!(err, ScanError::Directory { .. }));
        assert!(err.to_string().contains("/nonexistent/vault/path"));
    }

    // t[verify ssg.order.reading]
    #[test]
    fn pages_come_back_in_frontmatter_order() {
        let vault = TempVault::new(
            "order",
            &[
                ("third", "---\norder: 3\n---\n# Third"),
                ("first", "---\norder: 1\n---\n# First"),
                ("last", "# No order"),
            ],
        );
        let built = scan_with(&vault.0, render).expect("scan");
        let slugs: Vec<_> = built.pages.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["first", "third", "last"]);
    }

    #[test]
    fn a_title_falls_back_to_the_heading_then_the_slug() {
        let vault = TempVault::new(
            "titles",
            &[
                ("a", "---\ntitle: From Frontmatter\n---\n# Ignored"),
                ("b", "# From Heading"),
                ("key-changes", "no heading here"),
            ],
        );
        let built = scan_with(&vault.0, render).expect("scan");
        assert_eq!(built.page("a").expect("a").title, "From Frontmatter");
        assert_eq!(built.page("b").expect("b").title, "From Heading");
        assert_eq!(built.page("key-changes").expect("c").title, "key changes");
    }

    // t[verify ssg.order.backlinks]
    #[test]
    fn backlinks_are_the_reverse_of_links() {
        let vault = TempVault::new(
            "backlinks",
            &[
                ("chords", "see [[rhythm]]"),
                ("keys", "also [[rhythm]]"),
                ("rhythm", "the end"),
            ],
        );
        let built = scan_with(&vault.0, render).expect("scan");
        let back: Vec<_> = built
            .backlinks("rhythm")
            .iter()
            .map(|p| p.slug.as_str())
            .collect();
        assert_eq!(back, ["chords", "keys"]);
    }

    #[test]
    fn a_broken_cross_reference_is_reported_with_its_source() {
        let vault = TempVault::new("broken", &[("chords", "see [[typo]]")]);
        let built = scan_with(&vault.0, render).expect("scan");
        assert_eq!(built.broken_links(), vec![("chords", "typo")]);
    }
}
