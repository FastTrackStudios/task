//! `Goal` → markdown bytes + path helpers.
//!
//! Serialization lives in [`crate::entity`]; this module keeps the
//! historical `goal::write::*` paths working and adds the one thing
//! the shared store doesn't cover — writing a goal straight to a vault
//! root on disk, without an in-memory `Vault`.

use std::path::{Path, PathBuf};

use chrono::Utc;
use vault_entity::store::VaultEntity;

pub use vault_entity::WriteError;

use crate::entity::Goals;
use crate::model::Goal;

/// Render a goal as a full markdown page. Frontmatter always carries
/// `type: goal` so the parser has a single discriminator; empty
/// optional fields are skipped to keep new files terse.
pub fn serialize_goal(goal: &Goal) -> Result<String, WriteError> {
    Goals::to_markdown(goal)
}

/// Write `goal` to `<vault_root>/<goal.path>`, creating parent
/// directories.
pub fn write_goal(
    vault_root: &Path,
    goal: &mut Goal,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if goal.path.is_empty() {
        return Err(WriteError::BadPath("goal.path is empty".into()));
    }
    let abs = vault_root.join(&goal.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    let now = Utc::now();
    Goals::on_create(goal, now);
    Goals::on_update(goal, now);
    let body = serialize_goal(goal)?;
    std::fs::write(&abs, body).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}

/// Default layout: `Goals/<slug>.md` for top-level; nested
/// decompositions can live at `Goals/<parent-slug>/<slug>.md`
/// — pass `folder` to override.
#[must_use]
pub fn default_goal_path(title: &str, folder: Option<&str>) -> String {
    Goals::default_path(title, folder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Goal, Tags};
    use uuid::Uuid;

    #[test]
    fn round_trip_minimal() {
        let mut g = Goal {
            path: "Goals/buy-a-house.md".into(),
            id: Uuid::new_v4(),
            title: "Buy a House".into(),
            kind: "lifetime".into(),
            status: "aspiration".into(),
            parent_id: None,
            target_date: None,
            cycle_id: None,
            tags: Tags(vec!["housing".into()]),
            date_created: None,
            date_modified: None,
            details: "Vision: own a place by 35.".into(),
        };
        let tmp = tempfile::tempdir().unwrap();
        let path = write_goal(tmp.path(), &mut g, false).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("type: goal"));
        assert!(raw.contains("title: Buy a House"));
        assert!(raw.contains("Vision"));

        let parsed = crate::parse::parse_goal(&g.path, "buy-a-house", &raw).unwrap();
        assert_eq!(parsed.title, g.title);
        assert_eq!(parsed.kind, g.kind);
        assert_eq!(parsed.id, g.id);
    }
}
