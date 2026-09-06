//! The Signal patch on disk — a *directory* under
//! `<org>/resources/patches/`, and what an app may never clobber.
//!
//! - `<slug>/patch.json` — the patch definition, byte for byte as
//!   Signal sent it.
//! - `<slug>/patch.md` — the `type: resource`, `resource_kind: patch`
//!   manifest. The app owns the frontmatter keys that describe the
//!   patch ([`APP_OWNED`]); every other key, and the whole body, belong
//!   to whoever wrote them and survive a re-upsert verbatim
//!   ([`crate::asset::refresh_manifest`]).
//!
//! A directory rather than the flat pair a chart gets, because a patch
//! grows sidecars — an impulse response, a capture, a photo of the
//! pedalboard — and ADR 0003's `locate()` already looks for the
//! directory. What it does *not* grow is content: anything large the
//! patch plays lives in a File Root and is named by
//! `resources_proto::ContentRef`, for the reasons that type states.

use resources_proto::PatchDoc;

use crate::ResourceError;
use crate::asset::{self, Owned};

/// Frontmatter keys the patch's owning app rewrites on every upsert.
/// `title` is among them, as it is for a chart: a patch's title is the
/// app's field.
pub const APP_OWNED: &[&str] = &[
    "title",
    "rig",
    "tags",
    "content_root",
    "content_path",
    "updated_at",
];

/// `source:` value on a patch manifest — which app owns it.
pub const SOURCE: &str = "signal";

/// `resource_kind:` of a patch manifest.
pub const KIND: &str = "patch";

/// The manifest inside a patch's directory.
pub const MANIFEST: &str = "patch.md";

/// The definition file beside it.
pub const BODY: &str = "patch.json";

/// The definition file beside a patch manifest (`patch.json`).
#[must_use]
pub fn body_path(md_path: &std::path::Path) -> std::path::PathBuf {
    md_path.with_file_name(BODY)
}

/// The slug this patch gets — [`asset::slug_for`]'s rule, which every
/// lane on this tier shares.
#[must_use]
pub fn slug_for(taken: &[String], patch: &PatchDoc) -> String {
    asset::slug_for(taken, &patch.slug, &patch.title)
}

/// The app-owned frontmatter as YAML, in the order it is written.
fn owned_values(patch: &PatchDoc) -> Vec<Owned> {
    let mut v: Vec<Owned> = vec![
        ("title", patch.title.clone().into()),
        ("rig", patch.rig.clone().into()),
        ("tags", asset::strings(&patch.tags)),
    ];
    v.extend(asset::content_keys(
        &patch.content.root_id,
        &patch.content.path,
    ));
    v.push(("updated_at", patch.updated_at.clone().into()));
    v
}

/// A fresh manifest: the frontmatter plus a body pointing at the
/// definition. Nothing of the patch is duplicated into the markdown —
/// `patch.json` is the patch.
pub fn render_manifest(patch: &PatchDoc, slug: &str) -> Result<String, ResourceError> {
    asset::render_manifest(
        KIND,
        slug,
        SOURCE,
        owned_values(patch),
        &render_body(patch, slug),
    )
}

fn render_body(patch: &PatchDoc, slug: &str) -> String {
    format!(
        "<!-- The patch itself is `{BODY}` beside this file; edit it there. Reference it as patch:{slug}. -->\n\
# {}\n\
\n\
## Notes\n\
\n\
_Notes about this patch go here. Its samples and captures live in a File Root, not in this directory._\n",
        patch.title,
    )
}

/// Re-upsert an existing manifest: rewrite only the [`APP_OWNED`]
/// frontmatter keys, keep every other key and the whole body.
pub fn refresh_manifest(existing: &str, patch: &PatchDoc) -> Result<String, ResourceError> {
    asset::refresh_manifest(existing, owned_values(patch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse_manifest;
    use crate::types::ResourceKind;
    use resources_proto::ContentRef;

    fn patch() -> PatchDoc {
        PatchDoc {
            slug: String::new(),
            title: "Warm Analog Pad".into(),
            rig: "helix".into(),
            tags: vec!["pad".into(), "ambient".into()],
            body: "{\"blocks\":[\"reverb\"]}".into(),
            content: ContentRef::default(),
            updated_at: "2026-09-05T10:00:00Z".into(),
        }
    }

    #[test]
    fn manifest_renders_and_parses_back() {
        let md = render_manifest(&patch(), "warm-analog-pad").unwrap();
        let r = parse_manifest(&md).unwrap();
        assert_eq!(r.kind, ResourceKind::Patch);
        assert_eq!(r.slug, "warm-analog-pad");
        assert_eq!(r.title, "Warm Analog Pad");
        assert_eq!(r.rig, "helix");
        assert_eq!(r.tags, ["pad", "ambient"]);
        assert_eq!(r.source, SOURCE);
        assert!(md.contains(BODY), "the body points at the definition: {md}");
        // Nothing bound yet is the ordinary state of a new patch.
        assert_eq!(r.content_root, "");
    }

    /// The manifest carries the File Root binding rather than the
    /// bytes, and unbinding is expressible.
    #[test]
    fn content_binding_round_trips_and_can_be_cleared() {
        let mut p = patch();
        p.content = ContentRef {
            root_id: "acme-library".into(),
            path: "Patches/Warm Analog Pad.hlx".into(),
        };
        let md = render_manifest(&p, "warm-analog-pad").unwrap();
        let r = parse_manifest(&md).unwrap();
        assert_eq!(r.content_root, "acme-library");
        assert_eq!(r.content_path, "Patches/Warm Analog Pad.hlx");

        let cleared = refresh_manifest(&md, &patch()).unwrap();
        let r = parse_manifest(&cleared).unwrap();
        assert_eq!(r.content_root, "", "an empty binding really unbinds");
        assert_eq!(r.content_path, "");
    }

    #[test]
    fn refresh_keeps_body_and_foreign_keys() {
        let existing = "---\ntype: resource\nresource_kind: patch\nslug: warm-analog-pad\ntitle: Old\namp: ac30\n---\n# Warm Analog Pad\n## Notes\n- roll the tone back\n";
        let out = refresh_manifest(existing, &patch()).unwrap();
        assert!(out.ends_with("---\n# Warm Analog Pad\n## Notes\n- roll the tone back\n"));
        let r = parse_manifest(&out).unwrap();
        assert_eq!(r.title, "Warm Analog Pad", "title IS app-owned");
        assert_eq!(r.rig, "helix");
        assert!(out.contains("amp: ac30"), "foreign key survives: {out}");
    }
}
