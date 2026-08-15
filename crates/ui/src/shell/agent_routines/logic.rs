//! Pure derivations for the routines panel.

/// Read back a schedule expression the way the backend will parse
/// it, so a typo is visible before the routine is created rather
/// than as a 400 afterwards. `None` while there's nothing to say.
///
/// Mirrors hermes-agent's `cron.jobs.parse_schedule` grammar:
/// duration (one-shot), `every <duration>` (recurring), a 5-field
/// cron expression, or an ISO timestamp (one-shot).
#[must_use]
pub fn schedule_hint(input: &str) -> Option<String> {
    let raw = input.trim();
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_lowercase();

    if let Some(rest) = lower.strip_prefix("every ") {
        return Some(match parse_duration(rest.trim()) {
            Some(d) => format!("repeats {d}"),
            None => "unrecognized interval — try 'every 30m', 'every 2h'".to_string(),
        });
    }
    // Cron: 5+ fields of digits and the usual metacharacters.
    let fields: Vec<&str> = lower.split_whitespace().collect();
    if fields.len() >= 5
        && fields[..5]
            .iter()
            .all(|f| f.chars().all(|c| c.is_ascii_digit() || "*-,/".contains(c)))
    {
        return Some("cron expression — repeats on that schedule".to_string());
    }
    if lower.contains('t') && lower.len() >= 10 && lower.starts_with(|c: char| c.is_ascii_digit()) {
        return Some("runs once at that time".to_string());
    }
    if lower.len() >= 8 && lower.chars().take(4).all(|c| c.is_ascii_digit()) {
        return Some("runs once at that time".to_string());
    }
    if let Some(d) = parse_duration(&lower) {
        return Some(format!("runs once, {d} from now"));
    }
    Some("unrecognized — use '30m', 'every 2h', '0 8 * * *', or a timestamp".to_string())
}

/// `"30m"` / `"2h"` / `"1d"` → a spelled-out duration.
fn parse_duration(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let (digits, unit) = raw.split_at(raw.find(|c: char| !c.is_ascii_digit())?);
    let n: u32 = digits.parse().ok()?;
    if n == 0 {
        return None;
    }
    let word = match unit.trim() {
        "m" | "min" | "mins" | "minute" | "minutes" => "minute",
        "h" | "hr" | "hrs" | "hour" | "hours" => "hour",
        "d" | "day" | "days" => "day",
        "w" | "week" | "weeks" => "week",
        _ => return None,
    };
    let plural = if n == 1 { "" } else { "s" };
    Some(format!("{n} {word}{plural}"))
}

/// "in 4h" / "12m ago" for a routine's next/last run. `None` when
/// the stamp is absent or unparseable — nothing to show beats a
/// wrong relative time.
#[must_use]
pub fn relative_when(stamp: &str, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    if stamp.trim().is_empty() {
        return None;
    }
    let at = chrono::DateTime::parse_from_rfc3339(stamp.trim()).ok()?;
    let secs = (at.with_timezone(&chrono::Utc) - now).num_seconds();
    let ahead = secs >= 0;
    let mag = secs.abs();
    let span = match mag {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m", mag / 60),
        3600..=86_399 => format!("{}h", mag / 3600),
        _ => format!("{}d", mag / 86_400),
    };
    if span == "now" {
        return Some("now".to_string());
    }
    Some(if ahead {
        format!("in {span}")
    } else {
        format!("{span} ago")
    })
}

/// "3 of 5 runs" / "3 runs" (unbounded) / `None` before the first.
#[must_use]
pub fn runs_label(completed: u32, total: u32) -> Option<String> {
    if completed == 0 && total == 0 {
        return None;
    }
    if total > 0 {
        return Some(format!("{completed} of {total} runs"));
    }
    Some(format!(
        "{completed} run{}",
        if completed == 1 { "" } else { "s" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_hint_reads_back_each_grammar() {
        assert_eq!(schedule_hint(""), None);
        assert_eq!(schedule_hint("   "), None);
        assert!(
            schedule_hint("every 30m")
                .expect("hint")
                .contains("repeats 30 minutes")
        );
        assert!(
            schedule_hint("every 2h")
                .expect("hint")
                .contains("repeats 2 hours")
        );
        assert!(
            schedule_hint("0 8 * * *")
                .expect("hint")
                .contains("cron expression")
        );
        assert!(
            schedule_hint("2026-08-01T09:00:00")
                .expect("hint")
                .contains("once at that time")
        );
        assert!(
            schedule_hint("30m")
                .expect("hint")
                .contains("once, 30 minutes from now")
        );
    }

    #[test]
    fn schedule_hint_flags_what_the_backend_would_reject() {
        assert!(
            schedule_hint("soonish")
                .expect("hint")
                .contains("unrecognized")
        );
        assert!(
            schedule_hint("every fortnight")
                .expect("hint")
                .contains("unrecognized interval")
        );
        // A zero duration is not a schedule.
        assert!(schedule_hint("0m").expect("hint").contains("unrecognized"));
    }

    #[test]
    fn relative_when_reads_forward_and_backward() {
        let now = chrono::Utc::now();
        let in_4h = (now + chrono::Duration::hours(4)).to_rfc3339();
        let ago_12m = (now - chrono::Duration::minutes(12)).to_rfc3339();
        assert_eq!(relative_when(&in_4h, now).as_deref(), Some("in 4h"));
        assert_eq!(relative_when(&ago_12m, now).as_deref(), Some("12m ago"));
        assert_eq!(
            relative_when(&now.to_rfc3339(), now).as_deref(),
            Some("now")
        );
    }

    #[test]
    fn relative_when_stays_quiet_on_bad_input() {
        let now = chrono::Utc::now();
        // Never-run routines report an empty stamp.
        assert_eq!(relative_when("", now), None);
        assert_eq!(relative_when("not a date", now), None);
    }

    #[test]
    fn runs_label_distinguishes_bounded_from_forever() {
        assert_eq!(runs_label(0, 0), None);
        assert_eq!(runs_label(1, 0).as_deref(), Some("1 run"));
        assert_eq!(runs_label(4, 0).as_deref(), Some("4 runs"));
        assert_eq!(runs_label(3, 5).as_deref(), Some("3 of 5 runs"));
    }
}
