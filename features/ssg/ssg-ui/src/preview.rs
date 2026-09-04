//! Hover a link into the vault, see where it goes.
//!
//! Quartz's popover, and the reason it is worth having: a guide is full
//! of cross-references, and following one costs you your place. A card
//! that says what is on the other side lets a reader decide *not* to.
//!
//! ## How it works, and what it costs
//!
//! `ssg-build` marks every resolved wikilink with `data-ssg-link="slug"`
//! at build time. [`LinkPreviews`] wraps a region of the page and
//! listens for hover on it — one listener on the container rather than
//! one per link, because the links live inside opaque HTML and there is
//! no element here to bind to.
//!
//! The card's content costs nothing to fetch: the target page is already
//! `&'static` in the binary, so its title and summary are a lookup. Like
//! [`crate::search`], there is no index and no request.
//!
//! Nothing here runs before the bundle does, and nothing about the page
//! depends on it: with no JavaScript the links are ordinary links.

use dioxus::prelude::*;
use ssg_vault::StaticVault;

/// Wrap content so links into the vault preview on hover.
///
/// The wrapper renders its children unchanged; everything it adds is a
/// hover listener and a card that is absent until something is hovered.
#[component]
pub fn LinkPreviews(
    /// The vault the links point into.
    vault: StaticVault,
    /// The content — typically a rendered note.
    children: Element,
    /// Class for the wrapping element.
    #[props(default = "ssg-previews")]
    class: &'static str,
) -> Element {
    // The slug under the cursor, and where to draw its card.
    let mut hovered = use_signal(|| Option::<(String, f64, f64)>::None);

    rsx! {
        div {
            class,
            onmouseover: move |event| {
                match hovered_slug(&event) {
                    Some(slug) => {
                        let point = event.data().client_coordinates();
                        hovered.set(Some((slug, point.x, point.y)));
                    }
                    // Moving off a link and onto ordinary prose closes
                    // the card. Without this it would stay up until the
                    // pointer found another link.
                    None => hovered.set(None),
                }
            },
            onmouseleave: move |_| hovered.set(None),

            {children}

            if let Some((slug, x, y)) = hovered() {
                if let Some(page) = vault.page(&slug) {
                    div {
                        class: "ssg-preview",
                        // Positioned against the viewport, because the
                        // card is `position: fixed` — the alternative is
                        // measuring an offset parent that the site owns
                        // and this component does not.
                        style: "left: {x}px; top: {y}px;",
                        // Not interactive: the card is a glance, and a
                        // hoverable card that sits under the pointer
                        // fights the link it is describing.
                        "aria-hidden": "true",
                        span { class: "ssg-preview-title", "{page.title}" }
                        if !page.summary.is_empty() {
                            span { class: "ssg-preview-summary", "{page.summary}" }
                        }
                        span { class: "ssg-preview-meta",
                            "{page.reading_minutes()} min"
                            if !page.tags.is_empty() {
                                " · "
                                {page.tags.join(", ")}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The vault slug of the link under the pointer, if there is one.
///
/// Walks up from whatever was hovered to the nearest `[data-ssg-link]`,
/// so hovering the bold text *inside* a link still counts.
#[cfg(target_arch = "wasm32")]
fn hovered_slug(event: &Event<MouseData>) -> Option<String> {
    use wasm_bindgen::JsCast as _;

    let mouse = event.data().downcast::<web_sys::MouseEvent>()?.clone();
    let target = mouse.target()?.dyn_into::<web_sys::Element>().ok()?;
    target
        .closest("[data-ssg-link]")
        .ok()??
        .get_attribute("data-ssg-link")
}

/// Off the web there is no DOM to read the pointer out of — and this
/// crate still has to compile for the host, because that is where
/// `dx build --ssg` renders the page.
#[cfg(not(target_arch = "wasm32"))]
const fn hovered_slug(_event: &Event<MouseData>) -> Option<String> {
    None
}
