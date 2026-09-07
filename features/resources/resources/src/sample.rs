//! The Signal sample on disk — a *directory* under
//! `<org>/resources/samples/`, holding everything about a sample
//! except the sample.
//!
//! - `<slug>/sample.md` — the `type: resource`, `resource_kind: sample`
//!   manifest: title, tags, duration, rate, and the File Root binding.
//! - `<slug>/sample.json` — the caller's metadata notes, byte for byte.
//!
//! # The audio is not here
//!
//! It lives in a File Root, named by `resources_proto::ContentRef`, and
//! this lane never moves a byte of it. That is not an omission: the
//! Files layer has large-content versioning, selective sync with
//! dehydrated stubs, Peaks renditions and chunked streaming, and
//! `resources/` has none of it. Putting a sample library's audio in
//! `resources/` would make every subscriber to that library pull all of
//! it just to read a list of names — while the manifests, which are
//! what a `sample:<slug>` reference actually needs, are small enough to
//! carry across an org boundary for free.
//!
//! So a *sample library* is an ordinary `Collection` of kind `Library`
//! over `sample:<slug>` references (ADR 0003), and subscribing to it is
//! cheap by construction.

use resources_proto::SampleDoc;

use crate::ResourceError;
use crate::asset::{self, Owned};

/// Frontmatter keys the sample's owning app rewrites on every upsert.
pub const APP_OWNED: &[&str] = &[
    "title",
    "tags",
    "duration_secs",
    "sample_rate",
    "content_root",
    "content_path",
    "updated_at",
];

/// `source:` value on a sample manifest — which app owns it.
pub const SOURCE: &str = "signal";

/// `resource_kind:` of a sample manifest.
pub const KIND: &str = "sample";

/// The manifest inside a sample's directory.
pub const MANIFEST: &str = "sample.md";

/// The metadata file beside it. **Not the audio.**
pub const BODY: &str = "sample.json";

/// The metadata file beside a sample manifest (`sample.json`).
#[must_use]
pub fn body_path(md_path: &std::path::Path) -> std::path::PathBuf {
    md_path.with_file_name(BODY)
}

/// The slug this sample gets — [`asset::slug_for`]'s rule.
#[must_use]
pub fn slug_for(taken: &[String], sample: &SampleDoc) -> String {
    asset::slug_for(taken, &sample.slug, &sample.title)
}

/// The app-owned frontmatter as YAML, in the order it is written.
fn owned_values(sample: &SampleDoc) -> Vec<Owned> {
    let mut v: Vec<Owned> = vec![
        ("title", sample.title.clone().into()),
        ("tags", asset::strings(&sample.tags)),
        ("duration_secs", sample.duration_secs.into()),
        ("sample_rate", sample.sample_rate.into()),
    ];
    v.extend(asset::content_keys(
        &sample.content.root_id,
        &sample.content.path,
    ));
    v.push(("updated_at", sample.updated_at.clone().into()));
    v
}

/// A fresh manifest. Its body says where the audio is, because that is
/// the first question anyone opening this file will have.
pub fn render_manifest(sample: &SampleDoc, slug: &str) -> Result<String, ResourceError> {
    asset::render_manifest(
        KIND,
        slug,
        SOURCE,
        owned_values(sample),
        &render_body(sample, slug),
    )
}

fn render_body(sample: &SampleDoc, slug: &str) -> String {
    format!(
        "<!-- This directory holds what the sample *is*; the audio lives in the File Root named by `content_root` / `content_path`. Reference it as sample:{slug}, a region as sample:{slug}#t:0-2:400. -->\n\
# {}\n\
\n\
## Notes\n\
\n\
_How it was captured, and what it is for._\n",
        sample.title,
    )
}

/// Re-upsert an existing manifest: rewrite only the [`APP_OWNED`]
/// frontmatter keys, keep every other key and the whole body.
pub fn refresh_manifest(existing: &str, sample: &SampleDoc) -> Result<String, ResourceError> {
    asset::refresh_manifest(existing, owned_values(sample))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse_manifest;
    use crate::types::ResourceKind;
    use resources_proto::ContentRef;

    fn sample() -> SampleDoc {
        SampleDoc {
            slug: String::new(),
            title: "Room Kick 48k".into(),
            tags: vec!["kick".into(), "room".into()],
            duration_secs: 2,
            sample_rate: 48_000,
            body: "{\"mic\":\"D112\"}".into(),
            content: ContentRef {
                root_id: "acme-library".into(),
                path: "Samples/Kicks/Room Kick 48k.wav".into(),
            },
            updated_at: "2026-09-05T10:00:00Z".into(),
        }
    }

    #[test]
    fn manifest_renders_and_parses_back() {
        let md = render_manifest(&sample(), "room-kick-48k").unwrap();
        let r = parse_manifest(&md).unwrap();
        assert_eq!(r.kind, ResourceKind::Sample);
        assert_eq!(r.slug, "room-kick-48k");
        assert_eq!(r.duration_secs, 2);
        assert_eq!(r.sample_rate, 48_000);
        assert_eq!(r.tags, ["kick", "room"]);
        assert_eq!(r.source, SOURCE);
    }

    /// The manifest says where the audio is; it never holds the audio.
    #[test]
    fn the_manifest_names_a_file_root_rather_than_carrying_bytes() {
        let md = render_manifest(&sample(), "room-kick-48k").unwrap();
        let r = parse_manifest(&md).unwrap();
        assert_eq!(r.content_root, "acme-library");
        assert_eq!(r.content_path, "Samples/Kicks/Room Kick 48k.wav");
        assert!(
            md.contains("the audio lives in the File Root"),
            "the file says so in words too: {md}"
        );
    }

    #[test]
    fn refresh_keeps_body_and_foreign_keys() {
        let existing = "---\ntype: resource\nresource_kind: sample\nslug: room-kick-48k\ntitle: Old\nvelocity_layers: 4\n---\n# Room Kick\n- close mic only\n";
        let out = refresh_manifest(existing, &sample()).unwrap();
        assert!(out.ends_with("---\n# Room Kick\n- close mic only\n"));
        let r = parse_manifest(&out).unwrap();
        assert_eq!(r.title, "Room Kick 48k");
        assert!(out.contains("velocity_layers: 4"), "{out}");
    }
}
