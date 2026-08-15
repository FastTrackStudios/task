//! `/inventory` — the gear register.
//!
//! Inventory items are the instruments, amps, mics, cables,
//! outboard, and computers the org owns. They live as markdown
//! pages in the vault (`type: item`) and reference their physical
//! [`location`](crate::pages::locations) by id so location renames
//! don't break the link.
//!
//! This page lists the org's items and offers a friction-light "Add
//! item" form (name + category). State is the shared optimistic store
//! ([`crate::stores`]): one `AtomResult` list, typed `Id::Temp` rows
//! for in-flight creates, rollback + tray notification on failure.

use architect::Id;
use dioxus::prelude::*;
use architect_ui::prelude::*;
use inventory_proto::{Item, Status};
use uuid::Uuid;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores;

const INPUT_CLS: &str = "rounded-lg border border-input bg-input/30 px-3 py-2 text-sm transition-colors \
     focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] \
     focus-visible:ring-ring/50 placeholder:text-muted-foreground";

/// Common categories offered in the form's picker. `category` is
/// free-form on the model; these cover the usual gear without
/// forcing the set.
const CATEGORIES: &[&str] = &[
    "guitar", "amp", "mic", "cable", "outboard", "computer", "other",
];

/// Map a free-form status string onto a status-badge variant.
/// Unknown values fall back to neutral.
fn status_variant(status: &str) -> StatusBadgeVariant {
    match Status::from_str(status) {
        Some(Status::InUse) => StatusBadgeVariant::Success,
        Some(Status::InRepair | Status::Loaned) => StatusBadgeVariant::Warning,
        Some(Status::Missing) => StatusBadgeVariant::Danger,
        _ => StatusBadgeVariant::Neutral,
    }
}

#[component]
pub fn InventoryView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The org we create into (first selected, or home).
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    let mut name = use_signal(String::new);
    let category = use_signal(|| "other".to_string());

    // The shared store: one AtomResult for the list, optimistic create.
    let result = stores::use_item_list();
    let muts = stores::use_item_mutations();

    let mut create = move || {
        let n = name.read().trim().to_string();
        if n.is_empty() {
            return;
        }
        let Some(s) = slug() else { return };
        let c = category.read().clone();
        name.set(String::new());
        muts.create(s, stores::draft_item(n, c));
    };

    let store = stores::use_item_store();
    let rows: Vec<(Id<Uuid>, Item)> = result.value().cloned().unwrap_or_default();
    let load_err = result.error().cloned();
    let first_load = result.is_waiting() && result.value().is_none();

    rsx! {
        div { class: "mx-auto flex max-w-3xl flex-col gap-5 p-4 sm:p-6 lg:p-10",
            div { class: "flex items-center justify-between gap-3",
                Heading { level: HeadingLevel::H1, "Inventory" }
                Text { variant: TextVariant::Muted, class: "text-sm", "{rows.len()} items" }
            }
            Text {
                variant: TextVariant::Muted,
                class: "text-sm -mt-2",
                "Instruments, amps, mics, cables, and gear the org owns.",
            }

            // ── Add item ───────────────────────────────────────────
            div { class: "flex flex-col gap-2 rounded-xl border border-border bg-card/40 p-3 sm:flex-row sm:items-center",
                input {
                    class: "{INPUT_CLS} flex-1",
                    placeholder: "Item name…",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
                        }
                    },
                }
                Select {
                    value: category,
                    placeholder: "Category".to_string(),
                    SelectContent {
                        for (i, c) in CATEGORIES.iter().enumerate() {
                            SelectItem { key: "{c}", value: "{c}", index: i, "{c}" }
                        }
                    }
                }
                Button {
                    variant: ButtonVariant::Primary,
                    on_click: move |_| create(),
                    "Add"
                }
            }

            // ── The register ───────────────────────────────────────
            if first_load {
                crate::states::LoadingState {}
            } else if rows.is_empty() {
                if let Some(err) = load_err {
                    crate::states::ErrorState {
                        title: "Couldn't load inventory",
                        message: err,
                        on_retry: move |()| store.reload(),
                    }
                } else {
                    crate::states::EmptyState {
                        title: "No items yet",
                        hint: "Add your first piece of gear above.",
                    }
                }
            } else {
                div { class: "flex flex-col gap-2",
                    for (id, item) in rows {
                        ItemRow { key: "{id}", pending: id.is_temp(), item }
                    }
                }
            }
        }
    }
}

/// One item in the register: name + category + condition + status
/// badge + optional value / location. `pending` dims an optimistic row
/// whose write-through is in flight; failures roll back + notify.
#[component]
fn ItemRow(item: Item, pending: bool) -> Element {
    let name = item.name.clone();
    let category = item.category.clone();
    let condition = item.condition.clone();
    let status = item.status.clone();
    let value = item.value;
    let located = item.location_id.is_some();

    let state_cls = if pending {
        "border-border bg-card/40 opacity-60"
    } else {
        "border-border bg-card/40"
    };

    rsx! {
        div { class: "flex items-start gap-3 rounded-lg border px-3 py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-1",
                Text { class: "break-words text-sm font-medium", "{name}" }
                div { class: "flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground",
                    if !category.is_empty() {
                        span { class: "rounded bg-muted px-1.5 py-px", "{category}" }
                    }
                    span { "cond: {condition}" }
                    if let Some(v) = value {
                        span { "≈ {v}" }
                    }
                    if located {
                        span { "located" }
                    }
                }
            }
            div { class: "flex shrink-0 items-center gap-2",
                StatusBadge { variant: status_variant(&status), label: status.clone() }
            }
        }
    }
}
