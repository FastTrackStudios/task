//! `Milestone` → markdown bytes. Frontmatter carries
//! `type: milestone` for the parser discriminator.
//!
//! Serialization lives in [`crate::entity`]; this module keeps the
//! historical `milestone::write::*` paths working and owns the two
//! things the shared store doesn't cover — writing straight to a vault
//! root on disk, and the project-derived default path.

use std::path::{Path, PathBuf};

use chrono::Utc;
use vault_entity::store::VaultEntity;

pub use vault_entity::WriteError;

use crate::entity::Milestones;
use crate::model::Milestone;

pub fn serialize_milestone(m: &Milestone) -> Result<String, WriteError> {
    Milestones::to_markdown(m)
}

pub fn write_milestone(
    vault_root: &Path,
    m: &mut Milestone,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if m.path.is_empty() {
        return Err(WriteError::BadPath("milestone.path is empty".into()));
    }
    let abs = vault_root.join(&m.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    let now = Utc::now();
    Milestones::on_create(m, now);
    Milestones::on_update(m, now);
    let body = serialize_milestone(m)?;
    // Through the vault's write path rather than `std::fs::write`: atomic
    // on disk, and routed through the Files API once the vault is a File
    // Root (`project.vault.write-path`).
    vault::save_page_at(vault_root, &m.path, &body).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}

/// Default layout: a `milestones/` subdir inside the project's
/// own folder. Given the project's vault-relative path:
///
/// - `Projects/Health/Health.md` → `Projects/Health/milestones/<ms-slug>.md`
/// - `Projects/Mealplan.md`      → `Projects/Mealplan/milestones/<ms-slug>.md`
///
/// In the flat case the folder is created on first write; the
/// project file stays as a sibling of the new folder. Honors
/// the project's on-disk casing so existing `Projects/Health/`
/// trees stay one folder, not two.
///
/// Derived from the owning project rather than a fixed folder, so it
/// can't go through [`VaultEntity::default_path`] and stays here.
#[must_use]
pub fn default_milestone_path(project_rel_path: &str, title: &str) -> String {
    let ms = vault_entity::slugify(title, Milestones::TYPE);
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
            format!("{stem}/milestones/{ms}.md")
        } else {
            format!("{parent}/{stem}/milestones/{ms}.md")
        }
    } else {
        // Nested: project lives inside its own folder already.
        // Just append `milestones/`.
        format!("{parent}/milestones/{ms}.md")
    }
}
