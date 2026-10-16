//! Property chips — Status, Priority, Assignee, Project, Labels — each a
//! compact button that opens the [`PropertyPicker`], and each reachable
//! by its key (S, P, A, R for project, L) through a shared
//! [`Hotkey`] signal the surrounding view sets.
//!
//! The vocabulary is Linear's, mapped onto Task's stored values so
//! nothing on disk changes:
//!
//! | shown        | stored status  |   | shown       | stored priority |
//! |--------------|----------------|---|-------------|-----------------|
//! | Backlog      | `backlog`      |   | No priority | `none`          |
//! | Todo         | `open`         |   | Urgent      | `critical`      |
//! | In Progress  | `in-progress`  |   | High        | `high`          |
//! | Waiting      | `waiting`      |   | Medium      | `normal`        |
//! | Done         | `done`         |   | Low         | `low`           |
//! | Canceled     | `cancelled`    |   |             |                 |

use architect_ui::lucide_dioxus::{
    Box as BoxIcon, Circle, CircleCheck, CircleDashed, CircleDot, CirclePause, CircleX, Ellipsis,
    SignalHigh, SignalLow, SignalMedium, Tag, TriangleAlert, UserRound,
};
use dioxus::prelude::*;
use uuid::Uuid;

use super::picker::{PickOption, PropertyPicker};

/// Which picker a key press asked for. The view that owns the chips
/// sets it on S / P / A / R / L (when no text field has focus); the
/// matching chip opens and clears it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hotkey {
    Status,
    Priority,
    Assignee,
    Project,
    Labels,
}

impl Hotkey {
    /// The key for a typed character, if it is one of the five.
    #[must_use]
    pub fn from_key(c: &str) -> Option<Self> {
        match c {
            "s" | "S" => Some(Self::Status),
            "p" | "P" => Some(Self::Priority),
            "a" | "A" => Some(Self::Assignee),
            "r" | "R" => Some(Self::Project),
            "l" | "L" => Some(Self::Labels),
            _ => None,
        }
    }
}

// ── vocabularies ────────────────────────────────────────────────────

/// `(stored, shown)` statuses in picker order (numbered from 1).
pub const STATUSES: &[(&str, &str)] = &[
    ("backlog", "Backlog"),
    ("open", "Todo"),
    ("in-progress", "In Progress"),
    ("waiting", "Waiting"),
    ("done", "Done"),
    ("cancelled", "Canceled"),
];

/// `(stored, shown)` priorities in picker order (numbered from 0).
pub const PRIORITIES: &[(&str, &str)] = &[
    ("none", "No priority"),
    ("critical", "Urgent"),
    ("high", "High"),
    ("normal", "Medium"),
    ("low", "Low"),
];

/// The stored status spelled the way the vocabulary does (`todo` and
/// `in_progress` read as `open` and `in-progress`).
#[must_use]
pub fn canonical_status(raw: &str) -> &'static str {
    let s = raw.trim().to_ascii_lowercase();
    match s.as_str() {
        "backlog" => "backlog",
        "in-progress" | "in_progress" | "doing" => "in-progress",
        "waiting" | "blocked" => "waiting",
        "done" | "completed" | "complete" => "done",
        "cancelled" | "canceled" => "cancelled",
        _ => "open",
    }
}

/// The stored priority spelled the way the vocabulary does.
#[must_use]
pub fn canonical_priority(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "critical" | "urgent" => "critical",
        "high" => "high",
        "normal" | "medium" | "med" => "normal",
        "low" => "low",
        _ => "none",
    }
}

