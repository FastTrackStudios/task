//! The person's own arrangement of the sections — Linear's Customize.
//!
//! The section menu lists every section in an order the person chooses,
//! and leaves out the ones they hide. **The first three are the tab
//! bar**: put Schedule third and it is one tap away from anywhere. The
//! arrangement is kept per device (`crate::local_store`), by label, so
//! a section added later (a plugin switched on) simply appears at the
//! end of the list until it is moved.
//!
//! [`CustomizeSheet`] is the editor: drag a row by its handle to move
//! it, the red − to hide it, the green + to bring a hidden one back.

use architect_ui::lucide_dioxus::{Menu, Minus, Plus};
use dioxus::prelude::*;

use crate::nav::NavTab;

const KEY: &str = "task.nav.layout";

/// How many sections the tab bar holds.
pub const PRIMARY: usize = 3;

/// The order (by label) and what is hidden.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NavLayout {
    pub order: Vec<String>,
    pub hidden: Vec<String>,
}

impl NavLayout {
    /// This device's layout, or the default (the shell's own order).
    #[must_use]
    pub fn load() -> Self {
        crate::local_store::get(KEY).map_or_else(Self::default, |raw| Self::parse(&raw))
    }

    pub fn save(&self) {
        crate::local_store::set(KEY, &self.encode());
    }

    /// `order:a,b,c;hidden:x,y` — labels carry no `,` or `;`.
    fn encode(&self) -> String {
        format!(
            "order:{};hidden:{}",
            self.order.join(","),
            self.hidden.join(",")
        )
    }

