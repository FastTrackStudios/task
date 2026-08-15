//! `Meal` → markdown bytes + path helpers.

use std::path::{Path, PathBuf};

use chrono::Utc;
use thiserror::Error;

use crate::model::Meal;

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("yaml: {0}")]
    Yaml(String),
    #[error("io: {0}")]
    Io(String),
    #[error("file exists at {0}; refusing to overwrite (pass overwrite=true)")]
    Exists(String),
    #[error("bad path: {0}")]
    BadPath(String),
}

pub fn serialize_meal(meal: &Meal) -> Result<String, WriteError> {
    let mut wrapper = serde_yaml::Mapping::new();
    wrapper.insert("type".into(), "meal".into());
    let body_yaml = serde_yaml::to_value(meal).map_err(|e| WriteError::Yaml(e.to_string()))?;
    if let serde_yaml::Value::Mapping(m) = body_yaml {
        for (k, v) in m {
            wrapper.insert(k, v);
        }
    }
    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(wrapper))
        .map_err(|e| WriteError::Yaml(e.to_string()))?;
    let body = if meal.details.is_empty() {
        String::new()
    } else if meal.details.starts_with('\n') {
        meal.details.clone()
    } else {
        format!("\n{}", meal.details)
    };
    Ok(format!("---\n{yaml}---\n{body}"))
}

pub fn write_meal(
    vault_root: &Path,
    meal: &mut Meal,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if meal.path.is_empty() {
        return Err(WriteError::BadPath("meal.path is empty".into()));
    }
    let abs = vault_root.join(&meal.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    let now = Utc::now();
    if meal.date_created.is_none() {
        meal.date_created = Some(now);
    }
    meal.date_modified = Some(now);
    let body = serialize_meal(meal)?;
    std::fs::write(&abs, body).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}

/// Default layout: `Projects/Mealplan/meals/<YYYY-MM-DD>-<slot>.md`. Pass
/// `name` for the disambiguating slug when multiple meals
/// share a slot (e.g. `"prep batch"`); the date stays at the
/// front so weekly views read cleanly in directory listings.
#[must_use]
pub fn default_meal_path(
    date: chrono::NaiveDate,
    slot: &str,
    name: Option<&str>,
    folder: Option<&str>,
) -> String {
    let slug = match name {
        Some(n) => slugify(&format!("{slot}-{n}")),
        None => slugify(slot),
    };
    let date_str = date.format("%Y-%m-%d");
    match folder {
        Some(f) => format!("{}/{date_str}-{slug}.md", f.trim_end_matches('/')),
        None => format!("Projects/Mealplan/meals/{date_str}-{slug}.md"),
    }
}

pub(crate) fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("meal");
    }
    out
}
