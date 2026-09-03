//! Bookings, as a Task app — the public front desk.
//!
//! An org publishes bookable event types ("30-minute consult") and
//! people book slots against them.
//!
//! Split out of core `scheduling`, which is the point of this app
//! existing at all. Day plans and calendar events are how *you*
//! organise your own time — something every org does, which is why
//! scheduling is core and a meal plan can rely on it. Selling time by
//! the slot is a different question, and most orgs never ask it. It was
//! only ever core by adjacency: it lived in the same service slice, so
//! it inherited the same id, so an org with no front desk had a front
//! desk anyway.
//!
//! Both its stores are live, off the one `SchedulingEvent` stream, so a
//! booking made on the public page shows up here without a refetch —
//! which is why `provide` installs them at the root rather than the
//! screen doing it on mount.

use bookings_ui::{BookingsView, provide_stores};
use task_plugin_ui::architect_ui::lucide_dioxus::CalendarClock;
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: bookings_ui::APP_ID,
    version: env!("CARGO_PKG_VERSION"),
    nav: &[PluginNav {
        label: "Bookings",
        icon,
        path: "",
        rail: false,
    }],
    view,
    panel: None,
    claim_file: None,
    provide: Some(provide),
    widgets: None,
    fences: None,
    claim_link: None,
    claim_href: None,
};

/// Both live stores, at the app root — their own wasm chunk on the
/// web, installed once it has downloaded.
fn provide() {
    task_plugin_ui::lazy_provide!("bookings_stores", provide_stores)
}

fn icon() -> Element {
    rsx! { CalendarClock { size: 16 } }
}

fn view(path: &str, query: &str) -> Option<Element> {
    // The screens are their own wasm chunk on the web, downloaded the
    // first time somebody opens this app; everything else the app
    // registers stays in the shell. A plain call everywhere else.
    task_plugin_ui::lazy_view!("bookings", bookings_screen, path, query)
}

fn bookings_screen(path: &str, _query: &str) -> Option<Element> {
    match path {
        "" => Some(rsx! { BookingsView {} }),
        _ => None,
    }
}
