//! Load a [`Resource`] manifest from its `type: resource` markdown
//! frontmatter (`<org>/resources/**/<slug>.md`).

use std::path::Path;

use crate::ResourceError;
use crate::types::Resource;

/// Parse a resource manifest from a markdown document — the YAML between
/// the leading `---` fences. Unknown frontmatter keys (`key`,
/// `progression`, …) are ignored.
pub fn parse_manifest(markdown: &str) -> Result<Resource, ResourceError> {
    let yaml = frontmatter(markdown).ok_or(ResourceError::NoFrontmatter)?;
    serde_yaml::from_str(yaml).map_err(|e| ResourceError::Yaml(e.to_string()))
}

/// Read + parse a manifest file from disk.
pub fn load_manifest(path: impl AsRef<Path>) -> Result<Resource, ResourceError> {
    let text = std::fs::read_to_string(path).map_err(|e| ResourceError::Io(e.to_string()))?;
    parse_manifest(&text)
}

/// Extract the YAML frontmatter block (between the first two `---` lines).
fn frontmatter(markdown: &str) -> Option<&str> {
    let rest = markdown.strip_prefix("---")?;
    // Tolerate `---\n` or `---\r\n`.
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ResourceKind;

    #[test]
    fn parses_song_manifest() {
        // Raw string so the YAML indentation survives verbatim.
        let md = r"---
type: resource
resource_kind: song
slug: keep-on-finding-more
title: Keep On Finding More
writers: [John Allan, Mack Brock]
readonly: true
media:
  - kind: video
    provider: youtube
    url: https://youtu.be/xu5N7GA_LjA
---

# body
";
        let r = parse_manifest(md).unwrap();
        assert_eq!(r.slug, "keep-on-finding-more");
        assert_eq!(r.kind, ResourceKind::Song);
        assert_eq!(r.writers.len(), 2);
        assert!(r.readonly);
        assert_eq!(r.media_of("video").unwrap().provider, "youtube");
    }

    #[test]
    fn missing_frontmatter_errors() {
        assert!(matches!(
            parse_manifest("# no frontmatter"),
            Err(ResourceError::NoFrontmatter)
        ));
    }
}
