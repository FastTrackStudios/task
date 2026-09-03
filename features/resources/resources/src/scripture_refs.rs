//! Find scripture references in spoken captions — no model, just the
//! grammar a preacher actually uses.
//!
//! Written notes cite `1 Peter 5:7`; captions render what was *said*:
//! `first Peter chapter five verse seven`, `John chapter 3`, `verses 1
//! through 4`, `Genesis one one`, `Psalm 23`. Both shapes are one
//! grammar once numbers are read as numbers:
//!
//! ```text
//! [ordinal] BOOK [chapter] NUM [ (":" | verse[s]) NUM ] [ (- | through | to) NUM [":" NUM] ]
//! ```
//!
//! The guard against false positives is the book name: a hit needs a
//! recognised book immediately followed by a number (digits or number
//! words), so "the next chapter of my life" and "mark my words" never
//! match. Short abbreviations (`Ps`, `Jn`, `Rom`) are accepted only in
//! the written form (`Rom 8:1`), never spoken.
//!
//! Preachers also lean on context — "we're in 1 Peter … chapter 5 …
//! verse one … verse two" — so a bare `chapter N` resolves against the
//! last book they said they were *in* (an ordinal, or `to`/`in`/`of` +
//! a full name), and a bare `verse N` against the last chapter cited.
//! A chapter the book does not have clears that context.
//!
//! Each hit carries the second it was spoken at, so the caller can mint
//! `sermon:<slug>#t:<secs> → verse:<osis>` links.

use resources_proto::TranscriptSegment;
use scripture_proto::{Book, VerseId, VerseRange};

/// One reference found in the captions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefHit {
    /// OSIS id: `1Pet.5.7`, a range `1Pet.5.7-1Pet.5.10`, or a
    /// chapter-only `1Pet.5`.
    pub osis: String,
    /// Whole second the reference starts at (its cue's start).
    pub secs: u32,
    /// `true` when the reference was spoken (number words, `chapter`,
    /// `verse`) rather than written (`5:7`).
    pub spoken: bool,
    /// `true` when only a chapter was named.
    pub chapter_only: bool,
    /// The cue the reference sits in, trimmed.
    pub excerpt: String,
}

/// A caption token with the cue it came from.
struct Tok {
    /// Lower-cased, outer punctuation stripped; digits keep `:` `-` `.`.
    word: String,
    secs: u32,
    cue: usize,
}

