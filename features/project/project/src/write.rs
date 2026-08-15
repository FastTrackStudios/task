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
