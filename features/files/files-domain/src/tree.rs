//! The org tree — one namespace over Projects, Vault, Wiki and Assets.
//!
//! The same resolver backs the explorer and the WebDAV mount, so the app
//! and a mounted share always show one tree. That is a requirement
//! (`files.catalogue.complete` reaches everything the principal may
//! reach) and it is also why the grammar belongs here rather than inside
//! a backend: two consumers, one set of rules.
//!
//! What is pure lives here — parsing a path, confining it, deciding what
//! a project *is*, and choosing which root sits behind a project's
//! `Media/` door. What touches a disk stays in `files`.
//!
//! # Two definitions of "project"
//!
//! `project.definition.single` requires exactly one definition in force,
//! and says the metadata-driven one wins. Today the tree finds projects
//! by listing two hardcoded directories, `Projects/` and `Albums/`, which
//! cannot express nesting at all — see [`ProjectHomes`], which exists to
//! be deleted.

use std::fmt;

use uuid::Uuid;

/// The four top-level areas, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Area {
    Projects,
    Vault,
    Wiki,
    Assets,
}

impl Area {
    pub const ALL: [Area; 4] = [Area::Projects, Area::Vault, Area::Wiki, Area::Assets];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Area::Projects => "Projects",
            Area::Vault => "Vault",
            Area::Wiki => "Wiki",
            Area::Assets => "Assets",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Area::ALL.into_iter().find(|a| a.as_str() == s)
    }
}

impl fmt::Display for Area {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a tree path was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    /// The tree serves virtual paths, but every segment still walks real
    /// directories underneath — so confinement is checked before
    /// anything is resolved, not after.
    #[error("{0}: path escapes")]
    Escapes(String),
    #[error("{0}: no such area")]
    NoSuchArea(String),
}

/// Where a tree path points, before anything touches a disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The empty path: list the areas.
    Areas,
    /// `Projects/` — every project.
    Projects,
    /// `Projects/<name>[/rest…]`.
    Project { name: String, rest: Vec<String> },
    /// A non-Projects area and the path within it.
    Within { area: Area, rest: Vec<String> },
}

/// Parse and confine a tree path.
// t[impl files.catalogue.complete] — one grammar for explorer and WebDAV
pub fn route(path: &str) -> Result<Route, RouteError> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.iter().any(|s| *s == "." || *s == "..") {
        return Err(RouteError::Escapes(path.to_string()));
    }

    let Some((first, rest)) = segments.split_first() else {
        return Ok(Route::Areas);
    };
    let Some(area) = Area::parse(first) else {
        return Err(RouteError::NoSuchArea((*first).to_string()));
    };

    let owned = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();

    Ok(match area {
        Area::Projects => match rest.split_first() {
            None => Route::Projects,
            Some((name, tail)) => Route::Project {
                name: (*name).to_string(),
                rest: owned(tail),
            },
        },
        other => Route::Within {
            area: other,
            rest: owned(rest),
        },
    })
}

/// The vault directories a project may live under.
///
/// **This type exists to be deleted.** `project.definition.single` says
/// one definition of a project is in force and the metadata-driven one
/// wins; two hardcoded directory names are the other one. They also
/// cannot express `project.nesting.explicit` — arbitrary nesting is not
/// expressible as two names — so every subproject in a real vault is
/// invisible to the tree.
///
/// It is a type rather than three string literals scattered across a
/// resolver so that deleting it is a compiler error rather than an
/// archaeology exercise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectHomes(Vec<String>);

impl ProjectHomes {
    /// The two directories the tree has always listed.
    #[must_use]
    pub fn legacy() -> Self {
        Self(vec!["Projects".into(), "Albums".into()])
    }

