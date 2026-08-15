//! Organization switcher + per-org theme picker.
//!
//! The dropdown drives the **data** selection — `All` (every hosted
//! org, the default) or a single org — via the `Signal<OrgSelection>`
//! context; the hosted-org list comes from the `Signal<Vec<OrgMeta>>`
//! context (discovered at app start). The Palette button is the
//! per-org theme picker, unchanged.

use architect_ui::lucide_dioxus::Palette;
use architect_ui::prelude::*;
use architect_ui::primitives::{ContentAlign, ContentSide};
use dioxus::prelude::*;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::theming::use_org_theme_switcher_state;

#[component]
pub fn OrgSwitcher(#[props(default = false)] compact: bool) -> Element {
    // Data selection + discovered org list.
    let mut selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    let mut open = use_signal(|| false);
    let mut theme_open = use_signal(|| false);

    // Only orgs this session actually belongs to — the switcher must not
    // offer (or let "All" span) orgs the server would refuse anyway
    // (#109 criterion 6). Signed out, this is the full hosted list, which
    // is what the sign-in path needs.
    let list = crate::orgs::my_orgs(&org_list());
    let current = selection();
    let trigger_label = current.label(&list);

    // Theme picker state — the shared active-org ↔ overrides bridge
    // (see `theming::use_org_theme_switcher_state`).
    let switcher_state = use_org_theme_switcher_state();

    rsx! {
        HStack { class: if compact { "items-center gap-1" } else { "items-center gap-1 w-full" },
            Dropdown {
                open: open(),
                on_open_change: move |o| open.set(o),
                class: if compact { "" } else { "w-full flex-1" },
                DropdownTrigger { class: if compact { "" } else { "w-full" },
                    button {
                        r#type: "button",
                        class: "flex items-center gap-2 rounded-full border border-border bg-card px-2.5 py-1.5 text-xs font-semibold text-foreground hover:bg-accent",
                        span { class: "max-w-[10rem] truncate", "{trigger_label}" }
                    }
                }
                DropdownContent {
                    side: if compact { "bottom" } else { "top" },
                    align: "end",
                    width: "w-64",
                    DropdownLabel { "View organization" }
                    // All — the default aggregate view.
                    DropdownItem {
                        value: "__all".to_string(),
                        index: 0,
                        on_select: move |_| {
                            selection.set(OrgSelection::All);
                            open.set(false);
                        },
                        div { class: "flex w-full items-center justify-between gap-2",
                            span { "All organizations" }
                            if current == OrgSelection::All {
                                span { class: "text-xs text-primary", "●" }
                            }
                        }
                    }
                    for (idx, org) in list.iter().enumerate() {
                        {
                            let slug = org.slug.clone();
                            let selected = current == OrgSelection::One(slug.clone());
                            rsx! {
                                DropdownItem {
                                    key: "{org.slug}",
                                    value: org.slug.clone(),
                                    index: idx + 1,
                                    on_select: move |_| {
                                        selection.set(OrgSelection::One(slug.clone()));
                                        open.set(false);
                                    },
                                    div { class: "flex w-full items-center justify-between gap-2",
                                        span { class: "truncate", "{org.name}" }
                                        if selected {
                                            span { class: "text-xs text-primary", "●" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Popover {
                open: theme_open(),
                is_modal: false,
                on_open_change: move |o| theme_open.set(o),
                PopoverTrigger { class: "inline-flex",
                    button {
                        r#type: "button",
                        class: "inline-flex h-8 w-8 items-center justify-center rounded-md border border-border bg-card text-muted-foreground hover:bg-accent hover:text-accent-foreground",
                        title: "Theme",
                        onclick: move |_| {
                            let v = !*theme_open.read();
                            theme_open.set(v);
                        },
                        Palette { size: 14 }
                    }
                }
                PopoverContent {
                    side: ContentSide::Top,
                    align: ContentAlign::End,
                    class: "w-[17rem] p-3 max-h-[70vh] overflow-y-auto",
                    div { class: "flex flex-col gap-2",
                        span { class: "text-xs font-semibold uppercase tracking-widest text-muted-foreground",
                            "Theme"
                        }
                        ThemeSwitcher { state: switcher_state }
                    }
                }
            }
        }
    }
}

// ── rail skins ──────────────────────────────────────────────────────
//
// The icon rail's foot carries the same jobs as icon-sized pieces,
// split so the rail controls their order (theme / account / org).

/// Rail theme button: Palette icon opening the per-org theme picker —
/// the same state bridge as [`OrgSwitcher`]'s Palette popover.
#[component]
pub fn RailThemeButton() -> Element {
    let mut theme_open = use_signal(|| false);

    // The shared active-org ↔ overrides bridge (see
    // `theming::use_org_theme_switcher_state`).
    let switcher_state = use_org_theme_switcher_state();

    rsx! {
        Popover {
            open: theme_open(),
            is_modal: false,
            on_open_change: move |o| theme_open.set(o),
            PopoverTrigger { class: "inline-flex",
                button {
                    r#type: "button",
                    class: "flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground hover:bg-accent/50 hover:text-foreground",
                    title: "Theme",
                    onclick: move |_| {
                        let v = !*theme_open.read();
                        theme_open.set(v);
                    },
                    Palette { size: 15 }
                }
            }
            PopoverContent {
                side: ContentSide::Top,
                align: ContentAlign::Start,
                class: "w-[17rem] p-3 max-h-[70vh] overflow-y-auto",
                div { class: "flex flex-col gap-2",
                    span { class: "text-xs font-semibold uppercase tracking-widest text-muted-foreground",
                        "Theme"
                    }
                    ThemeSwitcher { state: switcher_state }
                }
            }
        }
    }
}

/// Rail org button: the active org's initials as a chip (the rail's
/// brand spot), opening the org selection menu.
#[component]
pub fn RailOrgButton() -> Element {
    let mut selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let mut open = use_signal(|| false);

    // Only orgs this session actually belongs to — the switcher must not
    // offer (or let "All" span) orgs the server would refuse anyway
    // (#109 criterion 6). Signed out, this is the full hosted list, which
    // is what the sign-in path needs.
    let list = crate::orgs::my_orgs(&org_list());
    let current = selection();
    let trigger_label = current.label(&list);
    // Initials chip: "All" for the aggregate view, the org name's
    // initials otherwise.
    let chip = match &current {
        OrgSelection::All => "All".to_string(),
        OrgSelection::One(slug) => {
            let name = list
                .iter()
                .find(|o| &o.slug == slug)
                .map(|o| o.name.clone())
                .unwrap_or_else(|| slug.clone());
            crate::auth::initials(&name)
        }
    };

    rsx! {
        Dropdown {
            open: open(),
            on_open_change: move |o| open.set(o),
            DropdownTrigger {
                button {
                    r#type: "button",
                    class: "flex h-8 w-8 items-center justify-center rounded-lg bg-primary/15 text-[10px] font-bold uppercase tracking-wide text-primary hover:bg-primary/25",
                    title: "Organization — {trigger_label}",
                    "{chip}"
                }
            }
            DropdownContent { side: "top", align: "start", width: "w-64",
                DropdownLabel { "View organization" }
                DropdownItem {
                    value: "__all".to_string(),
                    index: 0,
                    on_select: move |_| {
                        selection.set(OrgSelection::All);
                        open.set(false);
                    },
                    div { class: "flex w-full items-center justify-between gap-2",
                        span { "All organizations" }
                        if current == OrgSelection::All {
                            span { class: "text-xs text-primary", "●" }
                        }
                    }
                }
                for (idx, org) in list.iter().enumerate() {
                    {
                        let slug = org.slug.clone();
                        let selected = current == OrgSelection::One(slug.clone());
                        rsx! {
                            DropdownItem {
                                key: "{org.slug}",
                                value: org.slug.clone(),
                                index: idx + 1,
                                on_select: move |_| {
                                    selection.set(OrgSelection::One(slug.clone()));
                                    open.set(false);
                                },
                                div { class: "flex w-full items-center justify-between gap-2",
                                    span { class: "truncate", "{org.name}" }
                                    if selected {
                                        span { class: "text-xs text-primary", "●" }
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
