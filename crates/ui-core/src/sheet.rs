//! The mobile bottom sheet, and the parts sheets are built from.
//!
//! Here rather than in the shell's mobile chrome because it is a
//! presentational primitive, not chrome: it takes no `Route`, reads no
//! shell context, and knows nothing about navigation. It sat next to
//! [`BottomTabBar`], which genuinely *is* chrome, and was mislocated by
//! association — which only showed up when an app outside the shell
//! needed one and could not reach it.
//!
//! The look is Linear's iOS sheets: a panel floating a few points off
//! the screen's edges with a large radius, a grab handle, and content in
//! **grouped inset rows** ([`SheetGroup`]) — a label on the left, the
//! value on the right ([`SheetSelect`] opens a check-marked popover,
//! [`SheetToggle`] is a switch), chips for multi-pick sets
//! ([`SheetChip`]), and a search field over a checkbox list for filters
//! ([`SheetSearch`], [`SheetCheckRow`]). Every slide-up menu in the app
//! is made of these, so they all read as one system.

use architect_ui::lucide_dioxus::{Check, ChevronsUpDown, ListFilter, X};
use dioxus::prelude::*;

/// Mobile bottom sheet: scrim + a rounded panel floating at the bottom
/// edge, safe-area padded, scrollable past `85vh`. Closes on scrim tap,
/// Escape, or the close button (≥44px hit area). An empty `title` drops
/// the title row — the content (a search field, a group) leads instead.
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
            class: "fixed inset-0 z-50 bg-black/55 supports-[backdrop-filter]:backdrop-blur-[2px] md:hidden",
            onclick: move |_| on_close.call(()),
        }
        div {
            class: "fixed inset-x-2 z-50 flex max-h-[85vh] flex-col overflow-hidden rounded-[30px] bg-popover text-foreground shadow-[0_24px_60px_-12px_rgb(0_0_0/0.7),inset_0_0_0_1px_rgb(255_255_255/0.06)] outline-none md:hidden",
            style: "bottom: max(0.5rem, env(safe-area-inset-bottom, 0px));",
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
            div { class: "mx-auto mt-2 h-[5px] w-9 shrink-0 rounded-full bg-foreground/20" }
            if !title.is_empty() {
                div { class: "flex shrink-0 items-center justify-between gap-2 pl-5 pr-3 pt-1.5",
                    h2 { class: "text-[17px] font-semibold tracking-[-0.01em] text-foreground",
                        "{title}"
                    }
                    SheetClose { on_close }
                }
            }
            div { class: "min-h-0 flex-1 overflow-y-auto overflow-x-hidden px-3 pb-3 pt-2",
                {children}
            }
        }
    }
}

/// The round ✕ a sheet closes with.
#[component]
pub fn SheetClose(on_close: EventHandler<()>) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "flex size-11 shrink-0 items-center justify-center rounded-full bg-foreground/[0.07] text-foreground transition-transform duration-150 active:scale-95",
            aria_label: "Close",
            onclick: move |_| on_close.call(()),
            X { size: 18 }
        }
    }
}

/// A grouped inset list: rows on one rounded surface, hairlines between.
#[component]
pub fn SheetGroup(children: Element) -> Element {
    rsx! {
        div { class: "mb-3 flex flex-col divide-y divide-foreground/[0.06] rounded-[20px] bg-foreground/[0.05]",
            {children}
        }
    }
}

/// A small heading above a group ("Type", "Row properties"), with an
/// optional trailing action ("Reset").
#[component]
pub fn SheetSection(
    label: String,
    #[props(default)] action: Option<(String, EventHandler<()>)>,
) -> Element {
    rsx! {
        div { class: "flex items-center justify-between px-2 pb-2 pt-1",
            span { class: "text-[13px] font-medium text-muted-foreground", "{label}" }
            if let Some((text, on)) = action {
                button {
                    r#type: "button",
                    class: "text-[13px] text-muted-foreground/70 active:text-foreground",
                    onclick: move |_| on.call(()),
                    "{text}"
                }
            }
        }
    }
}

/// One row: label left, anything right.
#[component]
pub fn SheetRow(label: String, children: Element) -> Element {
    rsx! {
        div { class: "relative flex min-h-[54px] items-center justify-between gap-3 px-4 text-[16px] text-foreground",
            span { class: "min-w-0 truncate", "{label}" }
            div { class: "flex shrink-0 items-center", {children} }
        }
    }
}

