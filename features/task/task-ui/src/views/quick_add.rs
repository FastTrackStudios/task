//! Quick-add input bar. The component only sets the title; the
//! natural-language extraction
//! (`"Buy strings tomorrow #errands @studio"`) runs in the consumer's
//! create path, which re-captures that title through
//! `task_proto::capture`.
//!
//! What that capture will make of the line is shown as you type — the
//! date, tags, contexts, project and priority it found, and the title
//! left over — so the syntax is learned by seeing it, not by recall.

use architect_ui::lucide_dioxus::{CalendarDays, Flag, Hash, Plus, Repeat};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::TaskInfo;

#[derive(Props, Clone, PartialEq)]
pub struct QuickAddProps {
    pub on_create: EventHandler<TaskInfo>,
    /// `(id, title)` of the projects a new task could file under —
    /// renders an inline picker when non-empty. Typing `[[Project]]`
    /// in the title infers the same thing; the picker is the
    /// explicit route.
    #[props(default)]
    pub projects: Vec<(Uuid, String)>,
    /// Take focus when shown — the phone's add sheet, where the only
    /// reason it opened is to type.
    #[props(default)]
    pub autofocus: bool,
}

/// What `capture` read out of the line, as short labels.
fn parsed_bits(line: &str) -> Vec<(&'static str, String)> {
    let t = task_proto::capture(line);
    let mut bits = Vec::new();
    if let Some(d) = t.due.as_deref() {
        // "Fri, Oct 9", not the stored ISO date.
        let label = d
            .get(..10)
            .and_then(|s| s.parse::<chrono::NaiveDate>().ok())
            .map_or_else(|| d.to_owned(), |day| day.format("%a, %b %-d").to_string());
        bits.push(("due", label));
    }
    if let Some(r) = t.recurrence.as_deref().filter(|r| !r.is_empty()) {
        bits.push(("repeat", r.to_owned()));
    }
    for p in t.projects.iter() {
        bits.push((
            "project",
            p.trim_start_matches("[[").trim_end_matches("]]").to_owned(),
        ));
    }
    // `#task` is the marker capture adds to every task, not something
    // the person typed.
    for tag in t.tags.iter().filter(|tag| tag.as_str() != "task") {
        bits.push(("tag", format!("#{tag}")));
    }
    // Contexts are stored with their `@` already.
    for c in t.contexts.iter() {
        bits.push(("context", format!("@{}", c.trim_start_matches('@'))));
    }
    if t.priority != "normal" && !t.priority.is_empty() {
        bits.push(("priority", t.priority.clone()));
    }
    bits
}

#[component]
pub fn QuickAdd(props: QuickAddProps) -> Element {
    let mut value = use_signal(String::new);
    let mut chosen_project = use_signal(String::new);

    let projects_for_submit = props.projects.clone();
    let mut submit = move || {
        let title = value.read().trim().to_string();
        if title.is_empty() {
            return;
        }
        let mut task = TaskInfo::new(title);
        let chosen = chosen_project.read().clone();
        if !chosen.is_empty() {
            task.project_id = Uuid::parse_str(&chosen)
                .ok()
                .filter(|id| projects_for_submit.iter().any(|(pid, _)| pid == id));
        }
        props.on_create.call(task);
        value.set(String::new());
    };

    let line = value.read().clone();
    let bits = if line.trim().is_empty() {
        Vec::new()
    } else {
        parsed_bits(&line)
    };
    let autofocus = props.autofocus;

    rsx! {
        div { class: "flex flex-col gap-1.5",
            form {
                // Quiet until used: hairline border, no card fill — the
                // list is the page, capture is one thin line above it.
                class: "flex items-center gap-2 rounded-lg border border-border/60 bg-transparent px-3 py-1.5 focus-within:border-border focus-within:bg-card focus-within:ring-2 focus-within:ring-ring",
                onsubmit: move |e: Event<FormData>| {
                    e.prevent_default();
                    submit();
                },
                div { class: "flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-primary/15 text-primary",
                    Plus { size: 14 }
                }
                input {
                    r#type: "text",
                    value: "{value}",
                    aria_label: "New task",
                    placeholder: "Add a task — \"Mix vocals friday #trackone\"",
                    // `min-w-0`: an input's intrinsic width otherwise
                    // pushes the project picker off a phone's edge.
                    class: "min-w-0 flex-1 bg-transparent text-base text-foreground placeholder:text-muted-foreground outline-none sm:text-sm",
                    oninput: move |e| value.set(e.value()),
                    onmounted: move |e| async move {
                        if autofocus {
                            let _ = e.set_focus(true).await;
                        }
                    },
                }
                // Explicit project filing — "(no project)" stays the
                // default; `[[Project]]` in the title infers instead.
                if !props.projects.is_empty() {
                    select {
                        aria_label: "Project",
                        class: "max-w-[35%] shrink-0 truncate rounded-md border border-border/60 bg-transparent px-1.5 py-1 text-xs text-muted-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring",
                        value: "{chosen_project}",
                        onchange: move |e| chosen_project.set(e.value()),
                        option { value: "", selected: chosen_project.read().is_empty(), "No project" }
                        for (id, title) in props.projects.iter() {
                            option {
                                key: "{id}",
                                value: "{id}",
                                selected: chosen_project.read().as_str() == id.to_string(),
                                "{title}"
                            }
                        }
                    }
                }
                // A keyboard hint where there is a keyboard; a button to
                // press where there is a thumb.
                kbd { class: "hidden rounded border border-border bg-muted/50 px-1.5 py-0.5 text-xs text-muted-foreground sm:inline",
                    "⏎"
                }
                button {
                    r#type: "submit",
                    class: "shrink-0 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground disabled:opacity-40 sm:hidden",
                    disabled: value.read().trim().is_empty(),
                    "Add"
                }
            }
            if !bits.is_empty() {
                // What Enter will create — the syntax, taught by example.
                div { class: "flex flex-wrap items-center gap-1.5 px-1 text-xs text-muted-foreground", aria_live: "polite",
                    span { "Will add:" }
                    for (kind, label) in bits {
                        span { key: "{kind}{label}", class: "inline-flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-foreground/85",
                            match kind {
                                "due" => rsx! { CalendarDays { size: 11 } },
                                "repeat" => rsx! { Repeat { size: 11 } },
                                "project" => rsx! { Hash { size: 11 } },
                                "priority" => rsx! { Flag { size: 11 } },
                                _ => rsx! {},
                            }
                            "{label}"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parsed_bits;

    #[test]
    fn the_preview_names_what_capture_found() {
        let bits = parsed_bits("Mix vocals tomorrow #trackone @studio");
        let kinds: Vec<&str> = bits.iter().map(|(k, _)| *k).collect();
        assert!(kinds.contains(&"due"), "{bits:?}");
        assert!(
            bits.iter().any(|(k, v)| *k == "tag" && v == "#trackone"),
            "{bits:?}"
        );
        assert!(
            bits.iter().any(|(k, v)| *k == "context" && v == "@studio"),
            "{bits:?}"
        );
    }

    #[test]
    fn a_plain_title_previews_nothing() {
        let bits = parsed_bits("Call the venue");
        assert!(bits.is_empty(), "{bits:?}");
    }
}
