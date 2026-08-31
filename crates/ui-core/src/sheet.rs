//! The mobile bottom sheet.
//!
//! Here rather than in the shell's mobile chrome because it is a
//! presentational primitive, not chrome: it takes no `Route`, reads no
//! shell context, and knows nothing about navigation. It sat next to
//! [`BottomTabBar`], which genuinely *is* chrome, and was mislocated by
//! association — which only showed up when an app outside the shell
//! needed one and could not reach it.

use dioxus::prelude::*;

/// Mobile bottom sheet: scrim + a rounded panel pinned to the bottom
/// edge, safe-area padded, scrollable past `85vh`. Closes on scrim
/// tap, Escape, or the explicit close button (≥44px hit area).
/// Children are only mounted while open.
#[component]
pub fn BottomSheet(
    open: bool,
    on_close: EventHandler<()>,
    title: String,
    children: Element,
) -> Element {
    if !open {
        return rsx! {};
    }
    rsx! {
        div {
            class: "fixed inset-0 z-50 bg-black/60 supports-[backdrop-filter]:backdrop-blur-xs md:hidden",
            onclick: move |_| on_close.call(()),
        }
        div {
            class: "fixed inset-x-0 bottom-0 z-50 flex max-h-[85vh] flex-col rounded-t-2xl border-t border-border bg-background text-foreground shadow-2xl outline-none md:hidden",
            style: "padding-bottom: env(safe-area-inset-bottom, 0px);",
            tabindex: "-1",
            // `autofocus` only fires on initial page load, not when the
            // sheet is inserted dynamically — focus on mount so Escape
            // lands on the panel (same trick as the fleeting modal).
            onmounted: move |e: Event<MountedData>| {
                spawn(async move {
                    let _ = e.data().set_focus(true).await;
                });
            },
            onclick: move |e| e.stop_propagation(),
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    on_close.call(());
                }
            },
            // Grab handle.
            div { class: "mx-auto mt-2 h-1.5 w-10 shrink-0 rounded-full bg-muted" }
            div { class: "flex shrink-0 items-center justify-between gap-2 px-4 pb-1 pt-2",
                h2 { class: "text-sm font-semibold uppercase tracking-widest text-muted-foreground",
                    "{title}"
                }
                button {
                    r#type: "button",
                    class: "flex h-11 w-11 items-center justify-center rounded-full text-muted-foreground active:bg-accent active:text-foreground",
                    aria_label: "Close",
                    onclick: move |_| on_close.call(()),
                    "✕"
                }
            }
            div { class: "min-h-0 flex-1 overflow-y-auto overflow-x-hidden px-4 pb-4",
                {children}
            }
        }
    }
}
