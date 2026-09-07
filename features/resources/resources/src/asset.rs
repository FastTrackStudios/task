//! The parts every asset lane of the resources tier shares — slug
//! identity, and the manifest contract an owning app may never clobber.
//!
//! ADR 0003 gives four asset kinds a home under `<org>/resources/`, and
//! they differ only in *what* the frontmatter says. How a slug is
//! chosen, and what a re-upsert is allowed to touch, are the same
//! question four times, so they are answered once here and the
//! [`crate::chart`], [`crate::patch`], [`crate::sample`] and
//! [`crate::lighting`] modules supply the per-kind fields.
//!
//! The contract, which is the whole reason these are markdown files and
//! not rows: **an app owns its own frontmatter keys and nothing else.**
//! Every other key, and the entire body, belong to whoever wrote them,
//! and survive a re-upsert byte for byte — the same promise
//! [`crate::sermon::refresh_manifest`] makes the sermon sync.

use serde_yaml::{Mapping, Value};

use crate::ResourceError;
use crate::sermon::{slugify, split};

/// One app-owned frontmatter key and the value to write into it.
pub type Owned = (&'static str, Value);

/// The slug an asset gets: its own when it names one, else the kebab
/// title, else — a title whose kebab is taken by a *different* asset —
/// that kebab with a numeric suffix, so two assets named the same never
/// land on one path.
///
/// `taken` is every slug already on disk in that lane. A non-empty
/// `slug` is the identity and is returned kebabbed whether or not it
/// collides: naming a slug is how an app says "the same one again".
#[must_use]
pub fn slug_for(taken: &[String], slug: &str, title: &str) -> String {
    if !slug.trim().is_empty() {
        return slugify(slug);
    }
    let base = slugify(title);
    if base.is_empty() || !taken.contains(&base) {
        return base;
    }
    // `hosanna`, `hosanna-2`, `hosanna-3` …
    (2u32..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c))
        .unwrap_or(base)
}

fn yaml(mapping: &Mapping) -> Result<String, ResourceError> {
    serde_yaml::to_string(mapping).map_err(|e| ResourceError::Yaml(e.to_string()))
}

/// A fresh manifest: `type: resource`, the kind, the slug, the
/// app-owned keys in the order given, a `source:` naming the app, and
/// the body.
pub fn render_manifest(
    kind: &str,
    slug: &str,
    source: &str,
    owned: Vec<Owned>,
    body: &str,
) -> Result<String, ResourceError> {
    let mut fm = Mapping::new();
    fm.insert("type".into(), "resource".into());
    fm.insert("resource_kind".into(), kind.into());
    fm.insert("slug".into(), slug.into());
    for (k, v) in owned {
        fm.insert(k.into(), v);
    }
    fm.insert("source".into(), source.into());
    Ok(format!("---\n{}---\n{body}", yaml(&fm)?))
}

/// Re-upsert an existing manifest: rewrite only the app-owned
/// frontmatter keys, keep every other key and the whole body byte for
/// byte.
pub fn refresh_manifest(existing: &str, owned: Vec<Owned>) -> Result<String, ResourceError> {
    let (fm_text, body) = split(existing).ok_or(ResourceError::NoFrontmatter)?;
    let mut fm: Mapping =
        serde_yaml::from_str(fm_text).map_err(|e| ResourceError::Yaml(e.to_string()))?;
    for (k, v) in owned {
        fm.insert(k.into(), v);
    }
    Ok(format!("---\n{}---\n{body}", yaml(&fm)?))
}

/// The two app-owned keys that bind an asset to its bytes. Written on
/// every upsert — including as empty strings, so *unbinding* content is
/// expressible and does not leave a stale path behind.
#[must_use]
pub fn content_keys(root_id: &str, path: &str) -> Vec<Owned> {
    vec![
        ("content_root", root_id.into()),
        ("content_path", path.into()),
    ]
}

/// A YAML sequence of strings, for a list-valued owned key.
#[must_use]
pub fn strings(items: &[String]) -> Value {
    Value::Sequence(items.iter().map(|s| Value::from(s.as_str())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity rule, which is the same in all four lanes: an
    /// explicit slug wins, an empty one derives, and a derived one that
    /// collides is suffixed rather than merged.
    #[test]
    fn an_explicit_slug_is_identity_and_a_derived_one_dodges_collisions() {
        let taken = vec!["hosanna".to_owned()];
        assert_eq!(slug_for(&[], "", "Hosanna"), "hosanna");
        assert_eq!(slug_for(&taken, "", "Hosanna"), "hosanna-2");
        assert_eq!(slug_for(&taken, "Hosanna", "Anything Else"), "hosanna");
        // A title with nothing sluggable in it yields nothing, and the
        // caller turns that into a `BadRequest` rather than a filename.
        assert_eq!(slug_for(&[], "", "—"), "");
    }

    /// Two suffixes deep, because a person really does have three
    /// patches called "Lead".
    #[test]
    fn collisions_keep_counting() {
        let taken = vec!["lead".to_owned(), "lead-2".to_owned()];
        assert_eq!(slug_for(&taken, "", "Lead"), "lead-3");
    }

    #[test]
    fn a_refresh_keeps_foreign_keys_and_the_whole_body() {
        let existing = "---\ntype: resource\nresource_kind: patch\nslug: lead\ntitle: Old\namp: ac30\n---\n# Lead\n\n- bright\n";
        let out = refresh_manifest(existing, vec![("title", "New".into())]).unwrap();
        assert!(out.ends_with("---\n# Lead\n\n- bright\n"), "{out}");
        assert!(out.contains("title: New"), "{out}");
        assert!(out.contains("amp: ac30"), "foreign key survives: {out}");
    }

    #[test]
    fn a_manifest_without_frontmatter_is_refused_rather_than_overwritten() {
        assert!(matches!(
            refresh_manifest("# just a note\n", vec![]),
            Err(ResourceError::NoFrontmatter)
        ));
    }
}
