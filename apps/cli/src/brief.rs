//! `task brief` — the morning digest. Aggregates six existing
//! org-scoped surfaces into one read: today's day plan + calendar
//! events, due/overdue + in-progress tasks, the active timer,
//! blocked agent tasks (awaiting input), open inbox items, and
//! today's meals + bookings. Zero new services — every section is
//! a client-side filter over an RPC the server already mounts.
//!
//! Each section degrades independently: a failing service lands
//! in `errors` (and a warning line in human output) instead of
//! sinking the whole digest.

use clap::Args;

use crate::plan;

/// `task brief` — morning digest across plan / tasks / timer /
/// agent queue / inbox / meals / bookings.
#[derive(Args)]
pub struct BriefArgs {
    #[arg(long)]
    pub org: Option<String>,
    #[arg(long)]
    pub server: Option<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(serde::Serialize)]
struct BriefView {
    date: String,
    org: String,
    plan: Option<scheduling_proto::DayPlan>,
    events: Vec<scheduling_proto::CalEvent>,
    due_tasks: Vec<task::TaskInfo>,
    in_progress: Vec<task::TaskInfo>,
    active_timer: Option<timer_proto::WorkSession>,
    blocked_agent_tasks: Vec<agent_proto::tasks::AgentTask>,
    inbox_open: Vec<inbox_proto::InboxItem>,
    meals: Vec<mealplan::Meal>,
    bookings: Vec<scheduling_proto::Booking>,
    /// Sections that failed to load (service name: error).
    errors: Vec<String>,
}

/// First 10 chars of an ISO date / timestamp — `YYYY-MM-DD`.
fn date_prefix(s: &str) -> &str {
    s.get(..10).unwrap_or(s)
}

