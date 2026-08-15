//! Hover-tooltip view layer.
//!
//! Ports CM6's `hoverTooltip` rendering (`view/src/tooltip.ts`) at v1 scope:
//! the editor's mousemove bridge resolves the pointer to a document offset,
//! runs the [`editor_state::HoverSource`], and stashes the result here as a
//! [`HoverPopup`]. This component renders it as a floating panel anchored to
//! the pointer, flipping/clamping at viewport edges (same JS approach as the
//! slash menu in [`crate::slash`]).

use dioxus::prelude::*;
use editor_state::HoverTooltip;

/// A resolved hover tooltip plus the viewport coords to anchor it at.
#[derive(Clone, Debug, PartialEq)]
pub struct HoverPopup {
    pub tip: HoverTooltip,
    /// Pointer position (viewport coords) the tooltip anchors near.
    pub x: f64,
    pub y: f64,
}

/// Floating hover-tooltip panel. Positioned imperatively after mount (the DOM
/// size isn't known until rendered), preferring below-right of the pointer and
/// flipping above/left when it would overflow the viewport.
#[component]
pub fn HoverTooltipView(popup: HoverPopup) -> Element {
    let x = popup.x;
    let y = popup.y;
    let html = popup.tip.html.clone();

    use_effect(move || {
        let script = format!(
            r"(()=>{{
                const t = document.querySelector('.editor-hover-tooltip');
                if (!t) return;
                const w = t.offsetWidth, h = t.offsetHeight;
                const vw = window.innerWidth, vh = window.innerHeight;
                let left = {x} + 12, top = {y} + 16;
                if (left + w > vw - 8) left = {x} - w - 12;
                if (top + h > vh - 8) top = {y} - h - 12;
                if (left < 8) left = 8;
                if (top < 8) top = 8;
                t.style.left = left + 'px';
                t.style.top = top + 'px';
            }})();"
        );
        let _ = document::eval(&script);
    });

    rsx! {
        div {
            class: "editor-hover-tooltip",
            dangerous_inner_html: "{html}",
        }
    }
}
