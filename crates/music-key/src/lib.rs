//! Musical key names as plain data.
//!
//! A key name is what a person types into a note's frontmatter: `D`,
//! `f#m`, `Bb minor`, `E♭`. This crate parses one, transposes it by a
//! number of semitones, and writes it back **in the input's own
//! spelling**: a flat key stays flat, a sharp key stays sharp, and the
//! letter case, accidental glyph (`#`/`♯`, `b`/`♭`) and mode suffix
//! (`m`, ` minor`, `min`, …) are kept as typed.
//!
//! It is the engine behind the `.base` formula `transpose_key(key,
//! offset)`, where `offset` is either semitones (`4`, `"+4"`, `"-4"`)
//! or an absolute key name (`"D"`), which replaces the key outright —
//! the per-use key of a setlist entry wins over the song's default.
//!
//! It deliberately does not use Keyflow's `from_midi`, which only ever
//! spells sharps.
//!
//! # Spelling rule
//!
//! The result's pitch class is `(tonic + offset) mod 12`, the mode is
//! unchanged, and the name is chosen like this:
//!
//! 1. A white-key result is the natural letter (`Bb` + 1 = `B`, never
//!    `Cb`).
//! 2. A black-key result keeps the input's accidental family: sharps
//!    from a sharp key, flats from a flat key (`C#` + 1 = `D#`,
//!    `Db` + 2 = `Eb`).
//! 3. A natural input has no family of its own, so a black-key result
//!    takes the conventional key signature for its mode — the one with
//!    fewer accidentals, and `F#`/`Eb` for the two six-accidental ties
//!    (major `Db Eb F# Ab Bb`, minor `C#m Ebm F#m G#m Bbm`).
//!
//! A [`Key`] value remembers the family it was transposed in, so
//! `k.transpose(n).transpose(-n) == k` holds exactly for every key.
//! A key *name* (a string) cannot carry that memory through a natural
//! result — `C#` + 1 = `D` and `Db` + 1 = `D` are the same string — so
//! the string round trip is exact except when the intermediate name is
//! a natural and the input used the unconventional spelling; it is
//! always enharmonically exact.
//!
//! # What parses
//!
//! A tonic letter `A`–`G` (either case), at most one accidental (`#`,
//! `♯`, `b`, `♭`), then an optional mode: nothing, `M`, `maj` or
//! `major` for major; `m`, `-`, `min` or `minor` for minor (words in
//! any case, optionally after a space). The theoretical spellings
//! `Cb`, `Fb`, `E#` and `B#`, double accidentals, chord qualities
//! (`Cmaj7`) and church modes are rejected.

use std::fmt;

/// Major or minor. The mode never changes under transposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// A major key (`C`, `Bb major`).
    Major,
    /// A minor key (`Am`, `f# minor`).
    Minor,
}

/// The accidental written on a tonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Accidental {
    /// No accidental: one of the seven white-key letters.
    Natural,
    /// `#` or `♯`.
    Sharp,
    /// `b` or `♭`.
    Flat,
}

/// Which family of accidentals a key is spelled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// Black keys are named with sharps.
    Sharps,
    /// Black keys are named with flats.
    Flats,
}

/// Why a key name or offset was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The input was empty or only whitespace.
    Empty,
    /// The input does not start with a tonic letter `A`–`G`.
    BadTonic(String),
    /// More than one accidental (`C##`, `Bbb`).
    DoubleAccidental(String),
    /// A theoretical spelling outside the 17 key names (`Cb`, `Fb`,
    /// `E#`, `B#`).
    UnsupportedSpelling(String),
    /// Text after the tonic that is not a major/minor marker.
    BadMode(String),
    /// An offset that is neither whole semitones nor a key name.
    BadOffset(String),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty key name"),
            Self::BadTonic(s) => write!(f, "{s:?}: a key starts with a letter A-G"),
            Self::DoubleAccidental(s) => write!(f, "{s:?}: at most one accidental"),
            Self::UnsupportedSpelling(s) => {
                write!(f, "{s:?}: Cb, Fb, E# and B# are not supported key names")
            }
            Self::BadMode(s) => write!(f, "{s:?}: the mode must be major or minor (e.g. m, minor)"),
            Self::BadOffset(s) => {
                write!(
                    f,
                    "{s:?}: an offset is whole semitones (+4, -4) or a key name"
                )
            }
        }
    }
}

impl std::error::Error for KeyError {}

/// How the key was written, kept so the output looks like the input.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Style {
    lowercase: bool,
    unicode_accidentals: bool,
    /// The mode suffix exactly as typed, leading whitespace included.
    suffix: String,
}

