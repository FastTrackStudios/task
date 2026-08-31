//! Display formatting shared across pages.
//!
//! These had been copied per page, and the copies disagreed. `money`
//! existed four times in three different behaviours — one dropped the
//! minus sign for amounts between -$1.00 and $0, another rendered
//! currency through an `f64` and produced `$-0.50`. Playback clocks had
//! four spellings under two names.

use architect_ui::components::StatusBadgeVariant;

/// Format signed cents as currency: `$12.34`, `-$0.50`.
///
/// The sign leads the whole amount rather than the dollar figure, which
/// is what breaks when you format the integer parts separately: for
/// -50 cents, `cents / 100` truncates to 0 and the sign disappears.
pub fn money(cents: i64) -> String {
    let neg = cents < 0;
    let v = cents.unsigned_abs();
    format!("{}${}.{:02}", if neg { "-" } else { "" }, v / 100, v % 100)
}

/// Format a duration in seconds as `m:ss`, clamping negatives to zero.
///
/// Used for playback position and remaining time.
pub fn duration_mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Format a duration in whole seconds as `m:ss`, or `h:mm:ss` past an
/// hour — for timers, where the hour matters.
pub fn duration_hms(secs: u32) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Badge variant for the shared project/task status ladder.
///
/// Slices with their own vocabulary (goals, inventory) map their own
/// statuses — this covers the strings project and task pages emit.
pub fn status_variant(status: &str) -> StatusBadgeVariant {
    match status {
        "active" | "open" | "in_progress" => StatusBadgeVariant::Success,
        "on_hold" | "on-hold" | "paused" | "waiting" => StatusBadgeVariant::Warning,
        "cancelled" | "canceled" | "abandoned" | "blocked" => StatusBadgeVariant::Danger,
        _ => StatusBadgeVariant::Neutral,
    }
}

/// First non-empty line of `text`, trimmed — the one-line preview used
/// in list rows.
pub fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_keeps_the_sign_on_sub_dollar_debits() {
        assert_eq!(money(-50), "-$0.50");
        assert_eq!(money(-1), "-$0.01");
        assert_eq!(money(-12_345), "-$123.45");
    }

    #[test]
    fn money_formats_positives_and_zero() {
        assert_eq!(money(0), "$0.00");
        assert_eq!(money(5), "$0.05");
        assert_eq!(money(12_345), "$123.45");
    }

    #[test]
    fn durations_clamp_and_pad() {
        assert_eq!(duration_mmss(-3.0), "0:00");
        assert_eq!(duration_mmss(9.9), "0:09");
        assert_eq!(duration_mmss(125.0), "2:05");
        assert_eq!(duration_hms(59), "0:59");
        assert_eq!(duration_hms(3_661), "1:01:01");
    }

    #[test]
    fn first_line_skips_leading_blanks() {
        assert_eq!(first_line("\n\n  hello \nworld"), "hello");
        assert_eq!(first_line(""), "");
    }
}

/// Lowercase, hyphenate, strip non-url-safe characters — a title into
/// something that can sit in a path.
///
/// Here rather than with either caller because both want it and
/// neither owns it: the inbox slugs a captured title into a note name,
/// and the bookings app slugs an event type into its public URL.
#[must_use]
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_owned()
}

#[cfg(test)]
mod slug_tests {
    use super::slugify;

    #[test]
    fn a_title_becomes_a_path_segment() {
        assert_eq!(slugify("30-Minute Consult"), "30-minute-consult");
        assert_eq!(slugify("  Spaced   Out  "), "spaced-out");
        assert_eq!(slugify("Ragu & Chips"), "ragu-chips");
        assert_eq!(slugify("!!!"), "");
    }
}
