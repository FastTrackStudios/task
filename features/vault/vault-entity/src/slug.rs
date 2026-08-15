//! Filename slugs.
//!
//! One definition, because the copies had drifted: some mapped every
//! non-alphanumeric run to a dash while others deleted punctuation
//! outright, so `"Don't Stop"` became `dont-stop` in one slice and
//! `don-t-stop` in another — and both spellings were load-bearing for
//! on-disk paths.
//!
//! The rule here is the majority one: a run of non-alphanumerics
//! collapses to a single dash, apostrophes included.

/// Slugify `s`, falling back to `fallback` when nothing survives
/// (e.g. a name that is entirely punctuation or emoji).
pub fn slugify(s: &str, fallback: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

/// `<folder>/<slug>.md`, using `default_folder` when the caller has no
/// override. Trailing slashes on either folder are tolerated.
pub fn entity_path(name: &str, folder: Option<&str>, default_folder: &str, fallback: &str) -> String {
    let slug = slugify(name, fallback);
    let dir = folder.unwrap_or(default_folder).trim_end_matches('/');
    format!("{dir}/{slug}.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_punctuation_runs_to_single_dashes() {
        assert_eq!(slugify("Don't Stop", "x"), "don-t-stop");
        assert_eq!(slugify("Hello -- World", "x"), "hello-world");
        assert_eq!(slugify("  Leading", "x"), "leading");
        assert_eq!(slugify("Trailing!!", "x"), "trailing");
    }

    #[test]
    fn lowercases_and_keeps_digits() {
        assert_eq!(slugify("Week 42 Review", "x"), "week-42-review");
    }

    #[test]
    fn falls_back_when_nothing_survives() {
        assert_eq!(slugify("", "body-metric"), "body-metric");
        assert_eq!(slugify("!!!", "body-metric"), "body-metric");
    }

    #[test]
    fn entity_path_honours_override_and_default() {
        assert_eq!(
            entity_path("Weight", None, "Projects/Fitness/body", "metric"),
            "Projects/Fitness/body/weight.md"
        );
        assert_eq!(
            entity_path("Weight", Some("Custom/"), "Projects/Fitness/body", "metric"),
            "Custom/weight.md"
        );
    }
}
