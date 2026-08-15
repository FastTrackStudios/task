//! Time section — logged vs estimated minutes + recurrence
//! summary. Pure display; the timer feature owns mutation.

use architect_ui::lucide_dioxus::{Repeat, Timer};
use chrono::Utc;
use dioxus::prelude::*;
use task_proto::TimeEntry as DbTimeEntry;

use crate::views::detail_full::SectionLabel;

/// Sum of completed entries, in whole minutes. A running entry
/// (`end_time: None`) is counted up to "now" so an in-progress
/// timer is visible in the total.
#[must_use]
pub fn sum_logged_minutes(entries: &[DbTimeEntry]) -> u32 {
    entries
        .iter()
        .map(|e| {
            let end = e.end_time.unwrap_or_else(Utc::now);
            (end - e.start_time).num_minutes().max(0)
        })
        .sum::<i64>()
        .try_into()
        .unwrap_or(u32::MAX)
}

/// `90` → `"1h 30m"`, `120` → `"2h"`, `45` → `"45m"`.
#[must_use]
pub fn format_minutes(minutes: u32) -> String {
    let h = minutes / 60;
    let m = minutes % 60;
    match (h, m) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// Human one-liner for an RFC 5545 RRULE. Handles the common
/// `FREQ` / `INTERVAL` / `BYDAY` combos the task feature writes;
/// anything fancier falls back to the raw rule string.
#[must_use]
pub fn recurrence_summary(rrule: &str) -> String {
    let mut freq: Option<&str> = None;
    let mut interval: u32 = 1;
    let mut bydays: Vec<&str> = Vec::new();

    for part in rrule.trim().trim_start_matches("RRULE:").split(';') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        match k.trim().to_ascii_uppercase().as_str() {
            "FREQ" => freq = Some(v.trim()),
            "INTERVAL" => interval = v.trim().parse().unwrap_or(1),
            "BYDAY" => bydays = v.split(',').map(str::trim).collect(),
            _ => {}
        }
    }

    let Some(freq) = freq else {
        return rrule.to_string();
    };
    let (every_one, unit) = match freq.to_ascii_uppercase().as_str() {
        "DAILY" => ("Daily", "days"),
        "WEEKLY" => ("Weekly", "weeks"),
        "MONTHLY" => ("Monthly", "months"),
        "YEARLY" => ("Yearly", "years"),
        _ => return rrule.to_string(),
    };
    let mut out = if interval <= 1 {
        every_one.to_string()
    } else {
        format!("Every {interval} {unit}")
    };
    if !bydays.is_empty() {
        let days: Vec<&str> = bydays.iter().map(|d| day_name(d)).collect();
        out.push_str(" on ");
        out.push_str(&days.join(", "));
    }
    out
}

fn day_name(code: &str) -> &'static str {
    match code.to_ascii_uppercase().as_str() {
        "MO" => "Mon",
        "TU" => "Tue",
        "WE" => "Wed",
        "TH" => "Thu",
        "FR" => "Fri",
        "SA" => "Sat",
        "SU" => "Sun",
        _ => "?",
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct TimeSectionProps {
    /// Estimated work in minutes (`TaskInfo::time_estimate`).
    #[props(default)]
    pub time_estimate: Option<u32>,
    pub entries: Vec<DbTimeEntry>,
    /// Raw RRULE (`TaskInfo::recurrence`).
    #[props(default)]
    pub recurrence: Option<String>,
    /// `"scheduled"` or `"completion"` (`TaskInfo::recurrence_anchor`).
    #[props(default)]
    pub recurrence_anchor: Option<String>,
    /// `YYYY-MM-DD` per finished instance
    /// (`TaskInfo::complete_instances`).
    #[props(default)]
    pub complete_instances: Vec<String>,
}

/// How the anchor reads to a person.
///
/// The distinction is the whole difference between a habit that
/// survives a missed week and one that buries you: a completion
/// anchor counts from when you actually did it, a scheduled one from
/// the calendar regardless.
fn anchor_label(anchor: Option<&str>) -> Option<&'static str> {
    match anchor?.trim().to_ascii_lowercase().as_str() {
        "completion" => Some("from completion"),
        "scheduled" => Some("on schedule"),
        _ => None,
    }
}

