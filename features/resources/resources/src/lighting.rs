//! The Ignition lighting document on disk — a *directory* under
//! `<org>/resources/lighting/`.
//!
//! - `<slug>/show.md` — the `type: resource`,
//!   `resource_kind: lighting` manifest.
//! - `<slug>/show.json` — the lighting definition, byte for byte as
//!   Ignition sent it.
//!
//! Two fields carry the whole of what makes this addressable:
//!
//! - **[`SCOPES`]** — a lighting document covers a `song`, a `setlist`
//!   or a `show`, and nothing else. The scope is what tells a reader
//!   whether `lighting:sunday-set` belongs beside one song or over an
//!   evening, so an unrecognised word is refused rather than stored:
//!   a scope nobody can act on is worse than no scope.
//! - **cues** — the labels an anchor may address. `lighting:sunday-set#cue:12`
//!   resolves only if `12` is listed, the same way a chart's sections
//!   are declared rather than parsed. Nothing on the server reads
//!   lighting source.
//!
//! Rendered media the show needs lives in a File Root, named by
//! `resources_proto::ContentRef` — the manifest declares what the show
//! is, the Files layer owns its content.

use resources_proto::LightingDoc;

use crate::ResourceError;
use crate::asset::{self, Owned};

/// Frontmatter keys the lighting app rewrites on every upsert.
pub const APP_OWNED: &[&str] = &[
    "title",
    "scope",
    "cues",
    "content_root",
    "content_path",
    "updated_at",
];

/// `source:` value on a lighting manifest — which app owns it.
pub const SOURCE: &str = "ignition";

/// `resource_kind:` of a lighting manifest.
pub const KIND: &str = "lighting";

/// The manifest inside a lighting document's directory.
pub const MANIFEST: &str = "show.md";

/// The definition file beside it.
pub const BODY: &str = "show.json";

/// What a lighting document may cover. Closed and small, for the same
/// reason `project.capability.closed` gives: three members stay
/// interpretable by a UI and a placement policy; a free string does
/// not.
pub const SCOPES: &[&str] = &["song", "setlist", "show"];

/// Whether `scope` is one Ignition can act on.
#[must_use]
pub fn is_scope(scope: &str) -> bool {
    SCOPES.contains(&scope)
}

/// The definition file beside a lighting manifest (`show.json`).
#[must_use]
pub fn body_path(md_path: &std::path::Path) -> std::path::PathBuf {
    md_path.with_file_name(BODY)
}

/// The slug this document gets — [`asset::slug_for`]'s rule.
#[must_use]
pub fn slug_for(taken: &[String], lighting: &LightingDoc) -> String {
    asset::slug_for(taken, &lighting.slug, &lighting.title)
}

/// The app-owned frontmatter as YAML, in the order it is written.
fn owned_values(lighting: &LightingDoc) -> Vec<Owned> {
    let mut v: Vec<Owned> = vec![
        ("title", lighting.title.clone().into()),
        ("scope", lighting.scope.clone().into()),
        ("cues", asset::strings(&lighting.cues)),
    ];
    v.extend(asset::content_keys(
        &lighting.content.root_id,
        &lighting.content.path,
    ));
    v.push(("updated_at", lighting.updated_at.clone().into()));
    v
}

/// A fresh manifest: the frontmatter plus a body pointing at the
/// definition.
pub fn render_manifest(lighting: &LightingDoc, slug: &str) -> Result<String, ResourceError> {
    asset::render_manifest(
        KIND,
        slug,
        SOURCE,
        owned_values(lighting),
        &render_body(lighting, slug),
    )
}

fn render_body(lighting: &LightingDoc, slug: &str) -> String {
    format!(
        "<!-- The cue list itself is `{BODY}` beside this file; edit it there. Reference it as lighting:{slug}, a cue as lighting:{slug}#cue:12. -->\n\
# {}\n\
\n\
## Notes\n\
\n\
_Notes about this {} go here; the source is `{BODY}`._\n",
        lighting.title,
        if lighting.scope.is_empty() {
            "show"
        } else {
            &lighting.scope
        },
    )
}

/// Re-upsert an existing manifest: rewrite only the [`APP_OWNED`]
/// frontmatter keys, keep every other key and the whole body.
pub fn refresh_manifest(existing: &str, lighting: &LightingDoc) -> Result<String, ResourceError> {
    asset::refresh_manifest(existing, owned_values(lighting))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse_manifest;
    use crate::types::ResourceKind;
    use resources_proto::ContentRef;

    fn lighting() -> LightingDoc {
        LightingDoc {
            slug: String::new(),
            title: "Sunday Set".into(),
            scope: "setlist".into(),
            cues: vec!["12".into(), "13".into()],
            body: "{\"cues\":[]}".into(),
            content: ContentRef::default(),
            updated_at: "2026-09-05T10:00:00Z".into(),
        }
    }

    /// The vocabulary is three words, and the check is the whole reason
    /// the field is not a free string.
    #[test]
    fn only_the_three_scopes_are_scopes() {
        for s in ["song", "setlist", "show"] {
            assert!(is_scope(s));
        }
        for s in ["", "Show", "evening", "tour"] {
            assert!(!is_scope(s), "{s} is not a scope");
        }
    }

    #[test]
    fn manifest_renders_and_parses_back() {
        let md = render_manifest(&lighting(), "sunday-set").unwrap();
        let r = parse_manifest(&md).unwrap();
        assert_eq!(r.kind, ResourceKind::Lighting);
        assert_eq!(r.slug, "sunday-set");
        assert_eq!(r.scope, "setlist");
        assert_eq!(r.cues, ["12", "13"]);
        assert_eq!(r.source, SOURCE);
        assert!(
            md.contains("lighting:sunday-set#cue:12"),
            "the anchor form is written down: {md}"
        );
    }

    #[test]
    fn refresh_keeps_body_and_foreign_keys() {
        let existing = "---\ntype: resource\nresource_kind: lighting\nslug: sunday-set\ntitle: Old\nconsole: grandma3\n---\n# Sunday Set\n- house lights at 60%\n";
        let out = refresh_manifest(existing, &lighting()).unwrap();
        assert!(out.ends_with("---\n# Sunday Set\n- house lights at 60%\n"));
        let r = parse_manifest(&out).unwrap();
        assert_eq!(r.title, "Sunday Set");
        assert_eq!(r.scope, "setlist");
        assert!(out.contains("console: grandma3"), "{out}");
    }

    #[test]
    fn a_content_binding_is_carried_not_followed() {
        let mut l = lighting();
        l.content = ContentRef {
            root_id: "acme-library".into(),
            path: "Shows/Sunday Set.m3d".into(),
        };
        let r = parse_manifest(&render_manifest(&l, "sunday-set").unwrap()).unwrap();
        assert_eq!(r.content_root, "acme-library");
        assert_eq!(r.content_path, "Shows/Sunday Set.m3d");
    }
}
