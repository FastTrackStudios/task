//! Tiny dependency-free fuzzy matcher for pickers (timer-page task
//! search). Case-insensitive; substring hits rank above scattered
//! subsequence hits, earlier and tighter matches rank higher.

/// Score `needle` against `haystack`.
///
/// Returns `None` when `needle` is empty or its characters don't all
/// appear in `haystack` in order. Higher scores are better matches:
///
/// - a contiguous substring hit scores in the 1000s (earlier start +
///   less unmatched tail → higher; a prefix of the title wins),
/// - otherwise an in-order subsequence hit scores in the 1..~100s,
///   rewarding consecutive runs and penalizing gaps.
#[must_use]
pub fn fuzzy_match(needle: &str, haystack: &str) -> Option<i64> {
    let n: Vec<char> = needle.trim().to_lowercase().chars().collect();
    if n.is_empty() {
        return None;
    }
    let h: Vec<char> = haystack.to_lowercase().chars().collect();
    if n.len() > h.len() {
        return None;
    }

    // Contiguous substring: the strong signal.
    if let Some(pos) = (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()] == n[..]) {
        let pos = i64::try_from(pos).unwrap_or(i64::MAX);
        let slack = i64::try_from(h.len() - n.len()).unwrap_or(i64::MAX);
        return Some(2000 - pos * 4 - slack);
    }

    // In-order subsequence: consecutive runs good, gaps bad.
    let mut score: i64 = 0;
    let mut cursor = 0usize;
    let mut prev: Option<usize> = None;
    for &c in &n {
        let found = (cursor..h.len()).find(|&i| h[i] == c)?;
        score += if prev == Some(found.wrapping_sub(1)) {
            12
        } else {
            4
        };
        score -= i64::try_from(found - cursor).unwrap_or(i64::MAX).min(6);
        prev = Some(found);
        cursor = found + 1;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::fuzzy_match;

    #[test]
    fn empty_needle_never_matches() {
        assert_eq!(fuzzy_match("", "anything"), None);
        assert_eq!(fuzzy_match("   ", "anything"), None);
    }

    #[test]
    fn is_case_insensitive() {
        assert!(fuzzy_match("FIX", "fix the login bug").is_some());
        assert!(fuzzy_match("fix", "Fix The Login Bug").is_some());
    }

    #[test]
    fn rejects_out_of_order_or_missing_chars() {
        assert_eq!(fuzzy_match("xyz", "fix the login bug"), None);
        assert_eq!(fuzzy_match("bug fix", "fix the bug"), None); // order matters
    }

    #[test]
    fn substring_beats_scattered_subsequence() {
        let substring = fuzzy_match("log", "login page").unwrap();
        let scattered = fuzzy_match("log", "large old gate").unwrap();
        assert!(substring > scattered);
    }

    #[test]
    fn earlier_substring_ranks_higher() {
        let prefix = fuzzy_match("fix", "fix the login bug").unwrap();
        let later = fuzzy_match("fix", "hotfix for login").unwrap();
        assert!(prefix > later);
    }

    #[test]
    fn subsequence_still_matches() {
        assert!(fuzzy_match("flb", "fix login bug").is_some());
    }
}
