//! Tags — the vault's cross-cutting axis.
//!
//! Reading order is one path through a vault: the one the author chose.
//! Tags are the other: a reader who wants "everything about tempo" is
//! not asking for a chapter range. A vault that carries no `tags:` in
//! its frontmatter renders none of this, which is why these components
//! are safe to place unconditionally.

use dioxus::prelude::*;
use ssg_vault::{StaticPage, StaticVault};

/// One page's tags.
#[component]
pub fn PageTags(
    /// The page whose tags to list.
    page: &'static StaticPage,
    /// URL prefix a tag links to — `{base}/{tag}`. Without one the tags
    /// render as plain labels, which is right for a site that has no
    /// tag pages to link to.
    #[props(default)]
    base: Option<&'static str>,
    /// Class for the wrapping element.
    #[props(default = "ssg-tags")]
    class: &'static str,
) -> Element {
    if page.tags.is_empty() {
        return rsx! {};
    }

    rsx! {
        ul { class,
            for tag in page.tags {
                li { key: "{tag}", class: "ssg-tag",
                    if let Some(base) = base {
                        a { href: "{base}/{tag}", "#{tag}" }
                    } else {
                        span { "#{tag}" }
                    }
                }
            }
        }
    }
}

/// Every tag in the vault, with how many pages carry it.
///
/// The index a `/tags` route renders. Ordered alphabetically rather than
/// by count: a tag list is something you scan for a word you already
/// have in mind, not a chart of what the author wrote most about.
#[component]
pub fn TagIndex(
    /// The vault to summarise.
    vault: StaticVault,
    /// URL prefix a tag links to — `{base}/{tag}`.
    base: &'static str,
    /// Class for the wrapping element.
    #[props(default = "ssg-tag-index")]
    class: &'static str,
) -> Element {
    let tags = vault.tags();
    if tags.is_empty() {
        return rsx! {};
    }

    rsx! {
        ul { class,
            for (tag , count) in tags {
                li { key: "{tag}", class: "ssg-tag",
                    a { href: "{base}/{tag}",
                        "#{tag}"
                        span { class: "ssg-tag-count", " {count}" }
                    }
                }
            }
        }
    }
}

/// The pages carrying one tag.
#[component]
pub fn TaggedPages(
    /// The vault to search.
    vault: StaticVault,
    /// The tag.
    tag: String,
    /// URL prefix the vault is published under.
    base: &'static str,
    /// Class for the wrapping list.
    #[props(default = "ssg-tagged")]
    class: &'static str,
) -> Element {
    let pages = vault.tagged(&tag);

    rsx! {
        if pages.is_empty() {
            p { class: "ssg-tagged-empty", "Nothing is tagged #{tag}." }
        } else {
            ul { class,
                for page in pages {
                    li { key: "{page.slug}",
                        a { href: "{base}/{page.slug}", "{page.title}" }
                        if !page.summary.is_empty() {
                            span { class: "ssg-tagged-summary", " — {page.summary}" }
                        }
                    }
                }
            }
        }
    }
}
