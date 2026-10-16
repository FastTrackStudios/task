//! The lock screen's sink on iOS: `ui::live` tells it what the timer and
//! the widgets show; it calls the app's Swift half (`ios/TaskLive`,
//! linked by `build.rs`) — ActivityKit for the timer's Live Activity, an
//! App Group's defaults + WidgetKit for the widgets. No-op elsewhere.

#[cfg(target_os = "ios")]
mod ios {
    use std::ffi::{CString, c_char};

    unsafe extern "C" {
        fn task_live_timer_show(title: *const c_char, started_at: f64);
        fn task_live_timer_end();
        fn task_widgets_publish(tasks_today: i32, now: *const c_char, next: *const c_char);
    }

    fn c(s: &str) -> CString {
        CString::new(s.replace('\0', "")).unwrap_or_default()
    }

    fn timer_show(title: &str, started_at: f64) {
        let t = c(title);
        // SAFETY: a valid C string for the call's duration.
        unsafe { task_live_timer_show(t.as_ptr(), started_at) }
    }

    fn timer_end() {
        // SAFETY: no arguments.
        unsafe { task_live_timer_end() }
    }

    fn widgets(tasks_today: i32, now: &str, next: &str) {
        let (n, x) = (c(now), c(next));
        // SAFETY: valid C strings for the call's duration.
        unsafe { task_widgets_publish(tasks_today, n.as_ptr(), x.as_ptr()) }
    }

    pub fn init() {
        ui::live::set_sink(ui::live::Sink {
            timer_show,
            timer_end,
            widgets,
        });
    }
}

/// Register the iOS sink; no-op elsewhere.
pub fn init() {
    #[cfg(target_os = "ios")]
    ios::init();
}
