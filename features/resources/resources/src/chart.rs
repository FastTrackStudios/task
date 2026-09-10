//! The Keyflow chart on disk — **one vault document** on the Assets
//! shelf, and what an app may never clobber.
//!
//! `<vault>/Assets/Charts/<slug>.md`, and nothing beside it. The
//! frontmatter is `type: asset`, `asset_kind: chart`; the chart source
//! is a ` ```keyflow ` fence in the body; everything else in the body
//! belongs to whoever wrote it.
//!
//! # Why one file, and why in the vault
//!
//! ADR 0004 decision 1. Under ADR 0003 a chart was two files under
//! `<org>/resources/charts/` — a manifest and a `<slug>.kf` holding the
//! source verbatim — and the resources tier is plain `std::fs`, so a
//! chart had no CRDT document, no collaborative editing, no wikilinks,
//! no tags, no search and no presence in Task's own UI. Two people
//! editing a chart in Keyflow got none of what two people editing a
//! note get.
//!
//! Assets are vault items. Not "backed by" the vault, not "synced to"
//! it — they *are* vault files, filed on a shelf named `Assets/`. That
//! single move is the whole feature: `vault-collab` already keys a Loro
//! document by `(vault_id, path)` for any vault file, the graph already
//! indexes any `.md`, search already walks the tree. Collaboration is
//! **inherited**, not built. See [`resources_proto::assets`] for the
//! shelf and [`vault_proto::assets`] for the tier.
//!
//! The two files became one because the vault walker collects `.md` and
//! `.base` and nothing else: a `.kf` under the vault root would be a
//! file the vault does not know about, and charts would have moved
//! house and gained nothing. So the source is a fenced block, and the
//! document a person opens in Task is the chart plus their notes about
//! it, converging together.
//!
//! # What is app-owned and what is not
//!
//! [`APP_OWNED`] frontmatter keys and the ` ```keyflow ` fence are
//! Keyflow's; a re-save rewrites exactly those. Every other frontmatter
//! key, and every other byte of the body, belongs to whoever wrote them
//! and survives verbatim — the same contract
//! [`crate::sermon::refresh_manifest`] holds the sermon sync to, now
//! extended to cover one block of the body as well.
//!
//! The slug is the chart's identity (`chart:<slug>` in the link graph
//! and in a `Library` collection), kebab-cased by the *same*
//! [`crate::sermon::slugify`] the sermon lane uses — one slug rule for
//! every kind Task files by name.
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
use resources_proto::assets::{CHART_FENCE, CHART_KIND, KIND_KEY, TYPE_ASSET, TYPE_KEY};
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

/// `source:` value on a chart document — where the chart came from.
pub const SOURCE: &str = "keyflow";

/// The frontmatter key ADR 0003 used for the kind, kept here so the
/// migration can recognise — and drop — a document it has already
/// rewritten. A chart still carrying this key is one nothing has
/// migrated yet, which is a fact worth being able to read off the file.
pub const LEGACY_KIND_KEY: &str = "resource_kind";

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
/// `taken` is every slug already on the shelf.
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

/// Rewrite just the default flag on an existing document, leaving every
/// other key and the whole body untouched.
///
/// This is how the backend clears the flag on a song's other charts
/// when one of them is made the default: it must not disturb a chart it
/// was not asked to write — including that chart's source — so it
/// touches exactly one key rather than round-tripping a [`ChartDoc`]
/// through [`refresh_document`].
pub fn set_default(existing: &str, is_default: bool) -> Result<String, ResourceError> {
    let (fm_text, body) = split(existing).ok_or(ResourceError::NoFrontmatter)?;
    let mut fm: Mapping =
        serde_yaml::from_str(fm_text).map_err(|e| ResourceError::Yaml(e.to_string()))?;
    fm.insert(IS_DEFAULT_KEY.into(), is_default.into());
    Ok(format!("---\n{}---\n{body}", yaml(&fm)?))
}

/// Serialise the frontmatter, with the sequence keys in flow form.
///
/// The flow pass is not cosmetic — see
/// [`crate::asset::inline_sequences`]: a block sequence `serde_yaml`
/// emits is read as *empty* by the parser behind the vault's folder
/// index, so a document written the other way would have no tags in
/// Task's own UI. An asset whose tags do not work like a note's tags is
/// not a vault item in the one way that matters.
fn yaml(mapping: &Mapping) -> Result<String, ResourceError> {
    let text = serde_yaml::to_string(mapping).map_err(|e| ResourceError::Yaml(e.to_string()))?;
    Ok(crate::asset::inline_sequences(&text, &["sections", "tags"]))
}

