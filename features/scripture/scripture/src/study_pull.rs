//! Pulling the word-study data — the Strong's lexicon and the
//! original-language texts — the way [`crate::pull`] pulls a Bible.
//!
//! A reader's word study (lexicon entry, every occurrence, the
//! interlinear) needs two things no corpus carries: the Strong's
//! dictionaries (`resources/lexicon/strongs/{greek,hebrew}.json`) and an
//! original-language edition (`resources/original/<ID>/`). Both are
//! openly licensed and both were install-it-yourself: a demo had a word
//! study panel and nothing behind it. This fetches them from their
//! published sources, caches the downloads beside the Bibles (a second
//! plant does not refetch), and writes the resource library the server
//! loads at start.
//!
//! Sources:
//! - Strong's dictionaries: OpenScriptures `strongs` (CC BY-SA).
//! - Hebrew: OpenScriptures Hebrew Bible, the Westminster Leningrad Codex
//!   with morphology (text public domain; morphology CC BY 4.0).
//! - Greek: MorphGNT's SBLGNT (morphology CC BY-SA; text under the SBLGNT
//!   licence, free to use with attribution).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::pull::{PullError, cache_dir};
use crate::{OrigMeta, OrigText, OrigWord, VerseId};

const STRONGS_BASE: &str = "https://raw.githubusercontent.com/openscriptures/strongs/master";
const OSHB_BASE: &str = "https://raw.githubusercontent.com/openscriptures/morphhb/master/wlc";
const SBLGNT_BASE: &str = "https://raw.githubusercontent.com/morphgnt/sblgnt/master";

/// The OSHB's per-book files, by OSIS book.
const OSHB_BOOKS: &[&str] = &[
    "Gen", "Exod", "Lev", "Num", "Deut", "Josh", "Judg", "Ruth", "1Sam", "2Sam", "1Kgs", "2Kgs",
    "1Chr", "2Chr", "Ezra", "Neh", "Esth", "Job", "Ps", "Prov", "Eccl", "Song", "Isa", "Jer",
    "Lam", "Ezek", "Dan", "Hos", "Joel", "Amos", "Obad", "Jonah", "Mic", "Nah", "Hab", "Zeph",
    "Hag", "Zech", "Mal",
];

/// MorphGNT's per-book files, numbered from 61 (Matthew).
const SBLGNT_BOOKS: &[&str] = &[
    "Mt", "Mk", "Lk", "Jn", "Ac", "Ro", "1Co", "2Co", "Ga", "Eph", "Php", "Col", "1Th", "2Th",
    "1Ti", "2Ti", "Tit", "Phm", "Heb", "Jas", "1Pe", "2Pe", "1Jn", "2Jn", "3Jn", "Jud", "Re",
];

/// The original-language editions that can be pulled whole.
pub const ORIGINALS: &[&str] = &["OSHB", "SBLGNT"];

/// Install the Strong's lexicon into `dest` (the org's
/// `resources/lexicon/strongs`). Returns how many entries each side has.
///
/// # Errors
///
/// A failed download, a dictionary that does not parse, or any
/// filesystem error.
pub async fn pull_lexicon(dest: &Path) -> Result<(usize, usize), PullError> {
    let mut counts = [0usize; 2];
    let mut docs = Vec::new();
    for (i, lang) in ["greek", "hebrew"].iter().enumerate() {
        let url = format!("{STRONGS_BASE}/{lang}/strongs-{lang}-dictionary.js");
        let js = fetch_cached(&url, &format!("strongs/{lang}.js")).await?;
        let js = String::from_utf8_lossy(&js).into_owned();
        let json = crate::js_to_json(&js)
            .ok_or_else(|| parse_error(&url, "no dictionary object in the file"))?
            .to_owned();
        let lexicon = crate::Lexicon::from_json(&json).map_err(|e| parse_error(&url, &e.to_string()))?;
        if let Some(n) = counts.get_mut(i) {
            *n = lexicon.len();
        }
        docs.push((*lang, json));
    }
    // Written only once both parsed: a lexicon with one side is not
    // installed half.
    std::fs::create_dir_all(dest).map_err(io(dest))?;
    for (lang, json) in docs {
        let path = dest.join(format!("{lang}.json"));
        std::fs::write(&path, json).map_err(io(&path))?;
    }
    Ok((counts[0], counts[1]))
}

