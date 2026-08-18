//! `ProjectInfo` → markdown bytes + write-to-disk.
//!
//! `id: <uuid>` is always emitted (nil ids are backfilled
//! here), so the file on disk always carries a stable identity
//! downstream features can FK against. Pages that were never
//! written through this path parse with a deterministic
//! uuid-v5-of-path fallback (see `parse_page`), which this
//! writer persists on the next save.
//!
//! Serialization lives in [`crate::entity`]; this module keeps the
//! historical `project::write::*` paths working and adds the one thing
//! the shared store doesn't cover — writing straight to a vault root
//! on disk, without an in-memory `Vault`.

use std::path::{Path, PathBuf};

use uuid::Uuid;
use vault_entity::store::VaultEntity;

pub use vault_entity::WriteError;

use crate::entity::Projects;
use crate::model::ProjectInfo;

/// Serialize a `ProjectInfo` into the full markdown source:
/// `---` fenced frontmatter + body. Round-trip-clean for the
/// fields the model tracks.
///
/// `type: project` is the scanner's discriminator and is emitted
/// first, so a fresh `task project create` round-trips through
/// `looks_like_project` without needing a `project` tag.
pub fn serialize_project(project: &ProjectInfo) -> Result<String, WriteError> {
    Projects::to_markdown(project)
}