/// The chart source held in a document's ` ```keyflow ` fence, or the
/// empty string when it has none.
///
/// Total rather than fallible on purpose: a chart whose fence a person
/// deleted is a chart with no source right now, not a corrupt file, and
/// the next save heals it (see
/// [`resources_proto::assets::replace_fenced`]). Refusing to read it
/// would make one bad edit look like a lost chart.
#[must_use]
pub fn source_of(document: &str) -> String {
    let body = split(document).map_or(document, |(_, body)| body);
    resources_proto::assets::extract_fenced(body, CHART_FENCE).unwrap_or_default()
}

/// A fresh chart document: the frontmatter, a heading, the source in
/// its fence, and an empty notes section for the person who opens it.
pub fn render_document(chart: &ChartDoc, slug: &str) -> Result<String, ResourceError> {
    let mut fm = Mapping::new();
    fm.insert(TYPE_KEY.into(), TYPE_ASSET.into());
    fm.insert(KIND_KEY.into(), CHART_KIND.into());
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
        "# {}\n\
\n\
{}\n\
## Notes\n\
\n\
_Notes about this chart go here. It is an ordinary vault document: \
[[wikilink]] it, tag it, search it, and edit it with somebody else. \
Sections anchor as `chart:{slug}#<section>`._\n",
        chart.title,
        resources_proto::assets::fence(&chart.source, CHART_FENCE),
    )
}

/// Re-save an existing chart document: rewrite the [`APP_OWNED`]
/// frontmatter keys and the ` ```keyflow ` fence, and keep every other
/// key and every other byte of the body.
///
/// The body clause is the one that matters for collaboration. Keyflow
/// saving a chart must not stamp on the paragraph somebody typed under
/// `## Notes` thirty seconds ago — and because the write goes through
/// `put_file` into a file `vault-collab` may hold an open document for,
/// anything this function *does* change is folded into that document by
/// the inbound three-way merge rather than reverting a live edit.
pub fn refresh_document(existing: &str, chart: &ChartDoc) -> Result<String, ResourceError> {
    let (fm_text, body) = split(existing).ok_or(ResourceError::NoFrontmatter)?;
    let mut fm: Mapping =
        serde_yaml::from_str(fm_text).map_err(|e| ResourceError::Yaml(e.to_string()))?;
    promote_frontmatter(&mut fm);
    for (k, v) in owned_values(chart) {
        fm.insert(k.into(), v);
    }
    let body = resources_proto::assets::replace_fenced(body, CHART_FENCE, &chart.source);
    Ok(format!("---\n{}---\n{body}", yaml(&fm)?))
}

/// Move a document's frontmatter onto the Assets tier's vocabulary,
/// in place: `type: resource` → `type: asset`, and `resource_kind` →
/// `asset_kind` at the same position rather than appended.
///
/// Called on every re-save, not only by the migration, so a chart that
/// somehow still carries ADR 0003's keys is corrected the next time
/// anybody touches it. Idempotent: a document already on the new
/// vocabulary is left exactly as it is, which is what lets the
/// migration run on every boot without rewriting settled files.
pub fn promote_frontmatter(fm: &mut Mapping) {
    fm.insert(TYPE_KEY.into(), TYPE_ASSET.into());
    if let Some(kind) = fm.remove(Value::from(LEGACY_KIND_KEY)) {
        // `insert` keeps an existing key's position and appends a new
        // one, so re-inserting under the new name puts the kind at the
        // end. That is a cosmetic difference in a migrated file and not
        // worth a rebuild of the mapping.
        fm.entry(KIND_KEY.into()).or_insert(kind);
    }
}

/// Build the vault document for a chart being migrated off the ADR 0003
/// resources tier: its old manifest, plus the `.kf` that sat beside it.
///
/// Everything the person wrote survives — foreign frontmatter keys, the
/// whole body — and the source is folded in as the fence. The one
/// rewrite is the tier vocabulary ([`promote_frontmatter`]).
///
/// Deliberately *not* expressed as "render a fresh document from the
/// parsed [`ChartDoc`]": that would round-trip through a type that
/// knows only the keys ADR 0003 declared, and quietly drop the `capo:`
/// somebody added by hand. A migration that loses data it did not
/// understand is not reversible by inspection.
pub fn migrate_document(existing: &str, source: &str) -> Result<String, ResourceError> {
    let (fm_text, body) = split(existing).ok_or(ResourceError::NoFrontmatter)?;
    let mut fm: Mapping =
        serde_yaml::from_str(fm_text).map_err(|e| ResourceError::Yaml(e.to_string()))?;
    promote_frontmatter(&mut fm);
    let body = strip_sidecar_pointer(body);
    let body = resources_proto::assets::replace_fenced(&body, CHART_FENCE, source);
    Ok(format!("---\n{}---\n{body}", yaml(&fm)?))
}

