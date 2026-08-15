//! Shared YAML helpers. Kept private to the `parse` module so
//! callers go through the per-entity functions.

use super::ParseError;

#[allow(dead_code)]
pub(crate) fn split_frontmatter(src: &str) -> Option<(&str, &str)> {
    let rest = src.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some((&rest[..end], &rest[end + 5..]))
}

pub(crate) fn parse_mapping(yaml: &str) -> Result<serde_yaml::Mapping, ParseError> {
    let value: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|e| ParseError::Frontmatter(e.to_string()))?;
    value
        .as_mapping()
        .cloned()
        .ok_or_else(|| ParseError::Frontmatter("expected mapping".into()))
}

pub(crate) fn take_str(map: &serde_yaml::Mapping, key: &str) -> Option<String> {
    map.get(serde_yaml::Value::String(key.into()))
        .and_then(|v| match v {
            serde_yaml::Value::String(s) => Some(s.clone()),
            serde_yaml::Value::Number(n) => Some(n.to_string()),
            serde_yaml::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        })
}

pub(crate) fn require_str(
    map: &serde_yaml::Mapping,
    key: &'static str,
) -> Result<String, ParseError> {
    take_str(map, key).ok_or(ParseError::MissingField { field: key })
}

pub(crate) fn take_u16(map: &serde_yaml::Mapping, key: &str) -> Option<u16> {
    map.get(serde_yaml::Value::String(key.into()))
        .and_then(serde_yaml::Value::as_u64)
        .and_then(|n| u16::try_from(n).ok())
}

pub(crate) fn take_bool(map: &serde_yaml::Mapping, key: &str) -> Option<bool> {
    map.get(serde_yaml::Value::String(key.into()))
        .and_then(serde_yaml::Value::as_bool)
}

pub(crate) fn take_sequence(
    map: &serde_yaml::Mapping,
    key: &str,
) -> Option<Vec<serde_yaml::Value>> {
    map.get(serde_yaml::Value::String(key.into()))
        .and_then(|v| v.as_sequence().cloned())
}
