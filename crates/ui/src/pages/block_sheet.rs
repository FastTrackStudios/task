//! The Schedule's block sheet: tap a block, this rises from the bottom.
//!
//! Drawn with the app's sheet kit (`task_ui_core::sheet`), the way
//! Linear's iOS edit sheets read: the block's name as the heading, then
//! grouped rows — when it runs, what kind of time it is, what it is for
//! (a task or project, picked with the typeable property picker, or just
//! a few words) — and, for the block running now whose kind steers the
//! hour, the mode's own action: **Start maintenance mode**, which opens
//! the timer session that *is* the mode (`crate::now_block`).
//!
//! Edits are held in the sheet and written on Done, so typing a name
//! doesn't save on every keystroke.

use architect_ui::lucide_dioxus::{FolderKanban, Play, Square, Trash2};
use dioxus::prelude::*;
use scheduling_proto::BlockCategory;
use task_ui::{PickOption, PropertyPicker};
use task_ui_core::sheet::{BottomSheet, SheetGroup, SheetRow, SheetSection, SheetSelect};

/// What the block is for, as the editor passes it back:
/// `(kind, title, ref_id)` — `kind` is `"label"` / `"task"` / `"project"`.
pub type Assign = (String, String, Option<String>);

/// The kinds a block can be, as the sheet offers them.
const KINDS: &[(BlockCategory, &str)] = &[
    (BlockCategory::Allocatable, "Work"),
    (BlockCategory::Maintenance, "Errands & chores"),
    (BlockCategory::Meal, "Meal"),
    (BlockCategory::Exercise, "Exercise"),
    (BlockCategory::Spiritual, "Quiet time"),
    (BlockCategory::Reset, "Morning reset"),
    (BlockCategory::Hygiene, "Getting ready"),
    (BlockCategory::WindDown, "Wind-down"),
    (BlockCategory::Sleep, "Sleep"),
    (BlockCategory::Other, "Other"),
];

fn kind_key(c: BlockCategory) -> String {
    format!("{c:?}")
}

fn kind_from_key(s: &str) -> BlockCategory {
    KINDS
        .iter()
        .find(|(c, _)| kind_key(*c) == s)
        .map_or(BlockCategory::Other, |(c, _)| *c)
}

/// `HH:MM` for a time input.
fn hhmm(min: u16) -> String {
    format!("{:02}:{:02}", min / 60, min % 60)
}

fn parse_hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.split_once(':')?;
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// What the sheet saves: name, start, end, what it's for, its kind.
pub type BlockSave = (String, u16, u16, Option<Assign>, BlockCategory);

/// The mode the sheet can start or end, when this is the block running
/// now and its kind steers the hour.
#[derive(Clone, PartialEq)]
pub enum ModeAction {
    /// Not running yet: "Start maintenance mode".
    Start(String),
    /// Running: its name, and the clock since it began.
    Running(String, String),
}

