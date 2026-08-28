//! The project sidebar — what the explorer column shows on
//! `/projects/:id`.
//!
//! On a project page the vault-as-a-whole is the wrong companion: the
//! reader is inside ONE piece of work, and the column beside it should
//! be that work's own map — its parts, its numbers, and the way to the
//! neighbouring projects — the way the Files page swaps the same column
//! for its file tree. The vault is one click away (its own page), not
//! gone.

use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::format::status_variant;
use crate::routes::Route;
use crate::stores;
use crate::task_sort::is_active;

/// One project's own navigation column.
#[component]
pub fn ProjectSidebar(id: String) -> Element {
    let project_res = stores::use_project(id.clone());
    let project_store = stores::use_project_store();

    // The neighbours: every other active project, for one-click moves
    // between pieces of work without a detour through /projects.
    let others: Vec<(String, String)> = project_store
        .list()
        .iter()
        .filter(|r| {
            !r.project.archived
                && is_active(&r.project.status)
                && r.project.id.to_string() != id
        })
        .map(|r| (r.project.id.to_string(), r.project.title.clone()))
        .collect();

    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-3",
            Link {
                to: Route::ProjectsRoute {},
                class: "text-xs text-muted-foreground hover:text-foreground",
                "‹ All projects"
            }

            match project_res.value() {
                Some(op) => {
                    let p = op.project.clone();
                    rsx! {
                        // Identity block: the same vocabulary as the
                        // page's masthead, at column width.
                        div { class: "flex flex-col gap-1.5",
                            if let Some(form) = p.form {
                                span { class: "text-[10px] font-semibold uppercase tracking-[0.14em] text-primary",
                                    "{form.as_str()}"
                                }
                            }
                            span { class: "break-words text-sm font-semibold leading-snug text-foreground",
                                "{p.title}"
                            }
                            div { class: "flex flex-wrap items-center gap-1.5",
                                StatusBadge { variant: status_variant(&p.status), label: p.status.clone() }
                                for cap in p.capabilities.held.iter() {
                                    span {
                                        key: "{cap.as_str()}",
                                        class: "rounded-full border border-border px-1.5 py-0.5 text-[10px] text-muted-foreground",
                                        "{cap.as_str()}"
                                    }
                                }
                            }
                        }

                        // The tracklist, in miniature — the same order
                        // the page shows, so the column reads as the
                        // work's table of contents.
                        if !p.parts.0.is_empty() {
                            div { class: "flex flex-col gap-1",
                                span { class: "px-1 text-[10px] font-semibold uppercase tracking-widest text-muted-foreground",
                                    "Parts"
                                }
                                div { class: "flex flex-col",
                                    for (i, part) in p.parts.0.iter().enumerate() {
                                        div {
                                            key: "{part.id}",
                                            class: "flex items-center gap-2 rounded-md px-1.5 py-1 text-sm text-muted-foreground",
                                            span { class: "w-5 shrink-0 text-right font-mono text-[10px] tabular-nums",
                                                {format!("{:02}", i + 1)}
                                            }
                                            span { class: "min-w-0 truncate", "{part.name}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                None => rsx! {
                    div { class: "flex flex-col gap-2 px-1",
                        Skeleton { class: "h-4 w-32" }
                        Skeleton { class: "h-3 w-24" }
                    }
                },
            }

            if !others.is_empty() {
                div { class: "flex flex-col gap-1",
                    span { class: "px-1 text-[10px] font-semibold uppercase tracking-widest text-muted-foreground",
                        "Other projects"
                    }
                    div { class: "flex flex-col",
                        for (pid, title) in others.iter() {
                            Link {
                                key: "{pid}",
                                to: Route::ProjectDetailRoute { id: pid.clone() },
                                class: "truncate rounded-md px-1.5 py-1 text-sm text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground",
                                "{title}"
                            }
                        }
                    }
                }
            }

            div { class: "mt-auto flex flex-col gap-0.5 border-t border-border/60 pt-2",
                Link {
                    to: Route::FilesRoute {},
                    class: "rounded-md px-1.5 py-1 text-xs text-muted-foreground transition-colors hover:text-foreground",
                    "Files"
                }
                Link {
                    to: Route::VaultRoute { path: String::new(), org: String::new() },
                    class: "rounded-md px-1.5 py-1 text-xs text-muted-foreground transition-colors hover:text-foreground",
                    "Vault"
                }
            }
        }
    }
}