/// A parsed key: tonic, mode, and how it was written.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    /// Letter index: 0 = C, 1 = D, … 6 = B.
    letter: u8,
    accidental: Accidental,
    mode: Mode,
    /// The family this key was transposed in, remembered through
    /// natural tonics so a typed round trip is exact.
    carried: Option<Family>,
    style: Style,
}

const LETTERS: [char; 7] = ['C', 'D', 'E', 'F', 'G', 'A', 'B'];
const LETTER_PC: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];

impl Key {
    /// Parse a key name. See the crate docs for what is accepted.
    pub fn parse(input: &str) -> Result<Self, KeyError> {
        let s = input.trim();
        if s.is_empty() {
            return Err(KeyError::Empty);
        }
        let mut chars = s.char_indices().peekable();
        let (_, first) = chars.next().ok_or(KeyError::Empty)?;
        let upper = first.to_ascii_uppercase();
        let letter = LETTERS
            .iter()
            .position(|&c| c == upper)
            .ok_or_else(|| KeyError::BadTonic(s.to_string()))? as u8;
        let lowercase = first.is_ascii_lowercase();

        let mut accidental = Accidental::Natural;
        let mut unicode_accidentals = false;
        if let Some(&(_, c)) = chars.peek() {
            if let Some((acc, uni)) = accidental_of(c) {
                accidental = acc;
                unicode_accidentals = uni;
                chars.next();
                // No mode marker starts with an accidental character, so
                // a second one is always a double accidental (`Bbb`).
                if let Some(&(_, c2)) = chars.peek()
                    && accidental_of(c2).is_some()
                {
                    return Err(KeyError::DoubleAccidental(s.to_string()));
                }
            }
        }
        let rest = match chars.peek() {
            Some(&(i, _)) => &s[i..],
            None => "",
        };
        let mode = parse_mode(rest).ok_or_else(|| KeyError::BadMode(s.to_string()))?;

        let unsupported = matches!(
            (LETTERS[letter as usize], accidental),
            ('C', Accidental::Flat)
                | ('F', Accidental::Flat)
                | ('E', Accidental::Sharp)
                | ('B', Accidental::Sharp)
        );
        if unsupported {
            return Err(KeyError::UnsupportedSpelling(s.to_string()));
        }

        Ok(Self {
            letter,
            accidental,
            mode,
            carried: None,
            style: Style {
                lowercase,
                unicode_accidentals,
                suffix: rest.to_string(),
            },
        })
    }

    /// The tonic's pitch class, 0 = C … 11 = B.
    pub fn pitch_class(&self) -> u8 {
        let base = LETTER_PC[self.letter as usize] as i16;
        let shift = match self.accidental {
            Accidental::Natural => 0,
            Accidental::Sharp => 1,
            Accidental::Flat => -1,
        };
        (base + shift).rem_euclid(12) as u8
    }

    /// Major or minor.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The accidental written on the tonic.
    pub fn accidental(&self) -> Accidental {
        self.accidental
    }

    /// The accidental family this key spells black keys in, if it has
    /// one: its own accidental, or the family it was transposed in.
    pub fn family(&self) -> Option<Family> {
        match self.accidental {
            Accidental::Sharp => Some(Family::Sharps),
            Accidental::Flat => Some(Family::Flats),
            Accidental::Natural => self.carried,
        }
    }

    /// Same tonic pitch class and mode, whatever the spelling
    /// (`C#m` and `Dbm`).
    pub fn enharmonic_eq(&self, other: &Self) -> bool {
        self.pitch_class() == other.pitch_class() && self.mode == other.mode
    }

