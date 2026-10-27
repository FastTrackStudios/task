//! The task board rendered to a picture through Blitz: no window, no
//! device, the paint path a dioxus-native host takes (`blitz_paint` into
//! `anyrender_vello`), at any size.
//!
//! ```text
//! cargo run -p task-ui --features shot --example task_shot -- OUT.png [W H] [list|kanban]
//! ```
//!
//! `W H` in points (default 1194 834, an 11" iPad sideways; 834 1194
//! upright). The board is themed through architect-ui's tokens the way a
//! host would theme it — a dark studio palette set on the wrapper — so
//! the picture also proves the theming hook.

use anyrender::ImageRenderer as _;
use anyrender_vello::VelloImageRenderer;
use blitz_dom::{Document as _, DocumentConfig};
use blitz_traits::shell::{ColorScheme, Viewport};
use chrono::{Duration, Local, Utc};
use dioxus::prelude::*;
use dioxus_native_dom::DioxusDocument;
use task_ui::{AnchorChip, TaskInfo, TaskMutation, TasksApp, TimeEntry, ViewMode};
use uuid::Uuid;
use workflows_proto::AgentRef;

/// Pixels per point, as on an iPad.
const SCALE: f32 = 2.0;

/// A host's palette, given through architect-ui's tokens on the wrapper.
const HOST_TOKENS: &str = "--background:#0e0e11;--foreground:#e4e4e7;--card:#141418;\
--card-foreground:#e4e4e7;--popover:#16161b;--popover-foreground:#e4e4e7;\
--primary:#2563eb;--primary-foreground:#f8fafc;--muted:#1b1b21;\
--muted-foreground:#8b8b96;--accent:#1d1d24;--accent-foreground:#e4e4e7;\
--border:#222228;--input:#222228;--ring:#bfdbfe;--destructive:#ef4444;";

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "tasks.png".into());
    let w: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(1194.0);
    let h: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(834.0);
    let view = match args.next().as_deref() {
        Some("kanban") => ViewMode::Kanban,
        _ => ViewMode::List,
    };

    let vdom = VirtualDom::new_with_props(Shot, ShotProps { view });
    let (pw, ph) = ((w * SCALE) as u32, (h * SCALE) as u32);
    let mut document = DioxusDocument::new(
        vdom,
        DocumentConfig {
            viewport: Some(Viewport::new(pw, ph, SCALE, ColorScheme::Dark)),
            ..Default::default()
        },
    );
    document.initial_build();
    for _ in 0..12 {
        document.poll(None);
        document.inner_mut().resolve(0.0);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut renderer = VelloImageRenderer::new(pw, ph);
    let mut buffer = Vec::new();
    renderer.render_to_vec(
        |painter| {
            let mut inner = document.inner_mut();
            blitz_paint::paint_scene(painter, &mut inner, f64::from(SCALE), pw, ph, 0, 0);
        },
        &mut buffer,
    );
    image::RgbaImage::from_raw(pw, ph, buffer)
        .expect("a buffer the size of the picture")
        .save(&out)
        .expect("write the picture");
}

#[component]
fn Shot(view: ViewMode) -> Element {
    let (tasks, anchors, triage) = board();
    let projects = vec![
        (Uuid::from_u128(0xA1), "Trackone EP".to_owned()),
        (Uuid::from_u128(0xA2), "Spring tour".to_owned()),
        (Uuid::from_u128(0xA3), "Studio upkeep".to_owned()),
    ];
    rsx! {
        // Real `<style>` nodes: a headless document has no head for
        // `document::Style` to land in.
        style { {"html,body{margin:0;padding:0;height:100%;background:#0e0e11;overflow:hidden;}*{box-sizing:border-box;}"} }
        task_ui::Styles {}
        div {
            style: "{HOST_TOKENS}width:100vw;height:100vh;display:flex;flex-direction:column;background:var(--background);color:var(--foreground);font-family:Inter,ui-sans-serif,system-ui,sans-serif;",
            TasksApp {
                tasks,
                anchors,
                triage,
                projects,
                initial_view: view,
                hidden_note: Some("12 done hidden".to_owned()),
                held_count: 2,
                held_by: vec!["@home".to_owned(), "@errands".to_owned()],
                on_show_held: move |()| {},
                on_event: move |_: TaskMutation| {},
            }
        }
    }
}

