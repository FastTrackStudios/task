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
    let Some((fm, _)) = split(src) else {
        return false;
    };
    // Exactly `serde_yaml::from_str`'s path, with a seed in place of a
    // type: one document, the same errors for anything else.
    serde::de::DeserializeSeed::deserialize(TypeProbe(ty), serde_yaml::Deserializer::from_str(fm))
        .unwrap_or(false)
}

/// [`has_type`]'s reading of a frontmatter block: a mapping whose
/// `type` is `ty` or whose `tags` sequence holds `ty`.
///
/// A visitor rather than a parse into `serde_yaml::Mapping`, which built
/// an owned tree of every key and value on the page (and dropped it) to
/// read two of them, on every page of every entity list. Here the two
/// values it decides on are read and every other value is skipped as
/// `IgnoredAny`. Like the mapping parse, it refuses anything that is
/// not a mapping, so a sequence or scalar frontmatter is never a match.
/// The one difference: a block that repeats a key, which the `Mapping`
/// refused as a whole, is read here (the decision is any `type` or
/// `tags` occurrence matching).
struct TypeProbe<'t>(&'t str);

impl<'de> serde::de::DeserializeSeed<'de> for TypeProbe<'_> {
    type Value = bool;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<bool, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> serde::de::Visitor<'de> for TypeProbe<'_> {
    type Value = bool;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a frontmatter mapping")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<bool, A::Error> {
        let mut found = false;
        while let Some(key) = map.next_key::<serde_yaml::Value>()? {
            match key.as_str() {
                Some("type") => {
                    let v: serde_yaml::Value = map.next_value()?;
                    found |= v.as_str() == Some(self.0);
                }
                Some("tags") => {
                    let v: serde_yaml::Value = map.next_value()?;
                    found |= v
                        .as_sequence()
                        .is_some_and(|seq| seq.iter().any(|t| t.as_str() == Some(self.0)));
                }
                _ => {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
        }
        Ok(found)
    }
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

    /// The old reading, kept here as the oracle: parse the whole
    /// frontmatter into a `Mapping` and look at two keys.
    fn has_type_by_mapping(src: &str, ty: &str) -> bool {
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

    /// The visitor decides every shape of frontmatter the way the
    /// mapping parse it replaced did — including the ones that are not
    /// a mapping at all, a broken document, and keys it skips without
    /// building.
    #[test]
    fn the_visitor_decides_like_the_mapping_parse() {
        let pages = [
            "---\ntype: goal\n---\n",
            "---\ntype: goal\ntitle: x\nnested:\n  deep: [1, 2, {a: b}]\n---\nbody",
            "---\ntags: [a, goal]\n---\n",
            "---\ntags: goal\n---\n",
            "---\ntags:\n  - 7\n  - goal\n---\n",
            "---\ntype: [goal]\n---\n",
            "---\ntype: Goal\n---\n",
            "---\n- type\n- goal\n---\n",
            "---\ngoal\n---\n",
            "---\n\n---\n",
            "---\ntype: goal\n  bad: indent: here\n---\n",
            "---\n7: goal\ntype: goal\n---\n",
            "---\ntype: task\ntags: [goal]\n---\n",
            "no frontmatter",
        ];
        for page in pages {
            for ty in ["goal", "task"] {
                assert_eq!(
                    has_type(page, ty),
                    has_type_by_mapping(page, ty),
                    "{page:?} as {ty}"
                );
            }
        }
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
