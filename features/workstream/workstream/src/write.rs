//! `Workstream` → markdown bytes. Frontmatter carries
//! `type: workstream` for the parser discriminator.
//!
//! Serialization lives in [`crate::entity`]; this module keeps the
//! historical `workstream::write::*` paths working and owns the two
//! things the shared store doesn't cover — writing straight to a vault
//! root on disk, and the project-derived default path.

use std::path::{Path, PathBuf};

use chrono::Utc;
use vault_entity::store::VaultEntity;

pub use vault_entity::WriteError;

use crate::entity::Workstreams;
use crate::model::Workstream;

pub fn serialize_workstream(w: &Workstream) -> Result<String, WriteError> {
    Workstreams::to_markdown(w)
}

pub fn write_workstream(
    vault_root: &Path,
    w: &mut Workstream,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if w.path.is_empty() {
        return Err(WriteError::BadPath("workstream.path is empty".into()));
    }
    let abs = vault_root.join(&w.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    let now = Utc::now();
    Workstreams::on_create(w, now);
    Workstreams::on_update(w, now);
    let body = serialize_workstream(w)?;
    std::fs::write(&abs, body).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}

/// Default layout: a `workstreams/` subdir inside the project's
/// own folder — the exact sibling of `milestones/`. Given the
/// project's vault-relative path:
///
/// - `Projects/Health/Health.md` → `Projects/Health/workstreams/<ws-slug>.md`
/// - `Projects/Mealplan.md`      → `Projects/Mealplan/workstreams/<ws-slug>.md`
///
/// In the flat case the folder is created on first write; the
/// project file stays as a sibling of the new folder. Honors
/// the project's on-disk casing so existing `Projects/Health/`
/// trees stay one folder, not two.
///
/// Derived from the owning project rather than a fixed folder, so it
/// can't go through [`VaultEntity::default_path`] and stays here.
#[must_use]
pub fn default_workstream_path(project_rel_path: &str, title: &str) -> String {
    let ws = vault_entity::slugify(title, Workstreams::TYPE);
    // Derive the project's folder:
    // - if the project file is `X/X.md` or `X/something.md`, use `X/`
    // - if it's a flat `X.md`, use the file stem
    let p = std::path::Path::new(project_rel_path);
    let parent = p.parent().and_then(|x| x.to_str()).unwrap_or("");
    let stem = p.file_stem().and_then(|x| x.to_str()).unwrap_or("");
    if parent == "Projects" || parent.is_empty() {
        // Flat project file. Create a sibling folder named
        // after the stem (preserving its on-disk casing).
        if parent.is_empty() {
            format!("{stem}/workstreams/{ws}.md")
        } else {
            format!("{parent}/{stem}/workstreams/{ws}.md")
        }
    } else {
        // Nested: project lives inside its own folder already.
        // Just append `workstreams/`.
        format!("{parent}/workstreams/{ws}.md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentRefList, Links, Workstream};
    use workflows_proto::AgentRef;

    fn sample() -> Workstream {
        Workstream {
            path: "Projects/Demo/workstreams/acp-adapter.md".into(),
            id: uuid::Uuid::new_v4(),
            title: "ACP adapter".into(),
            project_id: uuid::Uuid::new_v4(),
            status: "in-progress".into(),
            lead: Some(AgentRef::human("cody")),
            members: AgentRefList(vec![
                AgentRef::agent_versioned("hermes", "h4"),
                AgentRef::agent("claude"),
            ]),
            start_date: Some(chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()),
            target_date: Some(chrono::NaiveDate::from_ymd_opt(2026, 7, 15).unwrap()),
            links: Links(vec!["https://example.com/prd".into()]),
            date_created: None,
            date_modified: None,
            details: "The charter.".into(),
        }
    }

    #[test]
    fn workstream_round_trips_through_markdown() {
        let ws = sample();
        let md = serialize_workstream(&ws).expect("serialize");
        assert!(md.contains("type: workstream"), "missing type:\n{md}");
        let parsed =
            crate::parse::parse_workstream(&ws.path, "acp-adapter", &md).expect("parse back");
        assert_eq!(parsed.id, ws.id);
        assert_eq!(parsed.title, ws.title);
        assert_eq!(parsed.project_id, ws.project_id);
        assert_eq!(parsed.status, "in-progress");
        assert_eq!(parsed.lead, ws.lead);
        assert_eq!(parsed.members, ws.members);
        assert_eq!(parsed.start_date, ws.start_date);
        assert_eq!(parsed.target_date, ws.target_date);
        assert_eq!(parsed.links, ws.links);
        assert_eq!(parsed.details.trim(), "The charter.");
    }

    #[test]
    fn default_path_mirrors_milestone_layout() {
        assert_eq!(
            default_workstream_path("Projects/Health/Health.md", "Mobile Push"),
            "Projects/Health/workstreams/mobile-push.md"
        );
        assert_eq!(
            default_workstream_path("Projects/Mealplan.md", "Mobile Push"),
            "Projects/Mealplan/workstreams/mobile-push.md"
        );
    }
}
