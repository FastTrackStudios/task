//! YAML frontmatter: split, parse, and re-emit.
//!
//! Before this module every entity slice carried its own copy of the
//! four-line splitter under one of three names (`split_frontmatter`,
//! `frontmatter_split`, `parse_frontmatter`). They agreed on behaviour
//! by luck, not by construction.

use serde::Serialize;

use crate::error::WriteError;

/// Split `---\n<yaml>\n---\n<body>` into `(yaml, body)`.
///
/// Returns `None` when the text does not open with a frontmatter
/// fence or the fence is unterminated — callers treat that as "not
/// one of my pages" rather than an error.
pub fn split(src: &str) -> Option<(&str, &str)> {
    let rest = src.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some((&rest[..end], &rest[end + 5..]))
}

/// Split and parse the frontmatter into a YAML mapping.
pub fn mapping(src: &str) -> Option<(serde_yaml::Mapping, &str)> {
    let (fm, body) = split(src)?;
    let map = serde_yaml::from_str::<serde_yaml::Mapping>(fm).ok()?;
    Some((map, body))
}

/// True when the page's frontmatter marks it as `ty`, either via
/// `type: <ty>` or by carrying `<ty>` in its `tags` sequence.
///
/// This is the discriminator every slice's `looks_like_*` function
/// implemented by hand.
pub fn has_type(src: &str, ty: &str) -> bool {
    let Some((map, _)) = mapping(src) else {
        return false;
    };
    if map.get("type").and_then(|v| v.as_str()) == Some(ty) {
        return true;
    }
    map.get("tags")
        .and_then(|v| v.as_sequence())
        .is_some_and(|seq| seq.iter().any(|v| v.as_str() == Some(ty)))
}

/// Serialize `value` as frontmatter with a leading `type: <ty>` key,
/// followed by `body`.
///
/// The `type` key is inserted first so the discriminator stays at the
/// top of the file where a human reading the markdown will see it.
/// `body` is normalized to start on its own line.
pub fn document<T: Serialize>(ty: &str, value: &T, body: &str) -> Result<String, WriteError> {
    let mut wrapper = serde_yaml::Mapping::new();
    wrapper.insert("type".into(), ty.into());

    let serialized = serde_yaml::to_value(value).map_err(|e| WriteError::Yaml(e.to_string()))?;
    if let serde_yaml::Value::Mapping(map) = serialized {
        for (k, v) in map {
            wrapper.insert(k, v);
        }
    }

    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(wrapper))
        .map_err(|e| WriteError::Yaml(e.to_string()))?;

    let body = if body.is_empty() {
        String::new()
    } else if body.starts_with('\n') {
        body.to_string()
    } else {
        format!("\n{body}")
    };

    Ok(format!("---\n{yaml}---\n{body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_well_formed_page() {
        let (fm, body) = split("---\nid: 7\n---\nhello\n").unwrap();
        assert_eq!(fm, "id: 7");
        assert_eq!(body, "hello\n");
    }

    #[test]
    fn rejects_missing_and_unterminated_fences() {
        assert!(split("no frontmatter here").is_none());
        assert!(split("---\nid: 7\nstill going").is_none());
    }

    #[test]
    fn type_matches_via_key_or_tag() {
        assert!(has_type("---\ntype: goal\n---\n", "goal"));
        assert!(has_type("---\ntags:\n  - goal\n---\n", "goal"));
        assert!(!has_type("---\ntype: task\n---\n", "goal"));
        assert!(!has_type("not a page", "goal"));
    }

    #[test]
    fn document_round_trips_through_split() {
        #[derive(serde::Serialize)]
        struct Doc {
            name: String,
        }
        let out = document(
            "goal",
            &Doc {
                name: "Ship it".into(),
            },
            "details",
        )
        .unwrap();
        assert!(has_type(&out, "goal"));
        let (_, body) = split(&out).unwrap();
        assert_eq!(body, "\ndetails");
    }

    #[test]
    fn document_keeps_an_empty_body_empty() {
        #[derive(serde::Serialize)]
        struct Doc {
            name: String,
        }
        let out = document("goal", &Doc { name: "x".into() }, "").unwrap();
        assert!(out.ends_with("---\n"));
    }
}