/// Every reference in the cues, in time order. Duplicates (the same
/// reference said twice) are kept — each is a moment.
#[must_use]
pub fn extract(segments: &[TranscriptSegment]) -> Vec<RefHit> {
    let toks = tokenize(segments);
    let mut out = Vec::new();
    // What the preacher is "in": the last book named (for a bare
    // `chapter 5`) and the last chapter cited (for a bare `verse 7`).
    let mut last_book: Option<Book> = None;
    let mut ctx: Option<(Book, u16)> = None;
    let mut i = 0;
    while i < toks.len() {
        let mut push = |p: Parsed, i: usize, out: &mut Vec<RefHit>| {
            let cue = &segments[toks[i].cue];
            out.push(RefHit {
                osis: p.osis,
                secs: toks[i].secs,
                spoken: p.spoken,
                chapter_only: p.chapter_only,
                excerpt: cue.text.trim().to_string(),
            });
        };
        if let Some((p, book, chapter, next)) = match_at(&toks, i) {
            last_book = Some(book);
            ctx = Some((book, chapter));
            push(p, i, &mut out);
            i = next;
            continue;
        }
        // A bare book name ("we're in 1 Peter") sets the book context
        // when it cannot be an ordinary word (ordinal, or a long name).
        if let Some((book, _, next)) = match_book(&toks, i) {
            if context_worthy(&toks, i, next) {
                last_book = Some(book);
            }
            i = next;
            continue;
        }
        // "chapter 5" with a book in context. A chapter number the book
        // does not have ("chapter 26" while in 1 Peter — Matthew, left
        // unsaid) means the preacher has moved on: drop the verse
        // context rather than pin the next bare verse to the wrong
        // chapter.
        if word(&toks, i) == Some("chapter") {
            if let Some(book) = last_book {
                if let Some((p, chapter, next)) = parse_chapter(&toks, i + 1, book, true) {
                    ctx = Some((book, chapter));
                    push(p, i, &mut out);
                    i = next;
                    continue;
                }
                if number(&toks, i + 1).is_some() {
                    ctx = None;
                }
            }
        }
        // "verse 7" / "verses 1 through 4" with a chapter in context.
        if matches!(word(&toks, i), Some("verse" | "verses")) {
            if let Some((book, chapter)) = ctx {
                if let Some((p, next)) = parse_verse(&toks, i + 1, book, chapter, true) {
                    push(p, i, &mut out);
                    i = next;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// [`extract`] over plain text (one cue at second 0) — for tests and
/// for callers with an untimed transcript.
#[must_use]
pub fn extract_text(text: &str) -> Vec<RefHit> {
    extract(&[TranscriptSegment {
        start: 0.0,
        dur: 0.0,
        text: text.to_string(),
    }])
}

/// Distinct OSIS ids in first-mention order.
#[must_use]
pub fn distinct_osis(hits: &[RefHit]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    hits.iter()
        .filter(|h| seen.insert(h.osis.clone()))
        .map(|h| h.osis.clone())
        .collect()
}

fn tokenize(segments: &[TranscriptSegment]) -> Vec<Tok> {
    let mut toks = Vec::new();
    for (cue, seg) in segments.iter().enumerate() {
        let secs = seg.start.max(0.0) as u32;
        for raw in seg.text.split_whitespace() {
            // Hyphenated number words (`twenty-three`) split; a digit
            // range (`16-18`) stays one token.
            let lower = raw.to_ascii_lowercase();
            let starts_digit = lower.chars().next().is_some_and(|c| c.is_ascii_digit());
            if !starts_digit && lower.contains('-') {
                for part in lower.split('-') {
                    if let Some(w) = clean(part) {
                        toks.push(Tok { word: w, secs, cue });
                    }
                }
            } else if let Some(w) = clean(&lower) {
                toks.push(Tok { word: w, secs, cue });
            }
        }
    }
    toks
}

/// Strip outer punctuation; keep `:` `-` `–` `.` inside a digit token
/// (`3:16-18`, `5.7`), collapse an en dash to a hyphen.
fn clean(s: &str) -> Option<String> {
    let s = s.replace('\u{2013}', "-");
    let trimmed = s.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    if trimmed.is_empty() {
        return None;
    }
    let keep_inner = trimmed.chars().next().is_some_and(|c| c.is_ascii_digit());
    let out: String = trimmed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || (keep_inner && matches!(c, ':' | '-' | '.')))
        .collect();
    (!out.is_empty()).then_some(out)
}

struct Parsed {
    osis: String,
    spoken: bool,
    chapter_only: bool,
}

/// Try to read a book-led reference starting at `i`; on success return
/// it, the book and chapter it names, and the index of the first token
/// after it.
fn match_at(toks: &[Tok], i: usize) -> Option<(Parsed, Book, u16, usize)> {
    let (book, spoken_book, after_book) = match_book(toks, i)?;
    let mut j = after_book;
    let mut spoken = spoken_book;
    if word(toks, j) == Some("chapter") {
        spoken = true;
        j += 1;
    }
    // Short abbreviations (`Ps`, `Rom`) only count when what follows is
    // written (`Rom 8:1`), never spoken.
    let full = full_name(toks, i, after_book);
    let (p, chapter, next) = parse_chapter(toks, j, book, spoken)?;
    if !full && p.spoken {
        return None;
    }
    Some((p, book, chapter, next))
}

/// Read `NUM [verse-part]` at `j` for `book`: a bare chapter number, a
/// written `5:7[-10]` / `3:20-4:2`, or a chapter followed by a spoken
/// verse. Returns the reference, its chapter, and the next index.
fn parse_chapter(toks: &[Tok], j: usize, book: Book, spoken: bool) -> Option<(Parsed, u16, usize)> {
    let (num, consumed, num_spoken) = number(toks, j)?;
    let spoken = spoken || num_spoken;
    if num == 0 || num > u32::from(book.chapters()) {
        return None;
    }
    let chapter = num as u16;
    let after = j + consumed;

    // Written `chapter:verse` inline on the same token.
    if let Some(written) = written_tail(toks, j) {
        return written_ref(book, chapter, &written).map(|p| (p, chapter, after));
    }

    // Spoken / spaced verse: `verse 7`, `verses 1 through 4`, `3 16`.
    let mut k = after;
    let mut verse_word = false;
    if matches!(word(toks, k), Some("verse" | "verses")) {
        verse_word = true;
        k += 1;
    }
    if let Some((p, next)) = parse_verse(toks, k, book, chapter, spoken || verse_word) {
        return Some((p, chapter, next));
    }
    // Chapter-only (`Romans 8`, `John chapter 3`).
    Some((
        Parsed {
            osis: format!("{}.{}", book.osis(), chapter),
            spoken,
            chapter_only: true,
        },
        chapter,
        after,
    ))
}

/// Read a verse (and an optional `through`/`to`/`-` range end) at `k`
/// within `book chapter`. `None` when there is no verse number there.
fn parse_verse(
    toks: &[Tok],
    k: usize,
    book: Book,
    chapter: u16,
    spoken: bool,
) -> Option<(Parsed, usize)> {
    let (v, c, s) = number(toks, k)?;
    if !(1..=176).contains(&v) || looks_like_ordinal_book(toks, k) {
        return None;
    }
    let mut spoken = spoken || s;
    let verse = v as u16;
    let mut k = k + c;

    // Range end: `- 10`, `through 10`, `to 10`, `thru 10`.
    let start = VerseId::new(book, chapter, verse);
    let mut end = start;
    if matches!(word(toks, k), Some("through" | "thru" | "to" | "-")) {
        if let Some((v, c, s)) = number(toks, k + 1) {
            if (u32::from(verse) + 1..=176).contains(&v) {
                if s || word(toks, k) != Some("-") {
                    spoken = true;
                }
                end = VerseId::new(book, chapter, v as u16);
                k += 1 + c;
            }
        }
    }
    Some((
        Parsed {
            osis: VerseRange { start, end }.osis(),
            spoken,
            chapter_only: false,
        },
        k,
    ))
}

/// Whether a bare book mention can set the book context: an ordinal
/// (`we're in 1 Peter`), or a full name the preacher is going *to* /
/// is *in* (`turn to Romans`, `the book of James`) — so `James and
/// John`, the people, never become the book you are in.
fn context_worthy(toks: &[Tok], i: usize, after_book: usize) -> bool {
    if word(toks, i).and_then(ordinal).is_some() {
        return true;
    }
    let led_in = i > 0
        && matches!(
            word(toks, i - 1),
            Some("in" | "to" | "of" | "from" | "into" | "through" | "at")
        );
    led_in && full_name(toks, i, after_book)
}

/// The written tail of a digit token: `5:7`, `5:7-10`, `3:20-4:2`,
/// `5.7`. `None` for a bare number.
fn written_tail(toks: &[Tok], i: usize) -> Option<String> {
    let w = word(toks, i)?;
    (w.contains(':') || (w.contains('.') && w.chars().all(|c| c.is_ascii_digit() || c == '.')))
        .then(|| w.to_string())
}

fn written_ref(book: Book, chapter: u16, tail: &str) -> Option<Parsed> {
    // `chapter:verse[-end]` where end is `v` or `c:v`.
    let (head, rest) = tail
        .split_once('-')
        .map_or((tail, None), |(a, b)| (a, Some(b)));
    let (_, v) = head.split_once([':', '.'])?;
    let verse: u16 = v.parse().ok()?;
    if verse == 0 {
        return None;
    }
    let start = VerseId::new(book, chapter, verse);
    let end = match rest {
        None => start,
        Some(r) => {
            if let Some((c, v)) = r.split_once([':', '.']) {
                VerseId::new(book, c.parse().ok()?, v.parse().ok()?)
            } else {
                VerseId::new(book, chapter, r.parse().ok()?)
            }
        }
    };
    if end.numeric() < start.numeric() {
        return None;
    }
    Some(Parsed {
        osis: VerseRange { start, end }.osis(),
        spoken: false,
        chapter_only: false,
    })
}

fn word(toks: &[Tok], i: usize) -> Option<&str> {
    toks.get(i).map(|t| t.word.as_str())
}

/// A leading ordinal (`1`, `first`, `2nd`) for numbered books. Roman
/// numerals are left out on purpose: captions never render them, and
/// `i` is the commonest word in a sermon.
fn ordinal(w: &str) -> Option<u8> {
    Some(match w {
        "1" | "1st" | "first" => 1,
        "2" | "2nd" | "second" => 2,
        "3" | "3rd" | "third" => 3,
        _ => return None,
    })
}

/// Match a book name at `i`: `(book, spoken_ordinal, next index)`.
/// Tries the longest name first (`song of solomon`).
fn match_book(toks: &[Tok], i: usize) -> Option<(Book, bool, usize)> {
    let (ord, spoken_ord, start) = match word(toks, i).and_then(ordinal) {
        Some(n) => (
            Some(n),
            !word(toks, i).is_some_and(|w| w.chars().all(|c| c.is_ascii_digit())),
            i + 1,
        ),
        None => (None, false, i),
    };
    for len in (1..=3).rev() {
        let end = start + len;
        if end > toks.len() {
            continue;
        }
        let words = &toks[start..end];
        if !words
            .iter()
            .all(|t| t.word.chars().all(|c| c.is_ascii_alphabetic()))
        {
            continue;
        }
        let name: Vec<&str> = words.iter().map(|t| t.word.as_str()).collect();
        let name = name.join(" ");
        let candidate = match ord {
            Some(n) => format!("{n} {name}"),
            None => name.clone(),
        };
        // Common words that are also book aliases only count with an
        // ordinal or as the full name (`song` alone is a song).
        if len == 1 && ord.is_none() && matches!(name.as_str(), "song" | "ex" | "num" | "dt") {
            continue;
        }
        if let Some(book) = lookup(&candidate) {
            return Some((book, spoken_ord, end));
        }
    }
    None
}

/// `Book::lookup` plus the spellings people say but nobody writes.
fn lookup(name: &str) -> Option<Book> {
    match name {
        "revelations" => Book::lookup("Revelation"),
        "songs of solomon" | "song of songs" => Book::lookup("Song of Solomon"),
        "psalm" | "psalms" | "the psalms" => Book::lookup("Psalms"),
        _ => Book::lookup(name),
    }
}

/// Whether the matched book text is the full name (or long enough not
/// to be an abbreviation) — the bar for a *spoken* reference.
fn full_name(toks: &[Tok], i: usize, after_book: usize) -> bool {
    let start = if word(toks, i).and_then(ordinal).is_some() {
        i + 1
    } else {
        i
    };
    let text: Vec<&str> = toks[start..after_book]
        .iter()
        .map(|t| t.word.as_str())
        .collect();
    let text = text.join(" ");
    text.len() >= 4 || matches!(text.as_str(), "job")
}

/// `verse` slot guard: `1 Peter 5 1 John 4` — the `1` before `John`
/// is an ordinal, not verse 1.
fn looks_like_ordinal_book(toks: &[Tok], k: usize) -> bool {
    word(toks, k).and_then(ordinal).is_some() && match_book(toks, k).is_some()
}

/// Read a number at `i` — digits (`5`, or the head of `5:7`) or number
/// words (`twenty three`, `a hundred nineteen`). Returns `(value,
/// tokens consumed, spoken)`.
fn number(toks: &[Tok], i: usize) -> Option<(u32, usize, bool)> {
    let w = word(toks, i)?;
    if w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        let head = w.split([':', '-', '.']).next()?;
        if !head.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        return head.parse().ok().map(|n| (n, 1, false));
    }
    // Number words: [N] hundred [and] [tens] [unit]
    let mut j = i;
    let mut total: u32 = 0;
    let mut any = false;
    if word(toks, j) == Some("a") && word(toks, j + 1) == Some("hundred") {
        j += 1;
    }
    if let Some(n) = unit_or_teen(word(toks, j)?) {
        if word(toks, j + 1) == Some("hundred") {
            total = n * 100;
            j += 2;
            any = true;
            if word(toks, j) == Some("and") {
                j += 1;
            }
        }
    } else if word(toks, j) == Some("hundred") {
        total = 100;
        j += 1;
        any = true;
        if word(toks, j) == Some("and") {
            j += 1;
        }
    }
    if let Some(t) = word(toks, j).and_then(tens) {
        total += t;
        j += 1;
        any = true;
        if let Some(u) = word(toks, j).and_then(unit) {
            total += u;
            j += 1;
        }
    } else if let Some(n) = word(toks, j).and_then(unit_or_teen) {
        total += n;
        j += 1;
        any = true;
    }
    (any && j > i).then_some((total, j - i, true))
}

fn unit(w: &str) -> Option<u32> {
    Some(match w {
        "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        _ => return None,
    })
}

fn unit_or_teen(w: &str) -> Option<u32> {
    unit(w).or_else(|| {
        Some(match w {
            "ten" => 10,
            "eleven" => 11,
            "twelve" => 12,
            "thirteen" => 13,
            "fourteen" => 14,
            "fifteen" => 15,
            "sixteen" => 16,
            "seventeen" => 17,
            "eighteen" => 18,
            "nineteen" => 19,
            _ => return None,
        })
    })
}

fn tens(w: &str) -> Option<u32> {
    Some(match w {
        "twenty" => 20,
        "thirty" => 30,
        "forty" => 40,
        "fifty" => 50,
        "sixty" => 60,
        "seventy" => 70,
        "eighty" => 80,
        "ninety" => 90,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn osis(text: &str) -> Vec<String> {
        extract_text(text).into_iter().map(|h| h.osis).collect()
    }

    fn one(text: &str) -> RefHit {
        let hits = extract_text(text);
        assert_eq!(hits.len(), 1, "{text}: {hits:?}");
        hits.into_iter().next().unwrap()
    }

    #[test]
    fn written_verse_forms() {
        assert_eq!(osis("turn to 1 Peter 5:7 with me"), ["1Pet.5.7"]);
        assert_eq!(osis("John 3:16-18 says"), ["John.3.16-John.3.18"]);
        assert_eq!(osis("In Romans 8, Paul writes"), ["Rom.8"]);
        assert_eq!(osis("read John 3:20-4:2"), ["John.3.20-John.4.2"]);
        assert_eq!(osis("Ps 23:1."), ["Ps.23.1"]);
        assert_eq!(osis("(Rom. 5:8)"), ["Rom.5.8"]);
        let h = one("1 Peter 5:7");
        assert!(!h.spoken && !h.chapter_only);
    }

    #[test]
    fn spoken_verse_forms() {
        let h = one("look at first Peter chapter five verse seven okay");
        assert_eq!(h.osis, "1Pet.5.7");
        assert!(h.spoken);
        assert_eq!(osis("John chapter 3"), ["John.3"]);
        assert!(one("John chapter 3").chapter_only);
        assert_eq!(
            osis("first Peter chapter five verses one through four"),
            ["1Pet.5.1-1Pet.5.4"]
        );
        assert_eq!(osis("Genesis one one in the beginning"), ["Gen.1.1"]);
        assert_eq!(osis("Psalm 23 is the shepherd psalm"), ["Ps.23"]);
        assert_eq!(osis("in Psalm twenty-three verse four"), ["Ps.23.4"]);
        assert_eq!(
            osis("Psalm a hundred and nineteen verse eleven"),
            ["Ps.119.11"]
        );
        assert_eq!(osis("second Corinthians 5 17"), ["2Cor.5.17"]);
        assert_eq!(osis("Second Corinthians chapter 12 verse 9"), ["2Cor.12.9"]);
        assert_eq!(osis("the book of Revelations chapter 21"), ["Rev.21"]);
        assert_eq!(osis("song of solomon chapter two"), ["Song.2"]);
        assert_eq!(osis("1st John 4 verse 8"), ["1John.4.8"]);
        assert_eq!(osis("verse 7 of 1 Peter 5"), ["1Pet.5"]);
    }

    #[test]
    fn spoken_ranges_with_to_and_through() {
        assert_eq!(osis("John 21 verses 15 to 17"), ["John.21.15-John.21.17"]);
        assert_eq!(osis("Luke 22 54 thru 62"), ["Luke.22.54-Luke.22.62"]);
        // `to` followed by a non-number is not a range.
        assert_eq!(osis("John 3 16 to us all"), ["John.3.16"]);
    }

    #[test]
    fn false_positives_stay_rare() {
        assert!(osis("the next chapter of my life").is_empty());
        assert!(osis("mark my words, this is chapter one of the story").is_empty());
        assert!(osis("Peter said to him").is_empty());
        assert!(osis("I had three jobs and two kids").is_empty());
        assert!(osis("John said he was verse-averse").is_empty());
        // Short aliases need a written number: `ex 5` is not Exodus.
        assert!(osis("for ex five reasons").is_empty());
        assert!(osis("that song verse two").is_empty());
        // A chapter past the book's end is not a reference.
        assert!(osis("John 99").is_empty());
    }

    #[test]
    fn bare_chapter_and_verse_resolve_against_context() {
        // The book the preacher is in, then chapter (+ the verse that
        // follows it), then a later bare verse.
        assert_eq!(
            osis("open up to 1 Peter, and we're in chapter 5. Verse one. He says this. Verse two."),
            ["1Pet.5.1", "1Pet.5.2"]
        );
        assert_eq!(
            osis("open up to 1 Peter, and we're in chapter 5. It starts like this."),
            ["1Pet.5"]
        );
        // A chapter cited outright, then a bare verse.
        assert_eq!(
            osis("James chapter 4 verse 6 says it, and in verse 10 he adds"),
            ["Jas.4.6", "Jas.4.10"]
        );
        // Bare verse range with a chapter in context.
        assert_eq!(
            osis("Romans 8. Look at verses 1 through 4."),
            ["Rom.8", "Rom.8.1-Rom.8.4"]
        );
        // No context → a bare verse / chapter is nothing.
        assert!(osis("verse 7 is my favourite").is_empty());
        assert!(osis("in chapter 3 of the story").is_empty());
        // People are not books: `James and John` sets no context.
        assert!(osis("he had James and John with him, and in chapter 5 he").is_empty());
        // A context chapter past the book's end is rejected, and it
        // clears the verse context — `verse 33` is not 1 Peter 5:33.
        assert_eq!(
            osis("1 Peter 5. Peter declared this in chapter 26 verse 33"),
            ["1Pet.5"]
        );
    }

    #[test]
    fn ordinal_after_verse_is_a_new_book() {
        assert_eq!(osis("1 Peter 5 1 John 4:8"), ["1Pet.5", "1John.4.8"]);
    }

    #[test]
    fn hits_carry_cue_time_and_excerpt() {
        let segs = vec![
            TranscriptSegment {
                start: 12.4,
                dur: 3.0,
                text: "so open your bibles".into(),
            },
            TranscriptSegment {
                start: 109.8,
                dur: 4.0,
                text: "to first Peter chapter five verse seven".into(),
            },
        ];
        let hits = extract(&segs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].secs, 109);
        assert_eq!(hits[0].osis, "1Pet.5.7");
        assert!(hits[0].excerpt.contains("first Peter"));
    }

    #[test]
    fn distinct_keeps_first_mention_order() {
        let hits = extract_text("John 3:16 then Romans 8 then John 3:16 again");
        assert_eq!(distinct_osis(&hits), ["John.3.16", "Rom.8"]);
    }
}
