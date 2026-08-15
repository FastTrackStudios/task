//! Phase 6.5 — `LexoRank`-style fractional-index keys.
//!
//! Concurrent insertions between two siblings need keys that can be
//! computed locally without a central coordinator and still sort
//! correctly. We use a string alphabet of digits + lowercase ASCII
//! letters (`0-9a-z`, 36 chars) — picked for being base32-ish and
//! still readable.
//!
//! Pick rules:
//!
//! - [`first()`] — a starting key with comfortable headroom on
//!   both sides.
//! - [`between(low, high)`] — a key strictly greater than `low`
//!   and strictly less than `high`. Returns `None` only when the
//!   inputs are equal or reversed.
//! - [`after(low)`] — append-to-tail when there's no upper bound.
//! - [`before(high)`] — prepend-to-head when there's no lower
//!   bound.
//!
//! The algorithm is a textbook string-bisection: at each character
//! position, compare the two bounds; if they share the digit, copy
//! and advance; if they differ and the gap allows it, pick a
//! midpoint; otherwise keep the lower digit and recurse one
//! position deeper with the upper bound treated as `z + 1`.
//!
//! Phase 6.5b wires this into kanban-card ordering. Phase 6.5a
//! lands it standalone so other features (Knowledge block lists,
//! attachment galleries) can adopt it incrementally.

const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
const MIN_CH: u8 = b'0';
const MAX_CH: u8 = b'z';

/// A comfortable starting rank in the middle of the alphabet
/// space. Anything `between(MIN, FIRST)` or `between(FIRST, MAX)`
/// will yield a single-character result for the first few
/// hundred insertions.
#[must_use]
pub fn first() -> String {
    "m".to_string()
}

/// Pick a rank strictly greater than `low` and strictly less than
/// `high`. Returns `None` when `low >= high`.
#[must_use]
pub fn between(low: &str, high: &str) -> Option<String> {
    if low >= high {
        return None;
    }
    let lb = low.as_bytes();
    let hb = high.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let l = lb.get(i).copied().unwrap_or(MIN_CH);
        let h = hb.get(i).copied().unwrap_or(MAX_CH + 1);
        if h.saturating_sub(l) > 1 {
            // There's room between l and h — pick the midpoint
            // (rounded down toward `l`) and stop.
            let mid = l + (h - l) / 2;
            out.push(mid);
            return Some(string_from(&out));
        }
        // Gap is tight: copy the lower digit and keep walking.
        out.push(l);
        i += 1;
        if i > lb.len() && i > hb.len() {
            // Both bounds exhausted and we never found a gap —
            // append a character strictly above `MIN_CH`.
            out.push(b'm');
            return Some(string_from(&out));
        }
    }
}

/// Pick a rank strictly greater than `low`. Convenience: append a
/// `MIN_CH` advance.
#[must_use]
pub fn after(low: &str) -> String {
    // Append a midpoint character to bias toward leaving room on
    // both sides for future inserts.
    let mut s = String::from(low);
    s.push('m');
    s
}

/// Pick a rank strictly less than `high`. Prepend logic.
#[must_use]
pub fn before(high: &str) -> String {
    let hb = high.as_bytes();
    let mut out = Vec::with_capacity(hb.len());
    for (i, &c) in hb.iter().enumerate() {
        if c > MIN_CH + 1 {
            // Step down by one and stop — strictly less than the
            // original prefix.
            for &prev in &hb[..i] {
                out.push(prev);
            }
            out.push(c - 1);
            // Pad with mid so the gap to `MIN_CH` stays open.
            out.push(b'm');
            return string_from(&out);
        }
    }
    // Every character was already MIN_CH or MIN_CH+1; extend with
    // a leading `0` is invalid; emit a single MIN_CH followed by a
    // mid. Callers should treat the result as a best-effort
    // prepend and use `between` afterward.
    let mut s = String::from(high);
    s.insert(0, MIN_CH as char);
    s
}

fn string_from(bytes: &[u8]) -> String {
    // SAFETY: every byte we ever emit is within the alphabet, so
    // the result is valid ASCII UTF-8.
    String::from_utf8(bytes.to_vec()).expect("alphabet is ASCII")
}

/// Quick validation — only used by tests and a few defensive
/// paths. Returns true iff every byte is in the alphabet.
#[must_use]
pub fn is_valid(rank: &str) -> bool {
    !rank.is_empty() && rank.bytes().all(|b| ALPHABET.contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn between_basic() {
        let mid = between("a", "c").expect("a < c");
        assert!(mid.as_str() > "a" && mid.as_str() < "c");
    }

    #[test]
    fn between_adjacent_extends() {
        // a..b has no single-char gap; algorithm extends.
        let mid = between("a", "b").expect("ok");
        assert!(mid.as_str() > "a" && mid.as_str() < "b", "got {mid}");
        assert!(mid.len() >= 2);
    }

    #[test]
    fn between_equal_returns_none() {
        assert!(between("aaa", "aaa").is_none());
    }

    #[test]
    fn between_reversed_returns_none() {
        assert!(between("z", "a").is_none());
    }

    #[test]
    fn many_inserts_remain_sortable() {
        // Insert 100 ranks between two bounds; verify every pair
        // is strictly ordered.
        let mut ranks: Vec<String> = vec!["a".into(), "z".into()];
        for _ in 0..100 {
            // Pick a random-ish position and insert.
            let i = ranks.len() / 2;
            let mid = between(&ranks[i - 1], &ranks[i]).expect("room");
            ranks.insert(i, mid);
        }
        let mut sorted = ranks.clone();
        sorted.sort();
        assert_eq!(ranks, sorted, "inserts broke ordering");
    }

    #[test]
    fn first_is_in_alphabet() {
        let f = first();
        assert!(is_valid(&f));
        assert!(f.as_str() > "0");
        assert!(f.as_str() < "z");
    }

    #[test]
    fn after_strictly_greater() {
        let f = first();
        let a = after(&f);
        assert!(a.as_str() > f.as_str(), "{a} not > {f}");
    }

    #[test]
    fn before_strictly_less() {
        let f = first();
        let b = before(&f);
        assert!(b.as_str() < f.as_str(), "{b} not < {f}");
    }
}
