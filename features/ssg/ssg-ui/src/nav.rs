//! Getting around a vault: contents, chapter order, backlinks.
//!
//! All three navigate with plain `<a href>` rather than the router's
//! `Link`: `Link` is generic over the site's own `Routable` enum, and a
//! crate shared across four sites cannot name it. The anchors work
//! identically before and after hydration, which is the property that
//! matters for a pre-rendered page — see the crate docs.

use dioxus::prelude::*;
use ssg_vault::{StaticPage, StaticVault};

/// The table of contents, in reading order, under its stage headings.
///
/// A stage heading is emitted wherever the `stage:` frontmatter changes,
/// so the order of the headings *is* the reading order and a page cannot
/// appear under a stage it does not belong to. A vault whose notes carry
/// no `stage:` renders as one flat list, with no empty heading.
#[component]
pub fn VaultToc(
    /// The vault to list.
    vault: StaticVault,
    /// Slug of the page being read, marked `ssg-toc-current`.
    current: &'static str,
    /// URL prefix the vault is published under, e.g. `/guide`.
    base: &'static str,
    /// Class for the wrapping `<nav>`.
    #[props(default = "ssg-toc")]
    class: &'static str,
) -> Element {
    rsx! {
        nav { class,
            for (stage , pages) in vault.stages() {
                if !stage.is_empty() {
                    span { class: "ssg-toc-stage", "{stage}" }
                }
                for page in pages {
                    a {
                        key: "{page.slug}",
                        href: "{base}/{page.slug}",
                        class: if page.slug == current { "ssg-toc-link ssg-toc-current" } else { "ssg-toc-link" },
                        // `aria-current` is what a screen reader
                        // announces; the class is only paint.
                        aria_current: if page.slug == current { "page" },
                        "{page.title}"
                    }
                }
            }
        }
    }
}

/// Previous and next in reading order.
///
/// Renders nothing at all when the vault has one page — a nav with two
/// empty slots reads as broken, where absence reads as "there is only
/// this".
#[component]
pub fn ChapterNav(
    /// The vault being read.
    vault: StaticVault,
    /// Slug of the current page.
    current: &'static str,
    /// URL prefix the vault is published under.
    base: &'static str,
    /// Class for the wrapping `<nav>`.
    #[props(default = "ssg-chapter-nav")]
    class: &'static str,
) -> Element {
    let previous = vault.previous(current);
    let next = vault.next(current);
    if previous.is_none() && next.is_none() {
        return rsx! {};
    }

    rsx! {
        nav { class,
            if let Some(page) = previous {
                a { href: "{base}/{page.slug}", rel: "prev", class: "ssg-chapter-prev",
                    span { class: "ssg-chapter-label", "Previous" }
                    span { class: "ssg-chapter-title", "{page.title}" }
                }
            }
            if let Some(page) = next {
                a { href: "{base}/{page.slug}", rel: "next", class: "ssg-chapter-next",
                    span { class: "ssg-chapter-label", "Next" }
                    span { class: "ssg-chapter-title", "{page.title}" }
                }
            }
        }
    }
}

/// The pages that link here.
///
/// Backlinks are what make a vault read as a vault rather than a chapter
/// list: they are the half of a cross-reference the author did not have
/// to write. Nothing renders when there are none.
#[component]
pub fn Backlinks(
    /// The vault being read.
    vault: StaticVault,
    /// Slug of the current page.
    current: &'static str,
    /// URL prefix the vault is published under.
    base: &'static str,
    /// Heading above the list.
    #[props(default = "Referenced by")]
    heading: &'static str,
    /// Class for the wrapping `<aside>`.
    #[props(default = "ssg-backlinks")]
    class: &'static str,
) -> Element {
    let pages: Vec<&'static StaticPage> = vault.backlinks(current);
    if pages.is_empty() {
        return rsx! {};
    }

    rsx! {
        aside { class,
            h2 { class: "ssg-backlinks-heading", "{heading}" }
            ul {
                for page in pages {
                    li { key: "{page.slug}",
                        a { href: "{base}/{page.slug}", "{page.title}" }
                        if !page.summary.is_empty() {
                            span { class: "ssg-backlinks-summary", " — {page.summary}" }
                        }
                    }
                }
            }
        }
    }
}