    /// Transpose by `semitones` (taken modulo 12), keeping the mode,
    /// the written style and the spelling rule in the crate docs.
    #[must_use]
    pub fn transpose(&self, semitones: i64) -> Self {
        let pc = ((i64::from(self.pitch_class()) + semitones.rem_euclid(12)) % 12) as u8;
        let family = self.family();
        let (letter, accidental) = spell(pc, family, self.mode);
        Self {
            letter,
            accidental,
            mode: self.mode,
            carried: family,
            style: self.style.clone(),
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let letter = LETTERS[self.letter as usize];
        let letter = if self.style.lowercase {
            letter.to_ascii_lowercase()
        } else {
            letter
        };
        write!(f, "{letter}")?;
        match (self.accidental, self.style.unicode_accidentals) {
            (Accidental::Natural, _) => {}
            (Accidental::Sharp, false) => write!(f, "#")?,
            (Accidental::Sharp, true) => write!(f, "♯")?,
            (Accidental::Flat, false) => write!(f, "b")?,
            (Accidental::Flat, true) => write!(f, "♭")?,
        }
        write!(f, "{}", self.style.suffix)
    }
}

impl std::str::FromStr for Key {
    type Err = KeyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

fn accidental_of(c: char) -> Option<(Accidental, bool)> {
    match c {
        '#' => Some((Accidental::Sharp, false)),
        '♯' => Some((Accidental::Sharp, true)),
        'b' => Some((Accidental::Flat, false)),
        '♭' => Some((Accidental::Flat, true)),
        _ => None,
    }
}

fn parse_mode(rest: &str) -> Option<Mode> {
    let t = rest.trim();
    match t {
        "" | "M" => return Some(Mode::Major),
        "m" | "-" => return Some(Mode::Minor),
        _ => {}
    }
    match t.to_ascii_lowercase().as_str() {
        "maj" | "major" => Some(Mode::Major),
        "min" | "minor" => Some(Mode::Minor),
        _ => None,
    }
}

/// Name pitch class `pc` as (letter index, accidental).
fn spell(pc: u8, family: Option<Family>, mode: Mode) -> (u8, Accidental) {
    if let Some(letter) = LETTER_PC.iter().position(|&p| p == pc) {
        return (letter as u8, Accidental::Natural);
    }
    let family = family.unwrap_or_else(|| conventional_family(pc, mode));
    match family {
        // The letter below, sharpened.
        Family::Sharps => (letter_of(pc - 1), Accidental::Sharp),
        // The letter above, flattened. No black key is 11, so pc + 1
        // stays in range.
        Family::Flats => (letter_of(pc + 1), Accidental::Flat),
    }
}

fn letter_of(pc: u8) -> u8 {
    LETTER_PC
        .iter()
        .position(|&p| p == pc)
        .expect("neighbour of a black key is a white key") as u8
}

/// The key signature a natural-tonic input lands in on a black key.
fn conventional_family(pc: u8, mode: Mode) -> Family {
    match (mode, pc) {
        // Db Eb Ab Bb major; F# (6#) over Gb (6b).
        (Mode::Major, 6) => Family::Sharps,
        (Mode::Major, _) => Family::Flats,
        // C#m F#m G#m; Ebm (6b) over D#m (6#); Bbm over A#m.
        (Mode::Minor, 1 | 6 | 8) => Family::Sharps,
        (Mode::Minor, _) => Family::Flats,
    }
}

/// The second argument of `transpose_key`: a shift or a replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offset {
    /// Shift by whole semitones (taken modulo 12).
    Semitones(i64),
    /// Replace with this key, written exactly as given (trimmed).
    Absolute(String),
}

impl Offset {
    /// Parse `+4`, `-4`, `4`, `0`, or a key name like `D` / `f#m`.
    pub fn parse(input: &str) -> Result<Self, KeyError> {
        let s = input.trim();
        if s.is_empty() {
            return Err(KeyError::BadOffset(input.to_string()));
        }
        let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
        // A sign or a leading digit means semitones; anything else
        // must be a key name.
        if digits.len() != s.len() || digits.starts_with(|c: char| c.is_ascii_digit()) {
            if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                return Err(KeyError::BadOffset(s.to_string()));
            }
            let n: i64 = digits
                .parse()
                .map_err(|_| KeyError::BadOffset(s.to_string()))?;
            return Ok(Self::Semitones(if s.starts_with('-') { -n } else { n }));
        }
        match Key::parse(s) {
            Ok(_) => Ok(Self::Absolute(s.to_string())),
            Err(_) => Err(KeyError::BadOffset(s.to_string())),
        }
    }
}

/// Transpose the key name `key` by `offset` (semitones like `"+4"`,
/// or an absolute key name, which is returned unchanged). The result
/// is written in `key`'s own spelling and style.
///
/// An absolute offset replaces the key, so `key` is not consulted —
/// a setlist entry in `D` is in D even when the song's key is unset.
pub fn transpose_key(key: &str, offset: &str) -> Result<String, KeyError> {
    match Offset::parse(offset)? {
        Offset::Absolute(k) => Ok(k),
        Offset::Semitones(n) => transpose_key_by(key, n),
    }
}

