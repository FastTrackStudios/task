//! Convert the OpenScriptures Hebrew Bible (OSHB / morphhb — the
//! Westminster Leningrad Codex) into the unified [`OrigWord`] schema.
//!
//! Source: github.com/openscriptures/morphhb (text public domain;
//! annotations CC BY 4.0). One OSIS XML file per book, with container
//! verses:
//!
//! ```xml
//! <verse osisID="Gen.1.1">
//!   <w lemma="b/7225" morph="HR/Ncfsa" id="01xeN">בְּ/רֵאשִׁ֖ית</w>
//!   …
//! </verse>
//! ```
//!
//! The `lemma` attribute is the (augmented) Strong's number — possibly
//! prefixed (`b/7225`) or with a homonym letter (`1254 a`); we lift the
//! word's Strong's into `strong` as `H####`. OSHB carries no
//! transliteration or English gloss, and its lemma attr is a number, so
//! `translit` / `gloss` / `lemma` stay empty (it's a critical text,
//! complementary to STEPBible's richer amalgam).

use scripture_proto::VerseId;

use crate::original::OrigWord;

/// Parse one OSHB book's OSIS XML into `(VerseId, OrigWord)` pairs.
#[must_use]
pub fn parse_oshb_xml(xml: &str) -> Vec<(VerseId, OrigWord)> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for verse in doc.descendants().filter(|n| n.has_tag_name("verse")) {
        let Some(id) = verse
            .attribute("osisID")
            .and_then(|s| VerseId::parse(s).ok())
        else {
            continue;
        };
        for w in verse.descendants().filter(|n| n.has_tag_name("w")) {
            let surface: String = w
                .descendants()
                .filter(roxmltree::Node::is_text)
                .filter_map(|n| n.text())
                .collect::<String>()
                .trim()
                .to_string();
            if surface.is_empty() {
                continue;
            }
            out.push((
                id,
                OrigWord {
                    word: surface,
                    translit: String::new(),
                    lemma: String::new(),
                    strong: strong_of(w.attribute("lemma").unwrap_or_default()),
                    morph: w.attribute("morph").unwrap_or_default().to_string(),
                    gloss: String::new(),
                },
            ));
        }
    }
    out
}

/// Lift the word's Strong's from an OSHB `lemma` attribute. Prefers the
/// last content code (< 9000); OSHB's `9xxx` codes mark prefixes/suffixes.
fn strong_of(lemma: &str) -> String {
    let nums: Vec<u32> = lemma
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    let pick = nums
        .iter()
        .rev()
        .find(|n| **n < 9000)
        .or_else(|| nums.last());
    pick.map(|n| format!("H{n}")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0"?>
<osis><osisText><div><chapter osisID="Gen.1">
  <verse osisID="Gen.1.1">
    <w lemma="b/7225" morph="HR/Ncfsa" id="01xeN">בְּ/רֵאשִׁ֖ית</w>
    <w lemma="1254 a" morph="HVqp3ms" id="01Nvk">בָּרָ֣א</w>
    <w lemma="430" morph="HNcmpa" id="01TyA">אֱלֹהִ֑ים</w>
  </verse>
</chapter></div></osisText></osis>"#;

    #[test]
    fn parses_verse_words_and_strongs() {
        let v = parse_oshb_xml(XML);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].0, VerseId::parse("Genesis 1:1").unwrap());
        assert_eq!(v[0].1.word, "בְּ/רֵאשִׁ֖ית");
        assert_eq!(v[0].1.strong, "H7225"); // prefix `b` dropped
        assert_eq!(v[0].1.morph, "HR/Ncfsa");
        assert_eq!(v[1].1.strong, "H1254"); // homonym letter dropped
        assert_eq!(v[2].1.strong, "H430");
    }

    #[test]
    fn bad_xml_is_empty() {
        assert!(parse_oshb_xml("<not closed").is_empty());
    }
}