fn shown(table: &[(&str, &'static str)], stored: &str) -> &'static str {
    table
        .iter()
        .find(|(s, _)| *s == stored)
        .map_or("", |(_, l)| *l)
}

/// The ring for a status — the same glyph in the list, the chip and the
/// picker.
#[component]
pub fn StatusIcon(status: String, #[props(default = 14)] size: usize) -> Element {
    match canonical_status(&status) {
        "backlog" => rsx! { span { class: "text-muted-foreground", CircleDashed { size } } },
        "in-progress" => rsx! { span { class: "text-amber-400", CircleDot { size } } },
        "waiting" => rsx! { span { class: "text-sky-400", CirclePause { size } } },
        "done" => rsx! { span { class: "text-primary", CircleCheck { size } } },
        "cancelled" => rsx! { span { class: "text-muted-foreground", CircleX { size } } },
        _ => rsx! { span { class: "text-foreground/80", Circle { size } } },
    }
}

/// Bars for a priority (an alert for Urgent, an ellipsis for none).
#[component]
pub fn PriorityIcon(priority: String, #[props(default = 14)] size: usize) -> Element {
    match canonical_priority(&priority) {
        "critical" => rsx! { span { class: "text-rose-400", TriangleAlert { size } } },
        "high" => rsx! { span { class: "text-foreground/85", SignalHigh { size } } },
        "normal" => rsx! { span { class: "text-foreground/70", SignalMedium { size } } },
        "low" => rsx! { span { class: "text-foreground/60", SignalLow { size } } },
        _ => rsx! { span { class: "text-muted-foreground", Ellipsis { size } } },
    }
}

/// A person's initials on a small disc — the assignee everywhere.
#[component]
pub fn PersonDot(name: String, #[props(default = 18)] size: u32) -> Element {
    let initials: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let initials = if initials.is_empty() {
        "?".to_owned()
    } else {
        initials
    };
    // A stable hue per name, so a person keeps their colour.
    let hue = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)))
        % 360;
    rsx! {
        span {
            class: "inline-flex shrink-0 items-center justify-center rounded-full text-[9px] font-semibold leading-none text-white",
            style: "width: {size}px; height: {size}px; background: hsl({hue} 45% 42%);",
            "{initials}"
        }
    }
}

// ── the chip ────────────────────────────────────────────────────────

/// A property chip holding a value — the look every property shares, a
/// task's or a note's (`crates/ui`'s note properties wear it too).
// t[impl doc.properties.shared]
pub const CHIP_CLASS: &str = "inline-flex h-8 max-w-[12rem] items-center gap-1.5 rounded-md border border-border/70 px-2 text-[13px] font-medium text-foreground transition-colors hover:bg-accent md:h-7";

/// A chip for an unset property: the same shape, read as an invitation.
pub const CHIP_QUIET_CLASS: &str = "inline-flex h-8 max-w-[12rem] items-center gap-1.5 rounded-md border border-border/70 px-2 text-[13px] font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground md:h-7";

/// One row of the desk properties column: the name muted on the left,
/// the value (a chip) on the right. Tasks and notes both lay their
/// properties out with it.
#[component]
pub fn PropertyRow(label: String, children: Element) -> Element {
    rsx! {
        div { class: "flex min-h-8 items-center gap-3",
            span {
                class: "w-20 shrink-0 truncate text-[13px] font-medium text-muted-foreground",
                title: "{label}",
                "{label}"
            }
            div { class: "min-w-0 flex-1", {children} }
        }
    }
}

/// The chip shape shared by every property: icon, label, a hover ring.
/// `quiet` = the property is unset and the chip reads as an invitation.
#[component]
fn Chip(
    icon: Element,
    label: String,
    quiet: bool,
    on_click: EventHandler<()>,
    title: String,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: if quiet { CHIP_QUIET_CLASS } else { CHIP_CLASS },
            title: "{title}",
            onclick: move |_| on_click.call(()),
            {icon}
            span { class: "truncate", "{label}" }
        }
    }
}

/// Open when the shared hotkey names this property, then clear it.
fn follow_hotkey(hotkey: Option<Signal<Option<Hotkey>>>, me: Hotkey, mut open: Signal<bool>) {
    use_effect(move || {
        if let Some(mut hk) = hotkey {
            if hk() == Some(me) {
                open.set(true);
                hk.set(None);
            }
        }
    });
}

#[component]
pub fn StatusChip(
    status: String,
    on_change: EventHandler<String>,
    #[props(default)] hotkey: Option<Signal<Option<Hotkey>>>,
) -> Element {
    let mut open = use_signal(|| false);
    follow_hotkey(hotkey, Hotkey::Status, open);
    let current = canonical_status(&status);
    let options: Vec<PickOption> = STATUSES
        .iter()
        .map(|(id, label)| {
            PickOption::new(*id, *label).icon(rsx! { StatusIcon { status: (*id).to_owned() } })
        })
        .collect();
    rsx! {
        div { class: "relative",
            Chip {
                icon: rsx! { StatusIcon { status: current.to_owned() } },
                label: shown(STATUSES, current).to_owned(),
                quiet: false,
                title: "Status (S)",
                on_click: move |()| open.set(true),
            }
            PropertyPicker {
                open: open(),
                title: "Change status…",
                hotkey: "S".to_owned(),
                options,
                selected: vec![current.to_owned()],
                number_from: Some(1),
                on_pick: move |id: String| on_change.call(id),
                on_close: move |()| open.set(false),
            }
        }
    }
}

