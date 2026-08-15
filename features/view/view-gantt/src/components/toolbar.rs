//! Top toolbar — zoom + filter input + sort dropdown + scroll-to
//! buttons.

use architect_ui::prelude::*;
use dioxus::document;
use dioxus::prelude::*;

use crate::scales::x_for_date;
use crate::store::{GanttEvent, SortDir, SortKey};

use super::gantt::GanttContext;

#[component]
pub fn Toolbar() -> Element {
    let ctx = use_context::<GanttContext>();
    let state = ctx.state;
    let on_event = ctx.on_event.clone();

    let level = state.read().zoom.level;
    let levels = state.read().zoom.levels.len();
    let sort = state.read().sort;
    let filter = state.read().filter.clone();
    let readonly = state.read().readonly;

    let on_in = on_event.clone();
    let on_out = on_event.clone();
    let on_filter = on_event.clone();
    let on_sort = on_event.clone();

    rsx! {
        div { class: "flex-none flex items-center gap-2 px-3 py-2 border-b border-border bg-card flex-wrap",
            Text { variant: TextVariant::Muted, "Gantt" }

            // Filter input. Keeps the cursor inside the field by
            // routing through SetFilter rather than a local Signal —
            // single source of truth.
            input {
                class: "h-8 w-48 rounded-md border border-input bg-background px-3 text-sm",
                placeholder: "Filter tasks…",
                value: "{filter}",
                oninput: move |e: FormEvent| on_filter.call(GanttEvent::SetFilter { text: e.value() }),
            }

            // Sort dropdown — a native <select> keeps it accessible
            // without needing the architect-ui combobox dance for a 4-item
            // pick. Encoded as "key:dir" so both fields round-trip.
            select {
                class: "h-8 rounded-md border border-input bg-background px-2 text-sm",
                value: "{sort_to_string(sort)}",
                onchange: move |e: FormEvent| {
                    if let Some((key, dir)) = string_to_sort(&e.value()) {
                        on_sort.call(GanttEvent::SetSort { key, dir });
                    }
                },
                option { value: "none", "Sort: Tree" }
                option { value: "name:asc", "Name ↑" }
                option { value: "name:desc", "Name ↓" }
                option { value: "start:asc", "Start ↑" }
                option { value: "start:desc", "Start ↓" }
                option { value: "end:asc", "End ↑" }
                option { value: "end:desc", "End ↓" }
                option { value: "progress:asc", "Progress ↑" }
                option { value: "progress:desc", "Progress ↓" }
            }

            Spacer {}

            if readonly {
                StatusBadge { variant: StatusBadgeVariant::Neutral, label: "Read-only".to_string() }
            }

            // Scroll-to-today — uses the chart-pane id we render in
            // gantt-root, and computes a target scrollLeft from the
            // current scale.
            Button {
                variant: ButtonVariant::Secondary,
                size: ButtonSize::Small,
                on_click: move |_| {
                    let s = state.read();
                    let grid = s.build_grid();
                    let x = x_for_date(&grid, chrono::Utc::now());
                    drop(s);
                    let js = format!(
                        "const el = document.getElementById('gantt-chart-pane');\
                         if (el) {{ el.scrollTo({{ left: Math.max(0, {x} - el.clientWidth/2), behavior: 'smooth' }}); }}"
                    );
                    document::eval(&js);
                },
                "Today"
            }

            // Scroll-to-selection.
            Button {
                variant: ButtonVariant::Secondary,
                size: ButtonSize::Small,
                disabled: state.read().selected.is_empty(),
                on_click: move |_| {
                    let s = state.read();
                    let target_id = s.selected.iter().next().copied();
                    let Some(id) = target_id else { return };
                    let task = s.tasks.iter().find(|t| t.id == id).cloned();
                    let Some(task) = task else { return };
                    let grid = s.build_grid();
                    let x = x_for_date(&grid, task.start);
                    drop(s);
                    let js = format!(
                        "const el = document.getElementById('gantt-chart-pane');\
                         if (el) {{ el.scrollTo({{ left: Math.max(0, {x} - 100), behavior: 'smooth' }}); }}"
                    );
                    document::eval(&js);
                },
                "Find selected"
            }

            // Zoom group.
            Button {
                variant: ButtonVariant::Secondary,
                size: ButtonSize::Small,
                disabled: level == 0,
                on_click: move |_| on_in.call(GanttEvent::ZoomTo { level: level.saturating_sub(1) }),
                "−"
            }
            Button {
                variant: ButtonVariant::Secondary,
                size: ButtonSize::Small,
                disabled: level + 1 >= levels,
                on_click: move |_| on_out.call(GanttEvent::ZoomTo { level: (level + 1).min(levels.saturating_sub(1)) }),
                "+"
            }
        }
    }
}

fn sort_to_string((key, dir): (SortKey, SortDir)) -> String {
    let k = match key {
        SortKey::None => return "none".into(),
        SortKey::Name => "name",
        SortKey::Start => "start",
        SortKey::End => "end",
        SortKey::Progress => "progress",
    };
    let d = match dir {
        SortDir::Asc => "asc",
        SortDir::Desc => "desc",
    };
    format!("{k}:{d}")
}

fn string_to_sort(s: &str) -> Option<(SortKey, SortDir)> {
    if s == "none" {
        return Some((SortKey::None, SortDir::Asc));
    }
    let (k, d) = s.split_once(':')?;
    let key = match k {
        "name" => SortKey::Name,
        "start" => SortKey::Start,
        "end" => SortKey::End,
        "progress" => SortKey::Progress,
        _ => return None,
    };
    let dir = match d {
        "asc" => SortDir::Asc,
        "desc" => SortDir::Desc,
        _ => return None,
    };
    Some((key, dir))
}
