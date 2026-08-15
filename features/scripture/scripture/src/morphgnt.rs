//! Convert MorphGNT (the morphologically-tagged SBL Greek NT) into the
//! unified [`OrigWord`] schema.
//!
//! Source: github.com/morphgnt/sblgnt (CC BY-SA / SBLGNT licence).
//! Space-separated, one word per line:
//!
//! ```text
//! 010101 N- ----NSF- Βίβλος Βίβλος βίβλος βίβλος
//! BBCCVV POS PARSE    text   word   norm    lemma
//! ```
//!
//! The leading field is `BBCCVV` with MorphGNT's own NT book numbering
//! (`01` = Matthew … `27` = Revelation), so the canon ordinal is
//! `39 + BB`. MorphGNT is lemma-based and carries **no Strong's numbers**
//! — `strong` stays empty (it's a critical text, complementary to
//! STEPBible's Strong's-tagged amalgam).

use scripture_proto::{Book, VerseId};

use crate::original::OrigWord;

/// Parse a MorphGNT book file into `(VerseId, OrigWord)` pairs.
#[must_use]
pub fn parse_morphgnt_rows(text: &str) -> Vec<(VerseId, OrigWord)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 7 {
            continue;
        }
        let Some(id) = verse_of(cols[0]) else {
            continue;
        };
        out.push((
            id,
            OrigWord {
                word: cols[4].to_string(),
                translit: String::new(),
                lemma: cols[6].to_string(),
                strong: String::new(),
                // POS + parse code, e.g. `N- ----NSF-`.
                morph: format!("{} {}", cols[1], cols[2]),
                gloss: String::new(),
            },
        ));
    }
    out
}

/// `BBCCVV` (MorphGNT NT numbering) → [`VerseId`].
fn verse_of(code: &str) -> Option<VerseId> {
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let bb: u8 = code[0..2].parse().ok()?;
    let chapter: u16 = code[2..4].parse().ok()?;
    let verse: u16 = code[4..6].parse().ok()?;
    let book = Book::from_ordinal(39u8.checked_add(bb)?)?;
    Some(VerseId::new(book, chapter, verse))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_word_and_maps_the_book() {
        // Matthew 1:1 (book 01 → ordinal 40); John 3:16 (book 04 → 43).
        let text = "010101 N- ----NSF- Βίβλος Βίβλος βίβλος βίβλος\n\
                    040316 V- 3AAI-S-- ἠγάπησεν ἠγάπησεν ἠγάπησεν ἀγαπάω";
        let v = parse_morphgnt_rows(text);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].0, VerseId::parse("Matthew 1:1").unwrap());
        assert_eq!(v[0].1.word, "Βίβλος");
        assert_eq!(v[0].1.lemma, "βίβλος");
        assert_eq!(v[0].1.morph, "N- ----NSF-");
        assert!(v[0].1.strong.is_empty());
        // John 3:16 → ἀγαπάω.
        assert_eq!(v[1].0, VerseId::parse("John 3:16").unwrap());
        assert_eq!(v[1].1.lemma, "ἀγαπάω");
    }

    #[test]
    fn skips_short_or_nonnumeric_lines() {
        assert!(parse_morphgnt_rows("# comment\nfoo bar\n").is_empty());
    }
}
