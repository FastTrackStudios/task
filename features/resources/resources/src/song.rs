//! The song document on the Assets shelf — `<vault>/Assets/Songs/<slug>.md`.
//!
//! ADR 0004 decision 1, the sibling of [`crate::chart`]. A song is the
//! thing charts are arrangements *of*: a title, the key it is usually
//! in, who wrote it, tags, and whatever a person writes about it.
//!
//! # A song does not list its arrangements
//!
//! Deliberately, and it is the design decision worth the most in this
//! file. The vendored `song` crate's folder — the one `task song add`
//! used to write — kept an `arrangements:` list inside `song.md`,
//! keyed by uuid, alongside a `defaultArrangement` uuid, alongside a
//! nested `arrangements/<dir>/arrangement.md` for each. Four places
//! saying the same thing, and every one of them a place the others can
//! drift from.
//!
//! Here a chart names its song ([`resources_proto::ChartDoc::song`])
//! and the song says nothing back. That is not one-way out of laziness:
//! it is the only direction one write can keep true. Adding an
//! arrangement is a single `upsert_chart`, with nothing to keep in
//! step; the reverse question — "what arrangements does this song
//! have?" — is `list_charts("song:<slug>")`, and in Task's own UI it is
//! the backlinks panel, which every vault page already has.
//!
//! Which is the general shape of ADR 0004: *these are all just
//! manipulations of the markdown files and linking them together in the
//! vault.* A uuid in a nested directory cannot be a wikilink, cannot be
//! a backlink, and cannot be found by search. A `song: song:doxology`
//! key can be all three.
//!
//! # The audio did not move
//!
//! `<org>/resources/songs/<slug>/manifest.json` and the stems beside it
//! stay on the resources tier. They are imports: binary, not typed
//! into, served by `/org/{slug}/media/songs/...` and materialised by a
//! cross-org subscription. The tier rule sorts the two things that
//! happened to share a directory — *Resources are the things nobody
//! types into* — and it is why a cross-organisation `song:<slug>` keeps
//! resolving and keeps being fetchable, where a `chart:<slug>` no
//! longer is.

use resources_proto::SongDoc;
use resources_proto::assets::{KIND_KEY, SONG_KIND, TYPE_ASSET, TYPE_KEY};
use serde_yaml::{Mapping, Value};

use crate::ResourceError;
use crate::sermon::split;

/// Frontmatter keys the song's owning app rewrites on every upsert.
/// Everything else in the frontmatter, and the whole body, belongs to
/// whoever wrote it.
pub const APP_OWNED: &[&str] = &["title", "writers", "key", "tags", "updated_at"];

/// `source:` value on a song document — which lane wrote it.
pub const SOURCE: &str = "song";

/// The slug this song gets: its own when it names one, else a kebab of
/// the title, else — a kebab taken by a *different* song — that kebab
/// with a numeric suffix.
///
/// The same rule as [`crate::chart::slug_for`] and the sermon lane's,
/// through the same [`crate::asset::slug_for`], because a slug is a
/// node id and `song:opening-night` has to mean the same thing however
/// it was created — `task song add`, an RPC, or a migration reading a
/// folder somebody else wrote.
#[must_use]
pub fn slug_for(taken: &[String], song: &SongDoc) -> String {
    crate::asset::slug_for(taken, &song.slug, &song.title)
}

/// The app-owned frontmatter as YAML, in the order it is written.
fn owned_values(song: &SongDoc) -> Vec<(&'static str, Value)> {
    vec![
        ("title", song.title.clone().into()),
        (
            "writers",
            Value::Sequence(
                song.writers
                    .iter()
                    .map(|w| Value::from(w.as_str()))
                    .collect(),
            ),
        ),
        ("key", song.key.clone().into()),
        (
            "tags",
            Value::Sequence(song.tags.iter().map(|t| Value::from(t.as_str())).collect()),
        ),
        ("updated_at", song.updated_at.clone().into()),
    ]
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
    Ok(crate::asset::inline_sequences(&text, &["writers", "tags"]))
}

/// A fresh song document.
pub fn render_document(song: &SongDoc, slug: &str) -> Result<String, ResourceError> {
    let mut fm = Mapping::new();
    fm.insert(TYPE_KEY.into(), TYPE_ASSET.into());
    fm.insert(KIND_KEY.into(), SONG_KIND.into());
    fm.insert("slug".into(), slug.into());
    for (k, v) in owned_values(song) {
        fm.insert(k.into(), v);
    }
    fm.insert("source".into(), SOURCE.into());
    Ok(format!(
        "---\n{}---\n# {}\n\n## Arrangements\n\n_Every chart whose `song` is `song:{slug}` is one \
         of this song's arrangements, and one of them is the default. They are not listed here — \
         a chart names its song and the song says nothing back, so adding an arrangement is one \
         write with nothing to keep in step. Task's backlinks panel is the list._\n\n## Notes\n\n",
        yaml(&fm)?,
        song.title,
    ))
}

/// Re-save an existing song document: rewrite the [`APP_OWNED`]
/// frontmatter keys, keep every other key and the whole body.
pub fn refresh_document(existing: &str, song: &SongDoc) -> Result<String, ResourceError> {
    let (fm_text, body) = split(existing).ok_or(ResourceError::NoFrontmatter)?;
    let mut fm: Mapping =
        serde_yaml::from_str(fm_text).map_err(|e| ResourceError::Yaml(e.to_string()))?;
    crate::chart::promote_frontmatter(&mut fm);
    for (k, v) in owned_values(song) {
        fm.insert(k.into(), v);
    }
    Ok(format!("---\n{}---\n{body}", yaml(&fm)?))
}