/// Drop the HTML comment ADR 0003's `render_manifest` wrote at the top
/// of every chart manifest — the one pointing at the `.kf` beside it.
/// After the migration there is no file beside it, so the line would be
/// a wrong instruction rather than a stale one.
fn strip_sidecar_pointer(body: &str) -> String {
    let rest = body.trim_start_matches('\n');
    match rest.strip_prefix("<!-- The chart itself is ") {
        Some(after) => match after.find("-->") {
            Some(end) => after[end + 3..].trim_start_matches('\n').to_owned(),
            None => body.to_owned(),
        },
        None => body.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse_manifest;
    use crate::types::ResourceKind;

    const SOURCE_TEXT: &str = "[Verse 1]\n| A | E |\n";

    fn chart() -> ChartDoc {
        ChartDoc {
            slug: String::new(),
            title: "Great Are You Lord".into(),
            source: SOURCE_TEXT.into(),
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
        let existing = "---\ntype: asset\nasset_kind: chart\nslug: doxology\ntitle: Doxology\nis_default: true\ncapo: 2\n---\n# Doxology\n\n```keyflow\n| G |\n```\n";
        let out = set_default(existing, false).unwrap();
        assert!(out.contains("is_default: false"), "{out}");
        assert!(out.contains("capo: 2"), "foreign key survives: {out}");
        assert_eq!(source_of(&out), "| G |\n", "the source is untouched");
    }

    /// The whole shape of an asset in one assertion: it is a vault
    /// document, it declares its tier, and its source is a fence.
    #[test]
    fn a_chart_renders_as_an_asset_document_and_parses_back() {
        let md = render_document(&chart(), "great-are-you-lord").unwrap();
        assert!(md.contains("type: asset"), "{md}");
        assert!(md.contains("asset_kind: chart"), "{md}");
        assert!(
            !md.contains("resource_kind"),
            "the tier moved; the key moved with it: {md}"
        );
        assert!(
            !md.contains(".kf"),
            "there is no file beside it any more: {md}"
        );
        assert_eq!(source_of(&md), SOURCE_TEXT);

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
    }

    /// The contract a collaborator depends on: Keyflow saving a chart
    /// rewrites its own frontmatter and its own fence, and leaves the
    /// prose somebody else is typing exactly where it was.
    #[test]
    fn a_re_save_keeps_the_body_and_foreign_keys() {
        let existing = "---\ntype: asset\nasset_kind: chart\nslug: great-are-you-lord\ntitle: Old Title\ncapo: 2\n---\n# Great Are You Lord\n\n```keyflow\n| old |\n```\n\n## Notes\n\n- play it slower\n";
        let out = refresh_document(existing, &chart()).unwrap();
        assert!(
            out.ends_with("## Notes\n\n- play it slower\n"),
            "the prose moved: {out}"
        );
        assert_eq!(source_of(&out), SOURCE_TEXT, "the source is app-owned");
        assert!(out.contains("capo: 2"), "foreign key survives: {out}");
        let r = parse_manifest(&out).unwrap();
        assert_eq!(r.title, "Great Are You Lord", "title IS app-owned");
        assert_eq!(r.key, "A");
    }

    /// Migration off ADR 0003: the old manifest plus the `.kf` beside
    /// it become one asset document, and nothing a person wrote is
    /// lost — not a foreign key, not a paragraph.
    #[test]
    fn a_resource_manifest_migrates_into_an_asset_document() {
        let legacy = "---\ntype: resource\nresource_kind: chart\nslug: doxology\ntitle: Doxology\ncapo: 2\n---\n<!-- The chart itself is `doxology.kf` beside this file; edit it there. -->\n# Doxology\n\n## Notes\n\n- from the hymnal\n";
        let out = migrate_document(legacy, "[Verse]\n| G |\n").unwrap();

        assert!(out.contains("type: asset"), "{out}");
        assert!(out.contains("asset_kind: chart"), "{out}");
        assert!(!out.contains("resource_kind"), "{out}");
        assert!(!out.contains(".kf"), "the pointer is a lie now: {out}");
        assert!(out.contains("capo: 2"), "foreign key survives: {out}");
        assert!(out.contains("- from the hymnal"), "prose survives: {out}");
        assert_eq!(source_of(&out), "[Verse]\n| G |\n");

        // Idempotent by construction: migrating the result again is the
        // result. The server re-runs this on every boot.
        let again = migrate_document(&out, source_of(&out).as_str()).unwrap();
        assert_eq!(again, out);

        let r = parse_manifest(&out).unwrap();
        assert_eq!(r.kind, ResourceKind::Chart, "`asset_kind` still parses");
        assert_eq!(r.slug, "doxology");
    }

    /// A chart whose fence somebody deleted reads as sourceless rather
    /// than as an error, and the next save puts it back.
    #[test]
    fn a_missing_fence_is_empty_rather_than_broken() {
        let mangled = "---\ntype: asset\nasset_kind: chart\nslug: d\ntitle: D\n---\n# D\n\noops\n";
        assert_eq!(source_of(mangled), "");
        let healed = refresh_document(mangled, &chart()).unwrap();
        assert_eq!(source_of(&healed), SOURCE_TEXT);
        assert!(healed.contains("oops"), "{healed}");
    }
}