    fn parse(raw: &str) -> Self {
        let mut out = Self::default();
        for part in raw.split(';') {
            let list = |s: &str| -> Vec<String> {
                s.split(',')
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            if let Some(o) = part.strip_prefix("order:") {
                out.order = list(o);
            } else if let Some(h) = part.strip_prefix("hidden:") {
                out.hidden = list(h);
            }
        }
        out
    }

    /// The sections in this layout: `(shown, hidden)`. Shown follows the
    /// saved order, then any section the order doesn't know yet in the
    /// shell's own order; hidden keeps the order it was hidden in.
    #[must_use]
    pub fn arrange(&self, tabs: Vec<NavTab>) -> (Vec<NavTab>, Vec<NavTab>) {
        let label_of = |t: &NavTab| t.label.to_owned();
        let mut shown: Vec<NavTab> = self
            .order
            .iter()
            .filter(|l| !self.hidden.contains(l))
            .filter_map(|l| tabs.iter().find(|t| t.label == l.as_str()).cloned())
            .collect();
        for t in &tabs {
            let l = label_of(t);
            if !self.order.contains(&l) && !self.hidden.contains(&l) {
                shown.push(t.clone());
            }
        }
        let hidden = self
            .hidden
            .iter()
            .filter_map(|l| tabs.iter().find(|t| t.label == l.as_str()).cloned())
            .collect();
        (shown, hidden)
    }

    /// A layout naming exactly `shown` (in that order) and `hidden`.
    #[must_use]
    pub fn from_lists(shown: &[NavTab], hidden: &[NavTab]) -> Self {
        Self {
            order: shown.iter().map(|t| t.label.to_owned()).collect(),
            hidden: hidden.iter().map(|t| t.label.to_owned()).collect(),
        }
    }
}

/// The shared layout, so the tab bar, the section menu and Settings all
/// follow one edit.
#[derive(Clone, Copy)]
struct LayoutCtx(Signal<NavLayout>);

/// Provide the layout at the shell root.
pub fn provide_nav_layout() {
    let sig = use_signal(NavLayout::load);
    use_context_provider(|| LayoutCtx(sig));
}

/// The shared layout (or this device's, outside the shell).
pub fn use_nav_layout() -> Signal<NavLayout> {
    match try_use_context::<LayoutCtx>() {
        Some(c) => c.0,
        None => use_signal(NavLayout::load),
    }
}

/// Move `from` to `to` in a list.
fn moved<T: Clone>(list: &[T], from: usize, to: usize) -> Vec<T> {
    let mut v = list.to_vec();
    if from < v.len() {
        let item = v.remove(from);
        v.insert(to.min(v.len()), item);
    }
    v
}

/// Each row's height, which is what a drag's distance is counted in.
const ROW_PX: f64 = 56.0;

/// The Customize editor, in a sheet.
#[component]
pub fn CustomizeSheet(
    open: bool,
    on_close: EventHandler<()>,
    tabs: Vec<NavTab>,
    layout: Signal<NavLayout>,
) -> Element {
    let mut layout = layout;
    // `(row picked up, where the finger started)` and how far it's gone.
    let mut drag = use_signal(|| None::<(usize, f64)>);
    let mut dy = use_signal(|| 0.0_f64);

    let (shown, hidden) = layout.read().arrange(tabs.clone());
    let n = shown.len();
    let target = drag().map(|(from, _)| {
        let steps = (dy() / ROW_PX).round() as i64;
        (from as i64 + steps).clamp(0, n.saturating_sub(1) as i64) as usize
    });
    let preview = match (drag(), target) {
        (Some((from, _)), Some(to)) => moved(&shown, from, to),
        _ => shown.clone(),
    };

    let commit = {
        let shown = shown.clone();
        let hidden = hidden.clone();
        move || {
            if let (Some((from, _)), Some(to)) = (drag(), target) {
                if from != to {
                    let next = NavLayout::from_lists(&moved(&shown, from, to), &hidden);
                    next.save();
                    layout.set(next);
                }
            }
            drag.set(None);
            dy.set(0.0);
        }
    };
    let mut commit_up = commit.clone();
    let mut commit_leave = commit;

    rsx! {
        task_ui_core::sheet::BottomSheet {
            open,
            title: "Customize".to_owned(),
            on_close,
            p { class: "px-2 pb-3 text-[13px] text-muted-foreground",
                "Drag to reorder. The first three are your tab bar."
            }
            div {
                class: "relative mb-4 flex flex-col overflow-hidden rounded-[20px] bg-foreground/[0.05]",
                style: "touch-action: none;",
                onpointermove: move |e: PointerEvent| {
                    if let Some((_, start)) = drag() {
                        dy.set(e.client_coordinates().y - start);
                    }
                },
                onpointerup: move |_| commit_up(),
                onpointercancel: move |_| {
                    drag.set(None);
                    dy.set(0.0);
                },
                onpointerleave: move |_| commit_leave(),
                for (i , tab) in preview.into_iter().enumerate() {
                    {
                        let lifted = target == Some(i) && drag().is_some();
                        let label = tab.label;
                        let row = if lifted {
                            "relative z-10 flex h-14 items-center gap-3.5 bg-foreground/[0.12] px-3 shadow-[0_10px_30px_-8px_rgb(0_0_0/0.7)] scale-[1.02] transition-transform"
                        } else {
                            "relative flex h-14 items-center gap-3.5 px-3 transition-transform"
                        };
                        let primary = i < PRIMARY;
                        // The handle picks up the row it is on in the
                        // saved order (not the preview): the drag's own
                        // index, unchanged until it lands.
                        let from = shown.iter().position(|t| t.label == label).unwrap_or(i);
                        let shown_now = shown.clone();
                        let hidden_now = hidden.clone();
                        rsx! {
                            div { key: "{label}", class: row,
                                button {
                                    r#type: "button",
                                    class: "flex size-6 shrink-0 items-center justify-center rounded-full bg-red-500 text-white active:scale-90",
                                    aria_label: "Hide {label}",
                                    onclick: move |_| {
                                        let shown: Vec<NavTab> =
                                            shown_now.iter().filter(|t| t.label != label).cloned().collect();
                                        let mut hidden = hidden_now.clone();
                                        if let Some(t) = shown_now.iter().find(|t| t.label == label) {
                                            hidden.push(t.clone());
                                        }
                                        let next = NavLayout::from_lists(&shown, &hidden);
                                        next.save();
                                        layout.set(next);
                                    },
                                    Minus { size: 14 }
                                }
                                span { class: "flex size-6 shrink-0 items-center justify-center text-foreground [&_svg]:size-5",
                                    {(tab.icon)()}
                                }
                                span { class: "min-w-0 flex-1 truncate text-[16px] text-foreground", "{label}" }
                                if primary {
                                    span { class: "rounded-full bg-primary/15 px-2 py-0.5 text-[11px] font-medium text-primary",
                                        "Tab bar"
                                    }
                                }
                                span {
                                    class: "flex h-11 w-11 shrink-0 cursor-grab items-center justify-center text-muted-foreground",
                                    style: "touch-action: none;",
                                    aria_label: "Reorder {label}",
                                    onpointerdown: move |e: PointerEvent| {
                                        e.prevent_default();
                                        drag.set(Some((from, e.client_coordinates().y)));
                                        dy.set(0.0);
                                    },
                                    Menu { size: 20 }
                                }
                                if i + 1 < n && !lifted {
                                    span { class: "pointer-events-none absolute inset-x-3 bottom-0 h-px bg-foreground/[0.06]" }
                                }
                            }
                        }
                    }
                }
            }
            task_ui_core::sheet::SheetSection { label: "Hidden".to_owned() }
            div { class: "flex flex-col overflow-hidden rounded-[20px] bg-foreground/[0.05]",
                if hidden.is_empty() {
                    p { class: "py-6 text-center text-[15px] text-muted-foreground", "No hidden items" }
                }
                for tab in hidden.iter().cloned() {
                    {
                        let label = tab.label;
                        let shown_now = shown.clone();
                        let hidden_now = hidden.clone();
                        rsx! {
                            div { key: "{label}", class: "flex h-14 items-center gap-3.5 px-3",
                                button {
                                    r#type: "button",
                                    class: "flex size-6 shrink-0 items-center justify-center rounded-full bg-emerald-500 text-white active:scale-90",
                                    aria_label: "Show {label}",
                                    onclick: move |_| {
                                        let mut shown = shown_now.clone();
                                        if let Some(t) = hidden_now.iter().find(|t| t.label == label) {
                                            shown.push(t.clone());
                                        }
                                        let hidden: Vec<NavTab> =
                                            hidden_now.iter().filter(|t| t.label != label).cloned().collect();
                                        let next = NavLayout::from_lists(&shown, &hidden);
                                        next.save();
                                        layout.set(next);
                                    },
                                    Plus { size: 14 }
                                }
                                span { class: "flex size-6 shrink-0 items-center justify-center text-muted-foreground [&_svg]:size-5",
                                    {(tab.icon)()}
                                }
                                span { class: "min-w-0 flex-1 truncate text-[16px] text-muted-foreground", "{label}" }
                            }
                        }
                    }
                }
            }
            if *layout.read() != NavLayout::default() {
                button {
                    r#type: "button",
                    class: "mx-auto mt-3 block rounded-full px-4 py-2 text-[14px] text-muted-foreground active:text-foreground",
                    onclick: move |_| {
                        let next = NavLayout::default();
                        next.save();
                        layout.set(next);
                    },
                    "Reset to default"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(tabs: &[NavTab]) -> Vec<&'static str> {
        tabs.iter().map(|t| t.label).collect()
    }

    #[test]
    fn a_layout_orders_hides_and_keeps_new_sections() {
        let tabs = crate::nav::nav_tabs();
        let default = NavLayout::default().arrange(tabs.clone());
        assert_eq!(
            labels(&default.0),
            labels(&tabs),
            "no layout = the shell's order"
        );

        let layout = NavLayout {
            order: vec![
                "Schedule".into(),
                "Inbox".into(),
                "Tasks".into(),
                "Gone".into(),
            ],
            hidden: vec!["Favorites".into()],
        };
        let (shown, hidden) = layout.arrange(tabs.clone());
        assert_eq!(
            &labels(&shown)[..3],
            ["Schedule", "Inbox", "Tasks"],
            "first three = tab bar"
        );
        assert_eq!(labels(&hidden), ["Favorites"]);
        assert!(!labels(&shown).contains(&"Favorites"));
        assert!(
            labels(&shown).contains(&"Projects"),
            "a section the layout doesn't name still shows"
        );
        assert_eq!(
            shown.len() + hidden.len(),
            tabs.len(),
            "an unknown label is ignored"
        );
    }

    #[test]
    fn the_layout_round_trips_and_moves() {
        let l = NavLayout {
            order: vec!["Tasks".into(), "Inbox".into()],
            hidden: vec!["Vault".into()],
        };
        assert_eq!(NavLayout::parse(&l.encode()), l);
        assert_eq!(moved(&[1, 2, 3, 4], 0, 2), vec![2, 3, 1, 4]);
        assert_eq!(moved(&[1, 2, 3, 4], 3, 0), vec![4, 1, 2, 3]);
    }
}
