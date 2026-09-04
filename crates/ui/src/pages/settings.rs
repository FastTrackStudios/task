//! `/settings` — per-user preferences, backed by the server-side
//! `UserPrefs` entity (see [`crate::prefs`]): every control here
//! writes through `PrefsCtx::update`, so choices follow the account
//! to any device.

use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::prefs::PrefsCtx;
use crate::shortcuts::{BINDINGS, Binding, effective_sequence};
use crate::theming::use_org_theme_switcher_state;

/// The routes worth landing on. `""` = the app default (`/`).
const PAGE_CHOICES: &[(&str, &str)] = &[
    ("", "Tasks (default)"),
    ("/home", "Active work dashboard"),
    ("/inbox", "Inbox"),
    ("/projects", "Projects"),
    ("/schedule", "Schedule"),
    ("/vault", "Vault"),
];

#[component]
pub fn SettingsView() -> Element {
    let prefs_ctx = use_context::<PrefsCtx>();
    let prefs = prefs_ctx.prefs.read().clone();
    // Start-page picker binds a String signal; on_change writes it through
    // to the prefs context (the source of truth).
    let page_pick = use_signal(|| prefs.default_page.clone());

    // Same active-org ↔ overrides bridge as the shell's theme pickers;
    // persistence rides the App-root `use_theme_prefs_sync` effect.
    let theme_state = use_org_theme_switcher_state();

    rsx! {
        div { class: "mx-auto flex max-w-5xl flex-col gap-6 p-4 sm:p-6 lg:p-10",
            Heading { level: HeadingLevel::H1, "Settings" }

            section { class: "flex flex-col gap-3",
                Heading { level: HeadingLevel::H3, "Appearance" }
                Text {
                    variant: TextVariant::Muted,
                    "Theme preset and light/dark mode for the active organization. Follows your account on every device."
                }
                ThemeSwitcher { state: theme_state, class: "max-w-md" }
            }

            // Linked accounts live with the account, on the issuer:
            // one place to link GitHub and Google for every app. An
            // accepted Edit Request on a repo-sourced wiki lands as the
            // linked GitHub account, so this is where a wiki Editor
            // goes first.
            if let Some(issuer) = task_ui_core::central_auth::issuer() {
                section { class: "flex flex-col gap-3",
                    Heading { level: HeadingLevel::H3, "Account" }
                    Text {
                        variant: TextVariant::Muted,
                        "Link your GitHub and Google accounts to your FastTrackStudio account. Changes you accept on a repository-backed wiki are pushed and proposed as your GitHub account."
                    }
                    a {
                        class: "text-sm font-medium text-primary underline-offset-4 hover:underline",
                        href: "{issuer.trim_end_matches('/')}/account",
                        target: "_blank",
                        rel: "noopener",
                        "Manage linked accounts ↗"
                    }
                }
            }

            section { class: "flex flex-col gap-3",
                Heading { level: HeadingLevel::H3, "Start page" }
                Text {
                    variant: TextVariant::Muted,
                    "Where the app opens. Follows your account on every device."
                }
                Select {
                    value: page_pick,
                    placeholder: "Start page".to_string(),
                    class: "w-64".to_string(),
                    on_change: move |v: String| prefs_ctx.update(|p| p.default_page = v.clone()),
                    SelectContent {
                        for (i, (value, label)) in PAGE_CHOICES.iter().enumerate() {
                            SelectItem { key: "{value}", value: "{value}", index: i, "{label}" }
                        }
                    }
                }
            }

            section { class: "flex flex-col gap-3",
                Heading { level: HeadingLevel::H3, "Keyboard" }
                Text {
                    variant: TextVariant::Muted,
                    "App-wide shortcuts. One canonical keymap everywhere; combos the browser refuses to yield (Ctrl+N/T/W) are remapped on web. Rebinding arrives later."
                }
                label { class: "flex items-center gap-2 text-sm text-foreground",
                    input {
                        r#type: "checkbox",
                        checked: prefs.shortcuts_priority,
                        onchange: move |e| prefs_ctx.update(|p| p.shortcuts_priority = e.checked()),
                    }
                    "Prioritize app shortcuts"
                    span { class: "text-xs text-muted-foreground",
                        "(app shortcuts win over browser defaults like Ctrl+P print)"
                    }
                }
                label { class: "flex items-center gap-2 text-sm text-foreground",
                    input {
                        r#type: "checkbox",
                        checked: prefs.vim_mode,
                        onchange: move |e| prefs_ctx.update(|p| p.vim_mode = e.checked()),
                    }
                    "Vim keybindings in the note editor"
                    span { class: "text-xs text-muted-foreground",
                        "(off by default — when on, notes open in NORMAL mode)"
                    }
                }
                div { class: "overflow-x-auto rounded-lg border border-border",
                    table { class: "w-full min-w-[28rem] border-collapse text-sm",
                        thead {
                            tr { class: "border-b border-border text-left text-xs uppercase tracking-wide text-muted-foreground",
                                th { class: "px-3 py-2 font-medium", "Shortcut" }
                                th { class: "px-3 py-2 font-medium", "Action" }
                                th { class: "px-3 py-2 font-medium", "Notes" }
                            }
                        }
                        tbody {
                            for def in BINDINGS {
                                ShortcutRow {
                                    key: "{def.action}-{def.seq}",
                                    def,
                                    priority_on: prefs.shortcuts_priority,
                                }
                            }
                        }
                    }
                }
            }

            section { class: "flex flex-col gap-3",
                Heading { level: HeadingLevel::H3, "Task board defaults" }
                Text {
                    variant: TextVariant::Muted,
                    "The filters the board opens with (also toggleable inline on the board)."
                }
                div { class: "flex items-center gap-4",
                    label { class: "flex items-center gap-2 text-sm text-foreground",
                        input {
                            r#type: "checkbox",
                            checked: prefs.tasks_active,
                            onchange: move |e| prefs_ctx.update(|p| p.tasks_active = e.checked()),
                        }
                        "Active only"
                    }
                    label { class: "flex items-center gap-2 text-sm text-foreground",
                        input {
                            r#type: "checkbox",
                            checked: prefs.tasks_relevant,
                            onchange: move |e| prefs_ctx.update(|p| p.tasks_relevant = e.checked()),
                        }
                        "Relevant only"
                    }
                }
            }
        }
    }
}

