//! `<space><space>` — the telescope-style **everything search**.
//!
//! A large centered window (fzf/telescope shape): query input + ranked
//! result list on the left, a live preview of the selection on the
//! right. One index over *everything reachable* — vault notes across
//! the selected orgs, tasks, projects, and app commands — ranked by
//! [`neo_frizbee`], the same SIMD Smith-Waterman scorer the fff finder
//! embeds (scalar fallback on wasm).
//!
//! Toggled by the [`crate::chrome::SearchOpen`] context signal
//! (`<space><space>` via `fts.search.all`, or the palette entry).

use architect_ui::lucide_dioxus::{CircleCheck, FileText, Folder, Search, Zap};
use dioxus::prelude::*;

use crate::orgs::OrgMeta;
use crate::routes::Route;

/// Rows kept after ranking — enough to scroll, cheap to render.
const MAX_RESULTS: usize = 60;

/// What a result row IS — drives the icon, the tag, and what Enter
/// does.
#[derive(Clone, PartialEq)]
enum Hit {
    Note { path: String, slug: String },
    Task { id: uuid::Uuid },
    Project { id: uuid::Uuid },
    Command { id: String },
}

/// One searchable candidate: the hit + its display/match text.
#[derive(Clone, PartialEq)]
struct Candidate {
    label: String,
    /// Secondary text — path for notes, status/project for tasks,
    /// shortcut for commands. Matched too (label-weighted first).
    detail: String,
    hit: Hit,
}

impl Candidate {
    fn kind_tag(&self) -> &'static str {
        match self.hit {
            Hit::Note { .. } => "note",
            Hit::Task { .. } => "task",
            Hit::Project { .. } => "project",
            Hit::Command { .. } => "command",
        }
    }
}

/// A ranked row ready to render: the candidate + the matched char
/// positions inside `label` / `detail` (for highlight spans).
#[derive(Clone, PartialEq)]
struct Ranked {
    cand: Candidate,
    label_marks: Vec<usize>,
    detail_marks: Vec<usize>,
}

/// Rank `cands` against `query` with neo_frizbee; empty query keeps
/// the assembly order (notes, tasks, projects, commands).
fn rank(query: &str, cands: &[Candidate]) -> Vec<Ranked> {
    if query.trim().is_empty() {
        return cands
            .iter()
            .take(MAX_RESULTS)
            .map(|c| Ranked {
                cand: c.clone(),
                label_marks: Vec::new(),
                detail_marks: Vec::new(),
            })
            .collect();
    }
    let cfg = neo_frizbee::Config::default();
    let hays: Vec<String> = cands
        .iter()
        .map(|c| {
            if c.detail.is_empty() {
                c.label.clone()
            } else {
                format!("{} {}", c.label, c.detail)
            }
        })
        .collect();
    let mut matches = neo_frizbee::match_list(query, &hays, &cfg);
    // `sort` in the default config already orders by score, but be
    // explicit — the render order IS the ranking.
    matches.sort_by(|a, b| b.score.cmp(&a.score));
    matches
        .into_iter()
        .take(MAX_RESULTS)
        .map(|m| {
            let i = m.index as usize;
            let cand = cands[i].clone();
            // Highlight positions for just this row (the indices path
            // is the slow one — run it per visible row only). Char
            // indices into the combined haystack; split at the label
            // boundary.
            let mut marks: Vec<usize> =
                neo_frizbee::match_list_indices(query, &[hays[i].as_str()], &cfg)
                    .into_iter()
                    .next()
                    .map(|mi| mi.indices.into_iter().collect())
                    .unwrap_or_default();
            marks.sort_unstable();
            let label_chars = cand.label.chars().count();
            let (label_marks, rest): (Vec<usize>, Vec<usize>) =
                marks.into_iter().partition(|&p| p < label_chars);
            let detail_marks = rest
                .into_iter()
                .filter_map(|p| p.checked_sub(label_chars + 1))
                .collect();
            Ranked {
                cand,
                label_marks,
                detail_marks,
            }
        })
        .collect()
}