    #[must_use]
    pub fn new(homes: impl IntoIterator<Item = String>) -> Self {
        Self(homes.into_iter().collect())
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Whether a note's frontmatter declares its directory a project.
///
/// The metadata-driven definition: `type: project`, per
/// `project.identity.declaration`. Nothing outside the file is needed to
/// interpret it, which is what lets a project survive `cp -r`.
///
/// Silent on every failure — a missing note, unreadable file or malformed
/// YAML all mean "not declared" rather than an error. A vault must never
/// become unbrowsable because someone hand-edited frontmatter badly.
// t[impl project.identity.declaration]
#[must_use]
pub fn declares_project(note: &str) -> bool {
    frontmatter(note)
        .and_then(|doc| {
            doc.get("type")
                .and_then(serde_yaml::Value::as_str)
                .map(|t| t.trim().eq_ignore_ascii_case("project"))
        })
        .unwrap_or(false)
}

/// Root ids declared by a project note's `media_roots:` frontmatter, in
/// order.
///
/// A **list** because one project genuinely has several roots (a shoot
/// with separate camera, session and deliverable piles), and the same
/// root genuinely belongs to two projects (footage shared by a
/// collaboration).
#[must_use]
pub fn declared_media_roots(note: &str) -> Vec<Uuid> {
    let Some(doc) = frontmatter(note) else {
        return Vec::new();
    };
    let Some(field) = doc.get("media_roots") else {
        return Vec::new();
    };
    let parse = |v: &serde_yaml::Value| v.as_str().and_then(|s| s.trim().parse().ok());
    match field {
        serde_yaml::Value::Sequence(items) => items.iter().filter_map(parse).collect(),
        other => parse(other).into_iter().collect(),
    }
}

fn frontmatter(note: &str) -> Option<serde_yaml::Value> {
    let rest = note.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    serde_yaml::from_str(&rest[..end]).ok()
}

/// Choose the root behind a project's `Media/` door.
///
/// **By id first**, from the note's declaration; by name only as a
/// fallback for projects not yet linked.
///
/// Name-matching alone was the original rule and is too fragile to keep
/// as the primary: rename either side and `Media/` silently empties, with
/// no error to notice. Real material has already broken it three ways — a
/// folder carrying an invisible U+F022 from a Mac font, three spellings
/// of one project, and a client's name spelled two ways inside a single
/// project. An id survives all of it, and survives the renaming that
/// sorting a migration inevitably involves.
#[must_use]
pub fn select_media_root(
    project: &str,
    declared: &[Uuid],
    known: &[(Uuid, String)],
) -> Option<Uuid> {
    for id in declared {
        if known.iter().any(|(known_id, _)| known_id == id) {
            return Some(*id);
        }
    }
    let album_name = format!("Album — {project}");
    known
        .iter()
        .find(|(_, name)| name == project || *name == album_name)
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    // t[verify files.catalogue.complete]
    #[test]
    fn the_empty_path_lists_the_areas() {
        assert_eq!(route("").unwrap(), Route::Areas);
        assert_eq!(route("/").unwrap(), Route::Areas);
        assert_eq!(Area::ALL.len(), 4);
    }

    #[test]
    fn areas_route_to_themselves() {
        assert_eq!(route("Projects").unwrap(), Route::Projects);
        assert_eq!(
            route("Vault/Records").unwrap(),
            Route::Within {
                area: Area::Vault,
                rest: seg(&["Records"])
            }
        );
        assert_eq!(
            route("Wiki").unwrap(),
            Route::Within {
                area: Area::Wiki,
                rest: vec![]
            }
        );
    }

    #[test]
    fn a_project_carries_its_remainder() {
        assert_eq!(
            route("Projects/Alpha/Media/takes").unwrap(),
            Route::Project {
                name: "Alpha".into(),
                rest: seg(&["Media", "takes"])
            }
        );
    }

    #[test]
    fn confinement_is_checked_before_resolution() {
        assert!(matches!(
            route("Vault/../../etc/passwd"),
            Err(RouteError::Escapes(_))
        ));
        assert!(matches!(
            route("Projects/./Alpha"),
            Err(RouteError::Escapes(_))
        ));
    }

    #[test]
    fn an_unknown_area_is_not_found() {
        assert!(matches!(route("Nope"), Err(RouteError::NoSuchArea(_))));
    }

    // t[verify project.identity.declaration]
    #[test]
    fn a_note_declares_its_directory_a_project() {
        assert!(declares_project("---\ntype: project\ntitle: Mars\n---\n# Mars"));
        assert!(declares_project("---\ntype: Project\n---\n"));
        assert!(!declares_project("---\ntype: note\n---\n"));
        assert!(!declares_project("---\ntitle: Mars\n---\n"));
    }

    #[test]
    fn a_badly_edited_note_is_not_a_project_rather_than_an_error() {
        assert!(!declares_project("---\ntype: [unclosed\n---\n"));
        assert!(!declares_project("no frontmatter at all"));
        assert!(!declares_project(""));
    }

    #[test]
    fn media_roots_read_as_a_list_or_a_scalar() {
        let id = "3f9c1e88-0000-4000-8000-000000000001";
        let list = format!("---\nmedia_roots:\n  - {id}\n---\n");
        let scalar = format!("---\nmedia_roots: {id}\n---\n");
        assert_eq!(declared_media_roots(&list).len(), 1);
        assert_eq!(declared_media_roots(&scalar).len(), 1);
        assert_eq!(declared_media_roots(&list), declared_media_roots(&scalar));
    }

    #[test]
    fn a_malformed_link_is_skipped_not_fatal() {
        let note = "---\nmedia_roots:\n  - not-a-uuid\n  - 3f9c1e88-0000-4000-8000-000000000001\n---\n";
        assert_eq!(declared_media_roots(note).len(), 1);
    }

    #[test]
    fn a_declared_id_beats_a_matching_name() {
        let a = Uuid::from_bytes([1; 16]);
        let b = Uuid::from_bytes([2; 16]);
        let known = vec![(a, "Something Else".to_string()), (b, "Alpha".to_string())];
        assert_eq!(select_media_root("Alpha", &[a], &known), Some(a));
    }

    #[test]
    fn an_unresolvable_id_falls_back_to_the_name() {
        let ghost = Uuid::from_bytes([9; 16]);
        let real = Uuid::from_bytes([2; 16]);
        let known = vec![(real, "Alpha".to_string())];
        assert_eq!(select_media_root("Alpha", &[ghost], &known), Some(real));
    }

    #[test]
    fn the_album_naming_convention_still_resolves() {
        let id = Uuid::from_bytes([3; 16]);
        let known = vec![(id, "Album — Dusk".to_string())];
        assert_eq!(select_media_root("Dusk", &[], &known), Some(id));
    }

    #[test]
    fn legacy_homes_are_the_two_that_have_to_go() {
        let homes = ProjectHomes::legacy();
        assert_eq!(homes.iter().collect::<Vec<_>>(), vec!["Projects", "Albums"]);
    }
}
