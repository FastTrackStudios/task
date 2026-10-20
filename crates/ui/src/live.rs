//! What the lock screen shows: the running timer (a Live Activity — the
//! Dynamic Island, the lock screen, the Apple Watch's Smart Stack) and
//! the widgets' numbers (today's open tasks, the day's current and next
//! block).
//!
//! The platform registers a [`Sink`] (the iOS app: `apps/mobile/src/
//! live.rs`, over a small Swift library); [`LiveFeed`], mounted once in
//! the shell, watches the stores and tells the sink when something it
//! shows changed. Nothing registered (web, desktop): nothing happens.

use std::sync::RwLock;

use dioxus::prelude::*;

/// Where the lock screen's state goes.
#[derive(Clone, Copy)]
pub struct Sink {
    /// The timer runs: what it is for, and when it started (unix secs).
    pub timer_show: fn(title: &str, started_at: f64),
    /// No timer runs.
    pub timer_end: fn(),
    /// The widgets' data: open tasks today, the block now, the next one.
    pub widgets: fn(tasks_today: i32, now: &str, next: &str),
}

static SINK: RwLock<Option<Sink>> = RwLock::new(None);

/// Register the platform's sink (once, before launch).
pub fn set_sink(sink: Sink) {
    if let Ok(mut s) = SINK.write() {
        *s = Some(sink);
    }
}

fn sink() -> Option<Sink> {
    SINK.read().ok().and_then(|s| *s)
}

/// The block running at `minute` and the next to start, as the widget
/// reads them: "Studio block · until 12:30", "Lunch · 12:30".
#[must_use]
pub fn now_and_next(blocks: &[(u16, u16, String)], minute: u16) -> (String, String) {
    let clock = |m: u16| format!("{}:{:02}", m / 60, m % 60);
    let mut sorted: Vec<&(u16, u16, String)> = blocks.iter().collect();
    sorted.sort_by_key(|b| b.0);
    let now = sorted
        .iter()
        .find(|(s, e, _)| *s <= minute && minute < *e)
        .map(|(_, e, l)| format!("{l} · until {}", clock(*e)))
        .unwrap_or_else(|| "Free".to_owned());
    let next = sorted
        .iter()
        .find(|(s, _, _)| *s > minute)
        .map(|(s, _, l)| format!("{l} · {}", clock(*s)))
        .unwrap_or_default();
    (now, next)
}

/// Watches the timer, today's tasks and today's blocks; tells the sink.
#[component]
pub fn LiveFeed() -> Element {
    let active = crate::chrome::use_active_timer();
    let tasks = crate::stores::use_task_list();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let tick = use_signal(|| 0u64);
    crate::chrome::use_second_tick(tick);
    // Today's blocks, re-read every few minutes (a saved plan, or the
    // day's template).
    let mut blocks = use_signal(Vec::<(u16, u16, String)>::new);
    use_future(move || async move {
        loop {
            let slug = crate::orgs::active_slug(&selection.peek(), &org_list.peek());
            if !slug.is_empty() {
                let today = chrono::Local::now().date_naive();
                let saved = crate::feeds::fetch_day_plan(&slug, &today.to_string())
                    .await
                    .ok()
                    .flatten();
                let templates = crate::feeds::fetch_day_templates(&slug)
                    .await
                    .unwrap_or_default();
                let (_, resolved) =
                    scheduling_proto::resolve::resolve_day(today, saved, &templates);
                blocks.set(
                    resolved
                        .iter()
                        .map(|rb| {
                            (
                                rb.block.start.minutes_since_midnight,
                                rb.block.end.minutes_since_midnight,
                                rb.block.label.clone(),
                            )
                        })
                        .collect(),
                );
            }
            architect::sleep(architect::Duration::from_secs(300)).await;
        }
    });

    // The timer: shown while one runs, ended when none does.
    let timer_key = active.as_ref().map(|a| {
        (
            a.session.id,
            crate::chrome::session_title(&a.session),
            a.session.start_time.timestamp(),
        )
    });
    let mut last_timer = use_signal(|| None::<Option<(uuid::Uuid, String, i64)>>);
    if *last_timer.peek() != Some(timer_key.clone()) {
        last_timer.set(Some(timer_key.clone()));
        if let Some(s) = sink() {
            match &timer_key {
                Some((_, title, started)) => (s.timer_show)(title, *started as f64),
                None => (s.timer_end)(),
            }
        }
    }

    // The widgets: open tasks due or scheduled today, and the blocks.
    let _ = tick();
    let today = chrono::Local::now().date_naive();
    let open_today = tasks
        .value()
        .map(|rows| {
            crate::stores::unique_tasks(rows)
                .into_iter()
                .filter(|r| {
                    task_proto::status_is_open(&r.task.status)
                        && task_ui::task_section(&r.task, today) == task_ui::TaskSection::Today
                })
                .count()
        })
        .unwrap_or(0);
    let minute = {
        use chrono::Timelike as _;
        let n = chrono::Local::now();
        u16::try_from(n.hour() * 60 + n.minute()).unwrap_or(0)
    };
    let (now, next) = now_and_next(&blocks.read(), minute);
    let widget_key = (open_today, now.clone(), next.clone());
    let mut last_widgets = use_signal(|| None::<(usize, String, String)>);
    if last_widgets.peek().as_ref() != Some(&widget_key) {
        last_widgets.set(Some(widget_key));
        if let Some(s) = sink() {
            (s.widgets)(i32::try_from(open_today).unwrap_or(i32::MAX), &now, &next);
        }
    }
    rsx! {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_now_and_the_next() {
        let blocks = vec![
            (9 * 60, 12 * 60 + 30, "Studio block".to_owned()),
            (12 * 60 + 30, 13 * 60 + 30, "Lunch".to_owned()),
        ];
        let (now, next) = now_and_next(&blocks, 10 * 60);
        assert_eq!(now, "Studio block · until 12:30");
        assert_eq!(next, "Lunch · 12:30");
        let (now, next) = now_and_next(&blocks, 8 * 60);
        assert_eq!(now, "Free");
        assert_eq!(next, "Studio block · 9:00");
        let (_, next) = now_and_next(&blocks, 14 * 60);
        assert_eq!(next, "");
    }
}
