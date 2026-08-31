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

// ── the components ──────────────────────────────────────────────────
//
// Here for the same reason the palette is: a person has to look the
// same everywhere, and "everywhere" now includes crates that cannot
// depend on the shell — files-ui's review rail, and every app that
// shows who something belongs to.

use architect_ui::prelude::*;
use dioxus::prelude::*;

/// The initials disc: a deterministic gradient from the person's email
/// (or name, for presence rows that predate account identity) and their
/// initials over it.
#[component]
pub fn Avatar(name: String, email: String, #[props(default = 28)] size: u32) -> Element {
    let key = if email.is_empty() { &name } else { &email };
    let (from, to) = AVATAR_GRADIENTS[gradient_index(key)];
    let letters = initials(&name);
    // ~0.38em type within the disc, floored so 16px stays legible.
    let font = (size * 2 / 5).max(7);
    rsx! {
        span {
            class: "flex shrink-0 select-none items-center justify-center rounded-full font-semibold leading-none text-white",
            style: "width:{size}px;height:{size}px;font-size:{font}px;background:linear-gradient(135deg,{from},{to});",
            title: "{name}",
            "{letters}"
        }
    }
}

/// A person, rendered the same way everywhere: [`Avatar`], the display
/// name, an optional subtitle (email · org), and an optional badge
/// (source or role).
#[component]
pub fn PersonChip(
    name: String,
    #[props(default)] email: String,
    #[props(default)] subtitle: Option<String>,
    #[props(default)] badge_label: Option<String>,
    #[props(default)] badge_variant: StatusBadgeVariant,
    #[props(default = 34)] size: u32,
) -> Element {
    rsx! {
        div { class: "flex min-w-0 items-center gap-3",
            Avatar { name: name.clone(), email: email.clone(), size }
            div { class: "flex min-w-0 flex-col",
                div { class: "flex items-center gap-1.5",
                    span { class: "truncate text-sm font-medium text-foreground", "{name}" }
                    if let Some(label) = badge_label.clone() {
                        StatusBadge {
                            variant: badge_variant,
                            label,
                            class: "px-1.5 py-0 text-[10px]".to_string(),
                        }
                    }
                }
                if let Some(sub) = subtitle.clone() {
                    if !sub.is_empty() {
                        span { class: "truncate text-xs text-muted-foreground", "{sub}" }
                    }
                }
            }
        }
    }
}