fn human(name: &str) -> task_proto::model::WorkflowAttrs {
    task_proto::model::WorkflowAttrs {
        assignees: vec![AgentRef::Human {
            user_id: name.to_lowercase(),
            display_name: Some(name.to_owned()),
        }]
        .into(),
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn task(
    n: u128,
    title: &str,
    status: &str,
    priority: &str,
    due_in: Option<i64>,
    projects: &[&str],
    contexts: &[&str],
    owner: Option<&str>,
) -> TaskInfo {
    let today = Local::now().date_naive();
    let mut t = TaskInfo::new(title);
    t.id = Uuid::from_u128(n);
    t.status = status.to_owned();
    t.priority = priority.to_owned();
    t.due = due_in.map(|d| (today + Duration::days(d)).to_string());
    t.projects = projects.iter().map(|p| format!("[[{p}]]")).collect();
    t.contexts = contexts.iter().map(|c| format!("@{c}")).collect();
    t.workflow = owner.map(human);
    t
}

type Board = (Vec<TaskInfo>, Vec<(Uuid, AnchorChip)>, Vec<TaskInfo>);

fn board() -> Board {
    let mut mix = task(
        1,
        "Mix vocals for \"Northern Lights\" — comp the doubles and print the stems",
        "in-progress",
        "high",
        Some(0),
        &["Trackone EP"],
        &["studio"],
        Some("Alice Liddell"),
    );
    let now = Utc::now();
    mix.time_entries = vec![
        TimeEntry {
            start_time: now - Duration::minutes(150),
            end_time: Some(now - Duration::minutes(95)),
        },
        TimeEntry {
            start_time: now - Duration::minutes(40),
            end_time: None,
        },
    ]
    .into();
    let tasks = vec![
        mix,
        task(
            2,
            "Send the master to the label",
            "open",
            "critical",
            Some(-3),
            &["Trackone EP"],
            &[],
            Some("Bob Marley"),
        ),
        task(
            3,
            "Re-string the Telecaster before Friday's session",
            "open",
            "normal",
            Some(1),
            &["Studio upkeep"],
            &["studio"],
            None,
        ),
        task(
            4,
            "Book the van for the spring tour",
            "waiting",
            "normal",
            Some(4),
            &["Spring tour"],
            &["phone"],
            Some("Cody Wright"),
        ),
        task(
            5,
            "Rehearse the new set with the rhythm section",
            "open",
            "low",
            Some(6),
            &["Spring tour"],
            &[],
            Some("Alice Liddell"),
        ),
        task(
            6,
            "Pick up the repaired preamp",
            "open",
            "normal",
            Some(12),
            &[],
            &["errands"],
            None,
        ),
        task(
            7,
            "Write liner notes",
            "open",
            "normal",
            None,
            &["Trackone EP"],
            &[],
            None,
        ),
        task(
            8,
            "Archive last year's sessions to the NAS",
            "done",
            "normal",
            Some(-1),
            &["Studio upkeep"],
            &[],
            Some("Bob Marley"),
        ),
        task(
            9,
            "Draft the setlist",
            "cancelled",
            "normal",
            Some(-2),
            &["Spring tour"],
            &[],
            None,
        ),
        task(
            10,
            "Tune the drum kit for the tracking day",
            "in-progress",
            "normal",
            Some(2),
            &[],
            &[],
            Some("Cody Wright"),
        ),
    ];
    let anchors = vec![(
        Uuid::from_u128(10),
        AnchorChip {
            kind: "milestone".to_owned(),
            label: "Tracking week".to_owned(),
        },
    )];
    let triage = vec![
        task(
            11,
            "Call the venue about load-in",
            "open",
            "normal",
            None,
            &[],
            &[],
            None,
        ),
        task(
            12,
            "Renew the plugin licences",
            "open",
            "normal",
            None,
            &[],
            &[],
            None,
        ),
    ];
    (tasks, anchors, triage)
}
