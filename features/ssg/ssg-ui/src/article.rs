//! One note's prose.

use dioxus::prelude::*;
use ssg_vault::StaticPage;

/// A note, rendered.
///
/// `dangerous_inner_html` is the right tool here and not a shortcut. The
/// HTML was produced by `ssg-vault` at build time from markdown that is
/// in the repository — it is the site's own text, compiled in the same
/// build as the binary, and there is no path by which a reader's input
/// reaches it. The alternative, walking the markdown into an rsx tree at
/// runtime, would ship a markdown parser to every visitor to reproduce a
/// result already known at build time.
#[component]
pub fn VaultArticle(
    /// The page to render.
    page: &'static StaticPage,
    /// Class for the wrapping `<article>`. Defaults to `ssg-article`.
    #[props(default = "ssg-article")]
    class: &'static str,
) -> Element {
    rsx! {
        article { class,
            div { class: "ssg-prose", dangerous_inner_html: "{page.html}" }
        }
    }
}