/// Install an original-language edition (`OSHB` or `SBLGNT`) into
/// `dest` (the org's `resources/original/<ID>`). Returns the verse count.
///
/// # Errors
///
/// An unknown edition, a failed download, or any filesystem error.
pub async fn pull_original(id: &str, dest: &Path) -> Result<usize, PullError> {
    let mut map: BTreeMap<VerseId, Vec<OrigWord>> = BTreeMap::new();
    let (name, language, license) = match id.to_ascii_uppercase().as_str() {
        "OSHB" => {
            for book in OSHB_BOOKS {
                let url = format!("{OSHB_BASE}/{book}.xml");
                let raw = fetch_cached(&url, &format!("oshb/{book}.xml")).await?;
                for (vid, w) in crate::oshb::parse_oshb_xml(&String::from_utf8_lossy(&raw)) {
                    map.entry(vid).or_default().push(w);
                }
            }
            (
                "Open Scriptures Hebrew Bible (WLC)",
                "hebrew",
                "Public domain (text); CC BY 4.0 (morphology)",
            )
        }
        "SBLGNT" => {
            for (n, book) in (61..).zip(SBLGNT_BOOKS) {
                let file = format!("{n}-{book}-morphgnt.txt");
                let url = format!("{SBLGNT_BASE}/{file}");
                let raw = fetch_cached(&url, &format!("sblgnt/{file}")).await?;
                for (vid, w) in crate::morphgnt::parse_morphgnt_rows(&String::from_utf8_lossy(&raw)) {
                    map.entry(vid).or_default().push(w);
                }
            }
            (
                "SBL Greek New Testament (MorphGNT)",
                "greek",
                "SBLGNT licence (free with attribution); CC BY-SA (MorphGNT morphology)",
            )
        }
        _ => {
            return Err(parse_error(id, "not an edition this can pull (OSHB, SBLGNT)"));
        }
    };
    let text = OrigText::from_verses(map);
    let verses = text.len();
    std::fs::create_dir_all(dest).map_err(io(dest))?;
    let jsonl = dest.join("text.jsonl");
    std::fs::write(&jsonl, text.to_jsonl()).map_err(io(&jsonl))?;
    let meta = OrigMeta {
        id: id.to_ascii_uppercase(),
        name: name.to_owned(),
        language: language.to_owned(),
        license: license.to_owned(),
    };
    let meta_path = dest.join("meta.json");
    let meta_json = serde_json::to_string_pretty(&meta).map_err(|e| parse_error(id, &e.to_string()))?;
    std::fs::write(&meta_path, meta_json).map_err(io(&meta_path))?;
    Ok(verses)
}

/// `url`'s bytes, from the download cache when a previous pull kept them.
async fn fetch_cached(url: &str, key: &str) -> Result<Vec<u8>, PullError> {
    let path: PathBuf = cache_dir().join("study").join(key);
    if let Ok(bytes) = std::fs::read(&path) {
        return Ok(bytes);
    }
    let fetched = async {
        reqwest::get(url)
            .await?
            .error_for_status()?
            .bytes()
            .await
    }
    .await
    .map_err(|source| PullError::Fetch {
        url: url.to_owned(),
        source,
    })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io(dir))?;
    }
    std::fs::write(&path, &fetched).map_err(io(&path))?;
    Ok(fetched.to_vec())
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> PullError + '_ {
    move |source| PullError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn parse_error(what: &str, why: &str) -> PullError {
    PullError::Io {
        path: what.to_owned(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, why.to_owned()),
    }
}
