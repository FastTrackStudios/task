//! The property picker — one typeable dropdown for every property.
//!
//! Status, priority, assignee, project, labels: each is a short list a
//! person picks from many times a day, so the picker is built to be
//! driven without looking:
//!
//! - it opens with the cursor already in its search field ("Change
//!   priority to…"), its hotkey shown at the right of the field;
//! - typing filters (prefix matches first, then anywhere in the word);
//! - ↑ / ↓ move, Enter picks, Esc closes;
//! - with nothing typed, a number picks directly — each row shows its
//!   number, as Linear's do (priority 0–4, status 1–6);
//! - the current value carries a ✓; a multi-select (labels) shows
//!   checkboxes and stays open while you toggle;
//! - where new values make sense (labels), no match offers
//!   "Create “…”".
//!
//! On a desk it is a popover under its chip; on a phone, a bottom sheet
//! with the keyboard up — same rows, same keys on a hardware keyboard.
//! The caller owns `open` and positions nothing: put the picker inside a
//! `relative` box next to whatever opens it.

use architect_ui::lucide_dioxus::{Check, Plus};
use dioxus::prelude::*;

/// One row in a picker.
#[derive(Clone, PartialEq)]
pub struct PickOption {
    /// What `on_pick` reports.
    pub id: String,
    pub label: String,
    /// Leading icon (status ring, priority bars, avatar).
    pub icon: Option<Element>,
    /// A colour dot instead of an icon (labels) — any CSS colour.
    pub dot: Option<String>,
}

impl PickOption {
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            dot: None,
        }
    }

    #[must_use]
    pub fn icon(mut self, icon: Element) -> Self {
        self.icon = Some(icon);
        self
    }

    #[must_use]
    pub fn dot(mut self, color: impl Into<String>) -> Self {
        self.dot = Some(color.into());
        self
    }
}

/// How well `query` matches `label`: `None` = no match; smaller ranks
/// first (whole-label prefix, then a word's prefix, then anywhere).
#[must_use]
pub fn match_rank(query: &str, label: &str) -> Option<u8> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let l = label.to_lowercase();
    if l.starts_with(&q) {
        Some(0)
    } else if l
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w.starts_with(&q))
    {
        Some(1)
    } else if l.contains(&q) {
        Some(2)
    } else {
        None
    }
}

/// The rows `query` keeps, best first, each with its original index
/// (which is what the number keys address).
#[must_use]
pub fn filtered(options: &[PickOption], query: &str) -> Vec<(usize, PickOption)> {
    let mut hits: Vec<(u8, usize, PickOption)> = options
        .iter()
        .enumerate()
        .filter_map(|(i, o)| match_rank(query, &o.label).map(|r| (r, i, o.clone())))
        .collect();
    hits.sort_by_key(|(r, i, _)| (*r, *i));
    hits.into_iter().map(|(_, i, o)| (i, o)).collect()
}

