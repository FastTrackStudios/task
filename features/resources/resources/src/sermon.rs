//! The sermon resource on disk — what the sync writes and what it must
//! never touch.
//!
//! A synced sermon is three files under
//! `resources/sermons/<folder>/`:
//!
//! - `<slug>.md` — the `type: resource` manifest. The sync owns the
//!   frontmatter fields that describe the video and its captions
//!   ([`SYNC_OWNED`]); everything else — extra keys someone added, the
//!   whole body — is the reader's, and a re-sync keeps it verbatim.
//! - `<slug>.transcript.json` — the cues. The sync owns it outright.
//! - `<slug>.annotations.json` — the annotation sidecar. Created empty
//!   once; never rewritten by the sync.
//!
//! The slug is the kebab-case title, and it belongs to the *video id*:
//! a second video with the same title gets the id appended, and a
//! re-sync of a known id finds its slug through the manifest's
//! `media[].id` rather than re-deriving it.

use resources_proto::SermonResource;
use serde_yaml::{Mapping, Value};

use crate::ResourceError;
use crate::transcript::{Transcript, TranscriptSegment};
use crate::types::Resource;

/// Frontmatter keys the sync rewrites on every run. Every other key in
/// an existing manifest survives untouched.
pub const SYNC_OWNED: &[&str] = &[
    "media",
    "published",
    "duration_secs",
    "tags",
    "scripture",
    "source",
    "caption_kind",
    "language",
];

/// `source:` value for caption-sourced transcripts.
pub const SOURCE: &str = "youtube-captions";

/// Kebab-case a title: lower-case ASCII letters and digits, runs of
/// anything else collapsed to one hyphen, trimmed. Non-ASCII letters
/// are dropped (a title that is *only* those yields an empty slug —
/// callers fall back to the video id).
#[must_use]
pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut pending_dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c.to_ascii_lowercase());
        } else if c == '\'' || c == '\u{2019}' || (!c.is_ascii() && c.is_alphabetic()) {
            // "God's" → "gods", not "god-s"; a non-ASCII letter drops
            // out without breaking the word around it.
        } else {
            pending_dash = true;
        }
    }
    out
}

/// The slug this video gets, given the sermons already on disk:
/// the slug of the manifest whose `media[].id` matches, else the
/// kebab title, else (title collision with a *different* video) the
/// title with the video id appended.
#[must_use]
pub fn slug_for(existing: &[(String, Resource)], video_id: &str, title: &str) -> String {
    for (slug, r) in existing {
        if r.media.iter().any(|m| !m.id.is_empty() && m.id == video_id) {
            return slug.clone();
        }
    }
    let base = {
        let s = slugify(title);
        if s.is_empty() { slugify(video_id) } else { s }
    };
    let taken = existing.iter().any(|(slug, _)| *slug == base);
    if taken {
        format!("{base}-{}", slugify(video_id))
    } else {
        base
    }
}