/// Write a project to `<vault_root>/<project.path>`. Creates
/// parent directories. Refuses to overwrite an existing file
/// unless `overwrite` is true. Backfills `id` if nil.
pub fn write_project(
    vault_root: &Path,
    project: &mut ProjectInfo,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if project.path.is_empty() {
        return Err(WriteError::BadPath("project.path is empty".into()));
    }
    if project.id.is_nil() {
        project.id = Uuid::new_v4();
    }
    let abs = vault_root.join(&project.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    let serialized = serialize_project(project)?;
    std::fs::write(&abs, serialized).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}

/// Conventional path for a freshly captured project — slug
/// from the title, dropped under `Projects/`.
///
/// A title that slugifies to nothing falls back to
/// `Projects/untitled-project.md` — under the folder, like every other
/// title.
#[must_use]
pub fn default_project_path(title: &str) -> String {
    Projects::default_path(title, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::states::{StateDef, StateGroup, StatesConfig};

    /// `states:` config survives a serialize → parse round-trip,
    /// and absent config round-trips as `None` (so vaults
    /// without registries stay byte-stable).
    #[test]
    fn states_config_round_trips_through_frontmatter() {
        let mut p = crate::parse_str(
            "Projects/demo.md",
            "demo",
            "---\ntype: project\ntitle: Demo\n---\n",
        )
        .expect("minimal project parses");
        assert_eq!(p.states, None);
        p.id = Uuid::new_v4();

        // No config: serialized page carries no `states:` key.
        let plain = serialize_project(&p).expect("serialize");
        assert!(!plain.contains("states:"));

        p.states = Some(StatesConfig(vec![
            StateDef {
                name: "specced".into(),
                group: StateGroup::Unstarted,
                color: "#88ccff".into(),
                default: true,
                order: 1,
            },
            StateDef {
                name: "building".into(),
                group: StateGroup::Started,
                color: String::new(),
                default: false,
                order: 2,
            },
            StateDef {
                name: "shipped".into(),
                group: StateGroup::Completed,
                color: String::new(),
                default: false,
                order: 3,
            },
        ]));
        let raw = serialize_project(&p).expect("serialize");
        let back = crate::parse_str("Projects/demo.md", "demo", &raw).expect("re-parse");
        assert_eq!(back.states, p.states);
        assert_eq!(back.id, p.id);
    }

    /// The project-level verify command survives a serialize → parse
    /// round-trip, and a project that declares none stays byte-stable
    /// (no stray `verifyCommand:` key appears in existing vaults).
    #[test]
    fn verify_command_round_trips_through_frontmatter() {
        let mut p = crate::parse_str(
            "Projects/demo.md",
            "demo",
            "---\ntype: project\ntitle: Demo\n---\n",
        )
        .expect("minimal project parses");
        assert_eq!(p.verify_command, "", "absent key parses as no default");

        let plain = serialize_project(&p).expect("serialize");
        assert!(
            !plain.contains("verifyCommand"),
            "a project with no verify command must not grow the key"
        );

        p.verify_command = "cargo check -p task-proto".into();
        let raw = serialize_project(&p).expect("serialize");
        let back = crate::parse_str("Projects/demo.md", "demo", &raw).expect("re-parse");
        assert_eq!(back.verify_command, "cargo check -p task-proto");
    }

    /// Parts survive a serialize → parse round trip, ids intact.
    ///
    /// The ids are what the round trip is about. Everything that
    /// attaches to a part addresses it by id, so a save that reassigned
    /// them would break every reference on every write — silently, since
    /// the names would still look right.
    #[test]
    fn parts_round_trip_with_their_ids() {
        let mut p = crate::parse_str(
            "Projects/album.md",
            "album",
            "---\ntype: project\ntitle: Crescendum\n---\n",
        )
        .expect("minimal project parses");
        p.id = Uuid::new_v4();
        assert!(p.parts.is_empty(), "absent key parses as no parts");

        // A project with no parts must not grow the key.
        assert!(!serialize_project(&p).expect("serialize").contains("parts:"));

        let overture = Uuid::new_v4();
        p.parts = crate::Parts(vec![
            crate::Part {
                id: overture,
                name: "Overture".into(),
            },
            crate::Part {
                id: Uuid::new_v4(),
                name: "Daybreak".into(),
            },
        ]);
        let raw = serialize_project(&p).expect("serialize");
        let back = crate::parse_str("Projects/album.md", "album", &raw).expect("re-parse");
        assert_eq!(back.parts, p.parts);
        assert_eq!(
            back.parts.get(overture).map(|x| x.name.as_str()),
            Some("Overture")
        );
    }

    /// A hand-written `parts:` list of bare names gets stable ids.
    ///
    /// Stable meaning the same on every scan and every machine — the
    /// same v5-over-the-path trick the page's own id fallback uses,
    /// because a per-scan id breaks every pointer that was written
    /// against the last one.
    #[test]
    fn hand_written_part_names_get_ids_that_do_not_move() {
        let raw = "---\ntype: project\nid: 018f0000-0000-7000-8000-000000000001\ntitle: Crescendum\nparts:\n  - Overture\n  - Daybreak\n---\n";
        let once = crate::parse_str("Projects/album.md", "album", raw).expect("parses");
        let twice = crate::parse_str("Projects/album.md", "album", raw).expect("parses");

        assert_eq!(once.parts.len(), 2);
        assert_eq!(once.parts.0[0].name, "Overture");
        assert_eq!(
            once.parts, twice.parts,
            "two scans of one page produced different part ids"
        );
        assert!(!once.parts.0[0].id.is_nil());
    }

    /// A page carrying only the legacy `projectType` reads as a
    /// capability set, and saving migrates it.
    #[test]
    fn a_legacy_project_type_migrates_on_save() {
        let mut p = crate::parse_str(
            "Projects/doc.md",
            "doc",
            "---\ntype: project\ntitle: Doc\nprojectType: video\n---\n",
        )
        .expect("parses");
        p.id = Uuid::new_v4();
        assert_eq!(
            p.capabilities.held,
            vec![crate::Capability::VideoProduction]
        );

        let raw = serialize_project(&p).expect("serialize");
        assert!(
            raw.contains("capabilities"),
            "saving must write the field that supersedes: {raw}"
        );
        assert!(
            !raw.contains("projectType"),
            "and must drop the one it superseded, or a page has two: {raw}"
        );

        let back = crate::parse_str("Projects/doc.md", "doc", &raw).expect("re-parse");
        assert_eq!(
            back.capabilities.held,
            vec![crate::Capability::VideoProduction]
        );
    }

    /// A `projectType` nobody can interpret is left exactly as written.
    ///
    /// Dropping it would destroy the only record of what its author
    /// meant, on a save that had nothing to do with it.
    #[test]
    fn an_uninterpretable_project_type_is_not_thrown_away() {
        let mut p = crate::parse_str(
            "Projects/odd.md",
            "odd",
            "---\ntype: project\ntitle: Odd\nprojectType: wedding-thing\n---\n",
        )
        .expect("parses");
        p.id = Uuid::new_v4();
        assert!(p.capabilities.is_empty(), "nothing was recognised");

        let raw = serialize_project(&p).expect("serialize");
        assert!(
            raw.contains("wedding-thing"),
            "an uninterpretable type must survive a save that could not \
             read it: {raw}"
        );
    }

    /// The slug rule, and the fallback for a title that slugifies to
    /// nothing — which lands under `Projects/` like every other title.
    /// It used to drop the folder and write to the vault root.
    #[test]
    fn default_path_slugs_the_title() {
        assert_eq!(
            default_project_path("Mobile  Push!"),
            "Projects/mobile-push.md"
        );
        assert_eq!(default_project_path("!!!"), "Projects/untitled-project.md");
    }
}