#[component]
pub fn PriorityChip(
    priority: String,
    on_change: EventHandler<String>,
    #[props(default)] hotkey: Option<Signal<Option<Hotkey>>>,
) -> Element {
    let mut open = use_signal(|| false);
    follow_hotkey(hotkey, Hotkey::Priority, open);
    let current = canonical_priority(&priority);
    let options: Vec<PickOption> = PRIORITIES
        .iter()
        .map(|(id, label)| {
            PickOption::new(*id, *label).icon(rsx! { PriorityIcon { priority: (*id).to_owned() } })
        })
        .collect();
    rsx! {
        div { class: "relative",
            Chip {
                icon: rsx! { PriorityIcon { priority: current.to_owned() } },
                label: if current == "none" { "Priority".to_owned() } else { shown(PRIORITIES, current).to_owned() },
                quiet: current == "none",
                title: "Priority (P)",
                on_click: move |()| open.set(true),
            }
            PropertyPicker {
                open: open(),
                title: "Set priority to…",
                hotkey: "P".to_owned(),
                options,
                selected: vec![current.to_owned()],
                number_from: Some(0),
                on_pick: move |id: String| on_change.call(id),
                on_close: move |()| open.set(false),
            }
        }
    }
}

/// `people`: `(id, name)`; the first is listed first ("me" when the
/// caller puts the signed-in person there). `None` = unassigned.
#[component]
pub fn AssigneeChip(
    assignee: Option<String>,
    people: Vec<(String, String)>,
    on_change: EventHandler<Option<String>>,
    #[props(default)] hotkey: Option<Signal<Option<Hotkey>>>,
) -> Element {
    let mut open = use_signal(|| false);
    follow_hotkey(hotkey, Hotkey::Assignee, open);
    let name_of = |id: &str| {
        people
            .iter()
            .find(|(p, _)| p == id)
            .map_or_else(|| id.to_owned(), |(_, n)| n.clone())
    };
    let mut options = vec![
        PickOption::new("", "No assignee")
            .icon(rsx! { span { class: "text-muted-foreground", UserRound { size: 14 } } }),
    ];
    for (id, name) in &people {
        options.push(
            PickOption::new(id.clone(), name.clone())
                .icon(rsx! { PersonDot { name: name.clone(), size: 16 } }),
        );
    }
    let label = assignee.as_deref().map(name_of);
    rsx! {
        div { class: "relative",
            Chip {
                icon: match label.clone() {
                    Some(n) => rsx! { PersonDot { name: n } },
                    None => rsx! { span { class: "text-muted-foreground", UserRound { size: 14 } } },
                },
                label: label.clone().unwrap_or_else(|| "Assignee".to_owned()),
                quiet: label.is_none(),
                title: "Assignee (A)",
                on_click: move |()| open.set(true),
            }
            PropertyPicker {
                open: open(),
                title: "Assign to…",
                hotkey: "A".to_owned(),
                options,
                selected: vec![assignee.clone().unwrap_or_default()],
                number_from: Some(0),
                on_pick: move |id: String| on_change.call(Some(id).filter(|s| !s.is_empty())),
                on_close: move |()| open.set(false),
            }
        }
    }
}

#[component]
pub fn ProjectChip(
    project: Option<Uuid>,
    projects: Vec<(Uuid, String)>,
    on_change: EventHandler<Option<Uuid>>,
    #[props(default)] hotkey: Option<Signal<Option<Hotkey>>>,
    /// Offer "Create “…”" for a name no project has; the caller makes it.
    #[props(default)]
    on_create: Option<EventHandler<String>>,
    /// A project about to be made (named, no id yet) — shown as chosen.
    #[props(default)]
    pending: Option<String>,
) -> Element {
    let mut open = use_signal(|| false);
    follow_hotkey(hotkey, Hotkey::Project, open);
    let mut options = vec![
        PickOption::new("", "No project")
            .icon(rsx! { span { class: "text-muted-foreground", BoxIcon { size: 14 } } }),
    ];
    for (id, name) in &projects {
        options.push(
            PickOption::new(id.to_string(), name.clone())
                .icon(rsx! { span { class: "text-primary", BoxIcon { size: 14 } } }),
        );
    }
    let label = project
        .and_then(|p| {
            projects
                .iter()
                .find(|(id, _)| *id == p)
                .map(|(_, n)| n.clone())
        })
        .or(pending);
    rsx! {
        div { class: "relative",
            Chip {
                icon: rsx! { span { class: if label.is_some() { "text-primary" } else { "text-muted-foreground" }, BoxIcon { size: 14 } } },
                label: label.clone().unwrap_or_else(|| "Project".to_owned()),
                quiet: label.is_none(),
                title: "Project (R)",
                on_click: move |()| open.set(true),
            }
            PropertyPicker {
                open: open(),
                title: "Add to project…",
                hotkey: "R".to_owned(),
                options,
                selected: vec![project.map(|p| p.to_string()).unwrap_or_default()],
                number_from: None,
                on_pick: move |id: String| on_change.call(id.parse::<Uuid>().ok()),
                on_create,
                on_close: move |()| open.set(false),
            }
        }
    }
}

