//! `BodyMetric` → markdown bytes + path helpers.
//!
//! Serialization lives in [`crate::entity`]; this module keeps the
//! historical `body::write::*` paths working and adds the one thing
//! the shared store doesn't cover — writing a metric straight to a
//! vault root on disk, without an in-memory `Vault`.

use std::path::{Path, PathBuf};

use chrono::Utc;
use vault_entity::store::VaultEntity;

pub use vault_entity::WriteError;

use crate::entity::BodyMetrics;
use crate::model::BodyMetric;

/// Render a metric as a full markdown page.
pub fn serialize_metric(m: &BodyMetric) -> Result<String, WriteError> {
    BodyMetrics::to_markdown(m)
}

/// Default layout: `Projects/Fitness/body/<slug>.md` (e.g.
/// `Projects/Fitness/body/weight.md`, `.../bodyfat.md`).
pub fn default_metric_path(name: &str, folder: Option<&str>) -> String {
    BodyMetrics::default_path(name, folder)
}

/// Write `m` to `<vault_root>/<m.path>`, creating parent directories.
pub fn write_metric(
    vault_root: &Path,
    m: &mut BodyMetric,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if m.path.is_empty() {
        return Err(WriteError::BadPath("metric.path is empty".into()));
    }
    let abs = vault_root.join(&m.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    let now = Utc::now();
    BodyMetrics::on_create(m, now);
    BodyMetrics::on_update(m, now);
    let body = serialize_metric(m)?;
    std::fs::write(&abs, body).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}