/// Transpose the key name `key` by `semitones` (modulo 12).
pub fn transpose_key_by(key: &str, semitones: i64) -> Result<String, KeyError> {
    Ok(Key::parse(key)?.transpose(semitones).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // An oracle independent of the letter arithmetic above: plain
    // name tables indexed by pitch class.
    const SHARP: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    const FLAT: [&str; 12] = [
        "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
    ];
    // What a natural tonic lands on, per mode.
    const CONV_MAJOR: [&str; 12] = [
        "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
    ];
    const CONV_MINOR: [&str; 12] = [
        "C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B",
    ];

    /// Every supported tonic spelling: (name, pitch class, table it
    /// transposes through). 7 naturals + 5 sharps + 5 flats = all 12
    /// tonics in both spellings.
    fn tonics() -> Vec<(&'static str, usize, Option<&'static [&'static str; 12]>)> {
        let mut v = Vec::new();
        for pc in 0..12 {
            if SHARP[pc] == FLAT[pc] {
                v.push((SHARP[pc], pc, None));
            } else {
                v.push((SHARP[pc], pc, Some(&SHARP)));
                v.push((FLAT[pc], pc, Some(&FLAT)));
            }
        }
        v
    }

    fn expected(pc: usize, table: Option<&[&str; 12]>, minor: bool, n: i64) -> String {
        let to = (pc as i64 + n).rem_euclid(12) as usize;
        let name = match table {
            Some(t) => t[to],
            None if minor => CONV_MINOR[to],
            None => CONV_MAJOR[to],
        };
        if minor {
            format!("{name}m")
        } else {
            name.to_string()
        }
    }

    #[test]
    fn full_table_12_tonics_both_spellings_both_modes_offsets_pm11() {
        let tonics = tonics();
        assert_eq!(tonics.len(), 17);
        let mut checked = 0;
        for (name, pc, table) in &tonics {
            for minor in [false, true] {
                let key = if minor {
                    format!("{name}m")
                } else {
                    (*name).to_string()
                };
                for n in -11..=11 {
                    let want = expected(*pc, *table, minor, n);
                    let got = transpose_key_by(&key, n).unwrap();
                    assert_eq!(got, want, "{key} {n:+}");
                    // The string offset form agrees with the numeric one.
                    assert_eq!(transpose_key(&key, &format!("{n:+}")).unwrap(), want);
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 17 * 2 * 23);
    }

    #[test]
    fn named_examples() {
        let cases = [
            ("f#m", "+2", "g#m"),
            ("D", "+4", "F#"),
            ("D", "-4", "Bb"),
            ("Bb", "+2", "C"),
            ("Bb", "+1", "B"),
            ("Eb", "-1", "D"),
            ("Eb", "+3", "Gb"),
            ("Db", "+2", "Eb"),
            ("C#", "+2", "D#"),
            ("C", "+1", "Db"),
            ("C", "+6", "F#"),
            ("Am", "+1", "Bbm"),
            ("Am", "-1", "G#m"),
            ("Em", "-1", "Ebm"),
            ("Bbm", "-1", "Am"),
            ("Bb minor", "+2", "C minor"),
            ("bb", "+1", "b"),
            ("E♭", "+3", "G♭"),
            ("C♯m", "+1", "Dm"),
            ("G major", "+12", "G major"),
            ("G", "-13", "F#"),
            ("A", "0", "A"),
            ("F", "25", "F#"),
        ];
        for (key, off, want) in cases {
            assert_eq!(transpose_key(key, off).unwrap(), want, "{key} {off}");
        }
        // Extreme offsets reduce modulo 12 without overflowing:
        // i64::MAX = 7 (mod 12), i64::MIN = 4 (mod 12).
        assert_eq!(transpose_key_by("C", i64::MAX).unwrap(), "G");
        assert_eq!(transpose_key_by("C", i64::MIN).unwrap(), "E");
    }

    #[test]
    fn mode_is_kept() {
        for n in -11..=11 {
            assert_eq!(Key::parse("f#m").unwrap().transpose(n).mode(), Mode::Minor);
            assert_eq!(Key::parse("F#").unwrap().transpose(n).mode(), Mode::Major);
        }
    }

    #[test]
    fn absolute_key_passes_through_unchanged() {
        assert_eq!(transpose_key("C", "D").unwrap(), "D");
        assert_eq!(transpose_key("f#m", "Ebm").unwrap(), "Ebm");
        assert_eq!(transpose_key("C", "  bb minor ").unwrap(), "bb minor");
        // The per-use key wins even when the song key is unset or bad.
        assert_eq!(transpose_key("", "D").unwrap(), "D");
        assert_eq!(transpose_key("nonsense", "D").unwrap(), "D");
        assert_eq!(Offset::parse("D").unwrap(), Offset::Absolute("D".into()));
    }

    #[test]
    fn offsets_parse() {
        assert_eq!(Offset::parse("+4").unwrap(), Offset::Semitones(4));
        assert_eq!(Offset::parse("-4").unwrap(), Offset::Semitones(-4));
        assert_eq!(Offset::parse(" 4 ").unwrap(), Offset::Semitones(4));
        assert_eq!(Offset::parse("0").unwrap(), Offset::Semitones(0));
        assert_eq!(Offset::parse("+0").unwrap(), Offset::Semitones(0));
    }

    #[test]
    fn round_trip_typed_is_exact_for_every_key() {
        for (name, _, _) in tonics() {
            for suffix in ["", "m"] {
                let k = Key::parse(&format!("{name}{suffix}")).unwrap();
                for n in -11..=11 {
                    let back = k.transpose(n).transpose(-n);
                    assert_eq!(back.to_string(), k.to_string(), "{k} {n:+}");
                    assert!(back.enharmonic_eq(&k));
                }
            }
        }
    }

    #[test]
    fn round_trip_on_names() {
        // transpose(transpose(k, n), -n) == k as a string whenever the
        // spelling survives the intermediate name, and enharmonically
        // always.
        let mut exact = 0;
        for (name, pc, table) in tonics() {
            for suffix in ["", "m"] {
                let k = format!("{name}{suffix}");
                let minor = suffix == "m";
                let conv = if minor { CONV_MINOR } else { CONV_MAJOR };
                let conventional = conv[pc] == name;
                for n in -11..=11 {
                    let mid = transpose_key_by(&k, n).unwrap();
                    let back = transpose_key_by(&mid, -n).unwrap();
                    let mid_natural = Key::parse(&mid).unwrap().accidental() == Accidental::Natural;
                    assert!(
                        Key::parse(&back)
                            .unwrap()
                            .enharmonic_eq(&Key::parse(&k).unwrap())
                    );
                    if table.is_none() || conventional || !mid_natural {
                        assert_eq!(back, k, "{k} {n:+} via {mid}");
                        exact += 1;
                    } else {
                        // The name lost the spelling; it comes back in
                        // the conventional one.
                        assert_eq!(back, format!("{}{suffix}", conv[pc]), "{k} {n:+}");
                    }
                }
            }
        }
        // 17 tonics x 2 modes x 23 offsets = 782. The 10 unconventional
        // (spelling, mode) pairs (C# D# Gb G# A#, Dbm D#m Gbm Abm A#m)
        // pass through a natural name for 14 of their 23 offsets.
        assert_eq!(exact, 782 - 10 * 14);
    }

    #[test]
    fn bad_input_is_rejected() {
        for bad in [
            "", "   ", "H", "X#", "#C", "1", "C##", "Bbb", "C#b", "Cb", "Fb", "E#", "B#", "Cmaj7",
            "C dorian", "Cm m", "Am7", "C7", "Do", "minor",
        ] {
            assert!(Key::parse(bad).is_err(), "{bad:?} parsed");
            assert!(transpose_key_by(bad, 2).is_err(), "{bad:?} transposed");
        }
        for bad in [
            "", "4.5", "+", "-", "++4", "+-4", "four", "4m", "H", "1e3", "C##",
        ] {
            assert!(Offset::parse(bad).is_err(), "offset {bad:?} parsed");
            assert!(transpose_key("C", bad).is_err(), "offset {bad:?} applied");
        }
        assert!(matches!(
            Key::parse("Cb"),
            Err(KeyError::UnsupportedSpelling(_))
        ));
        assert!(matches!(
            Key::parse("C##"),
            Err(KeyError::DoubleAccidental(_))
        ));
        assert!(matches!(Key::parse("Cmaj7"), Err(KeyError::BadMode(_))));
        assert!(matches!(Key::parse("H"), Err(KeyError::BadTonic(_))));
        assert!(matches!(Key::parse(""), Err(KeyError::Empty)));
    }

    #[test]
    fn mode_spellings() {
        for (s, m) in [
            ("C", Mode::Major),
            ("CM", Mode::Major),
            ("C maj", Mode::Major),
            ("C Major", Mode::Major),
            ("Cm", Mode::Minor),
            ("C-", Mode::Minor),
            ("C min", Mode::Minor),
            ("c minor", Mode::Minor),
            ("C MINOR", Mode::Minor),
        ] {
            assert_eq!(Key::parse(s).unwrap().mode(), m, "{s}");
        }
    }

    #[test]
    fn pitch_classes() {
        for (name, pc, _) in tonics() {
            assert_eq!(
                Key::parse(name).unwrap().pitch_class() as usize,
                pc,
                "{name}"
            );
        }
    }
}
