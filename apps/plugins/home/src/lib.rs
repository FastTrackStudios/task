//! Home, as a Task app — where the org's things are, and what they are.
//!
//! The first app with more than one screen, and the reason it is one
//! app rather than two: an inventory item names the location it sits
//! in, so the two registers are one domain with two views of it.
//! Splitting them would have meant one app depending on another's
//! rows, which the seam has no way to express — and no reason to,
//! since `home` is a single id in Task's catalog and turns on and off
//! as one thing.
//!
//! Both registers keep their state in their own crates; this mounts
//! both stores at the app root through one `provide`.

use task_plugin_ui::architect_ui::lucide_dioxus::{MapPin, Package};
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: "home",
    version: env!("CARGO_PKG_VERSION"),
    nav: &[
        PluginNav {
            label: "Locations",
            icon: icon_locations,
            path: "",
            rail: false,
        },
        PluginNav {
            label: "Inventory",
            icon: icon_inventory,
            path: "inventory",
            rail: false,
        },
    ],
    view: view,
    provide: Some(provide),
    widgets: None,
    fences: None,
    claim_link: None,
    claim_href: None,
};

/// Both registers' stores, at the app root.
///
/// Order is not significant — they are independent contexts — but the
/// count is: this runs during the root render, so it must call the
/// same hooks every time. It does, because the app list is fixed
/// before launch.
fn provide() {
    locations_ui::provide_stores();
    inventory_ui::provide_stores();
}

fn icon_locations() -> Element {
    rsx! { MapPin { size: 16 } }
}

fn icon_inventory() -> Element {
    rsx! { Package { size: 16 } }
}

fn view(path: &str, _query: &str) -> Option<Element> {
    match path {
        "" => Some(rsx! { locations_ui::LocationsView {} }),
        "inventory" => Some(rsx! { inventory_ui::InventoryView {} }),
        _ => None,
    }
}
