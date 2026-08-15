//! Lenient accessors over a `serde_yaml::Mapping`.
//!
//! Hand-written vault frontmatter is loose: a scalar where a list was
//! expected, a number quoted as a string, a key that simply isn't
//! there. These readers coerce where it is unambiguous and return
//! `None`/empty otherwise, so a malformed field degrades one value
//! instead of failing the whole page.

use serde_yaml::Mapping;

/// String value at `key`, trimmed. Numbers and bools are accepted and
/// rendered as text; empty results become `None`.
pub fn str_at(map: &Mapping, key: &str) -> Option<String> {
    let v = map.get(key)?;
    let s = match v {
        serde_yaml::Value::String(s) => s.trim().to_string(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    (!s.is_empty()).then_some(s)
}

/// List of strings at `key`. A bare scalar is treated as a one-element
/// list — `tags: work` and `tags: [work]` mean the same thing to a
/// human, so they mean the same thing here.
pub fn string_list_at(map: &Mapping, key: &str) -> Vec<String> {
    match map.get(key) {
        Some(serde_yaml::Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|v| match v {
                serde_yaml::Value::String(s) => Some(s.trim().to_string()),
                serde_yaml::Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect(),
        Some(serde_yaml::Value::String(s)) if !s.trim().is_empty() => {
            vec![s.trim().to_string()]
        }
        _ => Vec::new(),
    }
}

/// Integer at `key`, accepting a numeric string.
pub fn i64_at(map: &Mapping, key: &str) -> Option<i64> {
    match map.get(key)? {
        serde_yaml::Value::Number(n) => n.as_i64(),
        serde_yaml::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Float at `key`, accepting a numeric string.
pub fn f64_at(map: &Mapping, key: &str) -> Option<f64> {
    match map.get(key)? {
        serde_yaml::Value::Number(n) => n.as_f64(),
        serde_yaml::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Bool at `key`, accepting `true/false/yes/no/1/0` in any case.
pub fn bool_at(map: &Mapping, key: &str) -> Option<bool> {
    match map.get(key)? {
        serde_yaml::Value::Bool(b) => Some(*b),
        serde_yaml::Value::Number(n) => n.as_i64().map(|i| i != 0),
        serde_yaml::Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "y" | "1" => Some(true),
            "false" | "no" | "n" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// RFC-3339 / ISO-8601 timestamp at `key`.
pub fn timestamp_at(map: &Mapping, key: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    str_at(map, key)?.parse().ok()
}

/// `YYYY-MM-DD` date at `key`.
pub fn date_at(map: &Mapping, key: &str) -> Option<chrono::NaiveDate> {
    str_at(map, key)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(src: &str) -> Mapping {
        serde_yaml::from_str(src).unwrap()
    }

    #[test]
    fn str_at_coerces_scalars_and_drops_empties() {
        let m = map("a: ' hi '\nb: 42\nc: true\nd: ''\ne: [1]");
        assert_eq!(str_at(&m, "a").as_deref(), Some("hi"));
        assert_eq!(str_at(&m, "b").as_deref(), Some("42"));
        assert_eq!(str_at(&m, "c").as_deref(), Some("true"));
        assert_eq!(str_at(&m, "d"), None);
        assert_eq!(str_at(&m, "e"), None);
        assert_eq!(str_at(&m, "missing"), None);
    }

    #[test]
    fn string_list_accepts_a_bare_scalar() {
        let m = map("tags: work\nother: [a, b]\nempty: []");
        assert_eq!(string_list_at(&m, "tags"), vec!["work"]);
        assert_eq!(string_list_at(&m, "other"), vec!["a", "b"]);
        assert!(string_list_at(&m, "empty").is_empty());
        assert!(string_list_at(&m, "missing").is_empty());
    }

    #[test]
    fn numbers_parse_from_strings() {
        let m = map("a: 7\nb: '8'\nc: 1.5\nd: '2.5'\ne: nope");
        assert_eq!(i64_at(&m, "a"), Some(7));
        assert_eq!(i64_at(&m, "b"), Some(8));
        assert_eq!(f64_at(&m, "c"), Some(1.5));
        assert_eq!(f64_at(&m, "d"), Some(2.5));
        assert_eq!(i64_at(&m, "e"), None);
    }

    #[test]
    fn bools_accept_the_usual_spellings() {
        let m = map("a: true\nb: 'Yes'\nc: NO\nd: 1\ne: maybe");
        assert_eq!(bool_at(&m, "a"), Some(true));
        assert_eq!(bool_at(&m, "b"), Some(true));
        assert_eq!(bool_at(&m, "c"), Some(false));
        assert_eq!(bool_at(&m, "d"), Some(true));
        assert_eq!(bool_at(&m, "e"), None);
    }
}
