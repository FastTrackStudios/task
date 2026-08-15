//! Versification reconciliation — line up editions that number verses
//! differently (Hebrew vs English: Psalm-title shifts, the chapter-break
//! offsets in Genesis/Exodus, the Psalm 9/10 split, …).
//!
//! Data: the Copenhagen Alliance `versification-mappings` (one JSON per
//! tradition), bundled at `<org>/resources/versification/<scheme>.json`.
//! Each file's `mappedVerses` maps that tradition's reference to the base
//! `org` (Hebrew/Leningrad) reference, e.g. `"GEN 31:55" → "GEN 32:1"`.
//! Every range stays within a single chapter, so expansion is simple;
//! the NT has no differences, so Greek editions map identically.
//!
//! [`Versification::map`] resolves a verse between any two schemes by
//! routing through `org` (`from → org → to`).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use scripture_proto::{VerseId, VerseRange};
use serde::Deserialize;

use crate::bible::LoadError;

#[derive(Debug, Default, Deserialize)]
struct RawVrs {
    #[serde(default, rename = "mappedVerses")]
    mapped: BTreeMap<String, String>,
}

/// Loaded versification mappings: scheme name → (`scheme_ref → org_ref`).
#[derive(Debug, Clone, Default)]
pub struct Versification {
    schemes: HashMap<String, BTreeMap<VerseId, VerseId>>,
}

impl Versification {
    /// Load every `<scheme>.json` in a directory. The file stem is the
    /// scheme name (`eng`, `lxx`, `vul`, …). A missing dir is empty.
    pub fn load_dir(dir: &Path) -> Result<Self, LoadError> {
        let mut schemes = HashMap::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(LoadError::Io {
                    path: dir.display().to_string(),
                    source,
                });
            }
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            if name == "org" {
                continue; // the base — identity
            }
            let text = std::fs::read_to_string(&path).map_err(|source| LoadError::Io {
                path: path.display().to_string(),
                source,
            })?;
            if let Ok(raw) = serde_json::from_str::<RawVrs>(&text) {
                schemes.insert(name, build_forward(&raw.mapped));
            }
        }
        Ok(Self { schemes })
    }

    /// Build from in-memory `scheme → {ref: ref}` mappings (tests).
    #[must_use]
    pub fn from_maps(schemes: HashMap<String, BTreeMap<String, String>>) -> Self {
        Self {
            schemes: schemes
                .iter()
                .map(|(k, m)| (k.clone(), build_forward(m)))
                .collect(),
        }
    }

    /// Map `verse` from scheme `from` to scheme `to`, routing through the
    /// `org` base. Returns the equivalent verse(s) — usually one, more
    /// when a verse splits. Unmapped verses pass through unchanged.
    #[must_use]
    pub fn map(&self, verse: VerseId, from: &str, to: &str) -> Vec<VerseId> {
        if from.eq_ignore_ascii_case(to) {
            return vec![verse];
        }
        // from → org
        let org = if from.eq_ignore_ascii_case("org") {
            verse
        } else {
            self.schemes
                .get(from)
                .and_then(|m| m.get(&verse))
                .copied()
                .unwrap_or(verse)
        };
        // org → to
        if to.eq_ignore_ascii_case("org") {
            return vec![org];
        }
        let Some(target) = self.schemes.get(to) else {
            return vec![org];
        };
        let rev: Vec<VerseId> = target
            .iter()
            .filter(|(_, o)| **o == org)
            .map(|(s, _)| *s)
            .collect();
        if rev.is_empty() { vec![org] } else { rev }
    }
}

/// Expand each `"A:x-y" → "B:p-q"` entry into per-verse `scheme → org`
/// pairs (positional zip; both sides stay within one chapter).
fn build_forward(mapped: &BTreeMap<String, String>) -> BTreeMap<VerseId, VerseId> {
    let mut out = BTreeMap::new();
    for (k, v) in mapped {
        let (Ok(kr), Ok(vr)) = (VerseRange::parse(k), VerseRange::parse(v)) else {
            continue;
        };
        let (ks, vs) = (expand(kr), expand(vr));
        for (i, &sref) in ks.iter().enumerate() {
            // Last org verse absorbs any overflow (rare verse merges).
            if let Some(oref) = vs.get(i).or_else(|| vs.last()) {
                out.insert(sref, *oref);
            }
        }
    }
    out
}

/// Verses of a single-chapter range, inclusive (`PSA 3:0-8` → 0..=8).
fn expand(r: VerseRange) -> Vec<VerseId> {
    if r.start.book == r.end.book && r.start.chapter == r.end.chapter {
        (r.start.verse..=r.end.verse)
            .map(|v| VerseId::new(r.start.book, r.start.chapter, v))
            .collect()
    } else {
        vec![r.start, r.end]
    }
}

/// The versification scheme an edition uses, from its language. Hebrew
/// editions follow the `org` base; Greek/English follow `eng` (the NT and
/// English OT share numbering with no relevant differences).
#[must_use]
pub fn scheme_for_language(language: &str) -> &'static str {
    if language.eq_ignore_ascii_case("hebrew") || language.eq_ignore_ascii_case("aramaic") {
        "org"
    } else {
        "eng"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vrs() -> Versification {
        let mut eng = BTreeMap::new();
        // Genesis chapter-break shift, and a Psalm-title whole-psalm shift.
        eng.insert("GEN 31:55".to_string(), "GEN 32:1".to_string());
        eng.insert("GEN 32:1-32".to_string(), "GEN 32:2-33".to_string());
        eng.insert("PSA 3:0-8".to_string(), "PSA 3:1-9".to_string());
        Versification::from_maps(HashMap::from([("eng".to_string(), eng)]))
    }

    #[test]
    fn eng_to_org_shifts() {
        let v = vrs();
        // English Gen 31:55 is Hebrew Gen 32:1.
        assert_eq!(
            v.map(VerseId::parse("Genesis 31:55").unwrap(), "eng", "org"),
            vec![VerseId::parse("Genesis 32:1").unwrap()]
        );
        // English Psalm 3:1 is Hebrew Psalm 3:2 (title is Hebrew v1).
        assert_eq!(
            v.map(VerseId::parse("Psalms 3:1").unwrap(), "eng", "org"),
            vec![VerseId::parse("Psalms 3:2").unwrap()]
        );
        // Unmapped verse passes through.
        assert_eq!(
            v.map(VerseId::parse("John 3:16").unwrap(), "eng", "org"),
            vec![VerseId::parse("John 3:16").unwrap()]
        );
    }

    #[test]
    fn org_to_eng_reverses() {
        let v = vrs();
        assert_eq!(
            v.map(VerseId::parse("Psalms 3:2").unwrap(), "org", "eng"),
            vec![VerseId::parse("Psalms 3:1").unwrap()]
        );
    }

    #[test]
    fn identity_when_same_scheme() {
        let id = VerseId::parse("Psalms 3:1").unwrap();
        assert_eq!(vrs().map(id, "eng", "eng"), vec![id]);
    }

    #[test]
    fn scheme_by_language() {
        assert_eq!(scheme_for_language("Hebrew"), "org");
        assert_eq!(scheme_for_language("Greek"), "eng");
    }
}
