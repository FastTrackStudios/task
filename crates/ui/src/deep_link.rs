//! Links into the app: `app.fasttrackstudio.task://<where>`.
//!
//! What the iPhone's Action button, a Shortcut, a home-screen or
//! lock-screen widget opens. The host (the mobile app's event loop)
//! pushes each URL it is handed with [`push`]; [`DeepLinks`], mounted
//! once in the shell, takes them in order and goes there:
//!
//! | link | does |
//! |---|---|
//! | `…://capture` | opens fleeting capture |
//! | `…://search` | opens search across every vault and wiki |
//! | `…://tasks` | the task list |
//! | `…://calendar` | the day (the Planner) |
//! | `…://timer` | starts a timer now and opens it |
//!
//! `…://auth/…` is the sign-in callback, which the sign-in sheet takes
//! itself; it is ignored here.

use std::sync::Mutex;

use dioxus::prelude::*;

use crate::routes::Route;

static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Hand the app a URL it was opened with (any thread).
pub fn push(url: impl Into<String>) {
    if let Ok(mut q) = PENDING.lock() {
        q.push(url.into());
    }
}

fn take() -> Vec<String> {
    PENDING
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

/// What a link asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Capture,
    Search,
    Tasks,
    Calendar,
    Timer,
}

/// Read a link: its host (or first path segment) names the place.
#[must_use]
pub fn parse(url: &str) -> Option<Link> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let place = rest.trim_start_matches('/').split(['/', '?', '#']).next()?;
    match place.to_ascii_lowercase().as_str() {
        "capture" | "fleeting" => Some(Link::Capture),
        "search" => Some(Link::Search),
        "tasks" | "today" => Some(Link::Tasks),
        "calendar" | "schedule" | "plan" => Some(Link::Calendar),
        "timer" => Some(Link::Timer),
        _ => None,
    }
}

/// Takes queued links and goes where they say. Mount once, in the shell.
#[component]
pub fn DeepLinks() -> Element {
    let nav = use_navigator();
    let new_issue = use_context::<crate::new_issue::NewIssue>();
    let mut search = crate::notes_search::use_notes_search();
    let start_timer = crate::pages::timer_focus::use_start_timer_now();
    use_future(move || async move {
        loop {
            for url in take() {
                match parse(&url) {
                    Some(Link::Capture) => new_issue.capture(String::new()),
                    Some(Link::Search) => search.set(true),
                    Some(Link::Tasks) => {
                        nav.push(Route::TasksRoute {});
                    }
                    Some(Link::Calendar) => {
                        nav.push(Route::ScheduleRoute {});
                    }
                    Some(Link::Timer) => start_timer(String::new()),
                    None => {}
                }
            }
            architect::sleep(architect::Duration::from_millis(250)).await;
        }
    });
    rsx! {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_name_their_place() {
        assert_eq!(
            parse("app.fasttrackstudio.task://capture"),
            Some(Link::Capture)
        );
        assert_eq!(
            parse("app.fasttrackstudio.task://search?q=x"),
            Some(Link::Search)
        );
        assert_eq!(
            parse("app.fasttrackstudio.task:///tasks"),
            Some(Link::Tasks)
        );
        assert_eq!(
            parse("app.fasttrackstudio.task://calendar"),
            Some(Link::Calendar)
        );
        assert_eq!(parse("app.fasttrackstudio.task://timer"), Some(Link::Timer));
        assert_eq!(
            parse("app.fasttrackstudio.task://auth/callback?code=1"),
            None
        );
    }
}