/// One read-only keymap row: the effective chord(s) as keycaps, the
/// action description, and provenance notes (web remap of a reserved
/// combo / overrides a browser default / inactive while the priority
/// toggle is off).
#[component]
fn ShortcutRow(def: &'static Binding, priority_on: bool) -> Element {
    let inactive = def.overrides_browser && !priority_on;
    let seq = effective_sequence(def);
    rsx! {
        tr {
            class: if inactive {
                "border-b border-border/60 text-muted-foreground/60 last:border-0"
            } else {
                "border-b border-border/60 last:border-0"
            },
            td { class: "whitespace-nowrap px-3 py-2",
                for (i, chord) in seq.split(' ').enumerate() {
                    if i > 0 {
                        span { class: "px-1 text-xs text-muted-foreground", "then" }
                    }
                    kbd { class: "rounded border border-border bg-card px-1.5 py-0.5 font-mono text-xs uppercase",
                        "{chord}"
                    }
                }
            }
            td { class: "px-3 py-2", "{def.label}" }
            td { class: "px-3 py-2 text-xs text-muted-foreground",
                if def.web.is_some() {
                    span { class: "mr-2", "web remap of {def.seq} (browser-reserved)" }
                }
                if def.overrides_browser {
                    if inactive {
                        span { "inactive — overrides a browser shortcut" }
                    } else {
                        span { "overrides a browser shortcut" }
                    }
                }
            }
        }
    }
}