/// `MM:SS` (or `H:MM:SS` past an hour).
#[must_use]
pub fn mmss(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The sync-owned frontmatter as YAML values, in the order they are
/// written.
fn owned_values(sermon: &SermonResource, scripture: &[String]) -> Vec<(&'static str, Value)> {
    let mut media = Mapping::new();
    media.insert("kind".into(), "video".into());
    media.insert("provider".into(), "youtube".into());
    media.insert("url".into(), sermon.video_url.clone().into());
    media.insert("id".into(), sermon.video_id.clone().into());
    let strs = |v: &[String]| Value::Sequence(v.iter().map(|s| Value::from(s.as_str())).collect());
    vec![
        ("tags", strs(&sermon.tags)),
        ("published", sermon.published.clone().into()),
        ("duration_secs", Value::from(sermon.duration_secs)),
        ("media", Value::Sequence(vec![Value::Mapping(media)])),
        ("source", SOURCE.into()),
        ("caption_kind", sermon.caption_kind.clone().into()),
        ("language", sermon.language.clone().into()),
        ("scripture", strs(scripture)),
    ]
}

fn yaml(mapping: &Mapping) -> Result<String, ResourceError> {
    serde_yaml::to_string(mapping).map_err(|e| ResourceError::Yaml(e.to_string()))
}

/// A fresh manifest: frontmatter plus the body the reader will fill
/// in. Nothing here is invented — the outline is theirs to write.
pub fn render_manifest(
    sermon: &SermonResource,
    slug: &str,
    scripture: &[String],
) -> Result<String, ResourceError> {
    let mut fm = Mapping::new();
    fm.insert("type".into(), "resource".into());
    fm.insert("resource_kind".into(), "sermon".into());
    fm.insert("slug".into(), slug.into());
    fm.insert("title".into(), sermon.title.clone().into());
    fm.insert(
        "writers".into(),
        Value::Sequence(vec![sermon.channel.clone().into()]),
    );
    fm.insert("readonly".into(), Value::Bool(true));
    for (k, v) in owned_values(sermon, scripture) {
        fm.insert(k.into(), v);
    }
    Ok(format!(
        "---\n{}---\n{}",
        yaml(&fm)?,
        render_body(sermon, slug)
    ))
}

fn render_body(sermon: &SermonResource, slug: &str) -> String {
    let cues = sermon.segments.len();
    format!(
        "<!-- READ-ONLY RESOURCE. The full timestamped transcript is the sidecar `{slug}.transcript.json` ({cues} cues, {}). Annotations attach by timestamp: sermon:{slug}#t:<seconds> -->\n\
# {}\n\
*{} · [video]({}) · {}*\n\
\n\
## Notes\n\
\n\
_Timestamped notes go here; each `MM:SS` is a sermon:{slug}#t:<secs> anchor._\n",
        mmss(sermon.duration_secs),
        sermon.title,
        sermon.channel,
        sermon.video_url,
        mmss(sermon.duration_secs),
    )
}

/// Re-sync an existing manifest: rewrite only the [`SYNC_OWNED`]
/// frontmatter keys, keep every other key and the whole body byte for
/// byte.
pub fn refresh_manifest(
    existing: &str,
    sermon: &SermonResource,
    scripture: &[String],
) -> Result<String, ResourceError> {
    let (fm_text, body) = split(existing).ok_or(ResourceError::NoFrontmatter)?;
    let mut fm: Mapping =
        serde_yaml::from_str(fm_text).map_err(|e| ResourceError::Yaml(e.to_string()))?;
    for (k, v) in owned_values(sermon, scripture) {
        fm.insert(k.into(), v);
    }
    Ok(format!("---\n{}---\n{body}", yaml(&fm)?))
}

/// `(frontmatter yaml, body after the closing fence)`.
fn split(markdown: &str) -> Option<(&str, &str)> {
    let rest = markdown.strip_prefix("---")?;
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))?;
    let end = rest.find("\n---")?;
    let after = &rest[end + 4..];
    let body = after
        .strip_prefix("\r\n")
        .or_else(|| after.strip_prefix('\n'))
        .unwrap_or(after);
    Some((&rest[..end], body))
}