/// Build a song's vault document from the vendored `song` crate's
/// `song.md`, which is what `task song add` used to write.
///
/// # What is dropped, and why that is the right call
///
/// The vendored frontmatter carries `id` and `defaultArrangement`
/// uuids and an embedded `arrangements:` list. None of it survives as
/// *song* state:
///
/// - the `id` uuid is replaced by the slug, because the slug is what a
///   `song:<slug>` reference names and a second identity for one thing
///   is a second thing to keep in step;
/// - `defaultArrangement` becomes `is_default` on the chart it pointed
///   at — the flag moves to the thing it is a fact about, where the
///   server can maintain the invariant;
/// - `arrangements:` becomes nothing at all, because each entry
///   *becomes a chart* that names this song, and the list was only ever
///   a cache of that.
///
/// Nothing is lost by inspection: the original folder is left on disk
/// untouched, so every uuid is still there to read. What is dropped is
/// dropped from the new representation, not from the disk.
///
/// Foreign keys the vendored writer did not define — anything a person
/// added by hand — survive verbatim, as everywhere else in this crate.
pub fn migrate_document(existing: &str, slug: &str) -> Result<String, ResourceError> {
    let (fm_text, body) = split(existing).ok_or(ResourceError::NoFrontmatter)?;
    let mut fm: Mapping =
        serde_yaml::from_str(fm_text).map_err(|e| ResourceError::Yaml(e.to_string()))?;
    for key in ["id", "defaultArrangement", "arrangements"] {
        fm.remove(Value::from(key));
    }
    fm.insert(TYPE_KEY.into(), TYPE_ASSET.into());
    fm.insert(KIND_KEY.into(), SONG_KIND.into());
    fm.insert("slug".into(), slug.into());
    fm.entry("source".into()).or_insert(SOURCE.into());
    let body = if body.trim().is_empty() {
        format!("# {}\n\n## Notes\n\n", title_of(&fm, slug))
    } else {
        body.to_owned()
    };
    Ok(format!("---\n{}---\n{body}", yaml(&fm)?))
}

fn title_of(fm: &Mapping, slug: &str) -> String {
    fm.get(Value::from("title"))
        .and_then(Value::as_str)
        .unwrap_or(slug)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse_manifest;
    use crate::types::ResourceKind;

    fn song() -> SongDoc {
        SongDoc {
            slug: String::new(),
            title: "Opening Night".into(),
            writers: vec!["A. Wright".into()],
            key: "C Major".into(),
            tags: vec!["album".into()],
            updated_at: "2026-09-09T10:00:00Z".into(),
        }
    }

    #[test]
    fn a_song_renders_as_an_asset_document_and_parses_back() {
        let md = render_document(&song(), "opening-night").unwrap();
        assert!(md.contains("type: asset"), "{md}");
        assert!(md.contains("asset_kind: song"), "{md}");
        let r = parse_manifest(&md).unwrap();
        assert_eq!(r.kind, ResourceKind::Song);
        assert_eq!(r.slug, "opening-night");
        assert_eq!(r.title, "Opening Night");
        assert_eq!(r.writers, ["A. Wright"]);
        assert_eq!(r.tags, ["album"]);
    }

    #[test]
    fn a_re_save_keeps_the_body_and_foreign_keys() {
        let existing = "---\ntype: asset\nasset_kind: song\nslug: opening-night\ntitle: Old\nbpm: 188\n---\n# Old\n\n## Notes\n\n- written on a bus\n";
        let out = refresh_document(existing, &song()).unwrap();
        assert!(out.ends_with("- written on a bus\n"), "{out}");
        assert!(out.contains("bpm: 188"), "foreign key survives: {out}");
        assert!(out.contains("title: Opening Night"), "{out}");
    }

    /// The vendored folder's uuids do not come across, and the reason
    /// is stated in the migration's own docs: the slug is the identity,
    /// and the default flag is a fact about a chart.
    #[test]
    fn the_vendored_song_md_migrates_without_its_uuids() {
        let vendored = "---\nid: 75e30481-6a81-4759-98b7-816c8c605d46\ntitle: Opening Night\ntags: []\ndefaultArrangement: be760d4e-e43a-4297-9bae-30ef49925f89\narrangements:\n- id: be760d4e-e43a-4297-9bae-30ef49925f89\n  name: Default\n  dir: default\n  key: C Major\nbpm: 188\n---\n";
        let out = migrate_document(vendored, "opening-night").unwrap();

        assert!(out.contains("type: asset"), "{out}");
        assert!(out.contains("asset_kind: song"), "{out}");
        assert!(out.contains("slug: opening-night"), "{out}");
        assert!(!out.contains("75e30481"), "the song uuid is gone: {out}");
        assert!(!out.contains("defaultArrangement"), "{out}");
        assert!(
            !out.contains("arrangements:"),
            "each entry becomes a chart that names this song: {out}"
        );
        assert!(out.contains("bpm: 188"), "a hand-added key survives: {out}");
        assert!(
            out.contains("# Opening Night"),
            "an empty vendored body gets a heading to write under: {out}"
        );

        let r = parse_manifest(&out).unwrap();
        assert_eq!(r.kind, ResourceKind::Song);
        assert_eq!(r.slug, "opening-night");
        assert_eq!(r.title, "Opening Night");

        // Idempotent: the server re-runs the migration on every boot.
        assert_eq!(migrate_document(&out, "opening-night").unwrap(), out);
    }
}