/// A row whose value is one of `options` (`(value, label)`): the current
/// label with ⌃⌄, and a check-marked popover to change it.
#[component]
pub fn SheetSelect(
    label: String,
    value: String,
    options: Vec<(String, String)>,
    on_change: EventHandler<String>,
) -> Element {
    let mut open = use_signal(|| false);
    let current = options
        .iter()
        .find(|(v, _)| *v == value)
        .map_or_else(|| value.clone(), |(_, l)| l.clone());
    rsx! {
        div { class: "relative",
            button {
                r#type: "button",
                class: "flex min-h-[54px] w-full items-center justify-between gap-3 px-4 text-left text-[16px] text-foreground active:bg-foreground/[0.04]",
                onclick: move |_| open.toggle(),
                span { class: "min-w-0 truncate", "{label}" }
                span { class: "flex shrink-0 items-center gap-1 text-muted-foreground",
                    "{current}"
                    ChevronsUpDown { size: 15 }
                }
            }
            if open() {
                // Click-away layer, then the menu over the row's value.
                div {
                    class: "fixed inset-0 z-10",
                    onclick: move |_| open.set(false),
                }
                div { class: "absolute right-2 top-1 z-20 flex min-w-[13rem] flex-col rounded-[22px] bg-popover py-1.5 shadow-[0_20px_50px_-10px_rgb(0_0_0/0.75),inset_0_0_0_1px_rgb(255_255_255/0.08)]",
                    for (v , l) in options.iter().cloned() {
                        {
                            let picked = v == value;
                            rsx! {
                                button {
                                    key: "{v}",
                                    r#type: "button",
                                    class: "flex h-11 items-center gap-2.5 px-4 text-left text-[16px] text-foreground active:bg-foreground/[0.06]",
                                    onclick: move |_| {
                                        open.set(false);
                                        on_change.call(v.clone());
                                    },
                                    span { class: "flex w-4 justify-center",
                                        if picked {
                                            Check { size: 16 }
                                        }
                                    }
                                    "{l}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// An on/off switch, iOS-sized.
#[component]
pub fn Switch(on: bool, on_toggle: EventHandler<bool>, label: String) -> Element {
    let track = if on { "bg-primary" } else { "bg-foreground/15" };
    let knob = if on { "translate-x-5" } else { "translate-x-0" };
    rsx! {
        button {
            r#type: "button",
            role: "switch",
            aria_checked: "{on}",
            aria_label: "{label}",
            class: "relative h-[31px] w-[51px] shrink-0 rounded-full p-[2px] transition-colors duration-200 {track}",
            onclick: move |_| on_toggle.call(!on),
            span { class: "block size-[27px] rounded-full bg-white shadow-[0_2px_6px_rgb(0_0_0/0.3)] transition-transform duration-200 {knob}" }
        }
    }
}

/// A row with a [`Switch`].
#[component]
pub fn SheetToggle(
    label: String,
    on: bool,
    on_toggle: EventHandler<bool>,
    #[props(default)] hint: Option<String>,
) -> Element {
    rsx! {
        div { class: "flex min-h-[54px] items-center justify-between gap-3 px-4 py-2",
            div { class: "flex min-w-0 flex-col",
                span { class: "truncate text-[16px] text-foreground", "{label}" }
                if let Some(h) = hint {
                    span { class: "truncate text-[13px] text-muted-foreground", "{h}" }
                }
            }
            Switch { on, on_toggle, label: label.clone() }
        }
    }
}

/// A pill in a multi-pick set (row properties, quick filters).
#[component]
pub fn SheetChip(label: String, on: bool, on_toggle: EventHandler<bool>) -> Element {
    let cls = if on {
        "bg-foreground/[0.12] text-foreground shadow-[inset_0_0_0_1px_rgb(255_255_255/0.08)]"
    } else {
        "text-muted-foreground shadow-[inset_0_0_0_1px_rgb(255_255_255/0.1)]"
    };
    rsx! {
        button {
            r#type: "button",
            aria_pressed: "{on}",
            class: "h-10 rounded-full px-4 text-[15px] transition-colors active:scale-[0.97] {cls}",
            onclick: move |_| on_toggle.call(!on),
            "{label}"
        }
    }
}

/// The filter sheet's search field, with the sheet's ✕ beside it.
#[component]
pub fn SheetSearch(
    value: String,
    placeholder: String,
    on_input: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "flex items-center gap-2.5 pb-3",
            label { class: "flex h-12 min-w-0 flex-1 items-center gap-2.5 rounded-full bg-foreground/[0.07] px-4 text-muted-foreground",
                ListFilter { size: 18 }
                input {
                    class: "min-w-0 flex-1 bg-transparent text-[16px] text-foreground outline-none placeholder:text-muted-foreground",
                    placeholder: "{placeholder}",
                    value: "{value}",
                    oninput: move |e| on_input.call(e.value()),
                }
            }
            SheetClose { on_close }
        }
    }
}

/// A checkbox row in a filter list.
#[component]
pub fn SheetCheckRow(
    label: String,
    on: bool,
    on_toggle: EventHandler<bool>,
    #[props(default)] lead: Option<Element>,
) -> Element {
    let boxed = if on {
        "bg-primary text-primary-foreground shadow-none"
    } else {
        "shadow-[inset_0_0_0_1.5px_rgb(255_255_255/0.18)]"
    };
    rsx! {
        button {
            r#type: "button",
            role: "checkbox",
            aria_checked: "{on}",
            class: "flex min-h-[54px] w-full items-center gap-3.5 px-4 text-left text-[16px] text-foreground active:bg-foreground/[0.04]",
            onclick: move |_| on_toggle.call(!on),
            span { class: "flex size-[22px] shrink-0 items-center justify-center rounded-[7px] {boxed}",
                if on {
                    Check { size: 15 }
                }
            }
            if let Some(l) = lead {
                {l}
            }
            span { class: "min-w-0 flex-1 truncate", "{label}" }
        }
    }
}
