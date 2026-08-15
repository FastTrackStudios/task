//! The app-wide avatar identity: deterministic per-person gradient +
//! initials. ONE hash and ONE palette — the same person must look the
//! same in the org chrome, presence rows, and the review rail (which
//! lives in files-ui and can't depend on the `ui` crate, hence this
//! home).

/// Tasteful gradient palette — `(from, to)` CSS colors. Six entries;
/// [`gradient_index`] picks one deterministically per key.
pub const AVATAR_GRADIENTS: [(&str, &str); 6] = [
    ("#f59e0b", "#ef4444"), // amber → red
    ("#8b5cf6", "#6366f1"), // violet → indigo
    ("#06b6d4", "#3b82f6"), // cyan → blue
    ("#10b981", "#14b8a6"), // emerald → teal
    ("#ec4899", "#f43f5e"), // pink → rose
    ("#84cc16", "#22c55e"), // lime → green
];

/// FNV-1a over the key, mod the palette size. Deterministic across
/// targets and sessions — the same account always gets the same
/// gradient, with no external requests or asset files.
#[must_use]
pub fn gradient_index(key: &str) -> usize {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash % AVATAR_GRADIENTS.len() as u64) as usize
}

/// The key's gradient as a CSS `background` value.
#[must_use]
pub fn gradient_css(key: &str) -> String {
    let (from, to) = AVATAR_GRADIENTS[gradient_index(key)];
    format!("linear-gradient(135deg,{from},{to})")
}

/// Two-letter initials: first letters of the first two words, or the
/// first two characters of a single word. Uppercased; `?` when empty.
#[must_use]
pub fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    match (words.next(), words.next()) {
        (Some(a), Some(b)) => {
            let mut s = String::new();
            s.extend(a.chars().next().map(|c| c.to_ascii_uppercase()));
            s.extend(b.chars().next().map(|c| c.to_ascii_uppercase()));
            s
        }
        (Some(a), None) => a.chars().take(2).map(|c| c.to_ascii_uppercase()).collect(),
        _ => "?".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_deterministic() {
        assert_eq!(
            gradient_index("cody@fts.app"),
            gradient_index("cody@fts.app")
        );
        assert!(gradient_index("anything") < AVATAR_GRADIENTS.len());
        assert!(gradient_css("x").starts_with("linear-gradient(135deg,#"));
    }

    #[test]
    fn initials_cover_the_name_shapes() {
        assert_eq!(initials("Cody Wright"), "CW");
        assert_eq!(initials("ripley"), "RI");
        assert_eq!(initials(""), "?");
    }
}
