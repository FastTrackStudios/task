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
//!
//! **One chart is one arrangement.** A song played three ways is three
//! charts, told apart by [`ChartDoc::arrangement`] and joined by
//! [`ChartDoc::song`], with one of them flagged
//! [`ChartDoc::is_default`]. That flag is the backend's to maintain
//! rather than an app's to assert — see
//! [`resources_proto::ResourcesService::upsert_chart`] — and
//! [`set_default`] is the surgical write it maintains it with.

use links_proto::{NodeKind, NodeRef};
use resources_proto::ChartDoc;
use serde_yaml::{Mapping, Value};

use crate::ResourceError;
use crate::sermon::split;

/// Frontmatter keys the chart's owning app rewrites on every upsert.
/// Unlike a sermon, `title` is among them: a chart's title is the
/// app's field, not a reader's annotation of somebody else's video.
pub const APP_OWNED: &[&str] = &[
    "title",
    "key",
    "notation",
    "sections",
    "song",
    "arrangement",
    "is_default",
    "updated_at",
];

/// The frontmatter key holding the default flag. Named here because
/// the backend rewrites *only* this key when it clears a sibling
/// chart's flag, and a second spelling of it would silently stop the
/// invariant working.
pub const IS_DEFAULT_KEY: &str = "is_default";

/// `source:` value on a chart manifest — where the `.kf` came from.
pub const SOURCE: &str = "keyflow";

/// The source file beside a chart manifest (`<slug>.kf`).
#[must_use]
pub fn source_path(md_path: &std::path::Path) -> std::path::PathBuf {
    md_path.with_extension("kf")
}

/// The slug this chart gets: its own when it names one, else a kebab of
/// the title *and the arrangement label*, else — a kebab taken by a
/// *different* chart — that kebab with a numeric suffix, so two charts
/// named the same never land on one file.
///
/// Folding the arrangement in is the point: a song's second chart wants
/// to be `doxology-condensed-live`, which says what it is, rather than
/// `doxology-2`, which says only that somebody got there first. The
/// numeric suffix stays as the fallback, for the case the label does
/// not disambiguate either (two charts both labelled `live`).
///
/// An explicit `slug` still wins outright, which is what keeps this
/// from renaming a chart that already exists: re-saving a chart names
/// its slug, and naming a slug means "the same one again".
///
/// `taken` is every slug already on disk.
#[must_use]
pub fn slug_for(taken: &[String], chart: &ChartDoc) -> String {
    let title = if chart.arrangement.trim().is_empty() {
        chart.title.clone()
    } else {
        format!("{} {}", chart.title, chart.arrangement)
    };
    crate::asset::slug_for(taken, &chart.slug, &title)
}

/// The canonical `song:<slug>` token for what a caller wrote in
/// [`ChartDoc::song`], or `None` when they wrote nothing.
///
/// ADR 0003 keeps reference parsing total — "anything that does not fit
/// is read as a local reference rather than refused" — so a bare
/// `doxology` becomes `song:doxology` rather than an error. The one
/// refusal is a token that parses as some *other* kind: `chart:x` in
/// the song field is not a lenient spelling of a song, it is a
/// different node, and storing it would quietly break every
/// arrangement query for that song.
pub fn song_token(raw: &str) -> Result<Option<String>, ResourceError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    match NodeRef::parse(raw) {
        Some(node) if node.kind == NodeKind::Song => Ok(Some(node.to_token())),
        Some(node) => Err(ResourceError::Yaml(format!(
            "`song` names a {}, not a song: {raw}",
            node.kind.as_str()
        ))),
        // No `kind:` prefix at all — a bare slug, read as this org's
        // own song.
        None if !raw.contains(':') => Ok(Some(NodeRef::song(raw).to_token())),
        None => Err(ResourceError::Yaml(format!(
            "`song` is not a node reference: {raw}"
        ))),
    }
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
        ("song", chart.song.clone().into()),
        ("arrangement", chart.arrangement.clone().into()),
        (IS_DEFAULT_KEY, chart.is_default.into()),
        ("updated_at", chart.updated_at.clone().into()),
    ]
}

/// Rewrite just the default flag on an existing manifest, leaving every
/// other key and the whole body untouched.
///
/// This is how the backend clears the flag on a song's other charts
/// when one of them is made the default: it must not disturb a chart it
/// was not asked to write, so it touches exactly one key rather than
/// round-tripping a [`ChartDoc`] through [`refresh_manifest`].
pub fn set_default(existing: &str, is_default: bool) -> Result<String, ResourceError> {
    let (fm_text, body) = split(existing).ok_or(ResourceError::NoFrontmatter)?;
    let mut fm: Mapping =
        serde_yaml::from_str(fm_text).map_err(|e| ResourceError::Yaml(e.to_string()))?;
    fm.insert(IS_DEFAULT_KEY.into(), is_default.into());
    Ok(format!("---\n{}---\n{body}", yaml(&fm)?))
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
            song: "song:great-are-you-lord".into(),
            arrangement: String::new(),
            is_default: true,
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

    /// The whole reason the arrangement label exists on the slug path:
    /// a song's second chart is named for what it is.
    #[test]
    fn an_arrangement_names_the_slug_rather_than_a_number() {
        let taken = vec!["great-are-you-lord".to_string()];
        let mut live = chart();
        live.arrangement = "Condensed Live".into();
        assert_eq!(
            slug_for(&taken, &live),
            "great-are-you-lord-condensed-live",
            "a labelled arrangement should not land on `-2`"
        );
        // Two charts with the *same* label still cannot collide: the
        // numeric suffix is the fallback it always was.
        let taken = vec![
            "great-are-you-lord".to_string(),
            "great-are-you-lord-condensed-live".to_string(),
        ];
        assert_eq!(
            slug_for(&taken, &live),
            "great-are-you-lord-condensed-live-2"
        );
    }

    /// Parsing stays total for anything that could be a song, and
    /// refuses only a reference that names something else.
    #[test]
    fn a_song_reference_is_normalised_and_a_wrong_kind_is_refused() {
        assert_eq!(song_token("").unwrap(), None);
        assert_eq!(song_token("  ").unwrap(), None);
        assert_eq!(
            song_token("doxology").unwrap(),
            Some("song:doxology".to_owned()),
            "a bare slug is this org's own song"
        );
        assert_eq!(
            song_token("song:doxology").unwrap(),
            Some("song:doxology".to_owned())
        );
        assert_eq!(
            song_token("guest.example/song:hosanna").unwrap(),
            Some("guest.example/song:hosanna".to_owned()),
            "a qualified reference survives verbatim — ADR 0003"
        );
        assert!(
            song_token("chart:doxology").is_err(),
            "a chart is not a lenient spelling of a song"
        );
    }

    #[test]
    fn setting_the_default_flag_touches_one_key_only() {
        let existing = "---\ntype: resource\nresource_kind: chart\nslug: doxology\ntitle: Doxology\nis_default: true\ncapo: 2\n---\n# Doxology\n- notes\n";
        let out = set_default(existing, false).unwrap();
        assert!(out.contains("is_default: false"), "{out}");
        assert!(out.contains("capo: 2"), "foreign key survives: {out}");
        assert!(out.ends_with("---\n# Doxology\n- notes\n"), "{out}");
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
        assert_eq!(r.song, "song:great-are-you-lord");
        assert_eq!(r.arrangement, "");
        assert!(r.is_default);
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
