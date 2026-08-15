//! Quick-add input bar. The component only sets the title; the
//! natural-language extraction
//! (`"Buy milk tomorrow #errands @shopping"`) runs in the
//! consumer's create path, which re-captures that title through
//! `task_proto::capture`.

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::Plus;
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

    rsx! {
        form {
            // Quiet until used: hairline border, no card fill — the
            // list is the page, capture is one thin line above it.
            class: "flex items-center gap-2 rounded-lg border border-border/60 bg-transparent px-3 py-1.5 focus-within:border-border focus-within:bg-card focus-within:ring-2 focus-within:ring-primary/50",
            onsubmit: move |e: Event<FormData>| {
                e.prevent_default();
                submit();
            },
            div { class: "flex h-6 w-6 items-center justify-center rounded-md bg-primary/15 text-primary",
                Plus { size: 14 }
            }
            input {
                r#type: "text",
                value: "{value}",
                placeholder: "Add a task — try \"Buy milk tomorrow #errands\"",
                class: "flex-1 bg-transparent text-sm text-foreground placeholder:text-muted-foreground outline-none",
                oninput: move |e| value.set(e.value()),
            }
            // Explicit project filing — "(no project)" stays the
            // default; `[[Project]]` in the title infers instead.
            if !props.projects.is_empty() {
                select {
                    class: "shrink-0 rounded-md border border-border/60 bg-transparent px-1.5 py-0.5 text-xs text-muted-foreground outline-none focus:border-border",
                    value: "{chosen_project}",
                    onchange: move |e| chosen_project.set(e.value()),
                    option { value: "", selected: chosen_project.read().is_empty(), "(no project)" }
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
            kbd { class: "rounded border border-border bg-muted/50 px-1.5 py-0.5 text-[10px] text-muted-foreground",
                "⏎"
            }
        }
    }
}
