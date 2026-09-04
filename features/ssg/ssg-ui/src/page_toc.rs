//! The other table of contents: the one *inside* a page.
//!
//! [`VaultToc`](crate::VaultToc) lists the vault's pages. This lists one
//! page's headings, which is the thing a long note needs and a short one
//! does not — so it renders nothing below a threshold rather than
//! putting a one-item list beside a two-paragraph page.
//!
//! Links are `#fragment`s into the page that is already loaded, so they
//! work with no wasm and no router: the browser scrolls. Which is also
//! why the headings had to be given `id`s at build time.

use dioxus::prelude::*;
use ssg_vault::StaticPage;

/// A page's headings, as links into it.
#[component]
pub fn PageToc(
    /// The page to outline.
    page: &'static StaticPage,
    /// Heading levels to include, inclusive. `(2, 3)` by default: `#` is
    /// the page's own title and belongs in the outline no more than a
    /// book's cover belongs in its contents.
    #[props(default = (2, 3))]
    levels: (u8, u8),
    /// Below this many headings, render nothing. A contents list needs
    /// to save the reader a scroll, and two entries do not.
    #[props(default = 3)]
    minimum: usize,
    /// Heading above the list.
    #[props(default = "On this page")]
    heading: &'static str,
    /// Class for the wrapping `<nav>`.
    #[props(default = "ssg-page-toc")]
    class: &'static str,
) -> Element {
    let shown: Vec<&'static ssg_vault::StaticHeading> = page
        .headings
        .iter()
        .filter(|h| h.level >= levels.0 && h.level <= levels.1)
        .collect();

    if shown.len() < minimum {
        return rsx! {};
    }

    rsx! {
        nav { class, aria_label: heading,
            span { class: "ssg-page-toc-heading", "{heading}" }
            ul {
                for entry in shown {
                    li {
                        key: "{entry.id}",
                        // The level as a data attribute rather than
                        // nested lists: the depth is one CSS rule away,
                        // and nesting would need a stack to build from a
                        // flat list that can skip a level.
                        "data-level": "{entry.level}",
                        a { href: "#{entry.id}", "{entry.text}" }
                    }
                }
            }
        }
    }
}
