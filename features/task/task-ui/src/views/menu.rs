//! A small choice menu built from `div`s and buttons — what a `<select>`
//! was, in a form every renderer draws: Blitz has no `<select>` (nor
//! `change` events), and a native one ignores the theme in a browser.
//!
//! A button names the current choice; pressing it opens the list under
//! it, right-aligned to the button so it never runs off the right edge
//! of the row it sits at the end of. Picking closes it.

use architect_ui::lucide_dioxus::{Check, ChevronDown};
use dioxus::prelude::*;

#[component]
pub fn ChoiceMenu(
    /// The accessible name of the control ("Project").
    aria_label: String,
    /// What the button says when nothing is chosen ("No project").
    empty_label: String,
    /// `(id, label)` rows, in order.
    options: Vec<(String, String)>,
    /// The chosen id; `None` shows `empty_label`.
    #[props(default)]
    selected: Option<String>,
    /// Offer a first row that clears the choice (reports `""`).
    #[props(default)]
    clearable: bool,
    on_pick: EventHandler<String>,
) -> Element {
    let mut open = use_signal(|| false);
    let shown = selected
        .as_ref()
        .and_then(|id| options.iter().find(|(o, _)| o == id))
        .map_or_else(|| empty_label.clone(), |(_, label)| label.clone());
    let mut rows: Vec<(String, String)> = Vec::with_capacity(options.len() + 1);
    if clearable {
        rows.push((String::new(), empty_label.clone()));
    }
    rows.extend(options);

    rsx! {
        div { class: "relative shrink-0",
            button {
                r#type: "button",
                aria_label: "{aria_label}",
                aria_haspopup: "listbox",
                aria_expanded: open().to_string(),
                class: "flex max-w-[12rem] items-center justify-start gap-1 overflow-hidden whitespace-nowrap rounded-md border border-border/60 bg-transparent px-1.5 py-0.5 text-xs text-muted-foreground transition-colors hover:border-border hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                onclick: move |e: MouseEvent| {
                    e.stop_propagation();
                    open.toggle();
                },
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Escape {
                        open.set(false);
                    }
                },
                span { class: "min-w-0 overflow-hidden whitespace-nowrap", "{shown}" }
                span { class: "flex shrink-0 items-center", ChevronDown { size: 12 } }
            }
            if open() {
                // Click-away. Page-wide in a browser; under Blitz `fixed`
                // is absolute to the parent, so there the button (or a
                // pick, or Esc) closes it.
                div {
                    class: "fixed inset-0 z-30",
                    onclick: move |e: MouseEvent| {
                        e.stop_propagation();
                        open.set(false);
                    },
                }
                div {
                    class: "absolute right-0 top-full z-40 mt-1 flex max-h-64 w-56 flex-col overflow-y-auto rounded-lg border border-border bg-popover p-1 text-popover-foreground shadow-xl",
                    role: "listbox",
                    aria_label: "{aria_label}",
                    onclick: move |e: MouseEvent| e.stop_propagation(),
                    for (id, label) in rows {
                        {
                            let chosen = selected.as_deref().unwrap_or("") == id.as_str();
                            rsx! {
                                button {
                                    key: "{id}",
                                    r#type: "button",
                                    role: "option",
                                    aria_selected: chosen.to_string(),
                                    class: "flex min-h-8 w-full items-center justify-start gap-2 rounded-md px-2 text-left text-[13px] text-foreground hover:bg-accent",
                                    onclick: move |_| {
                                        open.set(false);
                                        on_pick.call(id.clone());
                                    },
                                    span { class: "min-w-0 flex-1 overflow-hidden whitespace-nowrap", "{label}" }
                                    if chosen {
                                        span { class: "flex shrink-0 items-center", Check { size: 13 } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
