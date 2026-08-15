//! Minimal natural-language task capture.
//!
//! Input: `"Buy milk tomorrow #errands @shopping !high"`.
//! Output: a `TaskInfo` with the extractions applied + the
//! remaining text as `title`.
//!
//! Extraction rules (v1 — kept small on purpose):
//! - `#tag` → push to `tags`. The literal `#task` is
//!   auto-added so the page passes the
//!   `looks_like_task` discriminator.
//! - `@context`     → push to `contexts`.
//! - `[[Wikilink]]` → push to `projects`.
//! - `!low|normal|high|critical` → set `priority`.
//! - Bare date tokens (case-insensitive):
//!   - `today` → today (`due`)
//!   - `tomorrow` → today + 1
//!   - `next monday` … → next Mon…Sun
//!   - `mon` / `tue` / … → next occurrence of that weekday
//!   - `YYYY-MM-DD` → that date
//!
//! - Recurrence phrases (`daily`, `every week`, `every 2 weeks`,
//!   `every other monday`, `every weekday`) → an RFC 5545 RRULE in
//!   `recurrence`, plus the `recurrence_anchor` that decides what
//!   "next" means. See [`consume_recurrence_phrase`].
//!
//! Whatever's left after extraction becomes the `title`. NLP
//! expansion (priority words "asap"/"urgent", deadline phrases
//! "by Friday", duration "for 30m") lives in a later slice — port
//! the rules from `tasknotes-nlp-core` when we need them.

use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};

use crate::model::TaskInfo;

/// Parse an input line into a `TaskInfo`. The returned task
/// has `path = ""` — the vault-backed sibling crate's
/// `task::write::default_task_path` computes one, and
/// `task::write::write_task` puts it on disk.
pub fn capture(input: &str) -> TaskInfo {
    let mut tags: Vec<String> = Vec::new();
    let mut contexts: Vec<String> = Vec::new();
    let mut projects: Vec<String> = Vec::new();
    let mut priority: Option<String> = None;
    let mut due: Option<String> = None;

    let today = Local::now().date_naive();

    // Tokenize on whitespace, but preserve `[[multi word]]` as
    // a single token. We scan the source once with a small state
    // machine so the link bracket can hold spaces.
    let tokens = tokenize(input);
    // Recurrence first: "every monday" must swallow `monday` before
    // any date rule reads it as a due date. A repeating task that
    // silently became a one-off due next Monday is exactly the bug
    // this ordering prevents.
    let (tokens, recur) = consume_recurrence_phrase(tokens, today);
    // Multi-token date phrase pass next ("next monday" swallows
    // both tokens), so the single-token weekday rule below
    // doesn't grab "monday" before we see "next".
    let (tokens, phrase_due) = consume_date_phrase(tokens, today);
    let mut title_parts: Vec<String> = Vec::new();
    for tok in tokens {
        if let Some(tag) = tok.strip_prefix('#') {
            if !tag.is_empty() {
                tags.push(tag.to_string());
            }
            continue;
        }
        if let Some(ctx) = tok.strip_prefix('@') {
            if !ctx.is_empty() {
                contexts.push(format!("@{ctx}"));
            }
            continue;
        }
        if let Some(rest) = tok.strip_prefix("[[").and_then(|s| s.strip_suffix("]]")) {
            if !rest.is_empty() {
                projects.push(format!("[[{rest}]]"));
            }
            continue;
        }
        if let Some(prio) = tok.strip_prefix('!').and_then(canonical_priority) {
            priority = Some(prio.to_string());
            continue;
        }
        if let Some(date) = parse_date_token(&tok, today) {
            due = Some(date.format("%Y-%m-%d").to_string());
            continue;
        }
        title_parts.push(tok);
    }
    // An explicit date the user typed always wins over the start
    // date a recurrence implies.
    let due = due
        .or(phrase_due)
        .or_else(|| recur.as_ref().and_then(|r| r.first_due.clone()));

    // Ensure `task` is in tags for the discriminator.
    if !tags.iter().any(|t| t == "task") {
        tags.push("task".into());
    }

    let title = title_parts.join(" ").trim().to_string();
    TaskInfo {
        id: uuid::Uuid::new_v4(),
        path: String::new(),
        title: if title.is_empty() {
            "Untitled task".into()
        } else {
            title
        },
        status: "open".into(),
        priority: priority.unwrap_or_else(|| "normal".into()),
        due,
        scheduled: None,
        tags: crate::model::StringList(tags),
        contexts: crate::model::StringList(contexts),
        projects: crate::model::StringList(projects),
        project_id: None,
        milestone_id: None,
        time_estimate: None,
        time_entries: crate::model::TimeEntries::default(),
        recurrence: recur.as_ref().map(|r| r.rrule.clone()),
        recurrence_anchor: recur.as_ref().map(|r| r.anchor.to_string()),
        complete_instances: crate::model::StringList::default(),
        completed_date: None,
        agent_profile: String::new(),
        dispatched_agent_tasks: crate::model::StringList::default(),
        date_created: None,
        date_modified: None,
        details: String::new(),
        workflow: None,
    }
}

