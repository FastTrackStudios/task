//! What the shell says when a plugin screen does not resolve.
//!
//! Three different nothings reach here, and the whole point of the page
//! is keeping them apart. "Not installed" is a build. "Turned off" is a
//! setting. "No such screen" is a bad link. A single "not found" would
//! send somebody to check the wrong one of those every time.

use architect_ui::prelude::*;
use dioxus::prelude::*;

#[component]
pub fn Missing(title: String, detail: String) -> Element {
    rsx! {
        section { class: "flex flex-col items-center justify-center gap-3 p-12 text-center",
            Heading { level: HeadingLevel::H2, "{title}" }
            Text { variant: TextVariant::Muted, class: "max-w-prose", "{detail}" }
        }
    }
}
