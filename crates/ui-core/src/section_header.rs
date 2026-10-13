//! A section's top bar, the Linear iOS way: the section's name, large,
//! on the left; on the right one pill holding the section's **main
//! action** (compose an issue, process the inbox) and a **⋯** that opens
//! everything else the section offers — Filter, Display options, and
//! whatever is particular to it.
//!
//! The ⋯ menu's items are children ([`SectionMenuItem`]), so each page
//! says what is in its own menu; an item closes the menu when picked.

use architect_ui::lucide_dioxus::Ellipsis;
use dioxus::prelude::*;

/// Whether the ⋯ menu is open — shared with its items so a pick closes it.
#[derive(Clone, Copy)]
struct MenuOpen(Signal<bool>);

/// The header. `action` is the main action's icon; `action_label` names
/// it for screen readers. `badge` marks the ⋯ (how many filters narrow
/// the view), so a filtered section is never a mystery.
#[component]
pub fn SectionHeader(
    title: String,
    action: Element,
    action_label: String,
    on_action: EventHandler<()>,
    /// Show the action's words beside its icon ("Process 5") — for a
    /// section whose main action wants saying, not just showing.
    #[props(default)]
    action_text: Option<String>,
    #[props(default)] badge: usize,
    /// The ⋯ menu's items.
    children: Element,
) -> Element {
    let mut open = use_signal(|| false);
    use_context_provider(|| MenuOpen(open));
    rsx! {
        header { class: "relative flex items-center justify-between gap-3 px-4 pb-2 pt-3",
            h1 { class: "min-w-0 truncate text-[30px] font-bold leading-9 tracking-[-0.025em] text-foreground",
                "{title}"
            }
            div { class: "flex h-12 shrink-0 items-center rounded-full bg-foreground/[0.06] px-1 shadow-[inset_0_0_0_1px_rgb(255_255_255/0.08),0_8px_24px_-12px_rgb(0_0_0/0.6)] supports-[backdrop-filter]:backdrop-blur-md",
                button {
                    r#type: "button",
                    class: if action_text.is_some() { "flex h-10 items-center gap-2 rounded-full pl-3 pr-3.5 text-[15px] font-semibold text-foreground transition-transform duration-150 active:scale-95 active:bg-foreground/10" } else { "flex size-10 items-center justify-center rounded-full text-foreground transition-transform duration-150 active:scale-90 active:bg-foreground/10" },
                    aria_label: "{action_label}",
                    title: "{action_label}",
                    onclick: move |_| on_action.call(()),
                    {action}
                    if let Some(t) = action_text.clone() {
                        span { class: "tabular-nums", "{t}" }
                    }
                }
                span { class: "h-5 w-px bg-foreground/10" }
                button {
                    r#type: "button",
                    class: "relative flex size-10 items-center justify-center rounded-full text-foreground transition-transform duration-150 active:scale-90 active:bg-foreground/10",
                    aria_label: "More",
                    aria_expanded: "{open()}",
                    onclick: move |_| open.toggle(),
                    Ellipsis { size: 20 }
                    if badge > 0 {
                        span { class: "absolute right-1 top-1 flex size-4 items-center justify-center rounded-full bg-primary text-[10px] font-semibold text-primary-foreground",
                            "{badge}"
                        }
                    }
                }
            }
            if open() {
                div {
                    class: "fixed inset-0 z-40",
                    onclick: move |_| open.set(false),
                }
                div { class: "absolute right-4 top-[3.9rem] z-50 flex min-w-[15rem] origin-top-right flex-col rounded-[22px] bg-popover py-1.5 shadow-[0_20px_50px_-10px_rgb(0_0_0/0.75),inset_0_0_0_1px_rgb(255_255_255/0.08)] animate-in fade-in zoom-in-95 duration-150",
                    {children}
                }
            }
        }
    }
}

/// One item in the ⋯ menu: icon, label, an optional trailing note
/// ("2 on"), and the action. Picking it closes the menu.
#[component]
pub fn SectionMenuItem(
    label: String,
    icon: Element,
    on_click: EventHandler<()>,
    #[props(default)] note: Option<String>,
) -> Element {
    let mut open = use_context::<MenuOpen>().0;
    rsx! {
        button {
            r#type: "button",
            class: "flex h-12 items-center gap-3 px-4 text-left text-[16px] text-foreground active:bg-foreground/[0.06]",
            onclick: move |_| {
                open.set(false);
                on_click.call(());
            },
            span { class: "flex w-5 justify-center text-muted-foreground", {icon} }
            span { class: "min-w-0 flex-1 truncate", "{label}" }
            if let Some(n) = note {
                span { class: "text-[13px] text-muted-foreground", "{n}" }
            }
        }
    }
}

/// A hairline between groups of menu items.
#[component]
pub fn SectionMenuDivider() -> Element {
    rsx! {
        div { class: "mx-4 my-1 h-px bg-foreground/[0.08]" }
    }
}
