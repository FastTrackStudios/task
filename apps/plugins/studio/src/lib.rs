//! FastTrackStudio, as a Task app — songs, setlists, charts, video.
//!
//! The first app whose main contribution is not a screen. Its screens
//! are almost beside the point: what it actually does is change what a
//! **note** is. A note with `type: song` becomes a multitrack player. A
//! `type: setlist` becomes a queue that owns the whole window. A ```kf
//! fence stops being source and becomes engraved notation.
//!
//! That makes it the first user of [`PluginApp::widgets`] and
//! [`PluginApp::fences`], which existed before anything filled them —
//! the shell named `task_player_ui` directly, in its widget roster and
//! its fence registration, which is precisely the coupling the widget
//! contribution was declared to remove.
//!
//! ## What is still the shell's
//!
//! The global now-playing surfaces — the headless playback engine, the
//! status-bar tab, the mobile float, the setlist-row highlighter — are
//! still mounted by the shell, and this app does not carry them.
//!
//! They are the one part that "off" already handles correctly by
//! accident: every one of them renders nothing until something plays,
//! and nothing can play unless a widget above matched a note. Turn this
//! app off and the widgets stop matching, so the chrome stays silent on
//! its own. Moving them would mean a second global-surface contribution
//! alongside `panel`, designed for one consumer, to fix a problem that
//! is not currently visible. It waits for a second app that wants a
//! persistent bar.

use resources_ui::WatchView;
use task_plugin_ui::architect_ui::lucide_dioxus::Youtube;
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: resources_ui::APP_ID,
    version: env!("CARGO_PKG_VERSION"),
    nav: &[PluginNav {
        label: "Watch",
        icon,
        path: "",
        rail: false,
    }],
    view,
    panel: None,
    claim_file: None,
    provide: None,
    // Song, setlist, and song/setlist embeds. The specs already carried
    // this app's id — they were tagged `.plugin("fasttrackstudio")`
    // long before there was a `PluginApp` to hang them on, which is as
    // clear a sign as any that this was always an app.
    widgets: Some(widgets),
    // ```kf fences, engraved rather than shown as source. The editor
    // renders fences through a registry precisely so that the notation
    // domain can sit above it; this is the registration that used to
    // happen in the shell.
    fences: Some(task_player_ui::register_chart_fences),
    claim_link: None,
    claim_href: None,
};

fn icon() -> Element {
    rsx! { Youtube { size: 16 } }
}

fn view(path: &str, query: &str) -> Option<Element> {
    match path {
        // `v` is the YouTube id, `node` the NodeRef token the
        // timestamped notes hang on. Both empty is the paste-a-URL
        // landing, which is why neither is required.
        "" => {
            let v = task_plugin_ui::query_param(query, "v").unwrap_or_default();
            let node = task_plugin_ui::query_param(query, "node").unwrap_or_default();
            Some(rsx! { WatchView { v, node } })
        }
        _ => None,
    }
}

/// Everything this app makes of a note: the player's song/setlist
/// widgets, and the watch screen for a `type: video` note.
///
/// Two crates' worth, one contribution — the shell asks the *app* what
/// its notes look like, not each crate behind it.
fn widgets() -> Vec<task_plugin_ui::task_widgets::WidgetSpec> {
    let mut all = task_player_ui::widgets();
    all.extend(resources_ui::widgets());
    all
}
