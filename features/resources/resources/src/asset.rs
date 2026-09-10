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

/// Rewrite the named top-level keys' block sequences into **flow**
/// form (`tags: [a, b]`), in already-serialised frontmatter.
///
/// # Why this exists, and it is not cosmetic
///
/// A vault page's tags reach the folder index — and from there the
/// sidebar, the tag tree and every UI that groups by tag — through
/// `editor_state::markdown::parse_frontmatter`, whose block-sequence
/// branch requires each `- item` line to be **indented**. `serde_yaml`
/// emits them unindented:
///
/// ```text
/// tags:
/// - album
/// ```
///
/// which that parser reads as an *empty* property. So a vault document
/// this crate writes with block-form tags has, as far as Task's own UI
/// is concerned, no tags at all — silently, with the file looking
/// perfectly correct to a person and to `serde_yaml`.
///
/// That is exactly the class of bug ADR 0004 is supposed to remove:
/// an asset is a vault item, and if its tags do not work like a note's
/// tags then it is not one. Rather than teach an external parser a
/// second YAML shape, this writes the shape the vault already uses —
/// every seeded note in `examples/studio` spells tags inline — so the
/// files this crate produces and the files a person writes by hand
/// parse the same way.
///
/// Only *top-level* keys, and only the ones named: nested sequences
/// (a `media:` list of mappings) are left alone, because flow form
/// cannot carry them and nothing reads them through that parser.
#[must_use]
pub fn inline_sequences(yaml: &str, keys: &[&str]) -> String {
    let mut out = String::with_capacity(yaml.len());
    let mut lines = yaml.lines().peekable();
    while let Some(line) = lines.next() {
        let is_target = line
            .strip_suffix(':')
            .is_some_and(|k| keys.contains(&k) && !k.starts_with(char::is_whitespace));
        if !is_target {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Collect the unindented `- item` lines that follow. A scalar
        // item only — a nested mapping under one of these keys is not
        // something the caller declared, so leave the block alone.
        let mut items: Vec<String> = Vec::new();
        while let Some(next) = lines.peek() {
            let Some(item) = next.strip_prefix("- ") else {
                break;
            };
            items.push(item.trim().to_owned());
            lines.next();
        }
        let key = line.trim_end_matches(':');
        out.push_str(&format!("{key}: [{}]\n", items.join(", ")));
    }
    out
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

    /// The bug this guards: a block sequence `serde_yaml` emits is
    /// invisible to the vault's own frontmatter parser, so a chart or
    /// song written with block-form tags would have no tags in Task's
    /// UI — which would make it not-a-vault-item in the one way that
    /// matters.
    #[test]
    fn sequences_are_written_the_way_the_vault_reads_them() {
        let yaml = "title: Opening Night\nwriters:\n- A. Wright\n- B. Lee\nkey: C\ntags:\n- album\nmedia:\n- kind: video\n";
        let out = inline_sequences(yaml, &["writers", "tags"]);
        assert!(out.contains("writers: [A. Wright, B. Lee]"), "{out}");
        assert!(out.contains("tags: [album]"), "{out}");
        assert!(out.contains("title: Opening Night"), "{out}");
        assert!(out.contains("key: C"), "{out}");
        assert!(
            out.contains("media:\n- kind: video"),
            "a key nobody named is left alone: {out}"
        );
    }

    /// An empty sequence still has to *be* one, or a re-read turns
    /// `tags: []` into a string.
    #[test]
    fn an_empty_sequence_stays_a_sequence() {
        assert_eq!(inline_sequences("tags: []\n", &["tags"]), "tags: []\n");
        assert_eq!(inline_sequences("tags:\n", &["tags"]), "tags: []\n");
    }

    #[test]
    fn a_manifest_without_frontmatter_is_refused_rather_than_overwritten() {
        assert!(matches!(
            refresh_manifest("# just a note\n", vec![]),
            Err(ResourceError::NoFrontmatter)
        ));
    }
}