#[component]
pub fn PropertyPicker(
    /// Whether it shows. The caller owns it; `on_close` asks to clear it.
    open: bool,
    /// The search field's placeholder ("Change priority to…").
    title: String,
    /// The key that opens it, shown at the right of the field ("P").
    #[props(default)]
    hotkey: Option<String>,
    options: Vec<PickOption>,
    /// Ids currently chosen (✓, or checked boxes when `multi`).
    #[props(default)]
    selected: Vec<String>,
    /// Several at once (labels): rows toggle and the picker stays open.
    #[props(default)]
    multi: bool,
    /// The number shown on the first row: 0 (priority) or 1 (status).
    /// `None` hides the numbers.
    #[props(default = Some(1))]
    number_from: Option<u8>,
    /// Offer "Create “…”" when nothing matches (labels, projects).
    #[props(default)]
    on_create: Option<EventHandler<String>>,
    on_pick: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    let mut query = use_signal(String::new);
    let mut cursor = use_signal(|| 0usize);
    // Fresh each time it opens.
    let mut was_open = use_signal(|| false);
    if open != *was_open.peek() {
        was_open.set(open);
        if open {
            query.set(String::new());
            cursor.set(0);
        }
    }
    if !open {
        return rsx! {};
    }

    let rows = filtered(&options, &query.read());
    let q = query.read().trim().to_owned();
    let can_create = on_create.is_some()
        && !q.is_empty()
        && !options.iter().any(|o| o.label.eq_ignore_ascii_case(&q));
    // The create row sits after the matches and is reachable by arrows.
    let total = rows.len() + usize::from(can_create);
    let at = (*cursor.read()).min(total.saturating_sub(1));

    let pick = {
        let rows = rows.clone();
        move |i: usize| {
            if let Some((_, o)) = rows.get(i) {
                on_pick.call(o.id.clone());
                if !multi {
                    on_close.call(());
                }
            } else if can_create {
                if let Some(create) = on_create {
                    create.call(query.peek().trim().to_owned());
                    query.set(String::new());
                    if !multi {
                        on_close.call(());
                    }
                }
            }
        }
    };
    let mut pick_kb = pick.clone();
    let options_kb = options.clone();
    let on_key = move |e: KeyboardEvent| match e.key() {
        Key::Escape => {
            e.prevent_default();
            e.stop_propagation();
            on_close.call(());
        }
        Key::ArrowDown => {
            e.prevent_default();
            if total > 0 {
                cursor.set((at + 1) % total);
            }
        }
        Key::ArrowUp => {
            e.prevent_default();
            if total > 0 {
                cursor.set((at + total - 1) % total);
            }
        }
        Key::Enter => {
            e.prevent_default();
            pick_kb(at);
        }
        Key::Character(c) if query.peek().is_empty() => {
            // A number with nothing typed picks that row directly.
            if let (Some(from), Ok(n)) = (number_from, c.parse::<usize>()) {
                if let Some(idx) = n.checked_sub(usize::from(from)) {
                    if let Some(o) = options_kb.get(idx) {
                        e.prevent_default();
                        on_pick.call(o.id.clone());
                        if !multi {
                            on_close.call(());
                        }
                    }
                }
            }
        }
        _ => {}
    };

    rsx! {
        // Click-away: invisible on a desk, a dim layer behind the sheet
        // on a phone.
        div {
            class: "fixed inset-0 z-40 bg-black/50 md:bg-transparent",
            onclick: move |_| on_close.call(()),
        }
        div {
            class: "fixed inset-x-0 bottom-0 z-50 flex max-h-[70vh] flex-col overflow-hidden rounded-t-2xl border-t border-border bg-popover pb-[env(safe-area-inset-bottom)] text-popover-foreground shadow-2xl md:absolute md:inset-x-auto md:bottom-auto md:left-0 md:top-full md:mt-1 md:max-h-80 md:w-64 md:rounded-lg md:border md:pb-0",
            role: "listbox",
            aria_label: "{title}",
            onclick: move |e| e.stop_propagation(),
            div { class: "flex items-center gap-2 border-b border-border/70 px-3",
                input {
                    r#type: "text",
                    class: "min-w-0 flex-1 bg-transparent py-3 text-[15px] text-foreground outline-none placeholder:text-muted-foreground md:py-2 md:text-[13px]",
                    placeholder: "{title}",
                    value: "{query}",
                    autofocus: true,
                    onmounted: move |e| async move {
                        let _ = e.set_focus(true).await;
                    },
                    oninput: move |e| {
                        query.set(e.value());
                        cursor.set(0);
                    },
                    onkeydown: on_key,
                }
                if let Some(k) = hotkey.as_ref() {
                    kbd { class: "hidden shrink-0 rounded border border-border/60 px-1.5 font-mono text-[10px] text-muted-foreground md:inline",
                        "{k}"
                    }
                }
            }
            div { class: "min-h-0 flex-1 overflow-y-auto p-1",
                if rows.is_empty() && !can_create {
                    p { class: "px-2.5 py-2 text-[13px] text-muted-foreground", "No matches" }
                }
                for (row, (idx, o)) in rows.iter().cloned().enumerate() {
                    {
                        let chosen = selected.contains(&o.id);
                        let active = row == at;
                        let mut pick = pick.clone();
                        let number = number_from.map(|from| idx + usize::from(from)).filter(|n| *n < 10);
                        rsx! {
                            button {
                                key: "{o.id}",
                                r#type: "button",
                                role: "option",
                                aria_selected: chosen.to_string(),
                                class: if active {
                                    "flex min-h-11 w-full items-center gap-2.5 rounded-md bg-accent px-2.5 text-left text-[15px] text-foreground md:min-h-8 md:text-[13px]"
                                } else {
                                    "flex min-h-11 w-full items-center gap-2.5 rounded-md px-2.5 text-left text-[15px] text-foreground md:min-h-8 md:text-[13px]"
                                },
                                onmouseenter: move |_| cursor.set(row),
                                onclick: move |_| pick(row),
                                if multi {
                                    span { class: if chosen { "flex size-4 shrink-0 items-center justify-center rounded border border-primary bg-primary text-primary-foreground" } else { "flex size-4 shrink-0 items-center justify-center rounded border border-border" },
                                        if chosen { Check { size: 11 } }
                                    }
                                }
                                if let Some(icon) = o.icon.clone() {
                                    span { class: "flex size-4 shrink-0 items-center justify-center text-muted-foreground", {icon} }
                                } else if let Some(c) = o.dot.as_ref() {
                                    span { class: "size-2.5 shrink-0 rounded-full", style: "background: {c};" }
                                }
                                span { class: "min-w-0 flex-1 truncate", "{o.label}" }
                                if chosen && !multi {
                                    span { class: "shrink-0 text-foreground", Check { size: 14 } }
                                }
                                if let Some(n) = number {
                                    span { class: "w-3 shrink-0 text-right font-mono text-[11px] tabular-nums text-muted-foreground", "{n}" }
                                }
                            }
                        }
                    }
                }
                if can_create {
                    {
                        let row = rows.len();
                        let active = row == at;
                        let mut pick = pick.clone();
                        rsx! {
                            button {
                                r#type: "button",
                                class: if active {
                                    "flex min-h-11 w-full items-center gap-2.5 rounded-md bg-accent px-2.5 text-left text-[15px] text-foreground md:min-h-8 md:text-[13px]"
                                } else {
                                    "flex min-h-11 w-full items-center gap-2.5 rounded-md px-2.5 text-left text-[15px] text-foreground md:min-h-8 md:text-[13px]"
                                },
                                onmouseenter: move |_| cursor.set(row),
                                onclick: move |_| pick(row),
                                span { class: "flex size-4 shrink-0 items-center justify-center text-muted-foreground", Plus { size: 14 } }
                                span { class: "min-w-0 flex-1 truncate", "Create “{q}”" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(labels: &[&str]) -> Vec<PickOption> {
        labels.iter().map(|l| PickOption::new(*l, *l)).collect()
    }

    #[test]
    fn typing_keeps_matches_prefix_first_with_their_numbers() {
        let o = opts(&["No priority", "Urgent", "High", "Medium", "Low"]);
        let me: Vec<usize> = filtered(&o, "me").into_iter().map(|(i, _)| i).collect();
        assert_eq!(me, vec![3], "Medium, still addressed as row 3");
        let low: Vec<usize> = filtered(&o, "lo").into_iter().map(|(i, _)| i).collect();
        assert_eq!(low, vec![4]);
        // "pri" is a word prefix inside "No priority".
        assert_eq!(filtered(&o, "pri").len(), 1);
    }

    #[test]
    fn nothing_typed_keeps_everything_in_order() {
        let o = opts(&["Backlog", "Todo", "In Progress"]);
        let all: Vec<usize> = filtered(&o, "").into_iter().map(|(i, _)| i).collect();
        assert_eq!(all, vec![0, 1, 2]);
    }

    #[test]
    fn whole_label_prefix_beats_a_word_prefix_beats_anywhere() {
        assert_eq!(match_rank("in", "In Progress"), Some(0));
        assert_eq!(match_rank("pro", "In Progress"), Some(1));
        assert_eq!(match_rank("gres", "In Progress"), Some(2));
        assert_eq!(match_rank("zz", "In Progress"), None);
    }
}