fn tokenize(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_link = false;
    for ch in src.chars() {
        if in_link {
            cur.push(ch);
            if cur.ends_with("]]") {
                out.push(std::mem::take(&mut cur));
                in_link = false;
            }
            continue;
        }
        if ch == '[' && cur.is_empty() {
            cur.push(ch);
            // Look-ahead is annoying; let the chunk close itself
            // on `]]`. Mark in_link as soon as we see the
            // opening `[`; a stray `[` becomes a one-char token.
            in_link = true;
            continue;
        }
        if ch.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn canonical_priority(p: &str) -> Option<&'static str> {
    match p.to_ascii_lowercase().as_str() {
        "none" => Some("none"),
        "low" => Some("low"),
        "normal" | "medium" | "med" => Some("normal"),
        "high" => Some("high"),
        "critical" | "urgent" => Some("critical"),
        _ => None,
    }
}

fn parse_date_token(tok: &str, today: NaiveDate) -> Option<NaiveDate> {
    let lc = tok.to_ascii_lowercase();
    match lc.as_str() {
        "today" => Some(today),
        "tomorrow" | "tmrw" => Some(today + Duration::days(1)),
        // Single weekday → next occurrence (or today if it IS today).
        "mon" | "monday" => Some(next_weekday(today, Weekday::Mon)),
        "tue" | "tues" | "tuesday" => Some(next_weekday(today, Weekday::Tue)),
        "wed" | "weds" | "wednesday" => Some(next_weekday(today, Weekday::Wed)),
        "thu" | "thur" | "thurs" | "thursday" => Some(next_weekday(today, Weekday::Thu)),
        "fri" | "friday" => Some(next_weekday(today, Weekday::Fri)),
        "sat" | "saturday" => Some(next_weekday(today, Weekday::Sat)),
        "sun" | "sunday" => Some(next_weekday(today, Weekday::Sun)),
        _ => NaiveDate::parse_from_str(tok, "%Y-%m-%d").ok(),
    }
}

fn consume_date_phrase(tokens: Vec<String>, today: NaiveDate) -> (Vec<String>, Option<String>) {
    let mut out = Vec::with_capacity(tokens.len());
    let mut due: Option<String> = None;
    let mut i = 0;
    while i < tokens.len() {
        if due.is_none() && i + 1 < tokens.len() && tokens[i].eq_ignore_ascii_case("next") {
            if let Some(date) = parse_date_token(&tokens[i + 1], today) {
                due = Some(date.format("%Y-%m-%d").to_string());
                i += 2;
                continue;
            }
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    (out, due)
}

/// A parsed recurrence phrase.
struct Recurrence {
    /// RFC 5545 RRULE body, e.g. `FREQ=WEEKLY;INTERVAL=2`.
    rrule: String,
    /// `"scheduled"` or `"completion"` — see
    /// [`consume_recurrence_phrase`] for which is chosen when.
    anchor: &'static str,
    /// When the first instance lands, if the phrase implies one.
    first_due: Option<String>,
}

/// RRULE two-letter day code.
fn rrule_day(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

/// The weekday a token names, if any. Shares its vocabulary with
/// [`parse_date_token`] so `monday` means the same thing in
/// `every monday` and `next monday`.
fn weekday_token(tok: &str) -> Option<Weekday> {
    match tok.to_ascii_lowercase().as_str() {
        "mon" | "monday" => Some(Weekday::Mon),
        "tue" | "tues" | "tuesday" => Some(Weekday::Tue),
        "wed" | "weds" | "wednesday" => Some(Weekday::Wed),
        "thu" | "thur" | "thurs" | "thursday" => Some(Weekday::Thu),
        "fri" | "friday" => Some(Weekday::Fri),
        "sat" | "saturday" => Some(Weekday::Sat),
        "sun" | "sunday" => Some(Weekday::Sun),
        _ => None,
    }
}

/// The FREQ a unit word names, singular or plural.
fn freq_token(tok: &str) -> Option<&'static str> {
    match tok.to_ascii_lowercase().as_str() {
        "day" | "days" => Some("DAILY"),
        "week" | "weeks" => Some("WEEKLY"),
        "month" | "months" => Some("MONTHLY"),
        "year" | "years" => Some("YEARLY"),
        _ => None,
    }
}

/// Pull a recurrence phrase out of the token stream.
///
/// Recognized, case-insensitively:
///
/// - bare adverbs — `daily`, `weekly`, `monthly`, `yearly`/`annually`
/// - `every <unit>` — `every week`
/// - `every <n> <units>` — `every 3 days`
/// - `every other <unit>` — `every other week` (INTERVAL=2)
/// - `every <weekday>` — `every monday` (BYDAY=MO)
/// - `every other <weekday>` — `every other friday`
/// - `every weekday` — Mon–Fri
///
/// ## Which anchor, and why it matters
///
/// The anchor decides what "next" means after you finish one, and
/// getting it wrong is the difference between a habit that works and
/// one you start ignoring:
///
/// - **A phrase naming a weekday is `scheduled`.** `every monday`
///   means the calendar decides — a standup happens Monday whether
///   or not you made last Monday's.
/// - **A bare interval is `completion`.** `every 2 weeks` means two
///   weeks after you actually did it. This is the habit case, and
///   anchoring it to the calendar instead would generate a pile of
///   overdue copies the first week you miss — which is precisely how
///   a habit list becomes something you stop opening.
///
/// Only the FIRST phrase is consumed; a second one is left in the
/// title rather than silently overriding the first.
fn consume_recurrence_phrase(
    tokens: Vec<String>,
    today: NaiveDate,
) -> (Vec<String>, Option<Recurrence>) {
    let mut out = Vec::with_capacity(tokens.len());
    let mut found: Option<Recurrence> = None;
    let mut i = 0;

    while i < tokens.len() {
        if found.is_some() {
            out.push(tokens[i].clone());
            i += 1;
            continue;
        }

        // Bare adverbs — a single token carries the whole phrase.
        let bare = match tokens[i].to_ascii_lowercase().as_str() {
            "daily" => Some("DAILY"),
            "weekly" => Some("WEEKLY"),
            "monthly" => Some("MONTHLY"),
            "yearly" | "annually" => Some("YEARLY"),
            _ => None,
        };
        if let Some(freq) = bare {
            found = Some(Recurrence {
                rrule: format!("FREQ={freq}"),
                anchor: "completion",
                first_due: Some(today.format("%Y-%m-%d").to_string()),
            });
            i += 1;
            continue;
        }

        if !tokens[i].eq_ignore_ascii_case("every") {
            out.push(tokens[i].clone());
            i += 1;
            continue;
        }

        // `every …` — look at what follows.
        let mut j = i + 1;
        if j >= tokens.len() {
            out.push(tokens[i].clone());
            i += 1;
            continue;
        }

        // Optional interval: `other` (=2) or a number.
        let mut interval: u32 = 1;
        if tokens[j].eq_ignore_ascii_case("other") {
            interval = 2;
            j += 1;
        } else if let Ok(n) = tokens[j].parse::<u32>() {
            if n > 0 {
                interval = n;
                j += 1;
            }
        }
        if j >= tokens.len() {
            out.push(tokens[i].clone());
            i += 1;
            continue;
        }

        let unit = &tokens[j];
        let parsed = if unit.eq_ignore_ascii_case("weekday") || unit.eq_ignore_ascii_case("weekdays")
        {
            Some((
                "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".to_owned(),
                "scheduled",
                Some(next_weekday_or_today(today)),
            ))
        } else if let Some(w) = weekday_token(unit) {
            Some((
                format!("FREQ=WEEKLY;BYDAY={}", rrule_day(w)),
                "scheduled",
                Some(next_weekday(today, w)),
            ))
        } else {
            freq_token(unit).map(|freq| (format!("FREQ={freq}"), "completion", Some(today)))
        };

        let Some((base, anchor, start)) = parsed else {
            // `every` followed by something we don't understand —
            // leave it in the title rather than guessing.
            out.push(tokens[i].clone());
            i += 1;
            continue;
        };

        let rrule = if interval > 1 {
            format!("{base};INTERVAL={interval}")
        } else {
            base
        };
        found = Some(Recurrence {
            rrule,
            anchor,
            first_due: start.map(|d| d.format("%Y-%m-%d").to_string()),
        });
        i = j + 1;
    }

    (out, found)
}

/// Today if it's a weekday, else the next Monday — the first
/// instance of a Mon–Fri recurrence.
fn next_weekday_or_today(today: NaiveDate) -> NaiveDate {
    match today.weekday() {
        Weekday::Sat | Weekday::Sun => next_weekday(today, Weekday::Mon),
        _ => today,
    }
}

fn next_weekday(today: NaiveDate, target: Weekday) -> NaiveDate {
    let today_num = i64::from(today.weekday().num_days_from_monday());
    let target_num = i64::from(target.num_days_from_monday());
    let delta = (target_num - today_num).rem_euclid(7);
    // "next mon" when it IS monday: 7 days out (next-next-monday).
    let delta = if delta == 0 { 7 } else { delta };
    today + Duration::days(delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tags_contexts_projects() {
        let t = capture("Buy milk #errands #urgent @shopping [[Groceries]]");
        assert_eq!(t.title, "Buy milk");
        assert!(t.tags.0.contains(&"errands".into()));
        assert!(t.tags.0.contains(&"urgent".into()));
        assert!(t.tags.0.contains(&"task".into())); // auto-added
        assert_eq!(t.contexts.0, vec!["@shopping"]);
        assert_eq!(t.projects.0, vec!["[[Groceries]]"]);
    }

    #[test]
    fn extracts_priority() {
        let t = capture("Fix bug !critical");
        assert_eq!(t.title, "Fix bug");
        assert_eq!(t.priority, "critical");
    }

    #[test]
    fn extracts_today_tomorrow() {
        let today = Local::now().date_naive();
        let t = capture("Buy milk tomorrow");
        assert_eq!(
            t.due.as_deref(),
            Some(
                today
                    .succ_opt()
                    .unwrap()
                    .format("%Y-%m-%d")
                    .to_string()
                    .as_str()
            )
        );
        assert_eq!(t.title, "Buy milk");
    }

    #[test]
    fn extracts_next_weekday() {
        let t = capture("Standup next monday");
        assert!(t.due.is_some());
        assert_eq!(t.title, "Standup");
    }

    #[test]
    fn empty_input_gets_placeholder_title() {
        let t = capture("");
        assert_eq!(t.title, "Untitled task");
    }

    #[test]
    fn bare_adverbs_recur() {
        for (input, freq) in [
            ("Mixing practice daily", "FREQ=DAILY"),
            ("Mixing practice weekly", "FREQ=WEEKLY"),
            ("Rent monthly", "FREQ=MONTHLY"),
            ("Renew domain yearly", "FREQ=YEARLY"),
        ] {
            let t = capture(input);
            assert_eq!(t.recurrence.as_deref(), Some(freq), "{input}");
            assert!(!t.title.contains("daily"), "adverb left in title: {input}");
        }
    }

    #[test]
    fn every_unit_with_interval() {
        let t = capture("Mixing practice every 2 weeks");
        assert_eq!(t.recurrence.as_deref(), Some("FREQ=WEEKLY;INTERVAL=2"));
        assert_eq!(t.title, "Mixing practice");

        let t = capture("Deep clean every other month");
        assert_eq!(t.recurrence.as_deref(), Some("FREQ=MONTHLY;INTERVAL=2"));
        assert_eq!(t.title, "Deep clean");
    }

    #[test]
    fn a_named_weekday_is_calendar_anchored() {
        // "every monday" means Monday decides, not your last
        // completion — a standup happens whether or not you made the
        // previous one.
        let t = capture("Standup every monday");
        assert_eq!(t.recurrence.as_deref(), Some("FREQ=WEEKLY;BYDAY=MO"));
        assert_eq!(t.recurrence_anchor.as_deref(), Some("scheduled"));
        assert_eq!(t.title, "Standup");
    }

    #[test]
    fn a_bare_interval_is_completion_anchored() {
        // The habit case: two weeks after you ACTUALLY did it.
        // Calendar-anchoring this is what generates a pile of overdue
        // copies the first week you miss.
        let t = capture("Mixing practice every 2 weeks");
        assert_eq!(t.recurrence_anchor.as_deref(), Some("completion"));
    }

    #[test]
    fn every_weekday_is_mon_to_fri() {
        let t = capture("Inbox zero every weekday");
        assert_eq!(
            t.recurrence.as_deref(),
            Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR")
        );
        assert_eq!(t.recurrence_anchor.as_deref(), Some("scheduled"));
    }

    #[test]
    fn recurrence_wins_the_weekday_token() {
        // The ordering bug this guards: without the recurrence pass
        // running first, `monday` is read as a due date and the task
        // silently becomes a one-off.
        let t = capture("Standup every monday");
        assert!(t.recurrence.is_some(), "should repeat");
        assert!(!t.title.contains("monday"), "weekday left in title");
    }

    #[test]
    fn an_explicit_date_beats_the_recurrence_start() {
        let t = capture("Mixing practice weekly 2026-09-01");
        assert_eq!(t.recurrence.as_deref(), Some("FREQ=WEEKLY"));
        assert_eq!(t.due.as_deref(), Some("2026-09-01"));
    }

    #[test]
    fn a_weekly_habit_starts_today_so_it_surfaces() {
        // Without a start date a completion-anchored habit would sit
        // dateless and never appear in a day view.
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        let t = capture("Mixing practice weekly");
        assert_eq!(t.due.as_deref(), Some(today.as_str()));
    }

    #[test]
    fn unrecognized_every_stays_in_the_title() {
        // "every" is an ordinary English word; only consume it when
        // what follows actually names a cadence.
        let t = capture("Check every input gain");
        assert!(t.recurrence.is_none());
        assert_eq!(t.title, "Check every input gain");
    }

    #[test]
    fn a_second_phrase_is_left_alone() {
        let t = capture("Sync weekly and daily");
        assert_eq!(t.recurrence.as_deref(), Some("FREQ=WEEKLY"));
        assert!(t.title.contains("daily"), "got: {}", t.title);
    }

    #[test]
    fn plain_tasks_gain_no_recurrence() {
        let t = capture("Buy milk tomorrow #errands");
        assert!(t.recurrence.is_none());
        assert!(t.recurrence_anchor.is_none());
    }
}

/// Resolve a captured `[[Project]]` reference to the project's stable
/// id: first wikilink whose bare name matches a known project title
/// (case-insensitive, brackets stripped). The inference half of
/// project filing — the UI's explicit picker and the quick-add's
/// `[[...]]` syntax both end at `project_id`.
#[must_use]
pub fn infer_project_id(
    project_refs: &[String],
    known: &[(uuid::Uuid, String)],
) -> Option<uuid::Uuid> {
    project_refs.iter().find_map(|raw| {
        let name = raw
            .trim()
            .trim_start_matches("[[")
            .trim_end_matches("]]")
            .trim();
        known
            .iter()
            .find(|(_, title)| title.eq_ignore_ascii_case(name))
            .map(|(id, _)| *id)
    })
}

#[cfg(test)]
mod infer_tests {
    use super::{capture, infer_project_id};
    use uuid::Uuid;

    #[test]
    fn wikilink_capture_resolves_to_project_id() {
        let going_home = Uuid::new_v4();
        let known = vec![
            (Uuid::new_v4(), "Task".to_owned()),
            (going_home, "Going Home - Justin Hayward".to_owned()),
        ];
        let t = capture("Mix vocals [[going home - justin hayward]] tomorrow");
        assert_eq!(infer_project_id(&t.projects.0, &known), Some(going_home));
        // No wikilink → no inference.
        let t = capture("Mix vocals tomorrow");
        assert_eq!(infer_project_id(&t.projects.0, &known), None);
    }
}
