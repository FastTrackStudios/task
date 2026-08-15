//! `task task …` — first-party task management.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::milestone::connect_milestone_client;
use crate::milestone::resolve_milestone_target;
use crate::project::connect_project_client;
use crate::project::resolve_project_target;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;
use crate::shared::resolve_body;

#[derive(Subcommand)]
pub(crate) enum TaskCmd {
    /// Create a new task from a natural-language line.
    /// Extracts `#tag`s, `@context`s, `[[Project]]`s,
    /// `!priority`, and date keywords (`today`, `tomorrow`,
    /// `next monday`, `mon`, `YYYY-MM-DD`). Title = the
    /// remaining text. Pushes the result through the
    /// per-org RPC.
    Capture {
        text: String,
        /// Project id or vault-relative path. Sets
        /// `projectId` on the resulting task.
        #[arg(long)]
        project: Option<String>,
        /// Milestone id or path. Sets `milestoneId`. If both
        /// `--project` and `--milestone` are passed they must
        /// agree (CLI-side check).
        #[arg(long)]
        milestone: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List tasks. Filters compose (AND).
    List {
        /// Status slug (`open`, `in-progress`, `done`, …).
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        /// `@`-prefix optional.
        #[arg(long)]
        context: Option<String>,
        /// Restrict to one project (id or path).
        #[arg(long)]
        project: Option<String>,
        /// Restrict to one milestone (id or path). `none`
        /// lists tasks with no milestone.
        #[arg(long)]
        milestone: Option<String>,
        /// Only tasks whose status is not done.
        #[arg(long)]
        open: bool,
        /// Only *unfiled* tasks — no project, parent, workstream,
        /// milestone or `@context`. The triage queue: these are
        /// excluded from `--relevant` because a bare title says
        /// nothing about what it's for. Implies `--open`.
        #[arg(long)]
        untriaged: bool,
        /// Only tasks relevant *right now* (see task::relevance):
        /// time-window contexts (`@morning` / `@mealprep` /
        /// `@evening`) gate to their windows, `@<location>` /
        /// `@<device>` gate to `--location` / `--device`,
        /// due/scheduled-today always shows. Implies `--open`;
        /// active-timer-project rows sort first.
        #[arg(long)]
        relevant: bool,
        /// Override the clock for `--relevant` (`HH:MM`, local).
        #[arg(long)]
        at: Option<String>,
        /// Where you are, for `--relevant` (`home`, `studio`, …).
        #[arg(long)]
        location: Option<String>,
        /// What you're on, for `--relevant` (`phone`, `computer`).
        #[arg(long)]
        device: Option<String>,
        /// Page size — at most this many rows (applied
        /// server-side, after `--status`/`--project`, over a
        /// stable path ordering; other filters then apply
        /// client-side within the page).
        #[arg(long)]
        limit: Option<u32>,
        /// Rows to skip before `--limit` (server-side).
        #[arg(long)]
        offset: Option<u32>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Fetch one task by id or path.
    Get {
        target: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Create a task with explicit fields (no NLP parsing).
    /// Use `capture` for the conversational form.
    Create {
        title: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        priority: Option<String>,
        #[arg(long)]
        due: Option<String>,
        #[arg(long)]
        scheduled: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        contexts: Vec<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        milestone: Option<String>,
        #[arg(long)]
        details: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Start working: sets `status: in-progress`, which begins
    /// automatic time tracking (an inline TimeEntry on the task —
    /// edit it afterwards if the tracked time needs correcting).
    /// `done` stops the clock; `set-status open` pauses it.
    Start {
        target: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting task as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Mark done. Sets `status: done` + `completedDate`.
    Done {
        target: String,
        /// Reopen instead (clears `completedDate`, status
        /// = `open`).
        #[arg(long)]
        undo: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting task as JSON.
        #[arg(long)]
        json: bool,
    },
    SetStatus {
        target: String,
        status: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting task as JSON.
        #[arg(long)]
        json: bool,
    },
    SetPriority {
        target: String,
        priority: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting task as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set or clear (`none`) the due date.
    SetDue {
        target: String,
        due: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting task as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set or clear (`none`) the scheduled date.
    SetScheduled {
        target: String,
        scheduled: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting task as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set or clear (`none`) the owning project.
    SetProject {
        target: String,
        project: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting task as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set or clear (`none`) the milestone link.
    SetMilestone {
        target: String,
        milestone: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting task as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Replace the tag list.
    SetTags {
        target: String,
        #[arg(value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting task as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set or clear (`none`) the parent task — this task becomes a
    /// subtask (`workflow.parent`), rolled up in the parent's
    /// subtask list. Parent accepts an id or vault path.
    SetParent {
        target: String,
        parent: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting task as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Replace the GTD context list (`@`-prefix optional; it's
    /// added when missing). Relevancy gates ride on these — see
    /// `list --relevant`.
    SetContexts {
        target: String,
        #[arg(value_delimiter = ',')]
        contexts: Vec<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting task as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Move a task to ANOTHER ORG, preserving its id, history and
    /// tracked time.
    ///
    /// Org-scoped references cannot survive the trip and are dropped:
    /// `projectId`, `milestoneId`, `workflow.parent` and
    /// `workflow.workstream` are ids in the SOURCE org's vault and name
    /// nothing in the target. Carrying them over would leave a task
    /// that looks filed but resolves to a ghost — strictly worse than
    /// arriving unfiled, which at least surfaces in triage.
    ///
    /// Creates in the target first, deletes from the source second: a
    /// failure in between leaves a duplicate you can see, never a task
    /// that exists nowhere.
    MoveOrg {
        target: String,
        /// Slug of the org to move into. You must be a member of it.
        #[arg(long)]
        to_org: String,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Move backing markdown file. `id` preserved.
    Rename {
        target: String,
        new_path: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the renamed task as JSON.
        #[arg(long)]
        json: bool,
    },
    Delete {
        target: String,
        #[arg(long, short = 'y')]
        yes: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

pub(crate) async fn run_task(cmd: TaskCmd) -> eyre::Result<()> {
    match cmd {
        TaskCmd::Capture {
            text,
            project,
            milestone,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let mut info = task::capture(&text);
            info.path = task::write::default_task_path(&info.title, None);
            if let Some(p) = project {
                let pc = connect_project_client(&url).await?;
                info.project_id = Some(resolve_project_target(&pc, &p).await?.id);
            }
            if let Some(m) = milestone {
                let mc = connect_milestone_client(&url).await?;
                let ms = resolve_milestone_target(&mc, &m).await?;
                info.milestone_id = Some(ms.id);
                if info.project_id.is_none() {
                    info.project_id = Some(ms.project_id);
                }
            }
            let client = connect_task_client(&url).await?;
            let created = client
                .create(info)
                .await
                .map_err(|e| eyre::eyre!("create: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&created).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("captured {} ({})", created.title, created.path);
            println!("  id:       {}", created.id);
            println!("  status:   {}", created.status);
            println!("  priority: {}", created.priority);
            if let Some(d) = &created.due {
                println!("  due:      {d}");
            }
        }
        TaskCmd::List {
            status,
            tag,
            context,
            project,
            milestone,
            open,
            untriaged,
            relevant,
            at,
            location,
            device,
            limit,
            offset,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_task_client(&url).await?;
            let ctx_filter = context.map(|c| {
                if c.starts_with('@') {
                    c
                } else {
                    format!("@{c}")
                }
            });
            let project_id = match project {
                Some(p) => {
                    let pc = connect_project_client(&url).await?;
                    Some(resolve_project_target(&pc, &p).await?.id)
                }
                None => None,
            };
            let milestone_filter = match milestone.as_deref() {
                Some("none" | "null") => Some(None),
                Some(m) => {
                    let mc = connect_milestone_client(&url).await?;
                    Some(Some(resolve_milestone_target(&mc, m).await?.id))
                }
                None => None,
            };

            // Push --status/--project/--limit/--offset to the
            // server (`TaskService::query`) so big orgs don't
            // ship the whole list over the wire. A server that
            // predates the verb (schema skew — see `task
            // doctor`) falls back to the unfiltered `list()` +
            // the client-side filters below. Skip the server
            // path when a page window combines with
            // client-only filters: slicing before --tag /
            // --context / --milestone / --open would drop rows.
            let has_client_only_filters = tag.is_some()
                || ctx_filter.is_some()
                || milestone_filter.is_some()
                || open
                || untriaged
                || relevant;
            let want_server_query =
                (status.is_some() || project_id.is_some() || limit.is_some() || offset.is_some())
                    && !((limit.is_some() || offset.is_some()) && has_client_only_filters);
            let mut window_applied = false;
            let rows = if want_server_query {
                let filter = task::TaskListFilter {
                    project: project_id,
                    workstream: None,
                    status: status.clone(),
                    limit,
                    offset,
                    ..Default::default()
                };
                match client.query(filter).await {
                    Ok(rows) => {
                        window_applied = true;
                        rows
                    }
                    Err(e) => {
                        eprintln!(
                            "warning: server-side query failed ({e:?}); falling back to full \
                             list() + client-side filters (is task-server stale? run `task \
                             doctor`)"
                        );
                        client
                            .list()
                            .await
                            .map_err(|e| eyre::eyre!("list: {e:?}"))?
                    }
                }
            } else {
                client
                    .list()
                    .await
                    .map_err(|e| eyre::eyre!("list: {e:?}"))?
            };

            let mut rows: Vec<_> = rows
                .into_iter()
                .filter(|t| {
                    status
                        .as_deref()
                        .is_none_or(|s| t.status.eq_ignore_ascii_case(s))
                })
                .filter(|t| {
                    tag.as_deref()
                        .is_none_or(|tg| t.tags.iter().any(|x| x == tg))
                })
                .filter(|t| {
                    ctx_filter
                        .as_deref()
                        .is_none_or(|c| t.contexts.iter().any(|x| x == c))
                })
                .filter(|t| project_id.is_none_or(|pid| t.project_id == Some(pid)))
                .filter(|t| match &milestone_filter {
                    None => true,
                    Some(want) => &t.milestone_id == want,
                })
                .filter(|t| {
                    !open || !task::Status::from_str(&t.status).is_some_and(task::Status::is_done)
                })
                // The triage queue — the exact complement of what
                // `--relevant` keeps, so the two flags partition the
                // open set between them.
                .filter(|t| {
                    !untriaged || (task::status_is_open(&t.status) && task::is_unfiled(t))
                })
                .collect();
            // Same business logic the web store applies — one
            // relevance implementation (task::relevance), two
            // renderers. FUTURE: read the running timer session
            // for the active-project boost.
            let relevance_ctx = relevant.then(|| {
                let now = chrono::Local::now();
                task::RelevanceContext {
                    local_hhmm: Some(at.unwrap_or_else(|| now.format("%H:%M").to_string())),
                    local_date: Some(now.format("%Y-%m-%d").to_string()),
                    location,
                    device,
                    active_project: None,
                }
            });
            if let Some(ctx) = &relevance_ctx {
                // Unfiled tasks are triage, not "right now" — same
                // exclusion the server's `query` and the web board
                // apply, so `--relevant` means one thing everywhere.
                rows.retain(|t| {
                    task::status_is_open(&t.status)
                        && task::is_filed(t)
                        && task::is_relevant(t, ctx)
                });
                // One next action per anchor (project, parent epic,
                // workstream) — task-dumping can't inflate the list.
                task::condense_next_per_project(&mut rows);
            }
            rows.sort_by(|a, b| {
                let a_done = task::Status::from_str(&a.status).is_some_and(task::Status::is_done);
                let b_done = task::Status::from_str(&b.status).is_some_and(task::Status::is_done);
                a_done
                    .cmp(&b_done)
                    .then_with(|| a.due.is_none().cmp(&b.due.is_none()))
                    .then_with(|| a.due.cmp(&b.due))
                    .then_with(|| a.title.cmp(&b.title))
            });
            // Stable rank pass on top of the general order:
            // active-project / due-today rows lead.
            if let Some(ctx) = &relevance_ctx {
                rows.sort_by_key(|t| task::relevance_rank(t, ctx));
            }
            // Page window that couldn't go server-side (combined
            // with client-only filters, or the query fallback):
            // slice after filtering + sorting.
            if !window_applied && (limit.is_some() || offset.is_some()) {
                let off = offset.unwrap_or(0) as usize;
                rows = rows
                    .into_iter()
                    .skip(off)
                    .take(limit.map_or(usize::MAX, |n| n as usize))
                    .collect();
            }

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            if rows.is_empty() {
                println!("(no tasks)");
                return Ok(());
            }
            // Subtasks render indented under their parent when both
            // made the cut — same arrangement the web list uses.
            let arranged = task::arrange_families(
                rows,
                |t| t.id,
                |t| t.workflow.as_ref().and_then(|w| w.parent),
            );
            for (depth, t) in &arranged {
                let marker = match task::Status::from_str(&t.status) {
                    Some(s) if s.is_done() => "[x]",
                    Some(task::Status::InProgress) => "[~]",
                    _ => "[ ]",
                };
                let due = t
                    .due
                    .as_deref()
                    .map(|d| format!(" (due {d})"))
                    .unwrap_or_default();
                let prio = match t.priority.as_str() {
                    "critical" => " !!",
                    "high" => " !",
                    _ => "",
                };
                let ms = if t.milestone_id.is_some() { " *" } else { "" };
                let indent = if *depth > 0 { "  ↳ " } else { "" };
                println!("{marker} {indent}{}{prio}{due}{ms}    {}", t.title, t.path);
            }
        }
        TaskCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_task_client(&url).await?;
            let t = resolve_task_target(&client, &target).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&t).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{} [{}]\n", t.title, t.status);
            println!("  id:       {}", t.id);
            println!("  path:     {}", t.path);
            println!("  priority: {}", t.priority);
            if let Some(d) = &t.due {
                println!("  due:      {d}");
            }
            if let Some(s) = &t.scheduled {
                println!("  sched:    {s}");
            }
            if let Some(p) = t.project_id {
                println!("  project:  {p}");
            }
            if let Some(m) = t.milestone_id {
                println!("  milestone:{m}");
            }
            if !t.tags.is_empty() {
                println!("  tags:     {}", t.tags.join(", "));
            }
            if !t.contexts.is_empty() {
                println!("  contexts: {}", t.contexts.join(", "));
            }
            if !t.details.is_empty() {
                println!("\n{}", t.details);
            }
        }
        TaskCmd::Create {
            title,
            path,
            status,
            priority,
            due,
            scheduled,
            tags,
            contexts,
            project,
            milestone,
            details,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let project_id = match project {
                Some(p) => {
                    let pc = connect_project_client(&url).await?;
                    Some(resolve_project_target(&pc, &p).await?.id)
                }
                None => None,
            };
            let (milestone_id, project_id) = match milestone {
                Some(m) => {
                    let mc = connect_milestone_client(&url).await?;
                    let ms = resolve_milestone_target(&mc, &m).await?;
                    (Some(ms.id), project_id.or(Some(ms.project_id)))
                }
                None => (None, project_id),
            };
            let details = resolve_body(details)?;
            let new_task = task::TaskInfo {
                id: uuid::Uuid::nil(),
                path: path.unwrap_or_default(),
                title,
                status: status.unwrap_or_else(|| "open".into()),
                priority: priority.unwrap_or_else(|| "normal".into()),
                due,
                scheduled,
                tags: task::model::StringList(tags),
                contexts: task::model::StringList(contexts),
                projects: task::model::StringList::default(),
                project_id,
                milestone_id,
                time_estimate: None,
                time_entries: task::model::TimeEntries::default(),
                recurrence: None,
                recurrence_anchor: None,
                complete_instances: task::model::StringList::default(),
                completed_date: None,
                agent_profile: String::new(),
                dispatched_agent_tasks: task::model::StringList::default(),
                date_created: None,
                date_modified: None,
                details,
                workflow: None,
            };
            let client = connect_task_client(&url).await?;
            let created = client
                .create(new_task)
                .await
                .map_err(|e| eyre::eyre!("create: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&created).map_err(|e| eyre::eyre!("json: {e}"))?
                );
            } else {
                println!("created {} ({})", created.title, created.path);
                println!("  id: {}", created.id);
            }
        }
        TaskCmd::Start {
            target,
            org,
            server,
            json,
        } => {
            mutate_task(target, org, server, json, |t| {
                t.status = "in-progress".into();
            })
            .await?;
        }
        TaskCmd::Done {
            target,
            undo,
            org,
            server,
            json,
        } => {
            mutate_task(target, org, server, json, |t| {
                if undo {
                    t.status = "open".into();
                    t.completed_date = None;
                } else {
                    t.status = "done".into();
                    t.completed_date = Some(chrono::Local::now().date_naive());
                }
            })
            .await?;
        }
        TaskCmd::SetStatus {
            target,
            status,
            org,
            server,
            json,
        } => mutate_task(target, org, server, json, |t| t.status = status).await?,
        TaskCmd::SetPriority {
            target,
            priority,
            org,
            server,
            json,
        } => mutate_task(target, org, server, json, |t| t.priority = priority).await?,
        TaskCmd::SetDue {
            target,
            due,
            org,
            server,
            json,
        } => {
            let v = if matches!(due.as_str(), "none" | "null" | "") {
                None
            } else {
                Some(due)
            };
            mutate_task(target, org, server, json, |t| t.due = v).await?;
        }
        TaskCmd::SetScheduled {
            target,
            scheduled,
            org,
            server,
            json,
        } => {
            let v = if matches!(scheduled.as_str(), "none" | "null" | "") {
                None
            } else {
                Some(scheduled)
            };
            mutate_task(target, org, server, json, |t| t.scheduled = v).await?;
        }
        TaskCmd::SetProject {
            target,
            project,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org.clone())?;
            let url = resolve_org_vox_url(server.clone(), &slug);
            let new_proj = if matches!(project.as_str(), "none" | "null" | "") {
                None
            } else {
                let pc = connect_project_client(&url).await?;
                Some(resolve_project_target(&pc, &project).await?.id)
            };
            mutate_task(target, org, server, json, |t| t.project_id = new_proj).await?;
        }
        TaskCmd::SetMilestone {
            target,
            milestone,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org.clone())?;
            let url = resolve_org_vox_url(server.clone(), &slug);
            let (new_ms, new_proj) = if matches!(milestone.as_str(), "none" | "null" | "") {
                (None, None)
            } else {
                let mc = connect_milestone_client(&url).await?;
                let ms = resolve_milestone_target(&mc, &milestone).await?;
                (Some(ms.id), Some(ms.project_id))
            };
            mutate_task(target, org, server, json, |t| {
                t.milestone_id = new_ms;
                if let Some(p) = new_proj {
                    // Auto-fix project link when it's missing or
                    // points elsewhere — milestone is the
                    // narrower truth.
                    t.project_id = Some(p);
                }
            })
            .await?;
        }
        TaskCmd::SetTags {
            target,
            tags,
            org,
            server,
            json,
        } => {
            mutate_task(target, org, server, json, |t| {
                t.tags = task::model::StringList(tags);
            })
            .await?;
        }
        TaskCmd::SetParent {
            target,
            parent,
            org,
            server,
            json,
        } => {
            let parent_id = match parent.as_str() {
                "none" | "null" => None,
                p => {
                    let slug = resolve_active_org(org.clone())?;
                    let url = resolve_org_vox_url(server.clone(), &slug);
                    let client = connect_task_client(&url).await?;
                    Some(crate::json_out::resolve_task_flexible(&client, p).await?.id)
                }
            };
            mutate_task(target, org, server, json, |t| {
                let mut wf = t.workflow.clone().unwrap_or_default();
                wf.parent = parent_id;
                t.workflow = Some(wf);
            })
            .await?;
        }
        TaskCmd::SetContexts {
            target,
            contexts,
            org,
            server,
            json,
        } => {
            let contexts: Vec<String> = contexts
                .into_iter()
                .map(|c| {
                    if c.starts_with('@') {
                        c
                    } else {
                        format!("@{c}")
                    }
                })
                .collect();
            mutate_task(target, org, server, json, |t| {
                t.contexts = task::model::StringList(contexts);
            })
            .await?;
        }
        TaskCmd::Rename {
            target,
            new_path,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_task_client(&url).await?;
            let t = resolve_task_target(&client, &target).await?;
            let renamed = client
                .rename(t.id, new_path)
                .await
                .map_err(|e| eyre::eyre!("rename: {e:?}"))?;
            if json {
                crate::json_out::print_json(&renamed)?;
            } else {
                println!("renamed → {}", renamed.path);
            }
        }
        TaskCmd::MoveOrg {
            target,
            to_org,
            yes,
            org,
            server,
        } => {
            let from_slug = resolve_active_org(org.clone())?;
            if from_slug == to_org {
                return Err(eyre::eyre!(
                    "`{to_org}` is already this task's org — nothing to move"
                ));
            }
            let from_url = resolve_org_vox_url(server.clone(), &from_slug);
            let from = connect_task_client(&from_url).await?;
            let mut t = resolve_task_target(&from, &target).await?;

            // Everything org-scoped goes; see the verb's doc comment.
            let dropped: Vec<&str> = [
                t.project_id.is_some().then_some("project"),
                t.milestone_id.is_some().then_some("milestone"),
                t.workflow
                    .as_ref()
                    .and_then(|w| w.parent)
                    .is_some()
                    .then_some("parent"),
                t.workflow
                    .as_ref()
                    .and_then(|w| w.workstream)
                    .is_some()
                    .then_some("workstream"),
            ]
            .into_iter()
            .flatten()
            .collect();

            if !yes {
                println!("move `{}` ({})", t.title, t.path);
                println!("  from org: {from_slug}");
                println!("  to   org: {to_org}");
                if !dropped.is_empty() {
                    println!(
                        "  dropping (source-org ids, meaningless in `{to_org}`): {}",
                        dropped.join(", ")
                    );
                }
                if !confirm("proceed?")? {
                    println!("aborted");
                    return Ok(());
                }
            }

            t.project_id = None;
            t.milestone_id = None;
            t.projects = Default::default();
            if let Some(w) = t.workflow.as_mut() {
                w.parent = None;
                w.workstream = None;
            }

            let to_url = resolve_org_vox_url(server, &to_org);
            let to = connect_task_client(&to_url).await?;
            // Create first. If this fails the source is untouched.
            let created = to
                .create(t.clone())
                .await
                .map_err(|e| eyre::eyre!("create in `{to_org}`: {e:?}"))?;
            // Then remove the original. A failure here is visible as a
            // duplicate rather than a loss, so say exactly that.
            if let Err(e) = from.delete(t.id).await {
                println!("created {} in `{to_org}`", created.path);
                return Err(eyre::eyre!(
                    "moved into `{to_org}` but could NOT delete the original in \
                     `{from_slug}`: {e:?}\nthe task now exists in BOTH orgs — delete \
                     it from `{from_slug}` by hand: task --org {from_slug} task delete {}",
                    t.id
                ));
            }
            println!("moved {} → `{to_org}` ({})", t.title, created.path);
            println!("  id {} preserved", created.id);
            if !dropped.is_empty() {
                println!(
                    "  dropped {}: re-file it there with `task --org {to_org} task set-project …`",
                    dropped.join(", ")
                );
            }
        }
        TaskCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_task_client(&url).await?;
            let t = resolve_task_target(&client, &target).await?;
            if !yes && !confirm(&format!("delete `{}` ({})?", t.title, t.path))? {
                println!("aborted");
                return Ok(());
            }
            client
                .delete(t.id)
                .await
                .map_err(|e| eyre::eyre!("delete: {e:?}"))?;
            println!("deleted {}", t.path);
        }
    }
    Ok(())
}

pub(crate) async fn connect_task_client(url: &str) -> eyre::Result<task::TaskServiceClient> {
    establish_for_url(url).await
}

/// Resolve a task reference — uuid, vault path, title, or a unique
/// prefix of either (shared flexible resolver).
async fn resolve_task_target(
    client: &task::TaskServiceClient,
    target: &str,
) -> eyre::Result<task::TaskInfo> {
    crate::json_out::resolve_task_flexible(client, target).await
}

async fn mutate_task<F>(
    target: String,
    org: Option<String>,
    server: Option<String>,
    json: bool,
    apply: F,
) -> eyre::Result<()>
where
    F: FnOnce(&mut task::TaskInfo),
{
    let slug = resolve_active_org(org)?;
    let url = resolve_org_vox_url(server, &slug);
    let client = connect_task_client(&url).await?;
    let mut t = resolve_task_target(&client, &target).await?;
    apply(&mut t);
    let updated = client
        .update(t)
        .await
        .map_err(|e| eyre::eyre!("update: {e:?}"))?;
    if json {
        crate::json_out::print_json(&updated)?;
    } else {
        println!("{}  [{}]  {}", updated.title, updated.status, updated.path);
    }
    Ok(())
}
