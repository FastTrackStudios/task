//! The set of apps a Task build ships with.
//!
//! Task has three front ends — desktop, web, mobile — and they all want
//! the same apps. Written out in each `main.rs`, that list drifts: an
//! app gets added to desktop, the web build quietly does not have it,
//! and the symptom is a link that works on one machine and 404s on
//! another. So the list lives once, here, and each binary calls
//! [`register_all`].
//!
//! This crate is a composition root, and the only kind of crate allowed
//! to be: it names both the SDK and every app. Nothing below it names
//! anything in either direction — `task-ui` has not heard of scripture,
//! and `task-plugin-scripture` has not heard of `task-ui`. That is the
//! property the whole seam exists to hold, and this is where the wires
//! are allowed to meet.
//!
//! A build that wants a different set (a kiosk, an embedded viewer, a
//! test) calls [`task_plugin_ui::register`] itself and skips this.

/// Register every app this build ships.
///
/// Call before launch — the nav is built on first render, and an app
/// registered after that renders nothing until something else redraws.
///
/// Registering the same id twice keeps the last, so a binary may call
/// this and then override one app with its own build.
pub fn register_all() {
    task_plugin_ui::register(task_plugin_cooking::APP);
    task_plugin_ui::register(task_plugin_scripture::APP);
    task_plugin_ui::register(task_plugin_email::APP);
}

/// What [`register_all`] just installed, as `id@version` — for the log
/// line at startup.
///
/// Worth saying out loud once per launch: apps release from their own
/// repositories, so two machines on the same Task can be running
/// different versions of Session with nothing on screen to show for it.
#[must_use]
pub fn stamps() -> Vec<String> {
    task_plugin_ui::registered()
        .iter()
        .map(task_plugin_ui::PluginApp::stamp)
        .collect()
}

#[cfg(test)]
mod tests {
    /// The drift this crate exists to prevent: an app added to the
    /// bundle but reachable from only some of the front ends. If this
    /// list changes, it changed for every binary at once.
    #[test]
    fn the_shipped_set_is_registered_once_for_everybody() {
        super::register_all();
        let mut ids: Vec<_> = task_plugin_ui::installed()
            .into_iter()
            .map(|(id, _version)| id)
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, ["email", "mealplan", "scripture"]);
    }

    #[test]
    fn registering_twice_is_not_two_copies() {
        super::register_all();
        super::register_all();
        assert_eq!(task_plugin_ui::installed().len(), 3);
    }
}