/// A stable colour per label name, for its dot.
#[must_use]
pub fn label_color(name: &str) -> String {
    let hue = name
        .to_lowercase()
        .bytes()
        .fold(7u32, |h, b| h.wrapping_mul(33).wrapping_add(u32::from(b)))
        % 360;
    format!("hsl({hue} 65% 60%)")
}

/// Labels are tags (areas are tags here too); `known` is every label the
/// person has used, `chosen` this item's. Toggling keeps the picker open;
/// typing a new one offers to create it.
#[component]
pub fn LabelsChip(
    chosen: Vec<String>,
    known: Vec<String>,
    on_toggle: EventHandler<String>,
    #[props(default)] hotkey: Option<Signal<Option<Hotkey>>>,
) -> Element {
    let mut open = use_signal(|| false);
    follow_hotkey(hotkey, Hotkey::Labels, open);
    let mut names: Vec<String> = known.clone();
    for c in &chosen {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(c)) {
            names.push(c.clone());
        }
    }
    names.sort_by_key(|n| n.to_lowercase());
    let options: Vec<PickOption> = names
        .iter()
        .map(|n| PickOption::new(n.clone(), n.clone()).dot(label_color(n)))
        .collect();
    rsx! {
        div { class: "relative flex flex-wrap items-center gap-1",
            if chosen.is_empty() {
                Chip {
                    icon: rsx! { span { class: "text-muted-foreground", Tag { size: 14 } } },
                    label: "Labels".to_owned(),
                    quiet: true,
                    title: "Labels (L)",
                    on_click: move |()| open.set(true),
                }
            } else {
                for c in chosen.iter() {
                    Chip {
                        key: "{c}",
                        icon: rsx! { span { class: "size-2 rounded-full", style: "background: {label_color(c)};" } },
                        label: c.clone(),
                        quiet: false,
                        title: "Labels (L)",
                        on_click: move |()| open.set(true),
                    }
                }
            }
            PropertyPicker {
                open: open(),
                title: "Add labels…",
                hotkey: "L".to_owned(),
                options,
                selected: chosen.clone(),
                multi: true,
                number_from: None,
                on_create: move |name: String| on_toggle.call(name),
                on_pick: move |id: String| on_toggle.call(id),
                on_close: move |()| open.set(false),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_values_read_in_the_linear_vocabulary() {
        assert_eq!(shown(STATUSES, canonical_status("open")), "Todo");
        assert_eq!(
            shown(STATUSES, canonical_status("in_progress")),
            "In Progress"
        );
        assert_eq!(shown(STATUSES, canonical_status("canceled")), "Canceled");
        assert_eq!(shown(PRIORITIES, canonical_priority("critical")), "Urgent");
        assert_eq!(shown(PRIORITIES, canonical_priority("normal")), "Medium");
        assert_eq!(shown(PRIORITIES, canonical_priority("")), "No priority");
    }

    #[test]
    fn the_five_keys_and_nothing_else() {
        assert_eq!(Hotkey::from_key("s"), Some(Hotkey::Status));
        assert_eq!(Hotkey::from_key("P"), Some(Hotkey::Priority));
        assert_eq!(Hotkey::from_key("l"), Some(Hotkey::Labels));
        assert_eq!(Hotkey::from_key("x"), None);
    }

    #[test]
    fn a_label_keeps_its_colour() {
        assert_eq!(label_color("mixing"), label_color("Mixing"));
        assert_ne!(label_color("mixing"), label_color("mastering"));
    }
}