/// `text` with the chars at `marks` emphasized — consecutive runs
/// merged into single spans.
fn highlight(text: &str, marks: &[usize]) -> Element {
    let set: std::collections::HashSet<usize> = marks.iter().copied().collect();
    let mut runs: Vec<(String, bool)> = Vec::new();
    for (i, ch) in text.chars().enumerate() {
        let hot = set.contains(&i);
        match runs.last_mut() {
            Some((s, h)) if *h == hot => s.push(ch),
            _ => runs.push((ch.to_string(), hot)),
        }
    }
    rsx! {
        for (i, (s, hot)) in runs.into_iter().enumerate() {
            if hot {
                span { key: "{i}", class: "text-primary", "{s}" }
            } else {
                span { key: "{i}", "{s}" }
            }
        }
    }
}

/// The full-window search overlay. Render once (alongside the palette
/// overlays); shows nothing while [`crate::chrome::SearchOpen`] is off.
#[component]
pub fn SearchOverlay() -> Element {
    let mut open = use_context::<crate::chrome::SearchOpen>().0;
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let actions = use_context::<crate::actions::ActionsCtx>();
    let nav = use_navigator();

    // Shared stores (atom-cached with every other page's hooks).
    let _tasks = crate::stores::use_task_list();
    let _projects = crate::stores::use_project_list();
    let task_store = crate::stores::use_task_store();
    let project_store = crate::stores::use_project_store();

    let mut query = use_signal(String::new);
    let mut cursor = use_signal(|| 0usize);

    let slugs = use_memo(move || crate::orgs::selected_slugs(&selection.read(), &org_list.read()));
    let active = use_memo(move || crate::orgs::active_slug(&selection.read(), &org_list.read()));

    // Vault notes across the selected orgs — fetched on open, like the
    // omni-picker.
    let notes = use_resource(move || {
        let want = open();
        let slugs = slugs();
        async move {
            if !want {
                return Vec::new();
            }
            let mut out = Vec::new();
            for slug in slugs {
                if let Ok(pages) = crate::pages::vault::fetch_folder_index(
                    slug.clone(),
                    crate::document_session::VAULT_ID.to_owned(),
                )
                .await
                {
                    for p in pages {
                        out.push((p, slug.clone()));
                    }
                }
            }
            out
        }
    });

    let candidates = use_memo(move || {
        if !open() {
            return Vec::new();
        }
        let mut out: Vec<Candidate> = Vec::new();
        for (p, slug) in notes.read().as_deref().unwrap_or_default() {
            out.push(Candidate {
                label: p.title.clone(),
                detail: p.path.clone(),
                hit: Hit::Note {
                    path: p.path.clone(),
                    slug: slug.clone(),
                },
            });
        }
        let project_names: std::collections::HashMap<uuid::Uuid, String> = project_store
            .list()
            .iter()
            .map(|r| (r.project.id, r.project.title.clone()))
            .collect();
        for r in task_store.list() {
            let t = &r.task;
            let project = t
                .project_id
                .and_then(|id| project_names.get(&id).cloned())
                .or_else(|| t.projects.0.first().cloned())
                .unwrap_or_default();
            let detail = if project.is_empty() {
                t.status.clone()
            } else {
                format!("{} · {}", project, t.status)
            };
            out.push(Candidate {
                label: t.title.clone(),
                detail,
                hit: Hit::Task { id: t.id },
            });
        }
        for r in project_store.list() {
            out.push(Candidate {
                label: r.project.title.clone(),
                detail: r.project.status.clone(),
                hit: Hit::Project { id: r.project.id },
            });
        }
        for def in crate::actions::task_action_defs() {
            out.push(Candidate {
                label: def.name.clone(),
                detail: def.shortcut_hint.clone().unwrap_or_default(),
                hit: Hit::Command {
                    id: def.id.to_string(),
                },
            });
        }
        out
    });

    let ranked = use_memo(move || rank(&query.read(), &candidates.read()));

    // Keep the cursor on a real row as the ranking narrows.
    use_effect(move || {
        let len = ranked.read().len();
        if *cursor.peek() >= len.max(1) {
            cursor.set(len.saturating_sub(1));
        }
    });

    // ── Preview of the selected row ───────────────────────────
    let selected = use_memo(move || ranked.read().get(*cursor.read()).map(|r| r.cand.clone()));
    let preview = use_resource(move || {
        let sel = selected();
        async move {
            match sel {
                Some(Candidate {
                    hit: Hit::Note { path, slug },
                    ..
                }) => fetch_note_preview(slug, path).await,
                _ => None,
            }
        }
    });

    let registry = actions.registry.clone();
    let activate = use_callback(move |cand: Candidate| {
        open.set(false);
        query.set(String::new());
        cursor.set(0);
        match cand.hit {
            Hit::Note { path, slug } => {
                let org = if slug == active() {
                    String::new()
                } else {
                    slug
                };
                nav.push(Route::VaultRoute { path, org });
            }
            Hit::Task { id } => {
                nav.push(Route::TaskDetailRoute { id });
            }
            Hit::Project { id } => {
                nav.push(Route::ProjectDetailRoute { id: id.to_string() });
            }
            Hit::Command { id } => {
                let registry = registry.clone();
                spawn(async move {
                    registry.execute(id).await;
                });
            }
        }
    });

    if !open() {
        return rsx! {};
    }

    let rows = ranked.read().clone();
    let total = rows.len();
    let sel_cand = selected();
    rsx! {
        div {
            class: "fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 backdrop-blur-sm",
            onclick: move |_| open.set(false),
            div {
                class: "flex h-[min(70vh,40rem)] w-[min(92vw,64rem)] overflow-hidden rounded-xl border border-border bg-background shadow-2xl",
                onclick: move |e| e.stop_propagation(),
                // ── Left: prompt + results ────────────────────
                div { class: "flex min-w-0 flex-1 flex-col border-r border-border/60",
                    div { class: "flex items-center gap-2 border-b border-border/60 px-3 py-2.5",
                        Search { size: 15 }
                        input {
                            id: "everything-search-input",
                            class: "min-w-0 flex-1 bg-transparent text-sm text-foreground outline-none placeholder:text-muted-foreground",
                            placeholder: "Search notes, tasks, projects, commands…",
                            value: "{query}",
                            autofocus: true,
                            onmounted: move |el| async move {
                                let _ = el.set_focus(true).await;
                            },
                            oninput: move |e| {
                                query.set(e.value());
                                cursor.set(0);
                            },
                            onkeydown: move |e: KeyboardEvent| {
                                let ctrl = e.modifiers().ctrl();
                                let key = e.key();
                                let down = key == Key::ArrowDown
                                    || (ctrl && key == Key::Character("n".into()))
                                    || (ctrl && key == Key::Character("j".into()));
                                let up = key == Key::ArrowUp
                                    || (ctrl && key == Key::Character("p".into()))
                                    || (ctrl && key == Key::Character("k".into()));
                                if down {
                                    e.prevent_default();
                                    let len = ranked.peek().len();
                                    let cur = *cursor.peek();
                                    if len > 0 {
                                        cursor.set((cur + 1) % len);
                                    }
                                } else if up {
                                    e.prevent_default();
                                    let len = ranked.peek().len();
                                    let cur = *cursor.peek();
                                    if len > 0 {
                                        cursor.set((cur + len - 1) % len);
                                    }
                                } else if key == Key::Enter {
                                    e.prevent_default();
                                    let cand = ranked.peek().get(*cursor.peek()).map(|r| r.cand.clone());
                                    if let Some(cand) = cand {
                                        activate.call(cand);
                                    }
                                } else if key == Key::Escape {
                                    e.prevent_default();
                                    open.set(false);
                                } else {
                                    // Everything else is typing — keep it
                                    // out of the global shortcut engine.
                                    e.stop_propagation();
                                }
                            },
                        }
                        span { class: "shrink-0 text-[11px] tabular-nums text-muted-foreground",
                            "{total}"
                        }
                    }
                    div { class: "min-h-0 flex-1 overflow-y-auto py-1",
                        if rows.is_empty() {
                            div { class: "flex h-full items-center justify-center text-sm text-muted-foreground",
                                "No matches"
                            }
                        }
                        for (i, row) in rows.into_iter().enumerate() {
                            {
                                let is_sel = i == cursor();
                                let cand = row.cand.clone();
                                let row_cls = if is_sel {
                                    "flex w-full items-center gap-2.5 bg-accent px-3 py-1.5 text-left"
                                } else {
                                    "flex w-full items-center gap-2.5 px-3 py-1.5 text-left hover:bg-accent/40"
                                };
                                rsx! {
                                    button {
                                        key: "{i}",
                                        r#type: "button",
                                        class: "{row_cls}",
                                        onmouseenter: move |_| cursor.set(i),
                                        onclick: move |_| activate.call(cand.clone()),
                                        span { class: "shrink-0 text-muted-foreground",
                                            match row.cand.hit {
                                                Hit::Note { .. } => rsx! { FileText { size: 14 } },
                                                Hit::Task { .. } => rsx! { CircleCheck { size: 14 } },
                                                Hit::Project { .. } => rsx! { Folder { size: 14 } },
                                                Hit::Command { .. } => rsx! { Zap { size: 14 } },
                                            }
                                        }
                                        span { class: "min-w-0 flex-1 truncate text-sm text-foreground",
                                            {highlight(&row.cand.label, &row.label_marks)}
                                        }
                                        if !row.cand.detail.is_empty() {
                                            span { class: "max-w-[40%] shrink-0 truncate text-xs text-muted-foreground",
                                                {highlight(&row.cand.detail, &row.detail_marks)}
                                            }
                                        }
                                        span { class: "shrink-0 rounded bg-muted/50 px-1.5 py-0.5 text-[10px] uppercase tracking-wider text-muted-foreground",
                                            "{row.cand.kind_tag()}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "flex items-center gap-3 border-t border-border/60 px-3 py-1.5 text-[11px] text-muted-foreground",
                        span { kbd { class: "rounded border border-border/70 bg-muted/40 px-1", "↑↓" } " move" }
                        span { kbd { class: "rounded border border-border/70 bg-muted/40 px-1", "⏎" } " open" }
                        span { kbd { class: "rounded border border-border/70 bg-muted/40 px-1", "esc" } " close" }
                    }
                }
                // ── Right: preview ────────────────────────────
                div { class: "hidden w-[45%] min-w-0 flex-col md:flex",
                    div { class: "border-b border-border/60 px-4 py-2.5 text-xs font-medium uppercase tracking-wider text-muted-foreground",
                        "Preview"
                    }
                    div { class: "min-h-0 flex-1 overflow-y-auto p-4",
                        match &sel_cand {
                            Some(c @ Candidate { hit: Hit::Note { .. }, .. }) => rsx! {
                                div { class: "mb-2 text-sm font-semibold text-foreground", "{c.label}" }
                                div { class: "mb-3 text-xs text-muted-foreground", "{c.detail}" }
                                match preview.read().as_ref() {
                                    Some(Some(text)) => rsx! {
                                        pre { class: "whitespace-pre-wrap font-mono text-xs leading-5 text-foreground/90",
                                            "{text}"
                                        }
                                    },
                                    Some(None) => rsx! {
                                        div { class: "text-xs text-muted-foreground", "No preview available" }
                                    },
                                    None => rsx! {
                                        div { class: "text-xs text-muted-foreground", "Loading…" }
                                    },
                                }
                            },
                            Some(c) => rsx! {
                                div { class: "mb-2 text-sm font-semibold text-foreground", "{c.label}" }
                                div { class: "mb-1 text-xs uppercase tracking-wider text-muted-foreground", "{c.kind_tag()}" }
                                if !c.detail.is_empty() {
                                    div { class: "text-xs text-muted-foreground", "{c.detail}" }
                                }
                            },
                            None => rsx! {
                                div { class: "flex h-full items-center justify-center text-sm text-muted-foreground",
                                    "Nothing selected"
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}

/// The preview pane's note body: first ~200 lines of the file, YAML
/// frontmatter included (it reads as metadata here).
async fn fetch_note_preview(slug: String, path: String) -> Option<String> {
    let client = crate::vox_clients::vault_client(&slug).await.ok()?;
    let bytes = client
        // Search indexes the org's own vault; wiki pages have their
        // own results and their own route.
        .get_file(crate::document_session::VAULT_ID.to_owned(), path)
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&bytes.0);
    let clipped: String = text.lines().take(200).collect::<Vec<_>>().join("\n");
    Some(clipped)
}