#[allow(clippy::too_many_lines)]
pub async fn run_brief(args: BriefArgs) -> eyre::Result<()> {
    use timer_proto::service::TimerService;

    let BriefArgs { org, server, json } = args;
    let slug = crate::resolve_active_org(org.clone())?;
    let url = crate::resolve_org_vox_url(server, &slug);
    let today = chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();

    let mut view = BriefView {
        date: today.clone(),
        org: slug.clone(),
        plan: None,
        events: Vec::new(),
        due_tasks: Vec::new(),
        in_progress: Vec::new(),
        active_timer: None,
        blocked_agent_tasks: Vec::new(),
        inbox_open: Vec::new(),
        meals: Vec::new(),
        bookings: Vec::new(),
        errors: Vec::new(),
    };

    // ── Day plan + calendar events ──────────────────────────────
    match crate::establish_for_url::<scheduling_proto::DayPlansClient>(&url).await {
        Ok(c) => match c.get_day_plan(today.clone()).await {
            Ok(p) => view.plan = p,
            Err(e) => view.errors.push(format!("day plan: {e:?}")),
        },
        Err(e) => view.errors.push(format!("day plan: {e}")),
    }
    match crate::establish_for_url::<scheduling_proto::CalendarEventsClient>(&url).await {
        Ok(c) => match c.list_events().await {
            Ok(evs) => {
                view.events = evs
                    .into_iter()
                    .filter(|e| {
                        date_prefix(&e.start) <= today.as_str()
                            && date_prefix(&e.end) >= today.as_str()
                    })
                    .collect();
            }
            Err(e) => view.errors.push(format!("calendar events: {e:?}")),
        },
        Err(e) => view.errors.push(format!("calendar events: {e}")),
    }

    // ── Tasks: due/overdue + in-progress ────────────────────────
    // "Open + due-on-or-before-today" and the open/terminal status set
    // are domain rules — query them server-side (TaskService) so the
    // brief and the web agenda agree. Display order (by due date) is a
    // presentation choice and stays here.
    match crate::establish_for_url::<task::TaskServiceClient>(&url).await {
        Ok(c) => {
            match c
                .query(task::TaskListFilter {
                    open_only: true,
                    due_on_or_before: Some(today.clone()),
                    ..Default::default()
                })
                .await
            {
                Ok(mut due) => {
                    due.sort_by(|a, b| {
                        let key = |t: &task::TaskInfo| {
                            t.due
                                .clone()
                                .or_else(|| t.scheduled.clone())
                                .unwrap_or_default()
                        };
                        key(a).cmp(&key(b))
                    });
                    view.due_tasks = due;
                }
                Err(e) => view.errors.push(format!("tasks: {e:?}")),
            }
            match c
                .query(task::TaskListFilter {
                    status: Some("in-progress".into()),
                    ..Default::default()
                })
                .await
            {
                Ok(rows) => view.in_progress = rows,
                Err(e) => view.errors.push(format!("tasks: {e:?}")),
            }
        }
        Err(e) => view.errors.push(format!("tasks: {e}")),
    }

    // ── Active timer (local sqlite store, same as `task timer`) ─
    match plan::timer_ctx(org.as_deref()).await {
        Ok(t) => match t.store.active_timer(t.user_id).await {
            Ok(s) => view.active_timer = s,
            Err(e) => view.errors.push(format!("timer: {e}")),
        },
        Err(e) => view.errors.push(format!("timer: {e}")),
    }

    // ── Agent queue: blocked (awaiting input) ───────────────────
    match crate::establish_for_url::<agent_proto::service::tasks::AgentTaskQueueClient>(&url).await
    {
        Ok(c) => match c
            .read_queue(slug.clone(), agent_proto::tasks::QueueFilter::default())
            .await
        {
            Ok(snap) => {
                view.blocked_agent_tasks = snap
                    .tasks
                    .into_iter()
                    .filter(|t| t.status == agent_proto::tasks::status::BLOCKED)
                    .collect();
            }
            Err(e) => view.errors.push(format!("agent queue: {e:?}")),
        },
        Err(e) => view.errors.push(format!("agent queue: {e}")),
    }

    // ── Inbox: today's review queue (open, not snoozed) ─────────
    // The queue rule (open + resurface ≤ today, oldest first) is the
    // inbox domain's — ask the service for it rather than re-deriving.
    match crate::establish_for_url::<inbox_proto::InboxClient>(&url).await {
        Ok(c) => match c.review_queue(today.clone()).await {
            Ok(items) => view.inbox_open = items,
            Err(e) => view.errors.push(format!("inbox: {e:?}")),
        },
        Err(e) => view.errors.push(format!("inbox: {e}")),
    }

    // ── Meals today ─────────────────────────────────────────────
    match crate::establish_for_url::<mealplan::MealplanServiceClient>(&url).await {
        Ok(c) => match c.list().await {
            Ok(meals) => {
                view.meals = meals
                    .into_iter()
                    .filter(|m| m.scheduled_for.format("%Y-%m-%d").to_string() == today)
                    .collect();
            }
            Err(e) => view.errors.push(format!("meals: {e:?}")),
        },
        Err(e) => view.errors.push(format!("meals: {e}")),
    }

    // ── Bookings today (not cancelled) ──────────────────────────
    match crate::establish_for_url::<scheduling_proto::BookingsClient>(&url).await {
        Ok(c) => match c.list_bookings().await {
            Ok(bs) => {
                view.bookings = bs
                    .into_iter()
                    .filter(|b| date_prefix(&b.start_utc) == today)
                    .filter(|b| b.status != scheduling_proto::BookingStatus::Cancelled)
                    .collect();
            }
            Err(e) => view.errors.push(format!("bookings: {e:?}")),
        },
        Err(e) => view.errors.push(format!("bookings: {e}")),
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&view).map_err(|e| eyre::eyre!("json: {e}"))?
        );
        return Ok(());
    }

    render_human(&view);
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn render_human(v: &BriefView) {
    println!("Brief — {} (org {})\n", v.date, v.org);

    // Schedule.
    match &v.plan {
        Some(p) => {
            println!("Schedule ({} blocks)", p.blocks.len());
            for (i, b) in p.blocks.iter().enumerate() {
                println!("  {}", plan::render_block_line(i, b));
            }
        }
        None => println!(
            "Schedule: no plan saved — `task plan from-template {}` materializes one",
            v.date
        ),
    }
    for e in &v.events {
        println!("  event: {}  {}–{}", e.title, e.start, e.end);
    }
    println!();

    // Active timer.
    match &v.active_timer {
        Some(s) => {
            let elapsed = chrono::Utc::now().signed_duration_since(s.start_time);
            let mins =
                u16::try_from(elapsed.num_minutes().clamp(0, i64::from(u16::MAX))).unwrap_or(0);
            println!(
                "Timer: running {} — {}",
                plan::fmt_min(mins),
                if s.description.is_empty() {
                    "(no description)"
                } else {
                    &s.description
                }
            );
        }
        None => println!("Timer: idle"),
    }
    println!();

    // Tasks.
    println!("Due / overdue ({})", v.due_tasks.len());
    for t in &v.due_tasks {
        println!(
            "  {}  {}  [{}]",
            t.due
                .as_deref()
                .or(t.scheduled.as_deref())
                .unwrap_or("????-??-??"),
            t.title,
            t.status
        );
    }
    println!("In progress ({})", v.in_progress.len());
    for t in &v.in_progress {
        println!("  {}  ({})", t.title, t.id);
    }
    println!();

    // Agent queue.
    println!(
        "Agent tasks awaiting input ({})",
        v.blocked_agent_tasks.len()
    );
    for t in &v.blocked_agent_tasks {
        println!("  {}  ({})", t.title, t.id);
    }

    // Inbox.
    println!("Inbox open ({})", v.inbox_open.len());
    for i in &v.inbox_open {
        let first_line = i.body.lines().next().unwrap_or("");
        let trimmed: String = first_line.chars().take(72).collect();
        println!("  {}  {}", date_prefix(&i.created), trimmed);
    }
    println!();

    // Food + bookings.
    if !v.meals.is_empty() {
        println!("Meals today ({})", v.meals.len());
        for m in &v.meals {
            println!("  {:<10} {}  [{}]", m.slot, m.name, m.status);
        }
    }
    if !v.bookings.is_empty() {
        println!("Bookings today ({})", v.bookings.len());
        for b in &v.bookings {
            println!("  {}–{}  {}", b.start_utc, b.end_utc, b.attendee_name);
        }
    }

    if !v.errors.is_empty() {
        println!();
        for e in &v.errors {
            eprintln!("warning: {e}");
        }
    }
}
