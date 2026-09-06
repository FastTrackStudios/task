//! The Keyflow chart on disk — two files under
//! `<org>/resources/charts/`, and what an app may never clobber.
//!
//! - `<slug>.kf` — the chart source, byte for byte as Keyflow sent it.
//!   An editor that knows nothing about Task opens it and sees a chart.
//! - `<slug>.md` — the `type: resource`, `resource_kind: chart`
//!   manifest. The app owns the frontmatter keys that describe the
//!   chart ([`APP_OWNED`]); every other key, and the whole body, belong
//!   to whoever wrote them, and a re-upsert keeps them verbatim — the
//!   same contract [`crate::sermon::refresh_manifest`] holds the sermon
//!   sync to.
//!
//! The slug is the chart's identity (`chart:<slug>` in the link graph
//! and in a `Library` collection), and it is kebab-cased by the *same*
//! [`crate::sermon::slugify`] the sermon lane uses — one slug rule for
//! the whole resources tier.

use resources_proto::ChartDoc;
use serde_yaml::{Mapping, Value};

use crate::ResourceError;
use crate::sermon::{slugify, split};

/// Frontmatter keys the chart's owning app rewrites on every upsert.
/// Unlike a sermon, `title` is among them: a chart's title is the
/// app's field, not a reader's annotation of somebody else's video.
pub const APP_OWNED: &[&str] = &["title", "key", "notation", "sections", "updated_at"];

/// `source:` value on a chart manifest — where the `.kf` came from.
pub const SOURCE: &str = "keyflow";

/// The source file beside a chart manifest (`<slug>.kf`).
#[must_use]
pub fn source_path(md_path: &std::path::Path) -> std::path::PathBuf {
    md_path.with_extension("kf")
}

/// The slug this chart gets: its own when it names one, else the kebab
/// title, else — a title whose kebab is taken by a *different* chart —
/// that kebab with a numeric suffix, so two charts named the same never
/// land on one file.
///
/// `taken` is every slug already on disk.
#[must_use]
pub fn slug_for(taken: &[String], chart: &ChartDoc) -> String {
    if !chart.slug.trim().is_empty() {
        return slugify(&chart.slug);
    }
    let base = slugify(&chart.title);
    if !taken.contains(&base) {
        return base;
    }
    // `hosanna`, `hosanna-2`, `hosanna-3` …
    (2u32..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c))
        .unwrap_or(base)
}

/// The app-owned frontmatter as YAML, in the order it is written.
fn owned_values(chart: &ChartDoc) -> Vec<(&'static str, Value)> {
    vec![
        ("title", chart.title.clone().into()),
        ("key", chart.key.clone().into()),
        ("notation", chart.notation.clone().into()),
        (
            "sections",
            Value::Sequence(
                chart
                    .sections
                    .iter()
                    .map(|s| Value::from(s.as_str()))
                    .collect(),
            ),
        ),
        ("updated_at", chart.updated_at.clone().into()),
    ]
}

fn yaml(mapping: &Mapping) -> Result<String, ResourceError> {
    serde_yaml::to_string(mapping).map_err(|e| ResourceError::Yaml(e.to_string()))
}

/// A fresh manifest: the frontmatter plus a body pointing at the `.kf`.
/// Nothing of the chart is duplicated into the markdown — the source
/// file is the chart.
pub fn render_manifest(chart: &ChartDoc, slug: &str) -> Result<String, ResourceError> {
    let mut fm = Mapping::new();
    fm.insert("type".into(), "resource".into());
    fm.insert("resource_kind".into(), "chart".into());
    fm.insert("slug".into(), slug.into());
    for (k, v) in owned_values(chart) {
        fm.insert(k.into(), v);
    }
    fm.insert("source".into(), SOURCE.into());
    Ok(format!(
        "---\n{}---\n{}",
        yaml(&fm)?,
        render_body(chart, slug)
    ))
}

fn render_body(chart: &ChartDoc, slug: &str) -> String {
    format!(
        "<!-- The chart itself is `{slug}.kf` beside this file; edit it there. Sections anchor as chart:{slug}#<section>. -->\n\
# {}\n\
\n\
## Notes\n\
\n\
_Notes about this chart go here; the source is `{slug}.kf`._\n",
        chart.title,
    )
}

/// Re-upsert an existing manifest: rewrite only the [`APP_OWNED`]
/// frontmatter keys, keep every other key and the whole body byte for
/// byte.
pub fn refresh_manifest(existing: &str, chart: &ChartDoc) -> Result<String, ResourceError> {
    let (fm_text, body) = split(existing).ok_or(ResourceError::NoFrontmatter)?;
    let mut fm: Mapping =
        serde_yaml::from_str(fm_text).map_err(|e| ResourceError::Yaml(e.to_string()))?;
    for (k, v) in owned_values(chart) {
        fm.insert(k.into(), v);
    }
    Ok(format!("---\n{}---\n{body}", yaml(&fm)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse_manifest;
    use crate::types::ResourceKind;

    fn chart() -> ChartDoc {
        ChartDoc {
            slug: String::new(),
            title: "Great Are You Lord".into(),
            source: "[Verse 1]\n| A | E |\n".into(),
            key: "A".into(),
            notation: "keyflow".into(),
            sections: vec!["verse-1".into(), "chorus".into()],
            updated_at: "2026-09-05T10:00:00Z".into(),
        }
    }

    #[test]
    fn slug_comes_from_the_title_and_collisions_are_suffixed() {
        let c = chart();
        assert_eq!(slug_for(&[], &c), "great-are-you-lord");
        let taken = vec!["great-are-you-lord".to_string()];
        assert_eq!(slug_for(&taken, &c), "great-are-you-lord-2");
        // An explicit slug is the identity: the same chart, updated.
        let mut named = c.clone();
        named.slug = "Great Are You Lord".into();
        assert_eq!(slug_for(&taken, &named), "great-are-you-lord");
    }

    #[test]
    fn manifest_renders_and_parses_back() {
        let md = render_manifest(&chart(), "great-are-you-lord").unwrap();
        let r = parse_manifest(&md).unwrap();
        assert_eq!(r.kind, ResourceKind::Chart);
        assert_eq!(r.slug, "great-are-you-lord");
        assert_eq!(r.title, "Great Are You Lord");
        assert_eq!(r.key, "A");
        assert_eq!(r.notation, "keyflow");
        assert_eq!(r.sections, ["verse-1", "chorus"]);
        assert_eq!(r.updated_at, "2026-09-05T10:00:00Z");
        assert_eq!(r.source, SOURCE);
        assert!(md.contains("great-are-you-lord.kf"));
    }

    #[test]
    fn refresh_keeps_body_and_foreign_keys() {
        let existing = "---\ntype: resource\nresource_kind: chart\nslug: great-are-you-lord\ntitle: Old Title\ncapo: 2\n---\n# Great Are You Lord\n## Notes\n- play it slower\n";
        let out = refresh_manifest(existing, &chart()).unwrap();
        assert!(out.ends_with("---\n# Great Are You Lord\n## Notes\n- play it slower\n"));
        let r = parse_manifest(&out).unwrap();
        assert_eq!(r.title, "Great Are You Lord", "title IS app-owned");
        assert_eq!(r.key, "A");
        assert!(out.contains("capo: 2"), "foreign key survives: {out}");
    }
}
