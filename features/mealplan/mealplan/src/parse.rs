//! `vault::VaultPage` → `Meal`.

use thiserror::Error;
use uuid::Uuid;
use vault::VaultPage;

use crate::model::{Meal, PantryDeduction};

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("page has no frontmatter")]
    NoFrontmatter,
    #[error("frontmatter is not a YAML mapping")]
    NotAMapping,
    #[error("frontmatter parse: {0}")]
    Yaml(String),
    #[error("missing required field: {0}")]
    MissingField(&'static str),
}

#[must_use]
pub fn looks_like_meal(page: &VaultPage) -> bool {
    let Some((fm, _)) = split_frontmatter(&page.raw) else {
        return false;
    };
    let Ok(map) = serde_yaml::from_str::<serde_yaml::Mapping>(fm) else {
        return false;
    };
    if map.get("type").and_then(|v| v.as_str()) == Some("meal") {
        return true;
    }
    if let Some(seq) = map.get("tags").and_then(|v| v.as_sequence()) {
        return seq.iter().any(|v| v.as_str() == Some("meal"));
    }
    false
}

pub fn parse_page(page: &VaultPage) -> Result<Meal, ParseError> {
    let (fm, body) = split_frontmatter(&page.raw).ok_or(ParseError::NoFrontmatter)?;
    let map: serde_yaml::Mapping =
        serde_yaml::from_str(fm).map_err(|e| ParseError::Yaml(e.to_string()))?;

    let id = take_str(&map, "id")
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, page.rel_path.as_bytes()));
    let name = take_str(&map, "name").unwrap_or_else(|| page.basename.clone());
    let scheduled_for = take_str(&map, "scheduledFor")
        .and_then(|s| s.parse().ok())
        .ok_or(ParseError::MissingField("scheduledFor"))?;
    let slot = take_str(&map, "slot").unwrap_or_else(|| "dinner".into());
    let servings = map
        .get("servings")
        .and_then(serde_yaml::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(1);
    let recipe_paths = map
        .get("recipePaths")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str())
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let status = take_str(&map, "status").unwrap_or_else(|| "planned".into());
    let pantry_deductions = map
        .get("pantryDeductions")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|row| {
                    let m = row.as_mapping()?;
                    let item_id = m
                        .get("itemId")
                        .and_then(|v| v.as_str())
                        .and_then(|s| Uuid::parse_str(s).ok())?;
                    let qty = m.get("qty").and_then(serde_yaml::Value::as_f64)?;
                    let unit = m
                        .get("unit")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    Some(PantryDeduction { item_id, qty, unit })
                })
                .collect()
        })
        .unwrap_or_default();
    let tags = take_string_list(&map, "tags")
        .into_iter()
        .filter(|t| t != "meal")
        .collect();
    let date_created = take_str(&map, "dateCreated").and_then(|s| s.parse().ok());
    let date_modified = take_str(&map, "dateModified").and_then(|s| s.parse().ok());

    Ok(Meal {
        path: page.rel_path.clone(),
        id,
        name,
        scheduled_for,
        slot,
        servings,
        recipe_paths: crate::model::StringList(recipe_paths),
        status,
        pantry_deductions: crate::model::PantryDeductions(pantry_deductions),
        tags: crate::model::StringList(tags),
        date_created,
        date_modified,
        details: body.to_string(),
    })
}

pub(crate) fn split_frontmatter(src: &str) -> Option<(&str, &str)> {
    let rest = src.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some((&rest[..end], &rest[end + 5..]))
}

fn take_str(map: &serde_yaml::Mapping, key: &str) -> Option<String> {
    map.get(key).and_then(|v| match v {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

fn take_string_list(map: &serde_yaml::Mapping, key: &str) -> Vec<String> {
    let Some(v) = map.get(key) else {
        return Vec::new();
    };
    match v {
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|item| item.as_str().map(std::string::ToString::to_string))
            .collect(),
        serde_yaml::Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}