#[component]
pub fn BlockSheet(
    label: String,
    start_min: u16,
    end_min: u16,
    category: BlockCategory,
    assignment: Option<Assign>,
    tasks: Vec<(String, String)>,
    projects: Vec<(String, String)>,
    #[props(default)] mode: Option<ModeAction>,
    on_mode: EventHandler<()>,
    on_save: EventHandler<BlockSave>,
    on_delete: EventHandler<()>,
    on_close: EventHandler<()>,
) -> Element {
    // Working copies, seeded once (the page keys this per block).
    let mut name = use_signal(|| label.clone());
    let mut start = use_signal(|| start_min);
    let mut end = use_signal(|| end_min);
    let mut kind = use_signal(|| category);
    let mut what = use_signal(|| assignment.clone());
    let mut picking = use_signal(|| false);

    let options: Vec<PickOption> = tasks
        .iter()
        .map(|(id, t)| {
            PickOption::new(format!("task:{id}"), t.clone())
                .icon(rsx! { task_ui::StatusIcon { status: "open".to_owned() } })
        })
        .chain(projects.iter().map(|(id, t)| {
            PickOption::new(format!("project:{id}"), t.clone())
                .icon(rsx! { FolderKanban { size: 15 } })
        }))
        .collect();
    let selected: Vec<String> = what()
        .and_then(|(k, _, id)| id.map(|id| format!("{k}:{id}")))
        .into_iter()
        .collect();
    let what_label = what().map(|(_, t, _)| t);

    let save = move || {
        let (s, e) = (start(), end().max(start() + 5));
        on_save.call((name().trim().to_owned(), s, e, what(), kind()));
    };

    rsx! {
        BottomSheet { open: true, title: String::new(), on_close,
            // ── name, as the heading ──
            div { class: "flex items-center gap-2 px-2 pb-3",
                input {
                    class: "min-w-0 flex-1 bg-transparent text-[22px] font-semibold tracking-[-0.01em] text-foreground outline-none placeholder:text-muted-foreground",
                    aria_label: "Block name",
                    placeholder: "Block name",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
                button {
                    r#type: "button",
                    class: "h-10 shrink-0 rounded-full bg-primary px-4 text-[15px] font-semibold text-primary-foreground active:scale-95",
                    onclick: move |_| save(),
                    "Done"
                }
            }

            // ── the mode, when this block is the hour ──
            if let Some(m) = mode.clone() {
                button {
                    r#type: "button",
                    class: "mb-3 flex h-[54px] w-full items-center gap-3 rounded-[20px] bg-gradient-to-br from-primary/[0.18] to-primary/[0.06] px-4 text-left text-[16px] font-semibold text-foreground shadow-[inset_0_0_0_1px_rgb(255_255_255/0.06)] active:scale-[0.99]",
                    onclick: move |_| on_mode.call(()),
                    match m {
                        ModeAction::Start(label) => rsx! {
                            span { class: "flex size-8 items-center justify-center rounded-full bg-primary text-primary-foreground", Play { size: 15 } }
                            span { class: "flex-1", "Start {label.to_lowercase()}" }
                        },
                        ModeAction::Running(label, clock) => rsx! {
                            span { class: "flex size-8 items-center justify-center rounded-full bg-foreground/10 text-foreground", Square { size: 13 } }
                            span { class: "flex-1", "End {label.to_lowercase()}" }
                            span { class: "font-mono text-[13px] font-normal tabular-nums text-muted-foreground", "{clock}" }
                        },
                    }
                }
            }

            // ── when ──
            SheetGroup {
                SheetRow { label: "Starts",
                    input {
                        r#type: "time",
                        class: "bg-transparent text-right text-[16px] text-muted-foreground outline-none",
                        value: "{hhmm(start())}",
                        oninput: move |e| if let Some(m) = parse_hhmm(&e.value()) { start.set(m) },
                    }
                }
                SheetRow { label: "Ends",
                    input {
                        r#type: "time",
                        class: "bg-transparent text-right text-[16px] text-muted-foreground outline-none",
                        value: "{hhmm(end())}",
                        oninput: move |e| if let Some(m) = parse_hhmm(&e.value()) { end.set(m) },
                    }
                }
            }

            // ── what kind of time, and what it's for ──
            SheetGroup {
                SheetSelect {
                    label: "Kind",
                    value: kind_key(kind()),
                    options: KINDS.iter().map(|(c, l)| (kind_key(*c), (*l).to_owned())).collect::<Vec<_>>(),
                    on_change: move |v: String| kind.set(kind_from_key(&v)),
                }
                button {
                    r#type: "button",
                    class: "flex min-h-[54px] w-full items-center justify-between gap-3 px-4 text-left text-[16px] text-foreground active:bg-foreground/[0.04]",
                    onclick: move |_| picking.set(true),
                    span { "For" }
                    span { class: "min-w-0 truncate text-muted-foreground",
                        {what_label.clone().unwrap_or_else(|| "Nothing yet".to_owned())}
                    }
                }
            }
            p { class: "px-2 pb-3 text-[13px] text-muted-foreground",
                "A task or project, or type a few words."
            }

            // ── the rest ──
            SheetSection { label: String::new() }
            SheetGroup {
                button {
                    r#type: "button",
                    class: "flex min-h-[54px] w-full items-center gap-3 px-4 text-left text-[16px] text-red-400 active:bg-foreground/[0.04]",
                    onclick: move |_| on_delete.call(()),
                    Trash2 { size: 17 }
                    "Remove from this day"
                }
            }

            PropertyPicker {
                open: picking(),
                title: "Plan this block for…",
                options,
                selected,
                on_create: move |words: String| {
                    what.set(Some(("label".to_owned(), words, None)));
                    picking.set(false);
                },
                on_pick: move |id: String| {
                    let found = id
                        .split_once(':')
                        .and_then(|(k, rid)| {
                            let list = if k == "task" { &tasks } else { &projects };
                            list.iter()
                                .find(|(i, _)| i == rid)
                                .map(|(_, t)| (k.to_owned(), t.clone(), Some(rid.to_owned())))
                        });
                    // Picking the chosen one again clears it.
                    if selected_now(&what(), &id) {
                        what.set(None);
                    } else if let Some(a) = found {
                        what.set(Some(a));
                    }
                    picking.set(false);
                },
                on_close: move |()| picking.set(false),
            }
        }
    }
}

fn selected_now(what: &Option<Assign>, id: &str) -> bool {
    what.as_ref()
        .and_then(|(k, _, rid)| rid.as_ref().map(|r| format!("{k}:{r}")))
        .is_some_and(|s| s == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_kinds_round_trip() {
        assert_eq!(hhmm(16 * 60 + 5), "16:05");
        assert_eq!(parse_hhmm("16:05"), Some(16 * 60 + 5));
        assert_eq!(parse_hhmm("25:00"), None);
        for (c, _) in KINDS {
            assert_eq!(kind_from_key(&kind_key(*c)), *c);
        }
    }
}
