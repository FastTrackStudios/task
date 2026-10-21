//! `task brief` — the morning digest. Aggregates six existing
//! org-scoped surfaces into one read: today's day plan + calendar
//! events, the due queue (the one Home and the Tasks list show —
//! `task_proto::due`, with the running mode's focus), the active
//! timer, blocked agent tasks (awaiting input), the inbox's due queue,
//! and today's meals + bookings. Zero new services — every section is
//! an RPC the server already mounts.
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
    /// The due queue (`task_proto::due`) — what Home's Today rail and
    /// the Tasks list's Today section hold, in their order.
    due_tasks: Vec<task::TaskInfo>,
    /// How many of `due_tasks` are late.
    overdue: usize,
    /// The work in hand — the in-progress part of `due_tasks`.
    in_progress: Vec<task::TaskInfo>,
    /// What the running mode brings forward (empty when none runs).
    focus: Vec<String>,
    active_timer: Option<timer_proto::WorkSession>,
    blocked_agent_tasks: Vec<agent_proto::tasks::AgentTask>,
    /// The inbox's due queue — open and resurfaced, or suggested.
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
        overdue: 0,
        in_progress: Vec::new(),
        focus: Vec::new(),
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
                        e.tombstone.is_none()
                            && date_prefix(&e.start) <= today.as_str()
                            && date_prefix(&e.end) >= today.as_str()
                    })
                    .collect();
            }
            Err(e) => view.errors.push(format!("calendar events: {e:?}")),
        },
        Err(e) => view.errors.push(format!("calendar events: {e}")),
    }

    // ── Tasks: the due queue ────────────────────────────────────
    // The one due answer (`task_proto::due`): in progress or due today
    // or before, yours, in the order Home's Today rail and the Tasks
    // list show — asked of the server with the same context the app
    // builds (clock, this computer, the running session: its project,
    // and a mode's focus), so the brief's count is Home's count.
    let ctx = crate::now::context(org.as_deref(), None, None, None).await;
    view.focus = ctx.focus.clone();
    match crate::establish_for_url::<task::TaskServiceClient>(&url).await {
        Ok(c) => {
            let me = crate::now::signed_in_user(&url, &slug);
            match crate::now::due_queue(&c, me, &ctx).await {
                Ok(due) => {
                    view.overdue = task::overdue_count(&due, &today);
                    view.in_progress = due
                        .iter()
                        .filter(|t| {
                            task::Status::from_str(&t.status) == Some(task::Status::InProgress)
                        })
                        .cloned()
                        .collect();
                    view.due_tasks = due;
                }
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

    // ── Inbox: what is due for processing ───────────────────────
    // The inbox lane's due queue (open and resurfaced, or suggested and
    // awaiting a yes or no) — the count Home's "captures to process"
    // and the tab badge show. Asked of the service, not re-derived — on
    // the inbox's own calendar (UTC days, as the app's inbox counts).
    let inbox_today = chrono::Utc::now().date_naive().to_string();
    match crate::establish_for_url::<inbox_proto::InboxClient>(&url).await {
        Ok(c) => match c.due_queue(inbox_today).await {
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

    // Tasks — the due queue, in its order (work in hand marked [~]).
    if v.focus.is_empty() {
        println!("Due now ({}, {} overdue)", v.due_tasks.len(), v.overdue);
    } else {
        println!(
            "Due now ({}, {} overdue) — mode running, leading with {}",
            v.due_tasks.len(),
            v.overdue,
            v.focus
                .iter()
                .map(|f| format!("@{f}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    for t in &v.due_tasks {
        println!("  {}", crate::task_cmd::list_line(t, 0));
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
    println!("Inbox to process ({})", v.inbox_open.len());
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