#[component]
pub fn TimeSection(props: TimeSectionProps) -> Element {
    let logged = sum_logged_minutes(&props.entries);
    let running = props.entries.iter().any(|e| e.end_time.is_none());
    rsx! {
        section { class: "flex flex-col gap-1.5",
            SectionLabel { label: "Time" }
            div { class: "flex flex-col gap-1.5",
                div { class: "flex items-center gap-2 text-sm text-foreground",
                    Timer { size: 14 }
                    if let Some(est) = props.time_estimate {
                        span {
                            "{format_minutes(logged)} logged of {format_minutes(est)} estimated"
                        }
                    } else {
                        span { "{format_minutes(logged)} logged" }
                    }
                    if running {
                        span { class: "rounded-full bg-emerald-700/40 px-1.5 py-0 text-[10px] text-emerald-100",
                            "running"
                        }
                    }
                    span { class: "text-xs text-muted-foreground",
                        "({props.entries.len()} entries)"
                    }
                }
                if let Some(est) = props.time_estimate {
                    if est > 0 {
                        {
                            let pct = ((u64::from(logged) * 100) / u64::from(est)).min(100);
                            let over = logged > est;
                            let bar = if over { "bg-amber-500" } else { "bg-primary" };
                            rsx! {
                                div { class: "h-1.5 w-full max-w-xs overflow-hidden rounded-full bg-muted/40",
                                    div { class: "h-full rounded-full {bar}", style: "width: {pct}%" }
                                }
                            }
                        }
                    }
                }
                if let Some(rule) = props.recurrence.as_deref() {
                    div { class: "flex items-center gap-2 text-xs text-muted-foreground",
                        Repeat { size: 12 }
                        span { "{recurrence_summary(rule)}" }
                        if let Some(anchor) = anchor_label(props.recurrence_anchor.as_deref()) {
                            span { class: "rounded-full bg-muted/50 px-1.5 py-0 text-[10px]",
                                "{anchor}"
                            }
                        }
                    }
                    // Completion history. A recurring task never
                    // closes — it accumulates instances — so the count
                    // is the only evidence the habit is actually
                    // happening, and a bare "Weekly" tells you nothing.
                    if !props.complete_instances.is_empty() {
                        {
                            let done = props.complete_instances.len();
                            let last = props.complete_instances.iter().max().cloned();
                            rsx! {
                                div { class: "flex items-center gap-2 pl-5 text-xs text-muted-foreground",
                                    span { "{done} completed" }
                                    if let Some(last) = last {
                                        span { class: "text-[10px]", "· last {last}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    #[test]
    fn anchor_reads_as_english_or_not_at_all() {
        assert_eq!(anchor_label(Some("completion")), Some("from completion"));
        assert_eq!(anchor_label(Some("Scheduled")), Some("on schedule"));
        // Absent or unrecognized renders nothing rather than leaking a
        // raw field value into the UI.
        assert_eq!(anchor_label(None), None);
        assert_eq!(anchor_label(Some("weird")), None);
    }

    fn entry(mins: i64) -> DbTimeEntry {
        let start = Utc::now() - Duration::minutes(mins + 1000);
        DbTimeEntry {
            start_time: start,
            end_time: Some(start + Duration::minutes(mins)),
        }
    }

    #[test]
    fn sums_completed_entries() {
        assert_eq!(sum_logged_minutes(&[entry(30), entry(45)]), 75);
        assert_eq!(sum_logged_minutes(&[]), 0);
    }

    #[test]
    fn running_entry_counts_elapsed() {
        let running = DbTimeEntry {
            start_time: Utc::now() - Duration::minutes(10),
            end_time: None,
        };
        let total = sum_logged_minutes(&[running]);
        assert!((9..=11).contains(&total), "got {total}");
    }

    #[test]
    fn format_minutes_cases() {
        assert_eq!(format_minutes(45), "45m");
        assert_eq!(format_minutes(120), "2h");
        assert_eq!(format_minutes(90), "1h 30m");
        assert_eq!(format_minutes(0), "0m");
    }

    #[test]
    fn recurrence_summaries() {
        assert_eq!(recurrence_summary("FREQ=DAILY"), "Daily");
        assert_eq!(
            recurrence_summary("FREQ=WEEKLY;BYDAY=MO,WE"),
            "Weekly on Mon, Wed"
        );
        assert_eq!(
            recurrence_summary("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=FR"),
            "Every 2 weeks on Fri"
        );
        // Unknown shapes fall back to the raw rule.
        assert_eq!(recurrence_summary("FREQ=HOURLY"), "FREQ=HOURLY");
        assert_eq!(recurrence_summary("???"), "???");
    }
}
