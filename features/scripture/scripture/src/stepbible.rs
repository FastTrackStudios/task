//! Convert STEPBible TAGNT (Greek NT) / TAHOT (Hebrew OT) into the
//! unified [`OrigWord`] schema.
//!
//! Both are big tab-separated files (CC BY 4.0) with a long licence
//! header; data rows start with a reference like `Mat.1.1#01=…`. The two
//! have *different* column layouts, so there's a parser each. We keep
//! only the word-study columns (surface, translit, lemma, Strong's,
//! morphology, gloss) and drop the editions/variants apparatus.
//!
//! Each parser returns flat `(VerseId, OrigWord)` pairs in document
//! order; the installer groups them across the source's several files.

use std::sync::LazyLock;

use regex::Regex;
use scripture_proto::VerseId;

use crate::original::OrigWord;

/// The verse reference at the head of a data row (`Mat.1.1#01=NKO` →
/// `Mat.1.1`). Returns `None` for header / blank / non-data lines.
fn row_ref(col0: &str) -> Option<VerseId> {
    let reference = col0.split('#').next()?;
    if !reference.contains('.') {
        return None;
    }
    VerseId::parse(reference).ok()
}

/// TAGNT (Greek): `ref | "word (translit)" | english | Strong=Morph |
/// lemma=gloss | …`.
#[must_use]
pub fn parse_tagnt_rows(tsv: &str) -> Vec<(VerseId, OrigWord)> {
    let mut out = Vec::new();
    for line in tsv.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        let Some(id) = cols.first().and_then(|c| row_ref(c)) else {
            continue;
        };

        let (word, translit) = split_surface(cols.get(1).copied().unwrap_or_default());
        let (strong, morph) = split_eq(cols.get(3).copied().unwrap_or_default());
        let (lemma, gloss) = split_eq(cols.get(4).copied().unwrap_or_default());

        out.push((
            id,
            OrigWord {
                word,
                translit,
                lemma,
                strong: strong.trim_matches(['{', '}']).to_string(),
                morph,
                gloss,
            },
        ));
    }
    out
}

/// TAHOT (Hebrew): `ref | hebrew | translit | english | dStrongs |
/// grammar | … | expanded-strong-tags`.
#[must_use]
pub fn parse_tahot_rows(tsv: &str) -> Vec<(VerseId, OrigWord)> {
    let mut out = Vec::new();
    for line in tsv.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        let Some(id) = cols.first().and_then(|c| row_ref(c)) else {
            continue;
        };

        let word = cols.get(1).copied().unwrap_or_default().trim().to_string();
        let translit = cols.get(2).copied().unwrap_or_default().trim().to_string();
        let gloss = cols.get(3).copied().unwrap_or_default().trim().to_string();
        let strong = primary_strong(cols.get(4).copied().unwrap_or_default());
        let morph = cols.get(5).copied().unwrap_or_default().trim().to_string();
        // Expanded tag (col 11) carries `{Hxxxx=lemma=…}`.
        let lemma = expanded_lemma(cols.get(11).copied().unwrap_or_default());

        out.push((
            id,
            OrigWord {
                word,
                translit,
                lemma,
                strong,
                morph,
                gloss,
            },
        ));
    }
    out
}

/// `"Βίβλος (Biblos)"` → (`Βίβλος`, `Biblos`).
fn split_surface(s: &str) -> (String, String) {
    if let Some((w, rest)) = s.split_once(" (") {
        (
            w.trim().to_string(),
            rest.trim_end_matches(')').trim().to_string(),
        )
    } else {
        (s.trim().to_string(), String::new())
    }
}

/// `"G0976=N-NSF"` → (`G0976`, `N-NSF`); no `=` ⇒ all in the first.
fn split_eq(s: &str) -> (String, String) {
    match s.split_once('=') {
        Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
        None => (s.trim().to_string(), String::new()),
    }
}

/// The primary Strong's from a TAHOT dStrongs cell: the braced code
/// (`{H0776G}`) when compound (`H9003/{H0776G}\H9014`), else the first
/// `H####` token.
fn primary_strong(s: &str) -> String {
    static BRACED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{(H[0-9A-Za-z]+)\}").unwrap());
    static ANY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"H[0-9]+[A-Za-z]?").unwrap());
    if let Some(c) = BRACED.captures(s) {
        return c[1].to_string();
    }
    ANY.find(s)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

/// Lemma from a TAHOT expanded-tag cell: `{H0376G=אִישׁ=: man…}` → `אִישׁ`
/// (the text between the first and second `=` inside the first `{…}`).
fn expanded_lemma(s: &str) -> String {
    static BRACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{([^}]*)\}").unwrap());
    BRACE
        .captures(s)
        .and_then(|c| c.get(1))
        .and_then(|inner| inner.as_str().split('=').nth(1))
        .map(|l| l.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagnt_row_parses() {
        let row = "Mat.1.1#01=NKO\tΒίβλος (Biblos)\t[The] book\tG0976=N-NSF\tβίβλος=book\tNA28";
        let v = parse_tagnt_rows(row);
        assert_eq!(v.len(), 1);
        let (id, w) = &v[0];
        assert_eq!(*id, VerseId::parse("Matthew 1:1").unwrap());
        assert_eq!(w.word, "Βίβλος");
        assert_eq!(w.translit, "Biblos");
        assert_eq!(w.strong, "G0976");
        assert_eq!(w.morph, "N-NSF");
        assert_eq!(w.lemma, "βίβλος");
        assert_eq!(w.gloss, "book");
    }

    #[test]
    fn tahot_row_parses_simple_and_compound() {
        let rows = "\
Job.1.1#01=L\tאִ֛ישׁ\t'ish\ta man\t{H0376G}\tHNcmsa\t\t\tH0376G_A\t\t\t{H0376G=אִישׁ=: man»man:1_man}
Job.1.1#03=L\tבְ/אֶֽרֶץ\tve./'e.retz\tin [the] land\tH9003/{H0776G}\\H9014\tHR/Ncfsc\t\t\t\t\t\t{H0776G=אֶרֶץ=land}";
        let v = parse_tahot_rows(rows);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].1.word, "אִ֛ישׁ");
        assert_eq!(v[0].1.translit, "'ish");
        assert_eq!(v[0].1.strong, "H0376G");
        assert_eq!(v[0].1.morph, "HNcmsa");
        assert_eq!(v[0].1.lemma, "אִישׁ");
        assert_eq!(v[0].1.gloss, "a man");
        // Compound dStrongs → the braced primary.
        assert_eq!(v[1].1.strong, "H0776G");
        assert_eq!(v[1].1.lemma, "אֶרֶץ");
    }

    #[test]
    fn header_and_blank_lines_skipped() {
        let junk = "TAGNT Mat-Jhn …\n\t\t\t\nWord & Type\tGreek\tEnglish";
        assert!(parse_tagnt_rows(junk).is_empty());
    }
}
