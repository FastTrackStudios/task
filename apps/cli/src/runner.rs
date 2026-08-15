//! `task runner` — register this machine as a runner, list the
//! registry, heartbeat, and deregister.
//!
//! A runner is the thing that actually executes agent work. It
//! declares what it can do; the server routes tickets by matching
//! against that declaration. Capabilities are a closed set —
//! `records`, `shell`, `build`, `repo:<owner>/<name>` — and `build`
//! is deliberately separate from `shell` so a machine can read
//! source without being allowed to compile.

use agent_proto::backend::{AgentBackend, BackendKind};
use agent_proto::runner::{RunnerProfile, RunnerScope, parse_capabilities};
use agent_proto::service::backends::BackendsClient;
use agent_proto::service::questions::QuestionsClient;
use agent_proto::service::run_stream::RunStreamClient;
use agent_proto::service::runs::RunsClient;
use chrono::Utc;
use clap::Subcommand;

use crate::establish_for_url;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;

#[derive(Subcommand, Debug)]
pub enum RunnerCmd {
    /// Register this machine (or update its registration).
    ///
    /// Re-running with different flags updates in place — the id is
    /// the identity, so a runner that restarts does not duplicate.
    Register {
        /// Stable runner id. Defaults to this machine's hostname,
        /// which is almost always what you want.
        #[arg(long)]
        id: Option<String>,
        /// Human-facing label. Defaults to the id.
        #[arg(long)]
        label: Option<String>,
        /// Repeatable capability: `records`, `shell`, `build`, or
        /// `repo:<owner>/<name>`. Anything else is refused.
        #[arg(long = "cap", value_name = "CAPABILITY")]
        caps: Vec<String>,
        /// Repeatable org slug this runner serves. Omit for any.
        #[arg(long = "scope-org", value_name = "SLUG")]
        scope_orgs: Vec<String>,
        /// Repeatable project id this runner serves. Omit for any.
        #[arg(long = "scope-project", value_name = "UUID")]
        scope_projects: Vec<uuid::Uuid>,
        /// How many tickets to hold at once. `0` registers the
        /// runner but takes nothing — the way to drain a machine
        /// without deregistering it.
        #[arg(long, default_value_t = 2)]
        max_concurrent: u32,
        /// Send a heartbeat straight after registering, so the
        /// runner is immediately routable.
        #[arg(long, default_value_t = true)]
        beat: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// List every registered runner and whether it is live.
    List {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Tell the server this runner is alive.
    Beat {
        /// Runner id. Defaults to this machine's hostname.
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Deregister a runner.
    Remove {
        id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// What this runner may take right now — the queue as the
    /// runner sees it, filtered by capability, scope and free slots.
    ///
    /// `--why` also prints, for each ticket it cannot take, the
    /// reason — which is how you answer "why is my runner idle?".
    Takeable {
        /// Runner id. Defaults to this machine's hostname.
        #[arg(long)]
        id: Option<String>,
        /// How many tickets this runner already holds.
        #[arg(long, default_value_t = 0)]
        in_flight: u32,
        /// Also explain every refusal.
        #[arg(long)]
        why: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Take one ticket and work it: claim, cut a worktree and
    /// branch, run the agent, run the verify command, commit, and
    /// hand back.
    ///
    /// Exit zero on the verify command moves the ticket to
    /// `needs-review` with its branch. Anything else leaves the
    /// ticket unclaimed for another attempt, with the worktree still
    /// on disk so the failure can be inspected.
    ///
    /// Nothing is pushed and no mainline is touched — the branch is
    /// the handback.
    Work {
        /// Runner id. Defaults to this machine's hostname.
        #[arg(long)]
        id: Option<String>,
        /// Repository to cut worktrees from.
        #[arg(long)]
        repo: std::path::PathBuf,
        /// Where this runner puts worktrees. The runner chooses;
        /// the server only learns the path afterwards.
        #[arg(long)]
        worktree_root: std::path::PathBuf,
        /// Branch to cut from.
        #[arg(long, default_value = "main")]
        base: String,
        /// Command that runs the agent inside the worktree. It is
        /// given the ticket prompt on stdin and `TASK_TICKET_ID` /
        /// `TASK_TICKET_TITLE` in the environment. Omit to do
        /// everything except the agent — useful for checking the
        /// plumbing without spending tokens.
        #[arg(long)]
        agent_cmd: Option<String>,
        /// Work a specific ticket instead of the first takeable one.
        #[arg(long)]
        ticket: Option<String>,
        /// Go through the motions without claiming, committing or
        /// relabelling. Prints what it would do.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Run continuously: heartbeat, take whatever is takeable, work
    /// it, repeat. This is the daemon — triage without a human
    /// driving each step.
    ///
    /// A failed iteration is logged and the loop continues; a bad
    /// ticket must not take the runner down. Ctrl-C stops it between
    /// tickets, never mid-ticket.
    Serve {
        /// Runner id. Defaults to this machine's hostname.
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        repo: std::path::PathBuf,
        #[arg(long)]
        worktree_root: std::path::PathBuf,
        #[arg(long, default_value = "main")]
        base: String,
        /// Command that runs the agent inside the worktree. Omit to
        /// exercise the loop without spending tokens.
        #[arg(long)]
        agent_cmd: Option<String>,
        /// Seconds between polls.
        #[arg(long, default_value_t = 30)]
        interval: u64,
        /// Stop after this many tickets. `0` = run until stopped.
        #[arg(long, default_value_t = 0)]
        max_tickets: u32,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Everything blocking a human, in one place.
    ///
    /// Three panels: questions awaiting an answer, runs executing
    /// now, and branches awaiting review. Scoped to a project with
    /// `--project`, or the whole fleet without it.
    Surface {
        /// Project id, name, path or prefix. Omit for every project.
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Watch a run live — snapshot, then fold the event stream.
    ///
    /// A viewer arriving mid-run sees the recent output tail rather
    /// than an empty pane, which is the whole reason there is a
    /// snapshot as well as a stream.
    Watch {
        /// Run id (or a unique prefix). Omit to watch everything.
        run: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Attempt history — every run, or one ticket's.
    ///
    /// "This has died three times on the same verify command" is the
    /// most useful thing this system can tell you, and it is only
    /// answerable because failed attempts are kept.
    Runs {
        /// Only this ticket's attempts.
        #[arg(long)]
        ticket: Option<String>,
        /// Only this runner's.
        #[arg(long)]
        runner: Option<String>,
        /// Cap the list.
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// First move lapsed in-progress runs to `stale`.
        #[arg(long)]
        sweep: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Work a whole workstream: its tickets in dependency order,
    /// each on its own branch, merged into one workstream branch.
    ///
    /// The manager is itself a run, so it is as observable and as
    /// killable as the attempts it spawns. Its children carry
    /// `parent`, which is what makes the tree queryable.
    ///
    /// One reviewable branch comes out, not one per ticket.
    Workstream {
        /// Workstream id (or a unique prefix).
        workstream: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        repo: std::path::PathBuf,
        #[arg(long)]
        worktree_root: std::path::PathBuf,
        #[arg(long, default_value = "main")]
        base: String,
        #[arg(long)]
        agent_cmd: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Raise a question against a ticket and block it on a human.
    ///
    /// This is the interface an agent calls when it needs a decision.
    /// How a given agent decides to call it — a sentinel file, a
    /// stdout tag, a wrapper script — is deliberately left open.
    Ask {
        /// Ticket to block.
        ticket: String,
        /// The question text.
        text: String,
        /// Repeatable option label.
        #[arg(long = "option", value_name = "LABEL")]
        options: Vec<String>,
        /// Short chip-style header.
        #[arg(long, default_value = "Decision")]
        header: String,
        /// The run raising it, if any.
        #[arg(long)]
        run: Option<uuid::Uuid>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// The grill queue — questions agents are waiting on you for.
    Questions {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Answer a question, unblocking its ticket.
    ///
    /// Clears `needs-input` and restores `ready-for-agent`, so a
    /// runner takes the ticket up again. The run that raised the
    /// question is recorded on it, so the answer can resume that
    /// session rather than starting cold.
    Answer {
        /// Question request id (or a unique prefix).
        request: String,
        /// The chosen option label, or free text.
        answer: String,
        /// Optional note passed along with the choice.
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Agent-ready tickets nothing in the fleet can take.
    ///
    /// A ticket no live runner satisfies must be reported, not left
    /// sitting in the queue looking available. Malformed tickets —
    /// a capability nobody could ever offer because it is a typo —
    /// are listed separately, because the fix is editing the ticket
    /// rather than adding a machine.
    Unroutable {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

/// This machine's name, used as the default runner id.
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| "runner".into())
}

async fn client(org: Option<String>, server: Option<String>) -> eyre::Result<BackendsClient> {
    let slug = resolve_active_org(org)?;
    let url = resolve_org_vox_url(server, &slug);
    establish_for_url(&url).await
}

pub(crate) async fn run_runner(cmd: RunnerCmd) -> eyre::Result<()> {
    match cmd {
        RunnerCmd::Register {
            id,
            label,
            caps,
            scope_orgs,
            scope_projects,
            max_concurrent,
            beat,
            org,
            server,
        } => {
            let id = id.unwrap_or_else(hostname);
            // Parse before dialling: a bad capability should fail
            // here, naming the token, not as a wire error.
            let capabilities = parse_capabilities(&caps)?;

            let backend = AgentBackend {
                id: id.clone(),
                label: label.unwrap_or_else(|| id.clone()),
                kind: BackendKind::CliBridge,
                config_json: String::new(),
                registered_at: Utc::now(),
                last_seen: None,
                runner: RunnerProfile {
                    id: id.clone(),
                    capabilities,
                    scope: RunnerScope {
                        orgs: scope_orgs,
                        projects: scope_projects,
                    },
                    max_concurrent,
                },
            };

            let c = client(org, server).await?;
            let saved = c.upsert_backend(backend).await?;
            if beat {
                c.heartbeat_backend(saved.id.clone()).await?;
            }

            println!("registered {}", saved.id);
            println!(
                "  capabilities: {}",
                if saved.runner.capabilities.is_empty() {
                    "(none)".to_string()
                } else {
                    saved
                        .runner
                        .capabilities
                        .iter()
                        .map(agent_proto::runner::Capability::as_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            );
            println!("  max concurrent: {}", saved.runner.max_concurrent);
            let scope = &saved.runner.scope;
            println!(
                "  scope: {}",
                if scope.orgs.is_empty() && scope.projects.is_empty() {
                    "unrestricted".to_string()
                } else {
                    format!(
                        "orgs=[{}] projects=[{}]",
                        scope.orgs.join(", "),
                        scope
                            .projects
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            );
            if beat {
                println!("  heartbeat: sent (routable now)");
            }
        }

        RunnerCmd::List { org, server } => {
            let c = client(org, server).await?;
            let runners = c.list_backends().await?;
            if runners.is_empty() {
                println!("(no runners registered)");
                return Ok(());
            }
            for r in runners {
                let health = c.backend_health(r.id.clone()).await?;
                let state = if health.reachable { "live " } else { "stale" };
                let caps = r
                    .runner
                    .capabilities
                    .iter()
                    .map(agent_proto::runner::Capability::as_string)
                    .collect::<Vec<_>>()
                    .join(",");
                println!(
                    "{state}  {:<20} x{:<3} {caps}",
                    r.id, r.runner.max_concurrent
                );
            }
        }

        RunnerCmd::Beat { id, org, server } => {
            let id = id.unwrap_or_else(hostname);
            let c = client(org, server).await?;
            c.heartbeat_backend(id.clone()).await?;
            println!("beat {id}");
        }

        RunnerCmd::Remove { id, org, server } => {
            let c = client(org, server).await?;
            c.remove_backend(id.clone()).await?;
            println!("removed {id}");
        }

        RunnerCmd::Takeable {
            id,
            in_flight,
            why,
            org,
            server,
        } => {
            let id = id.unwrap_or_else(hostname);
            let (slug, url) = ctx(org, server)?;
            let backends: BackendsClient = establish_for_url(&url).await?;
            let me = backends
                .list_backends()
                .await?
                .into_iter()
                .find(|b| b.id == id)
                .ok_or_else(|| eyre::eyre!("runner `{id}` is not registered"))?;

            let tickets = agent_ready_tickets(&url).await?;
            let refs = ticket_refs(&tickets, &slug);

            let takeable = agent_proto::routing::takeable(&me.runner, &refs, in_flight);
            if takeable.is_empty() {
                println!("(nothing takeable)");
            }
            for tid in &takeable {
                if let Some(t) = tickets.iter().find(|t| t.id == *tid) {
                    println!("{}  {}", crate::shared::short_uuid(&t.id), t.title);
                }
            }

            if why {
                for r in agent_proto::routing::refusals(&me.runner, &refs, in_flight) {
                    if let Some(t) = tickets.iter().find(|t| t.id == r.ticket) {
                        println!(
                            "skip {}  {}  — {}",
                            crate::shared::short_uuid(&t.id),
                            t.title,
                            r.reason
                        );
                    }
                }
            }
        }

        RunnerCmd::Work {
            id,
            repo,
            worktree_root,
            base,
            agent_cmd,
            ticket,
            dry_run,
            org,
            server,
        } => {
            let id = id.unwrap_or_else(hostname);
            let (slug, url) = ctx(org, server)?;
            let job = Job {
                runner: id,
                slug,
                url,
                repo,
                worktree_root,
                base,
                agent_cmd,
                dry_run,
            };
            work_one(&job, ticket.as_deref(), 0).await?;
        }

        RunnerCmd::Serve {
            id,
            repo,
            worktree_root,
            base,
            agent_cmd,
            interval,
            max_tickets,
            org,
            server,
        } => {
            let id = id.unwrap_or_else(hostname);
            let (slug, url) = ctx(org, server)?;
            let job = Job {
                runner: id.clone(),
                slug,
                url: url.clone(),
                repo,
                worktree_root,
                base,
                agent_cmd,
                dry_run: false,
            };

            println!("serving as {id}; polling every {interval}s (ctrl-c to stop)");
            let mut worked: u32 = 0;
            loop {
                // Heartbeat first. A runner that is working must
                // still say so, or it goes stale mid-ticket and the
                // router stops offering it anything.
                let backends: BackendsClient = establish_for_url(&url).await?;
                if let Err(e) = backends.heartbeat_backend(id.clone()).await {
                    eprintln!("heartbeat failed: {e:?}");
                }

                // Supervise before taking anything new: a stuck
                // run holding a claim would otherwise make its
                // ticket invisible forever.
                if let Err(e) = supervise(&job).await {
                    eprintln!("supervision failed: {e}");
                }

                match work_one(&job, None, worked).await {
                    Ok(Outcome::Worked) => worked += 1,
                    Ok(Outcome::Idle) => {}
                    // A bad ticket or a lost race must not kill the
                    // daemon — log it and carry on to the next poll.
                    Err(e) => eprintln!("iteration failed: {e}"),
                }

                if max_tickets != 0 && worked >= max_tickets {
                    println!("worked {worked} ticket(s); stopping as asked");
                    break;
                }

                tokio::select! {
                    () = tokio::time::sleep(std::time::Duration::from_secs(interval)) => {}
                    r = tokio::signal::ctrl_c() => {
                        if r.is_ok() {
                            println!("\nstopping");
                        }
                        break;
                    }
                }
            }
        }

        RunnerCmd::Surface {
            project,
            org,
            server,
        } => {
            let (_slug, url) = ctx(org, server)?;
            let client = crate::task_cmd::connect_task_client(&url).await?;

            let scope = match &project {
                None => None,
                Some(p) => {
                    let pc = crate::project::connect_project_client(&url).await?;
                    Some(
                        crate::json_out::resolve_project_flexible(&pc, p)
                            .await?
                            .id,
                    )
                }
            };
            let in_scope = |t: &task::TaskInfo| scope.is_none_or(|s| t.project_id == Some(s));

            let all = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;

            // 1. Questions — the only panel where something is
            //    waiting on *you* specifically.
            let qc: QuestionsClient = establish_for_url(&url).await?;
            let pending = qc.unresolved_questions().await?;
            println!("── questions awaiting you ──");
            let mut shown = 0;
            for req in &pending {
                let Some(ticket) = qc.question_ticket(req.id.clone()).await? else {
                    continue;
                };
                let Some(t) = all.iter().find(|t| t.id == ticket) else {
                    continue;
                };
                if !in_scope(t) {
                    continue;
                }
                shown += 1;
                println!("  {}  {}", &req.id[..8.min(req.id.len())], t.title);
                for q in &req.questions {
                    println!("      {}", q.text);
                }
            }
            if shown == 0 {
                println!("  (nothing)");
            }

            // 2. Running now.
            let runs: RunsClient = establish_for_url(&url).await?;
            let live = runs
                .list_runs(agent_proto::run::RunFilter {
                    status: Some(agent_proto::run::RunStatus::InProgress),
                    ..Default::default()
                })
                .await?;
            println!("── running now ──");
            let mut any = false;
            for r in &live {
                let Some(t) = all.iter().find(|t| t.id == r.ticket) else {
                    continue;
                };
                if !in_scope(t) {
                    continue;
                }
                any = true;
                println!(
                    "  {}  {}  on {}",
                    crate::shared::short_uuid(&r.id),
                    t.title,
                    r.runner
                );
            }
            if !any {
                println!("  (nothing)");
            }

            // 3. Awaiting review — green branches.
            println!("── awaiting review ──");
            let review: Vec<&task::TaskInfo> = all
                .iter()
                .filter(|t| task::has_triage_label(t, task::TriageLabel::NeedsReview))
                .filter(|t| in_scope(t))
                .collect();
            if review.is_empty() {
                println!("  (nothing)");
            }
            for t in review {
                let branch = agent_worktree::branch_for(&crate::shared::short_uuid(&t.id));
                println!("  {}  {}  {branch}", crate::shared::short_uuid(&t.id), t.title);
            }
        }

        RunnerCmd::Watch { run, org, server } => {
            let (_slug, url) = ctx(org, server)?;
            let rs: RunStreamClient = establish_for_url(&url).await?;

            // Fetch once…
            let wanted = match &run {
                None => None,
                Some(prefix) => {
                    let runs: RunsClient = establish_for_url(&url).await?;
                    let all = runs
                        .list_runs(agent_proto::run::RunFilter::default())
                        .await?;
                    let hit = all
                        .iter()
                        .find(|r| r.id.to_string().starts_with(prefix.trim()))
                        .ok_or_else(|| eyre::eyre!("no run matching `{prefix}`"))?;
                    let snap = rs.snapshot(hit.id).await?;
                    println!("run {}  [{}]", crate::shared::short_uuid(&snap.run), snap.status.as_str());
                    if !snap.activity.is_empty() {
                        println!("activity: {}", snap.activity);
                    }
                    if !snap.tail.is_empty() {
                        println!("--- recent output ---");
                        print!("{}", snap.tail);
                        println!("--- live ---");
                    }
                    Some(hit.id)
                }
            };

            // …then fold.
            let (tx, mut rx) = architect::vox::channel::<agent_proto::run_event::RunEventEnvelope>();
            let events: agent_proto::service::run_stream::RunStreamStreamClient =
                establish_for_url(&url).await?;
            let subscribed = events.run_events(tx);
            tokio::pin!(subscribed);
            loop {
                let received = tokio::select! {
                    r = rx.recv() => r,
                    _ = &mut subscribed => break,
                };
                let Ok(Some(msg)) = received else { break };
                let mut owned: Option<agent_proto::run_event::RunEventEnvelope> = None;
                let _ = msg.map(|e| owned = Some(e.clone()));
                let Some(env) = owned else { continue };
                if wanted.is_some_and(|w| w != env.run) {
                    continue;
                }
                let tag = crate::shared::short_uuid(&env.run);
                match env.event {
                    agent_proto::run_event::RunEvent::Output(chunk) => print!("{chunk}"),
                    agent_proto::run_event::RunEvent::Activity(what) => {
                        println!("[{tag}] {what}");
                    }
                    agent_proto::run_event::RunEvent::Status(st) => {
                        println!("[{tag}] status {}", st.as_str());
                    }
                    agent_proto::run_event::RunEvent::Verdict { passed, exit_code } => {
                        println!(
                            "[{tag}] verdict {} ({exit_code:?})",
                            if passed { "pass" } else { "FAIL" }
                        );
                    }
                    agent_proto::run_event::RunEvent::Blocked { question_id } => {
                        println!("[{tag}] blocked on question {question_id}");
                    }
                }
            }
        }

        RunnerCmd::Runs {
            ticket,
            runner,
            limit,
            sweep,
            org,
            server,
        } => {
            let (_slug, url) = ctx(org, server)?;
            let runs: RunsClient = establish_for_url(&url).await?;

            if sweep {
                let n = runs.sweep_stale_runs().await?;
                println!("swept {n} run(s) to stale");
            }

            let ticket_id = match &ticket {
                None => None,
                Some(t) => {
                    let client = crate::task_cmd::connect_task_client(&url).await?;
                    Some(crate::issue::resolve_issue_id(&client, t).await?.id)
                }
            };

            let list = runs
                .list_runs(agent_proto::run::RunFilter {
                    ticket: ticket_id,
                    runner: runner.unwrap_or_default(),
                    parent: None,
                    status: None,
                    limit,
                })
                .await?;

            if list.is_empty() {
                println!("(no runs)");
                return Ok(());
            }
            for r in list {
                let code = r
                    .exit_code
                    .map_or_else(|| "-".to_string(), |c| c.to_string());
                println!(
                    "{:<13} {}  ticket {}  exit {code:<4} {}",
                    r.status.as_str(),
                    crate::shared::short_uuid(&r.id),
                    crate::shared::short_uuid(&r.ticket),
                    r.branch
                );
            }
        }

        RunnerCmd::Workstream {
            workstream,
            id,
            repo,
            worktree_root,
            base,
            agent_cmd,
            org,
            server,
        } => {
            let id = id.unwrap_or_else(hostname);
            let (slug, url) = ctx(org, server)?;
            let job = Job {
                runner: id.clone(),
                slug,
                url: url.clone(),
                repo: repo.clone(),
                worktree_root: worktree_root.clone(),
                base: base.clone(),
                agent_cmd,
                dry_run: false,
            };
            run_workstream(&job, &workstream).await?;
        }

        RunnerCmd::Ask {
            ticket,
            text,
            options,
            header,
            run,
            org,
            server,
        } => {
            let (_slug, url) = ctx(org, server)?;
            let client = crate::task_cmd::connect_task_client(&url).await?;
            let id = crate::issue::resolve_issue_id(&client, &ticket).await?.id;

            let question = agent_proto::question::Question {
                id: uuid::Uuid::new_v4().to_string(),
                header,
                text,
                options: options
                    .into_iter()
                    .map(|label| agent_proto::question::QuestionOption {
                        label,
                        description: String::new(),
                        preview: String::new(),
                    })
                    .collect(),
                multi_select: false,
            };
            block_on_question(&url, id, run, vec![question]).await?;
            println!(
                "{} → needs-input (asked)",
                crate::shared::short_uuid(&id)
            );
        }

        RunnerCmd::Questions { org, server } => {
            let (_slug, url) = ctx(org, server)?;
            let qc: QuestionsClient = establish_for_url(&url).await?;
            let pending = qc.unresolved_questions().await?;
            if pending.is_empty() {
                println!("(nothing waiting on you)");
                return Ok(());
            }
            let client = crate::task_cmd::connect_task_client(&url).await?;
            for req in pending {
                let ticket = qc.question_ticket(req.id.clone()).await?;
                let title = match ticket {
                    Some(t) => client
                        .get(t)
                        .await
                        .map(|x| x.title)
                        .unwrap_or_else(|_| "(unknown ticket)".into()),
                    None => "(no ticket)".into(),
                };
                println!("{}  {title}", &req.id[..8.min(req.id.len())]);
                for q in &req.questions {
                    println!("  [{}] {}", q.header, q.text);
                    for o in &q.options {
                        println!("      - {}  {}", o.label, o.description);
                    }
                }
            }
        }

        RunnerCmd::Answer {
            request,
            answer,
            notes,
            org,
            server,
        } => {
            let (_slug, url) = ctx(org, server)?;
            let qc: QuestionsClient = establish_for_url(&url).await?;

            let pending = qc.unresolved_questions().await?;
            let req = pending
                .iter()
                .find(|q| q.id.starts_with(request.trim()))
                .ok_or_else(|| eyre::eyre!("no unresolved question matching `{request}`"))?;
            let first = req
                .questions
                .first()
                .ok_or_else(|| eyre::eyre!("question {} carries no questions", req.id))?;

            let resolved = qc
                .answer_question(
                    req.id.clone(),
                    vec![agent_proto::question::QuestionAnswer {
                        question_id: first.id.clone(),
                        selected: vec![answer.clone()],
                        notes: notes.unwrap_or_default(),
                    }],
                )
                .await?;
            println!("answered {}", &resolved.id[..8.min(resolved.id.len())]);

            // Unblock the ticket: an answered question must put the
            // work back in the runner queue, or answering achieves
            // nothing.
            if let Some(ticket) = qc.question_ticket(resolved.id.clone()).await? {
                let client = crate::task_cmd::connect_task_client(&url).await?;
                let still_open = qc.questions_for_ticket(ticket).await?;
                if still_open.is_empty() {
                    mark_ready_again(&client, ticket).await?;
                    println!("{} → ready-for-agent", crate::shared::short_uuid(&ticket));
                } else {
                    println!(
                        "{} still has {} unanswered question(s)",
                        crate::shared::short_uuid(&ticket),
                        still_open.len()
                    );
                }
            }
        }

        RunnerCmd::Unroutable { org, server } => {
            let (slug, url) = ctx(org, server)?;
            let backends: BackendsClient = establish_for_url(&url).await?;

            // Live runners only: a registration nobody is behind
            // must not make a ticket look routable.
            let mut live = Vec::new();
            for b in backends.list_backends().await? {
                if backends.backend_health(b.id.clone()).await?.reachable {
                    live.push(b.runner);
                }
            }

            let tickets = agent_ready_tickets(&url).await?;
            let refs = ticket_refs(&tickets, &slug);

            let stuck = agent_proto::routing::unroutable(&refs, &live);
            let bad = agent_proto::routing::malformed(&refs);

            if stuck.is_empty() && bad.is_empty() {
                println!("(everything agent-ready can be taken by some runner)");
            }
            for (tid, reason) in stuck {
                if let Some(t) = tickets.iter().find(|t| t.id == tid) {
                    println!(
                        "unroutable  {}  {}  — {reason}",
                        crate::shared::short_uuid(&t.id),
                        t.title
                    );
                }
            }
            for (tid, reason) in bad {
                if let Some(t) = tickets.iter().find(|t| t.id == tid) {
                    println!(
                        "malformed   {}  {}  — {reason}",
                        crate::shared::short_uuid(&t.id),
                        t.title
                    );
                }
            }
        }
    }
    Ok(())
}

/// Run the agent inside the worktree, handing it the ticket.
fn run_agent(
    cmd: &str,
    cwd: &std::path::Path,
    ticket_id: &str,
    title: &str,
    prompt: &str,
) -> eyre::Result<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .env("TASK_TICKET_ID", ticket_id)
        .env("TASK_TICKET_TITLE", title)
        .stdin(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes())?;
    }
    let status = child.wait()?;
    Ok(status
        .code()
        .map_or_else(|| "signal".to_string(), |c| c.to_string()))
}

/// Drop our claim so the ticket can be attempted again.
async fn release_claim(
    client: &task::TaskServiceClient,
    ticket: &task::TaskInfo,
) -> eyre::Result<()> {
    // Re-fetch rather than reusing the copy read before the claim:
    // that one predates the assignee we are trying to remove, and
    // writing it back is a no-op the caller cannot see.
    let mut t = client
        .get(ticket.id)
        .await
        .map_err(|e| eyre::eyre!("re-read ticket: {e:?}"))?;
    if let Some(w) = t.workflow.as_mut() {
        w.assignees.0.clear();
    }
    client
        .update(t)
        .await
        .map_err(|e| eyre::eyre!("release claim: {e:?}"))?;

    // Verify: a claim that silently fails to release turns one bad
    // ticket into a runner that can never take it again.
    let after = client
        .get(ticket.id)
        .await
        .map_err(|e| eyre::eyre!("verify release: {e:?}"))?;
    let still_held = after
        .workflow
        .as_ref()
        .is_some_and(|w| !w.assignees.0.is_empty());
    if still_held {
        return Err(eyre::eyre!(
            "claim on {} did not release — it would never be retried",
            crate::shared::short_uuid(&ticket.id)
        ));
    }
    Ok(())
}

/// Move a green ticket to `needs-review`, dropping `ready-for-agent`
/// so it stops appearing in the runner queue.
async fn mark_needs_review(
    client: &task::TaskServiceClient,
    ticket: &task::TaskInfo,
) -> eyre::Result<()> {
    let mut t = ticket.clone();
    t.tags
        .0
        .retain(|tag| task::TriageLabel::parse(tag) != Some(task::TriageLabel::ReadyForAgent));
    if !task::has_triage_label(&t, task::TriageLabel::NeedsReview) {
        t.tags.0.push(task::TriageLabel::NeedsReview.as_str().into());
    }
    client
        .update(t)
        .await
        .map_err(|e| eyre::eyre!("mark needs-review: {e:?}"))?;
    Ok(())
}

/// Work every ticket in a workstream, merging each green branch into
/// one workstream branch.
///
/// Tickets are taken in dependency order — a ticket whose blockers
/// are still open is skipped this pass rather than failed, because
/// its turn simply has not come.
///
/// A ticket branch that will not merge stops the manager. Resolving
/// a conflict is a judgement about intent, and inventing one is how
/// you get a green branch nobody meant.
async fn run_workstream(job: &Job, workstream: &str) -> eyre::Result<()> {
    let ws_client: ::workstream::WorkstreamServiceClient = establish_for_url(&job.url).await?;
    let all = ws_client
        .list(None)
        .await
        .map_err(|e| eyre::eyre!("list workstreams: {e:?}"))?;
    let ws = all
        .iter()
        .find(|w| {
            w.id.to_string().starts_with(workstream.trim())
                || w.title.eq_ignore_ascii_case(workstream.trim())
        })
        .ok_or_else(|| eyre::eyre!("no workstream matching `{workstream}`"))?;

    let short_ws = crate::shared::short_uuid(&ws.id);
    println!("workstream {short_ws}  {}", ws.title);

    // The manager is a run. Its worktree is the integration branch
    // every ticket merges into.
    let ws_wt = agent_worktree::create(
        &job.repo,
        &job.worktree_root,
        &format!("ws-{short_ws}"),
        &job.base,
    )?;
    println!("branch     {}", ws_wt.branch);

    let runs: RunsClient = establish_for_url(&job.url).await?;
    let manager = runs
        .start_run(agent_proto::run::StartRun {
            ticket: ws.id,
            runner: job.runner.clone(),
            parent: None,
            branch: ws_wt.branch.clone(),
            worktree_path: ws_wt.path.display().to_string(),
            session_path: String::new(),
        })
        .await?;
    println!("manager    {}", crate::shared::short_uuid(&manager.id));

    let client = crate::task_cmd::connect_task_client(&job.url).await?;
    let mut merged = 0_u32;
    let mut skipped = Vec::new();

    loop {
        let tickets = agent_ready_tickets(&job.url).await?;
        let mine: Vec<task::TaskInfo> = tickets
            .into_iter()
            .filter(|t| t.workflow.as_ref().and_then(|w| w.workstream) == Some(ws.id))
            .filter(|t| !skipped.contains(&t.id))
            .collect();

        let Some(ticket) = mine.into_iter().next() else {
            break;
        };
        let short = crate::shared::short_uuid(&ticket.id);

        match work_one(job, Some(&ticket.id.to_string()), 0).await {
            Ok(Outcome::Worked) => {}
            Ok(Outcome::Idle) => {
                skipped.push(ticket.id);
                continue;
            }
            Err(e) => {
                eprintln!("{short} failed: {e}");
                skipped.push(ticket.id);
                continue;
            }
        }

        // Only a green ticket earns a merge.
        let fresh = client
            .get(ticket.id)
            .await
            .map_err(|e| eyre::eyre!("re-read ticket: {e:?}"))?;
        if !task::has_triage_label(&fresh, task::TriageLabel::NeedsReview) {
            println!("{short} did not go green — not merging");
            skipped.push(ticket.id);
            continue;
        }

        let branch = agent_worktree::branch_for(&short);
        agent_worktree::merge_into(&ws_wt, &branch).map_err(|e| {
            eyre::eyre!(
                "{branch} will not merge into {}: {e}. \
                 Resolve it by hand — guessing at intent is how you get \
                 a green branch nobody meant.",
                ws_wt.branch
            )
        })?;
        merged += 1;
        println!("merged {branch} → {}", ws_wt.branch);
        skipped.push(ticket.id);
    }

    runs.finish_run(agent_proto::run::FinishRun {
        run: manager.id,
        passed: merged > 0,
        exit_code: None,
        worktree_kept: true,
    })
    .await?;

    println!("{merged} ticket(s) merged into {}", ws_wt.branch);
    if merged == 0 {
        println!("(nothing was takeable — blockers open, or nothing agent-ready)");
    }
    Ok(())
}

/// Restart stuck runs, and hand over the ones out of restarts.
///
/// The supervisor's only recovery power is restart — it never
/// answers a question and never declares work done. See
/// `agent_proto::supervisor`.
async fn supervise(job: &Job) -> eyre::Result<()> {
    use agent_proto::supervisor::{MAX_RESTARTS, NO_PROGRESS_AFTER, Recovery, decide, is_supervisable};

    let runs: RunsClient = establish_for_url(&job.url).await?;

    // Fold lapsed heartbeats into `stale` first, so the policy sees
    // current liveness rather than yesterday's.
    runs.sweep_stale_runs().await?;

    let mine = runs
        .list_runs(agent_proto::run::RunFilter {
            runner: job.runner.clone(),
            ..Default::default()
        })
        .await?;

    let now = chrono::Utc::now();
    for run in mine.iter().filter(|r| is_supervisable(r.status)) {
        let attempts =
            u32::try_from(mine.iter().filter(|r| r.ticket == run.ticket).count()).unwrap_or(u32::MAX);

        match decide(run, now, NO_PROGRESS_AFTER, attempts, MAX_RESTARTS) {
            Recovery::Leave => {}
            Recovery::Restart => {
                end_stuck_run(&runs, run.id).await?;
                let client = crate::task_cmd::connect_task_client(&job.url).await?;
                if let Ok(t) = client.get(run.ticket).await {
                    release_claim(&client, &t).await?;
                }
                println!(
                    "restarting {} — no progress",
                    crate::shared::short_uuid(&run.ticket)
                );
            }
            Recovery::Escalate => {
                end_stuck_run(&runs, run.id).await?;
                let question = agent_proto::question::Question {
                    id: uuid::Uuid::new_v4().to_string(),
                    header: "Stuck".into(),
                    text: agent_proto::supervisor::escalation_text(attempts),
                    options: vec![],
                    multi_select: false,
                };
                block_on_question(&job.url, run.ticket, Some(run.id), vec![question]).await?;
                println!(
                    "escalating {} — out of restarts",
                    crate::shared::short_uuid(&run.ticket)
                );
            }
        }
    }
    Ok(())
}

/// Close out a run the supervisor is giving up on.
///
/// `worktree_kept` is true because a stuck run's worktree is exactly
/// what someone will want to look at.
async fn end_stuck_run(runs: &RunsClient, run: uuid::Uuid) -> eyre::Result<()> {
    runs.finish_run(agent_proto::run::FinishRun {
        run,
        passed: false,
        exit_code: None,
        worktree_kept: true,
    })
    .await?;
    Ok(())
}

/// Move a ticket off `needs-input` and back into the runner queue.
///
/// Only called once every question on it is answered — a ticket with
/// one answer and one outstanding question is still blocked.
async fn mark_ready_again(
    client: &task::TaskServiceClient,
    ticket: uuid::Uuid,
) -> eyre::Result<()> {
    let mut t = client
        .get(ticket)
        .await
        .map_err(|e| eyre::eyre!("read ticket: {e:?}"))?;
    t.tags
        .0
        .retain(|tag| task::TriageLabel::parse(tag) != Some(task::TriageLabel::NeedsInput));
    if !task::has_triage_label(&t, task::TriageLabel::ReadyForAgent) {
        t.tags
            .0
            .push(task::TriageLabel::ReadyForAgent.as_str().into());
    }
    client
        .update(t)
        .await
        .map_err(|e| eyre::eyre!("unblock ticket: {e:?}"))?;
    Ok(())
}

/// Put a ticket on the grill queue: record the question and flip it
/// to `needs-input` so it leaves the runner queue.
pub(crate) async fn block_on_question(
    url: &str,
    ticket: uuid::Uuid,
    run: Option<uuid::Uuid>,
    questions: Vec<agent_proto::question::Question>,
) -> eyre::Result<()> {
    let qc: QuestionsClient = establish_for_url(url).await?;
    qc.ask_question(agent_proto::question::AskQuestion {
        ticket,
        run,
        questions,
    })
    .await?;

    let client = crate::task_cmd::connect_task_client(url).await?;
    let mut t = client
        .get(ticket)
        .await
        .map_err(|e| eyre::eyre!("read ticket: {e:?}"))?;
    t.tags
        .0
        .retain(|tag| task::TriageLabel::parse(tag) != Some(task::TriageLabel::ReadyForAgent));
    if !task::has_triage_label(&t, task::TriageLabel::NeedsInput) {
        t.tags.0.push(task::TriageLabel::NeedsInput.as_str().into());
    }
    client
        .update(t)
        .await
        .map_err(|e| eyre::eyre!("block ticket: {e:?}"))?;
    Ok(())
}

/// The org's projects, for resolving a ticket's verify command.
async fn connect_project_client_for(url: &str) -> eyre::Result<Vec<project::ProjectInfo>> {
    let pc = crate::project::connect_project_client(url).await?;
    pc.list()
        .await
        .map_err(|e| eyre::eyre!("list projects: {e:?}"))
}

/// Everything one iteration of the runner needs.
struct Job {
    runner: String,
    slug: String,
    url: String,
    repo: std::path::PathBuf,
    worktree_root: std::path::PathBuf,
    base: String,
    agent_cmd: Option<String>,
    dry_run: bool,
}

/// What one iteration did.
///
/// Only "did work happen" matters to the loop — the verdict itself is
/// already recorded on the ticket, so carrying it here would be a
/// second copy that could disagree.
enum Outcome {
    /// A ticket was taken to a verdict, pass or fail.
    Worked,
    /// Nothing to do, or somebody else got there first.
    Idle,
}

/// Take one ticket to a verdict.
///
/// `in_flight` is how many this runner already holds, so a serving
/// runner stops offering itself work past its declared concurrency.
async fn work_one(
    job: &Job,
    want_ticket: Option<&str>,
    in_flight: u32,
) -> eyre::Result<Outcome> {
    let backends: BackendsClient = establish_for_url(&job.url).await?;
    let me = backends
        .list_backends()
        .await?
        .into_iter()
        .find(|b| b.id == job.runner)
        .ok_or_else(|| eyre::eyre!("runner `{}` is not registered", job.runner))?;

    let tickets = agent_ready_tickets(&job.url).await?;
    let chosen = match want_ticket {
        Some(want) => tickets
            .iter()
            .find(|t| t.id.to_string().starts_with(want.trim()))
            .cloned()
            .ok_or_else(|| eyre::eyre!("`{want}` is not an agent-ready ticket"))?,
        None => {
            let refs = ticket_refs(&tickets, &job.slug);
            let takeable = agent_proto::routing::takeable(&me.runner, &refs, in_flight);
            let Some(first) = takeable.first() else {
                return Ok(Outcome::Idle);
            };
            tickets
                .iter()
                .find(|t| t.id == *first)
                .cloned()
                .ok_or_else(|| eyre::eyre!("ticket vanished mid-selection"))?
        }
    };

    let short = crate::shared::short_uuid(&chosen.id);
    println!("ticket {short}  {}", chosen.title);

    // Resolve the verdict command before doing any work: a ticket
    // nobody can check is not worth a worktree.
    let projects = (connect_project_client_for(&job.url).await).unwrap_or_default();
    let verify_cmd = project::verify::resolve(
        chosen
            .workflow
            .as_ref()
            .and_then(|w| w.verify_command.as_deref()),
        chosen.project_id,
        &projects,
    )
    .ok_or_else(|| {
        eyre::eyre!("ticket {short} resolves to no verify command; refusing to work it")
    })?;
    println!("verify {verify_cmd}");

    if job.dry_run {
        println!("(dry run — not claiming, not working)");
        return Ok(Outcome::Idle);
    }

    // Claim before touching the disk, so a losing racer never
    // creates a worktree.
    let client = crate::task_cmd::connect_task_client(&job.url).await?;
    let agent_ref = crate::issue::parse_agent_ref(&format!("agent:{}", job.runner))?;
    let claim = crate::issue::try_claim(&client, &chosen.id, &agent_ref, false).await?;
    if !matches!(
        claim,
        crate::issue::ClaimOutcome::Won | crate::issue::ClaimOutcome::AlreadyMine
    ) {
        println!("lost the claim on {short} — another runner has it");
        return Ok(Outcome::Idle);
    }
    println!("claimed {short} as {}", job.runner);

    let wt = agent_worktree::create(&job.repo, &job.worktree_root, &short, &job.base)?;
    println!("worktree {}", wt.path.display());
    println!("branch   {}", wt.branch);

    // Report the attempt now that the paths exist. The server learns
    // them here, after the fact — it never handed them to us.
    let runs: RunsClient = establish_for_url(&job.url).await?;
    let run = runs
        .start_run(agent_proto::run::StartRun {
            ticket: chosen.id,
            runner: job.runner.clone(),
            parent: None,
            branch: wt.branch.clone(),
            worktree_path: wt.path.display().to_string(),
            session_path: String::new(),
        })
        .await?;
    println!("run      {}", crate::shared::short_uuid(&run.id));

    // Narrate. A monitor should be able to tell a working agent from
    // a stuck one without reading the worktree.
    let stream: RunStreamClient = establish_for_url(&job.url).await?;
    let say = |what: &str| {
        let s = stream.clone();
        let text = what.to_string();
        let id = run.id;
        async move {
            let _ = s
                .publish(id, agent_proto::run_event::RunEvent::Activity(text))
                .await;
        }
    };
    say("running the agent").await;

    if let Some(cmd) = &job.agent_cmd {
        let prompt = format!("{}\n\n{}", chosen.title, chosen.details);
        let status = run_agent(cmd, &wt.path, &chosen.id.to_string(), &chosen.title, &prompt)?;
        println!("agent exited {status}");
    } else {
        println!("(no --agent-cmd; skipping the agent)");
    }

    say("running the verify command").await;
    let verdict = agent_worktree::verify(&wt, &verify_cmd)?;
    // Output is ephemeral: it rides the stream and the bounded tail,
    // and is never written to the vault.
    let _ = stream
        .publish(
            run.id,
            agent_proto::run_event::RunEvent::Output(verdict.tail.clone()),
        )
        .await;
    println!(
        "verdict  {} ({:?})",
        if verdict.passed { "pass" } else { "FAIL" },
        verdict.code
    );

    if !verdict.passed {
        // Leave the worktree for inspection and release the claim so
        // another attempt can happen. `worktree_kept` is what makes
        // the run `needs-cleanup` rather than merely `failed`.
        println!("{}", verdict.tail.trim_end());
        runs.finish_run(agent_proto::run::FinishRun {
            run: run.id,
            passed: false,
            exit_code: verdict.code,
            worktree_kept: true,
        })
        .await?;
        release_claim(&client, &chosen).await?;
        println!("released {short}; worktree kept at {}", wt.path.display());
        return Ok(Outcome::Worked);
    }

    let sha = agent_worktree::commit_all(&wt, &format!("agent: {}", chosen.title))?;
    match &sha {
        Some(sha) => println!("commit   {sha}"),
        None => println!("commit   (nothing to commit)"),
    }

    runs.finish_run(agent_proto::run::FinishRun {
        run: run.id,
        passed: true,
        exit_code: verdict.code,
        worktree_kept: true,
    })
    .await?;

    mark_needs_review(&client, &chosen).await?;
    println!("{short} → needs-review on {}", wt.branch);
    Ok(Outcome::Worked)
}

fn ctx(org: Option<String>, server: Option<String>) -> eyre::Result<(String, String)> {
    let slug = resolve_active_org(org)?;
    let url = resolve_org_vox_url(server, &slug);
    Ok((slug, url))
}

/// Open, unblocked, unclaimed tickets tagged `ready-for-agent`.
///
/// The same frontier `issue ready` computes, narrowed to the agent
/// lane: a human-only ticket is not a routing failure.
async fn agent_ready_tickets(url: &str) -> eyre::Result<Vec<task::TaskInfo>> {
    let client = crate::task_cmd::connect_task_client(url).await?;
    let rows = client
        .list()
        .await
        .map_err(|e| eyre::eyre!("list: {e:?}"))?;

    let by_id: std::collections::HashMap<uuid::Uuid, &task::TaskInfo> =
        rows.iter().map(|t| (t.id, t)).collect();

    let done = |t: &task::TaskInfo| {
        matches!(
            task::Status::from_str(&t.status),
            Some(task::Status::Done | task::Status::Cancelled)
        )
    };

    Ok(rows
        .iter()
        .filter(|t| !done(t))
        .filter(|t| task::has_triage_label(t, task::TriageLabel::ReadyForAgent))
        .filter(|t| {
            // Unclaimed.
            t.workflow
                .as_ref()
                .is_none_or(|w| w.assignees.0.is_empty())
        })
        .filter(|t| {
            // Every blocker closed.
            let blockers = t.workflow.as_ref().map_or(&[][..], |w| &w.blockers.0[..]);
            blockers
                .iter()
                .all(|b| by_id.get(b).is_some_and(|b| done(b)))
        })
        .cloned()
        .collect())
}

fn ticket_refs<'a>(
    tickets: &'a [task::TaskInfo],
    org: &'a str,
) -> Vec<agent_proto::routing::TicketRef<'a>> {
    tickets
        .iter()
        .map(|t| agent_proto::routing::TicketRef {
            id: t.id,
            capabilities: t
                .workflow
                .as_ref()
                .map_or(&[][..], |w| &w.capabilities.0[..]),
            org,
            project: t.project_id,
        })
        .collect()
}
