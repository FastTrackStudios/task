//! Human version markers — `files.version.labels`.
//!
//! People put versions in filenames. The real tree is full of it:
//! `01 ALL THAT I AM 1.6 SommaPrep.ptx`, `2.1 Somma`, `2.2 Tracking
//! Prep`, `Copy of Copy of ONE805 LIVE TRACKS.ptx`, `… V3.mp4`.
//!
//! These are **read, never parsed into a lineage.** Ordering, ancestry
//! and currency are never inferred from a name. A convention we did not
//! define is one we can only read: the first file that breaks the pattern
//! would otherwise be silently misfiled, and in a tree this size there is
//! always a file that breaks the pattern.
//!
//! So this module extracts *labels to show beside a filename*, and
//! deliberately offers no comparison, no ordering, and no "latest".

/// A marker found in a filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Label {
    /// A dotted or plain number: `2.1`, `v3`, `V3`.
    Version(String),
    /// A word people use to mean finished: `FINAL`, `MASTER`, `APPROVED`.
    Status(String),
    /// Evidence of duplication: `Copy of`, `copy 2`.
    Duplicate,
    /// A date-like run of digits: `241011`, `2026-05-08`, `5.8.26`.
    Date(String),
}

const STATUS_WORDS: &[&str] = &[
    "final", "master", "mastered", "approved", "release", "print", "locked", "archive", "backup",
    "draft", "rough", "wip", "prep", "temp",
];

fn is_version_token(tok: &str) -> Option<String> {
    let t = tok.trim_matches(|c: char| !c.is_alphanumeric() && c != '.');
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    // v3 / V3 / v1.6
    if let Some(rest) = lower.strip_prefix('v') {
        if !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.')
            && rest.chars().any(|c| c.is_ascii_digit())
        {
            return Some(t.to_string());
        }
    }
    // 2.1 — a dotted number, at least one dot, digits only
    if t.contains('.')
        && t.chars().all(|c| c.is_ascii_digit() || c == '.')
        && t.chars().filter(|c| *c == '.').count() <= 2
        && t.chars().any(|c| c.is_ascii_digit())
    {
        // Three dot-separated parts that all look like a date are a date.
        let parts: Vec<&str> = t.split('.').filter(|p| !p.is_empty()).collect();
        if parts.len() == 3 && parts.iter().all(|p| p.len() <= 4) {
            return None;
        }
        return Some(t.to_string());
    }
    None
}

fn is_date_token(tok: &str) -> Option<String> {
    let t = tok.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '-');
    if t.is_empty() {
        return None;
    }
    // 2026-05-08
    if t.len() == 10 && t.split('-').count() == 3 && t.chars().all(|c| c.is_ascii_digit() || c == '-')
    {
        return Some(t.to_string());
    }
    // 5.8.26
    let parts: Vec<&str> = t.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() == 3
        && parts.iter().all(|p| p.len() <= 4 && p.chars().all(|c| c.is_ascii_digit()))
    {
        return Some(t.to_string());
    }
    // A bare 6- or 8-digit run: 241011
    if (t.len() == 6 || t.len() == 8) && t.chars().all(|c| c.is_ascii_digit()) {
        return Some(t.to_string());
    }
    None
}

/// Read the markers in a filename.
///
/// Returns what a UI may display beside the name. It intentionally
/// yields no ordering: two files' labels cannot be compared, because
/// doing so would be inferring a lineage.
#[must_use]
// t[impl files.version.labels]
pub fn read(filename: &str) -> Vec<Label> {
    let stem = filename.rsplit_once('.').map_or(filename, |(s, _)| s);
    let lower = stem.to_ascii_lowercase();

    let mut out = Vec::new();

    if lower.contains("copy of") || lower.contains(" copy") {
        out.push(Label::Duplicate);
    }

    for tok in stem.split([' ', '_', '-']) {
        if tok.is_empty() {
            continue;
        }
        if let Some(d) = is_date_token(tok) {
            let label = Label::Date(d);
            if !out.contains(&label) {
                out.push(label);
            }
            continue;
        }
        if let Some(v) = is_version_token(tok) {
            let label = Label::Version(v);
            if !out.contains(&label) {
                out.push(label);
            }
            continue;
        }
        let word = tok.trim_matches(|c: char| !c.is_alphanumeric()).to_ascii_lowercase();
        if STATUS_WORDS.contains(&word.as_str()) {
            let label = Label::Status(word);
            if !out.contains(&label) {
                out.push(label);
            }
        }
    }

    out
}

/// Whether a name carries any marker at all — the cheap check for
/// deciding whether to render a label row.
#[must_use]
pub fn has_labels(filename: &str) -> bool {
    !read(filename).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_dotted_version() {
        let l = read("01 ALL THAT I AM 2.1 Somma.ptx");
        assert!(l.contains(&Label::Version("2.1".into())));
    }

    #[test]
    fn reads_prep_as_status() {
        let l = read("01 ALL THAT I AM 2.2 Tracking Prep.ptx");
        assert!(l.contains(&Label::Version("2.2".into())));
        assert!(l.contains(&Label::Status("prep".into())));
    }

    #[test]
    fn reads_v_prefixed() {
        assert!(read("A Journey of Immigrants V3.mp4").contains(&Label::Version("V3".into())));
    }

    #[test]
    fn notices_duplication() {
        assert!(read("Copy of Copy of ONE805 LIVE TRACKS.ptx").contains(&Label::Duplicate));
    }

    #[test]
    // t[verify files.version.labels]
    fn a_date_is_a_date_not_a_version() {
        let l = read("Ancestro - 5.8.26 Mix.wav");
        assert!(l.contains(&Label::Date("5.8.26".into())));
        assert!(
            !l.iter().any(|x| matches!(x, Label::Version(_))),
            "5.8.26 is a date; calling it version 5.8 would be inventing a lineage"
        );
    }

    #[test]
    fn reads_a_recorder_timestamp() {
        assert!(read("148-V LEAD-241011_2006.wav").contains(&Label::Date("241011".into())));
    }

    #[test]
    fn a_plain_name_carries_nothing() {
        assert!(read("Mix.wav").is_empty());
        assert!(!has_labels("01 All That I Am.RPP"));
    }

    #[test]
    // t[verify files.version.labels]
    fn labels_offer_no_ordering() {
        // The point of the module: these are readable and not comparable.
        // If this ever compiles with `<`, someone has added a lineage.
        let a = read("Song 2.1.ptx");
        let b = read("Song 2.2.ptx");
        assert_ne!(a, b);
    }
}
