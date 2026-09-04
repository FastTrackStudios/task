//! Where you are in the vault.
//!
//! A vault is flat on disk — the slugs are the URLs and there is no
//! directory tree — so a crumb trail cannot come from the path. It comes
//! from the *stage*: the section of the reading order a note belongs to,
//! which is the structure the author actually declared.
//!
//! A vault whose notes carry no `stage:` renders one crumb, the vault
//! itself, which is the honest answer for a flat set of notes.

use dioxus::prelude::*;
use ssg_vault::StaticPage;

/// The trail to a page: the vault, its stage, and the page itself.
#[component]
pub fn Breadcrumbs(
    /// The page being read.
    page: &'static StaticPage,
    /// URL prefix the vault is published under.
    base: &'static str,
    /// What to call the vault in the first crumb.
    #[props(default = "Guide")]
    root: &'static str,
    /// Class for the wrapping `<nav>`.
    #[props(default = "ssg-breadcrumbs")]
    class: &'static str,
) -> Element {
    rsx! {
        nav { class, aria_label: "Breadcrumb",
            ol {
                li {
                    a { href: base, "{root}" }
                }
                // The stage is not a page, so it is not a link — there
                // is nothing to navigate to. A crumb that looks
                // clickable and is not is worse than plain text.
                if !page.stage.is_empty() {
                    li { span { "{page.stage}" } }
                }
                li {
                    // The last crumb is where you already are. `aria-current`
                    // says so; it is deliberately not a link to itself.
                    span { aria_current: "page", "{page.title}" }
                }
            }
        }
    }
}
