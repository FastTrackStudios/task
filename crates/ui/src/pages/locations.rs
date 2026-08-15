//! `/locations` — the place register.
//!
//! Locations are the studios, rooms, storage units, venues, and
//! homes the org works out of. They live as markdown pages in the
//! vault (`type: location`) and carry a stable `id` so other
//! features (notably inventory) reference them through renames.
//!
//! This page lists the org's locations and offers a friction-light
//! "Add location" form (name + kind + optional address). State is the
//! shared optimistic store ([`crate::stores`]): the list renders one
//! `AtomResult` (stale-while-revalidate across org switches), creates
//! appear instantly as typed `Id::Temp` rows, and failures roll back
//! and surface in the notification tray.

use architect::Id;
use dioxus::prelude::*;
use architect_ui::prelude::*;
use locations_proto::Location;
use uuid::Uuid;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores;

const INPUT_CLS: &str = "rounded-lg border border-input bg-input/30 px-3 py-2 text-sm transition-colors \
     focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] \
     focus-visible:ring-ring/50 placeholder:text-muted-foreground";

/// Canonical kinds offered in the form's picker. `kind` is free-form
/// on the model, but these cover the common cases without forcing it.
const KINDS: &[&str] = &["studio", "room", "storage", "venue", "home", "other"];

#[component]
pub fn LocationsView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The org we create into (first selected, or home).
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    let mut name = use_signal(String::new);
    let kind = use_signal(|| "other".to_string());
    let mut address = use_signal(String::new);

    // The shared store: one AtomResult for the list, optimistic create.
    let result = stores::use_location_list();
    let muts = stores::use_location_mutations();

    let mut create = move || {
        let n = name.read().trim().to_string();
        if n.is_empty() {
            return;
        }
        let Some(s) = slug() else { return };
        let k = kind.read().clone();
        let addr = {
            let a = address.read().trim().to_string();
            if a.is_empty() { None } else { Some(a) }
        };
        name.set(String::new());
        address.set(String::new());
        muts.create(s, stores::draft_location(n, k, addr));
    };

    let store = stores::use_location_store();
    let rows: Vec<(Id<Uuid>, Location)> = result.value().cloned().unwrap_or_default();
    let load_err = result.error().cloned();
    let first_load = result.is_waiting() && result.value().is_none();

    rsx! {
        div { class: "mx-auto flex max-w-3xl flex-col gap-5 p-4 sm:p-6 lg:p-10",
            div { class: "flex items-center justify-between gap-3",
                Heading { level: HeadingLevel::H1, "Locations" }
                Text { variant: TextVariant::Muted, class: "text-sm", "{rows.len()} places" }
            }
            Text {
                variant: TextVariant::Muted,
                class: "text-sm -mt-2",
                "Studios, rooms, storage, venues, and homes you work out of.",
            }

            // ── Add location ───────────────────────────────────────
            div { class: "flex flex-col gap-2 rounded-xl border border-border bg-card/40 p-3 sm:flex-row sm:items-center",
                input {
                    class: "{INPUT_CLS} flex-1",
                    placeholder: "Location name…",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
                        }
                    },
                }
                Select {
                    value: kind,
                    placeholder: "Kind".to_string(),
                    SelectContent {
                        for (i, k) in KINDS.iter().enumerate() {
                            SelectItem { key: "{k}", value: "{k}", index: i, "{k}" }
                        }
                    }
                }
                input {
                    class: "{INPUT_CLS} flex-1",
                    placeholder: "Address (optional)",
                    value: "{address}",
                    oninput: move |e| address.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
                        }
                    },
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
                        title: "Couldn't load locations",
                        message: err,
                        on_retry: move |()| store.reload(),
                    }
                } else {
                    crate::states::EmptyState {
                        title: "No locations yet",
                        hint: "Add your first place above.",
                    }
                }
            } else {
                div { class: "flex flex-col gap-2",
                    for (id, loc) in rows {
                        LocationRow { key: "{id}", pending: id.is_temp(), loc }
                    }
                }
            }
        }
    }
}

/// One location in the register: name + kind badge + optional address.
/// `pending` marks an optimistic row whose write-through is in flight
/// (dimmed); a failed write rolls the row back and reports to the tray.
#[component]
fn LocationRow(loc: Location, pending: bool) -> Element {
    let name = loc.name.clone();
    let kind = loc.kind.clone();
    let address = loc.address.clone();

    let state_cls = if pending {
        "border-border bg-card/40 opacity-60"
    } else {
        "border-border bg-card/40"
    };

    rsx! {
        div { class: "flex items-start gap-3 rounded-lg border px-3 py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-1",
                Text { class: "break-words text-sm font-medium", "{name}" }
                if let Some(addr) = address.as_ref() {
                    span { class: "text-[11px] text-muted-foreground", "{addr}" }
                }
            }
            div { class: "flex shrink-0 items-center gap-2",
                span { class: "rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "{kind}" }
            }
        }
    }
}