/// The transcript sidecar for a synced sermon.
#[must_use]
pub fn transcript_of(sermon: &SermonResource, slug: &str) -> Transcript {
    let source = match sermon.caption_kind.as_str() {
        "manual" => "youtube-manual",
        _ => "youtube-auto",
    };
    let mut t = Transcript::new(slug, source);
    t.segments = sermon
        .segments
        .iter()
        .map(|s| TranscriptSegment {
            start: s.start,
            dur: s.dur,
            text: s.text.clone(),
        })
        .collect();
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse_manifest;
    use crate::types::{MediaRef, ResourceKind};

    fn sermon() -> SermonResource {
        SermonResource {
            folder: "crossroads".into(),
            video_id: "YMypVgZXFIU".into(),
            video_url: "https://youtu.be/YMypVgZXFIU".into(),
            title: "God Restores Broken People".into(),
            channel: "Crossroads Church".into(),
            tags: vec!["sermon".into(), "crossroads".into()],
            published: "2026-06-14".into(),
            duration_secs: 2856,
            caption_kind: "auto".into(),
            language: "en".into(),
            segments: vec![resources_proto::TranscriptSegment {
                start: 0.08,
                dur: 4.4,
                text: "welcome".into(),
            }],
        }
    }

    #[test]
    fn slugify_kebabs() {
        assert_eq!(
            slugify("God Restores Broken People"),
            "god-restores-broken-people"
        );
        assert_eq!(slugify("  What's Next?! (Part 2) "), "whats-next-part-2");
        assert_eq!(slugify("Ünïcode — only"), "ncode-only");
        assert_eq!(slugify("Крест"), "");
    }

    #[test]
    fn slug_is_stable_per_video_id() {
        let known = parse_manifest(
            "---\ntype: resource\nresource_kind: sermon\nslug: old-title\nmedia:\n  - kind: video\n    provider: youtube\n    url: https://youtu.be/AAA\n    id: AAA\n---\n",
        )
        .unwrap();
        assert_eq!(known.kind, ResourceKind::Sermon);
        assert_eq!(
            known.media[0],
            MediaRef {
                kind: "video".into(),
                provider: "youtube".into(),
                url: "https://youtu.be/AAA".into(),
                id: "AAA".into(),
            }
        );
        let existing = vec![("old-title".to_string(), known)];
        // Same id, renamed video → same slug.
        assert_eq!(slug_for(&existing, "AAA", "New Title"), "old-title");
        // Different video whose title kebabs to a taken slug → id suffix.
        assert_eq!(slug_for(&existing, "BBB_x", "Old Title"), "old-title-bbb-x");
        // Fresh title → plain kebab.
        assert_eq!(slug_for(&existing, "CCC", "Fresh"), "fresh");
        // Untitled → the id.
        assert_eq!(slug_for(&[], "DDD", "???"), "ddd");
    }

    #[test]
    fn manifest_renders_frontmatter_and_placeholder_body() {
        let md = render_manifest(
            &sermon(),
            "god-restores-broken-people",
            &["1Pet.5.7".into()],
        )
        .unwrap();
        let r = parse_manifest(&md).unwrap();
        assert_eq!(r.slug, "god-restores-broken-people");
        assert_eq!(r.kind, ResourceKind::Sermon);
        assert_eq!(r.writers, ["Crossroads Church"]);
        assert_eq!(r.tags, ["sermon", "crossroads"]);
        assert_eq!(r.published, "2026-06-14");
        assert_eq!(r.duration_secs, 2856);
        assert_eq!(r.scripture, ["1Pet.5.7"]);
        assert_eq!(r.source, SOURCE);
        assert_eq!(r.caption_kind, "auto");
        assert_eq!(r.language, "en");
        assert!(r.readonly);
        let v = r.media_of("video").unwrap();
        assert_eq!(v.id, "YMypVgZXFIU");
        assert_eq!(v.url, "https://youtu.be/YMypVgZXFIU");
        assert!(md.contains("# God Restores Broken People\n"));
        assert!(md.contains("*Crossroads Church · [video](https://youtu.be/YMypVgZXFIU) · 47:36*"));
        assert!(md.contains("## Notes\n\n_Timestamped notes go here"));
        assert!(md.contains("sermon:god-restores-broken-people#t:<seconds>"));
    }

    #[test]
    fn refresh_keeps_body_and_foreign_keys() {
        let existing = "---\ntype: resource\nresource_kind: sermon\nslug: s\ntitle: Hand Title\nwriters: [Crossroads Church]\npassage: \"1 Peter 5\"\nreadonly: true\nmedia:\n  - kind: video\n    provider: youtube\n    url: https://youtu.be/YMypVgZXFIU\n---\n# Hand Title\n## Outline\n- `0:00` — Welcome\n";
        let out = refresh_manifest(existing, &sermon(), &["1Pet.5".into()]).unwrap();
        assert!(out.ends_with("---\n# Hand Title\n## Outline\n- `0:00` — Welcome\n"));
        let r = parse_manifest(&out).unwrap();
        assert_eq!(r.title, "Hand Title", "title is not sync-owned");
        assert_eq!(r.published, "2026-06-14");
        assert_eq!(r.media[0].id, "YMypVgZXFIU");
        assert_eq!(r.scripture, ["1Pet.5"]);
        assert!(
            out.contains("passage: 1 Peter 5"),
            "foreign key survives: {out}"
        );
    }

    #[test]
    fn mmss_formats() {
        assert_eq!(mmss(2856), "47:36");
        assert_eq!(mmss(5), "0:05");
        assert_eq!(mmss(3661), "1:01:01");
    }
}
