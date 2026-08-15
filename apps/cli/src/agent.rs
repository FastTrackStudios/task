//! `task agent …` — LLM-agent integration + the goal loop.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::issue::ClaimOutcome;
use crate::issue::parse_agent_ref;
use crate::issue::resolve_issue_id;
use crate::issue::try_claim;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::short_uuid;
use crate::task_cmd::connect_task_client;

// ── Agent task queue (agent-proto / agent-tasks) ─────────────────────

#[derive(Subcommand)]
pub(crate) enum AgentQueueCmd {
    /// Snapshot a queue's tasks + the latest event-log
    /// watermark in one round trip.
    Read {
        /// Queue id (slug). Defaults to the org slug.
        #[arg(long)]
        queue: Option<String>,
        /// Only my tasks (by handle).
        #[arg(long)]
        only_handle: Option<String>,
        #[arg(long)]
        include_archived: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Atomic claim — flips `ready` + unclaimed → `running`.
    Claim {
        task_id: String,
        /// Caller handle (e.g. `codex@host-1`). Defaults to
        /// `${USER}@${HOSTNAME}`.
        #[arg(long)]
        handle: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Set a non-`running` status. `running` rejected — use
    /// `claim`.
    SetStatus {
        task_id: String,
        new_status: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Mark `done` with a result blob (JSON-serialisable
    /// string; the queue stores it verbatim).
    Complete {
        task_id: String,
        /// Result payload (or `-` for stdin).
        result: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Link this agent task to an in-flight thread/session.
    Link {
        task_id: String,
        session_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// List edges where either endpoint belongs to `queue_id`.
    Links {
        #[arg(long)]
        queue: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum AgentCmd {
    /// Agent task queue lifecycle — read / claim / set-status
    /// / complete. Mirrors the `AgentTaskQueue` RPC the server
    /// mounts on `/org/<slug>/vox`.
    #[command(subcommand)]
    Queue(AgentQueueCmd),
    /// One-shot chat against `codex app-server`. Spawns the
    /// daemon rooted at `--workspace`, sends `thread/start` +
    /// `turn/start`, prints streamed assistant text until the
    /// turn completes.
    ///
    /// Example:
    ///   task agent chat -w . -m gpt-5.4-mini "summarize this repo"
    Chat {
        /// Workspace root the agent runs in. Default: cwd.
        #[arg(short, long, default_value = ".")]
        workspace: std::path::PathBuf,
        /// Model id (e.g. `gpt-5.4-mini`, `o3`). Default:
        /// daemon's configured default.
        #[arg(short, long)]
        model: Option<String>,
        /// Reasoning effort hint
        /// (`none|minimal|low|medium|high`).
        #[arg(long)]
        effort: Option<String>,
        /// Sandbox / access mode
        /// (`read-only|current|full-access`). Default
        /// `current` (matches `CodexMonitor`).
        #[arg(long)]
        access_mode: Option<String>,
        /// Override `codex` binary path. Falls back to
        /// `$PATH` lookup.
        #[arg(long)]
        codex_bin: Option<String>,
        /// `$CODEX_HOME` override.
        #[arg(long)]
        codex_home: Option<std::path::PathBuf>,
        /// Max time to wait for the turn to complete
        /// (seconds). Default 120.
        #[arg(long, default_value_t = 120)]
        timeout_secs: u64,
        /// The user message. Quote it.
        message: String,
    },
    /// Put an agent in an autonomous loop toward a completion
    /// condition, the way Claude Code's `/goal` does — but
    /// agent-agnostic and persisted. Each iteration runs the worker
    /// command (one "turn"), then a separate evaluator judges whether
    /// the condition holds against what the worker surfaced. "Not
    /// met" loops with the evaluator's reason fed back as guidance;
    /// "met" stops. Bounded by `--max-iters`.
    ///
    /// The run is a `WorkSession` (workflows-orchestrator), so every
    /// turn is logged and the run is resumable. Distinct from the
    /// life-Goal/OKR system (`task goal`).
    ///
    /// Example:
    ///   task agent goal run "all tests in features/git pass" \
    ///     --cmd 'claude -p' --eval-cmd 'claude -p' --as-agent claude
    ///
    /// The standing session can be inspected (`goal status`), parked
    /// (`goal pause`), continued with a fresh turn budget
    /// (`goal resume`), or dropped (`goal clear`).
    Goal {
        #[command(subcommand)]
        cmd: GoalLoopCmd,
    },
}

/// `task agent goal *` — the autonomous goal loop and its lifecycle
/// verbs. The loop persists a `GoalSession` (condition, turn budget,
/// progress) over a `WorkSession`, so it can be parked and resumed
/// without losing the directive.
#[derive(Subcommand)]
pub(crate) enum GoalLoopCmd {
    /// Start (or restart) the loop toward a completion condition.
    Run {
        /// The completion condition. Write it so the worker's own
        /// output can demonstrate it (e.g. "`cargo test -p x` exits
        /// 0"). Up to a few KB.
        condition: String,
        /// Worker command, run via `sh -c` once per turn. The prompt
        /// (condition + last evaluator reason) is piped to its stdin;
        /// `TASK_GOAL` / `TASK_GOAL_ITER` are set in its env. Falls
        /// back to `TASK_AGENT_CMD`.
        #[arg(long)]
        cmd: Option<String>,
        /// Evaluator command, run via `sh -c` after each turn. Reads
        /// the condition + the worker's captured output on stdin;
        /// exit `0` = met (stop), nonzero = not met (its stdout is
        /// the reason, fed into the next turn). Falls back to
        /// `TASK_GOAL_EVAL_CMD`. If unset and `--task` is given, the
        /// built-in evaluator checks whether the task is `done`.
        #[arg(long)]
        eval_cmd: Option<String>,
        /// Tie the run to an existing task (UUID or 8-char prefix):
        /// claim it, make it the session subject, and (default
        /// evaluator) treat `status == done` as the condition.
        #[arg(long)]
        task: Option<String>,
        /// Attribute the loop to this agent (`name[@version]`).
        #[arg(long = "as-agent", default_value = "claude")]
        as_agent: String,
        /// Turn ceiling before parking the session as resumable.
        #[arg(long, default_value_t = 25)]
        max_iters: u32,
        /// Render + print the first prompt and exit — no worker,
        /// no evaluator, no state change.
        #[arg(long)]
        dry_run: bool,
        /// Skip auto-triage. By default a `--task` with no subtasks
        /// is first decomposed into agent-sized subtasks (one
        /// decompose turn by the worker) before the loop executes;
        /// this works it as a single task instead.
        #[arg(long)]
        no_triage: bool,
        /// Hand the whole goal to the agent's own native goal loop in
        /// one shot, instead of our turn-by-turn loop. For agents with
        /// their own loop (Hermes/Codex/Claude). Also settable via
        /// `agent.json` `goal.mode = "delegate"`.
        #[arg(long)]
        delegate: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Show the active goal session: condition, turns used/budget,
    /// the last evaluator reason, and the session status.
    Status {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Park the active goal session (stop auto-continuation) without
    /// dropping it. Resume later with `goal resume`.
    Pause {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Resume the parked goal session: reset the turn counter to 0
    /// and continue the loop toward the stored condition.
    Resume {
        /// Worker command override (else env / per-org config).
        #[arg(long)]
        cmd: Option<String>,
        /// Evaluator command override (else env / per-org config).
        #[arg(long)]
        eval_cmd: Option<String>,
        /// Attribute the resumed loop to this agent (`name[@version]`).
        #[arg(long = "as-agent", default_value = "claude")]
        as_agent: String,
        /// Turn ceiling for the resumed run. Defaults to the stored
        /// budget from the original `goal run`.
        #[arg(long)]
        max_iters: Option<u32>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Drop the active goal session (cancel it + delete its row).
    Clear {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Steer a running loop: replace the active session's completion
    /// condition. A loop in another process re-reads it at the top of
    /// its next turn and re-steers — no restart needed.
    Update {
        /// The new completion condition.
        #[arg(long)]
        condition: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Manage the active session's subgoals — extra acceptance
    /// criteria the worker sees and the judge must also satisfy.
    ///
    ///   goal subgoal `"<text>"`   append a criterion
    ///   goal subgoal            list them (alias: `goal subgoal list`)
    ///   goal subgoal remove `<N>` drop the Nth (1-based)
    ///   goal subgoal clear      drop all
    ///
    /// A running loop in another process folds the current set into
    /// its next worker prompt and evaluator gate — no restart needed.
    Subgoal {
        /// The subverb + text. With no args: list. A bare string:
        /// append it as a criterion. `remove <N>`: drop the Nth.
        /// `clear`: drop all. `list`: list.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

pub(crate) async fn run_agent(cmd: AgentCmd) -> eyre::Result<()> {
    use std::io::Write;

    use agent_codex::{ChatOpts, CodexBackend};
    use agent_proto::event::AgentEvent;
    use futures::StreamExt;

    match cmd {
        AgentCmd::Queue(qc) => Box::pin(run_agent_queue(qc)).await,
        AgentCmd::Chat {
            workspace,
            model,
            effort,
            access_mode,
            codex_bin,
            codex_home,
            timeout_secs,
            message,
        } => {
            let workspace = workspace
                .canonicalize()
                .map_err(|e| eyre::eyre!("workspace {}: {e}", workspace.display()))?;
            let backend = CodexBackend::new();
            let opts = ChatOpts {
                codex_bin,
                codex_args: None,
                codex_home,
                model: model.clone(),
                effort,
                access_mode,
            };
            eprintln!(
                "› codex@{} workspace={}",
                model.as_deref().unwrap_or("default"),
                workspace.display()
            );
            let handle = backend
                .chat(workspace, message, opts)
                .await
                .map_err(|e| eyre::eyre!("chat: {e}"))?;
            eprintln!(
                "  session={} thread={}",
                handle.session_id, handle.thread_id
            );
            let mut events = handle.events;
            let mut stdout = std::io::stdout().lock();
            let deadline =
                tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);
            loop {
                let next = tokio::time::timeout_at(deadline, events.next()).await;
                match next {
                    Err(_) => {
                        eprintln!("\n(turn timed out after {timeout_secs}s)");
                        break;
                    }
                    Ok(None) => break,
                    Ok(Some(AgentEvent::MessageDelta { content_delta, .. })) => {
                        write!(stdout, "{content_delta}")?;
                        stdout.flush()?;
                    }
                    Ok(Some(AgentEvent::TurnFinished { .. })) => {
                        writeln!(stdout)?;
                        break;
                    }
                    Ok(Some(AgentEvent::TurnErrored { kind, message, .. })) => {
                        writeln!(stdout)?;
                        eprintln!("(turn error: {kind}: {message})");
                        break;
                    }
                    Ok(Some(_)) => {}
                }
            }
            Ok(())
        }
        AgentCmd::Goal { cmd } => match cmd {
            GoalLoopCmd::Run {
                condition,
                cmd,
                eval_cmd,
                task,
                as_agent,
                max_iters,
                dry_run,
                no_triage,
                delegate,
                org,
                server,
            } => {
                Box::pin(run_agent_goal(
                    condition, cmd, eval_cmd, task, as_agent, max_iters, dry_run, no_triage,
                    delegate, org, server,
                ))
                .await
            }
            GoalLoopCmd::Status { org, server } => Box::pin(run_goal_status(org, server)).await,
            GoalLoopCmd::Pause { org, server } => Box::pin(run_goal_pause(org, server)).await,
            GoalLoopCmd::Resume {
                cmd,
                eval_cmd,
                as_agent,
                max_iters,
                org,
                server,
            } => {
                Box::pin(run_goal_resume(
                    cmd, eval_cmd, as_agent, max_iters, org, server,
                ))
                .await
            }
            GoalLoopCmd::Clear { org, server } => Box::pin(run_goal_clear(org, server)).await,
            GoalLoopCmd::Update {
                condition,
                org,
                server,
            } => Box::pin(run_goal_update(condition, org, server)).await,
            GoalLoopCmd::Subgoal { args, org, server } => {
                Box::pin(run_goal_subgoal(args, org, server)).await
            }
        },
    }
}

/// `task agent goal` — the autonomous goal loop (worker turn +
/// evaluator gate, looped until the condition is met). See [`AgentCmd::Goal`].
#[allow(clippy::too_many_arguments)]
async fn run_agent_goal(
    condition: String,
    cmd: Option<String>,
    eval_cmd: Option<String>,
    task_ref: Option<String>,
    as_agent: String,
    max_iters: u32,
    dry_run: bool,
    no_triage: bool,
    delegate: bool,
    org: Option<String>,
    server: Option<String>,
) -> eyre::Result<()> {
    use workflows_orchestrator::{CodingWorkflow, WorkflowStore};

    let slug = resolve_active_org(org)?;
    let agent = parse_agent_ref(&format!("agent:{as_agent}"))?;
    let url = resolve_org_vox_url(server.clone(), &slug);

    // Resolve the optional linked task (claim it; it becomes the
    // session subject + the default evaluator's completion check).
    let (task_id, task_info, parent_info) = match &task_ref {
        Some(r) => {
            let client = connect_task_client(&url).await?;
            let t = resolve_issue_id(&client, r).await?;
            if let ClaimOutcome::Lost(holder) = try_claim(&client, &t.id, &agent, false).await? {
                return Err(eyre::eyre!("{} is held by {holder}", short_uuid(&t.id)));
            }
            let parent = match t.workflow.as_ref().and_then(|w| w.parent) {
                Some(pid) => client.get(pid).await.ok(),
                None => None,
            };
            (Some(t.id), Some(t), parent)
        }
        None => (None, None, None),
    };

    // Resolve the worker + evaluator commands. Precedence: explicit
    // flag → env var → per-org `agent.json` [goal] defaults. The
    // config layer (mirroring Hermes's `config.yaml: goals/auxiliary`)
    // lets an org set its agent once instead of passing --cmd every
    // run — the executor seam. The command itself is agent-agnostic:
    // `claude -p`, `codex exec`, `hermes -p`, etc. all satisfy the
    // stdin-prompt contract.
    let cfg = load_goal_config(&slug);
    let worker = cmd
        .or_else(|| std::env::var("TASK_AGENT_CMD").ok())
        .or(cfg.worker_cmd);
    let evaluator = eval_cmd
        .or_else(|| std::env::var("TASK_GOAL_EVAL_CMD").ok())
        .or(cfg.eval_cmd);

    // Auto-triage (Hermes kanban-orchestrator style: "decompose,
    // don't execute"). A linked task with no subtasks gets one
    // decompose turn — the worker breaks the PRD into agent-sized
    // titles, or judges it a one-shot and emits none (we don't
    // fragment small tasks). Skipped on --no-triage / --dry-run.
    let mut subtasks_md = String::new();
    if let Some(t) = &task_info {
        let client = connect_task_client(&url).await?;
        let mut children = subtasks_of(&client, t.id).await?;
        if children.is_empty() && !no_triage && !dry_run {
            let w = worker.as_deref().ok_or_else(|| {
                eyre::eyre!(
                    "auto-triage needs a worker: pass --cmd / set TASK_AGENT_CMD / goal.worker_cmd, or --no-triage"
                )
            })?;
            let dprompt = decompose_prompt(&render_task_prompt(t, parent_info.as_ref()));
            println!("⊙ auto-triage: decomposing {} …", short_uuid(&t.id));
            let out = run_subprocess(w, &dprompt, 0, &condition)?;
            let titles = parse_subtask_titles(&out.stdout);
            if titles.is_empty() {
                println!("  one-shot task — no subtasks created");
            } else {
                let mut made = 0usize;
                for title in &titles {
                    if create_subtask(&client, t, title).await? {
                        made += 1;
                        println!("    + {title}");
                    } else {
                        println!("    ~ {title} (already exists — skipped)");
                    }
                }
                if made > 0 {
                    // Flip the parent into the working state.
                    let mut p = t.clone();
                    p.status = "in-progress".into();
                    p.completed_date = None;
                    let _ = client.update(p).await;
                }
                println!("  created {made}/{} subtask(s)", titles.len());
                children = subtasks_of(&client, t.id).await?;
            }
        }
        subtasks_md = render_subtask_checklist(&children);
    }

    // The first prompt: the condition is the directive (matching
    // Claude Code's `/goal`, where the condition itself starts the
    // turn). When tied to a task, lead with its rendered PRD + the
    // subtask checklist. Subsequent turns append the evaluator reason.
    let preamble = task_info
        .as_ref()
        .map(|t| render_task_prompt(t, parent_info.as_ref()))
        .unwrap_or_default();
    // Static preamble (task PRD + subtask checklist, or empty) — the
    // part of the prompt that doesn't change turn to turn. The
    // condition is read live from the store each turn so `goal update`
    // can steer the loop, so it's not baked in here.
    let static_preamble = if preamble.is_empty() {
        String::new()
    } else {
        format!("{preamble}{subtasks_md}")
    };
    if dry_run {
        // No session yet at dry-run time, so no subgoals to fold in.
        println!("{}", goal_prompt(&static_preamble, &condition, &[], ""));
        return Ok(());
    }

    let worker = worker.ok_or_else(|| {
        eyre::eyre!(
            "no worker command: pass --cmd, set TASK_AGENT_CMD, or set goal.worker_cmd in ~/.task/orgs/{slug}/agent.json"
        )
    })?;
    // Native-delegate uses the worker's exit code as the verdict, so
    // it needs no separate evaluator.
    let delegate = delegate || cfg.mode.as_deref() == Some("delegate");
    if !delegate && evaluator.is_none() && task_id.is_none() {
        return Err(eyre::eyre!(
            "no evaluator: pass --eval-cmd, set TASK_GOAL_EVAL_CMD / goal.eval_cmd, or pass --task for the built-in done-check"
        ));
    }

    // Open the work session (subject = the task if linked, else a
    // custom "goal" subject keyed by a fresh id).
    let store_dir = org_workflows_dir(&slug)?;
    let wf = CodingWorkflow::new(WorkflowStore::open(store_dir));
    let subject_task = task_id.unwrap_or_else(uuid::Uuid::new_v4);
    let session = wf.start(subject_task, agent.clone())?;
    println!(
        "goal session {} — “{condition}” (max {max_iters} turns)",
        short_uuid(&session.id)
    );

    // Persist the goal-loop state (condition, budget, progress) so it
    // can be inspected (`goal status`), parked (`goal pause`), and
    // resumed (`goal resume`) independently of this process.
    wf.store().put_goal(&workflows_proto::GoalSession::new(
        session.id, &condition, max_iters,
    ))?;

    // Pick the executor: --delegate (or agent.json goal.mode) hands
    // the whole goal to the agent's native loop; default is our
    // turn-by-turn subprocess loop. (`delegate` resolved above.)
    let executor = if delegate {
        GoalExecutor::NativeDelegate { worker: &worker }
    } else {
        GoalExecutor::Subprocess {
            worker: &worker,
            evaluator: evaluator.as_deref(),
        }
    };
    let run = executor.drive(&wf, session.id, &agent, &static_preamble, max_iters)?;

    finalize_goal_run(&run, max_iters, task_id, &url, &wf, session.id).await
}

/// Drive the worker/evaluator loop for one goal session, persisting
/// per-turn progress (`turns_used`, `last_reason`) onto the
/// [`GoalSession`](workflows_proto::GoalSession) row so a concurrent
/// `goal status` reads live state. Shared by `goal run` and
/// `goal resume`.
#[allow(clippy::too_many_arguments)]
/// Assemble a turn's worker prompt from the static preamble (the task
/// PRD with its subtask checklist, or empty), the live completion
/// condition, and the evaluator's last reason. Kept in one place so
/// `goal run`, `goal resume`, the per-turn loop, and `--dry-run` agree.
fn goal_prompt(
    static_preamble: &str,
    condition: &str,
    subgoals: &[String],
    reason: &str,
) -> String {
    let mut p = if static_preamble.is_empty() {
        format!(
            "Goal: {condition}\n\nWork toward this goal. When you believe it is fully met, stop."
        )
    } else {
        format!(
            "{static_preamble}\n---\n\nGoal (stop when met): {condition}\n\nWork toward this goal using the task spec above; complete the subtasks in order."
        )
    };
    // Subgoals are extra acceptance criteria added mid-run — the
    // worker must satisfy every one in addition to the condition.
    if !subgoals.is_empty() {
        p.push_str("\n\nAdditional acceptance criteria (ALL must also be met):");
        for (i, sg) in subgoals.iter().enumerate() {
            p.push_str(&format!("\n  {}. {sg}", i + 1));
        }
    }
    if !reason.is_empty() {
        p.push_str(&format!(
            "\n\nThe goal is NOT yet met. Evaluator feedback:\n{reason}\n\nContinue."
        ));
    }
    p
}

pub(crate) fn drive_goal_loop(
    wf: &workflows_orchestrator::CodingWorkflow,
    session_id: uuid::Uuid,
    agent: &workflows_proto::AgentRef,
    static_preamble: &str,
    worker: &str,
    evaluator: Option<&str>,
    budget: u32,
) -> eyre::Result<workflows_orchestrator::SessionRun> {
    use workflows_orchestrator::IterationOutcome;

    // The evaluator's latest reason, carried into the next worker turn.
    let last_reason = std::cell::RefCell::new(String::new());
    // The condition seen on the previous turn — to detect live edits.
    let last_condition = std::cell::RefCell::new(String::new());

    let run = wf.run_session(session_id, agent.clone(), budget, |iter| {
        // Re-read the condition + subgoals from the store every turn
        // so an out-of-band `goal update` / `goal subgoal` steers the
        // loop in real time (#174). The store is the steering channel.
        let (condition, subgoals) = wf
            .store()
            .goal(session_id)
            .map(|g| (g.condition, g.subgoals.0))
            .unwrap_or_default();
        {
            let mut lc = last_condition.borrow_mut();
            if !lc.is_empty() && *lc != condition {
                println!("  ◎ goal updated — re-steering toward: {condition}");
            }
            *lc = condition.clone();
        }

        // 1. Worker turn — prompt assembled from the static preamble
        //    + the (possibly just-updated) condition + last reason.
        let reason = last_reason.borrow().clone();
        let prompt = goal_prompt(static_preamble, &condition, &subgoals, &reason);
        println!("▶ turn {iter} — worker running…");
        let work = run_subprocess(worker, &prompt, iter, &condition)
            .map_err(|e| workflows_proto::WorkflowError::Backend(format!("worker: {e}")))?;

        // 2. Evaluator gate — judge the worker's output against the
        //    current condition. Built-in done-check if no eval command.
        let verdict = match evaluator {
            Some(ev) => {
                println!("⧖ turn {iter} — judging…");
                // Fold any subgoals into the condition block so the
                // judge must verify them too — they're extra acceptance
                // criteria, equal in weight to the condition.
                let criteria = if subgoals.is_empty() {
                    condition.clone()
                } else {
                    let mut c = condition.clone();
                    c.push_str("\n\nALSO required (every one must be met):");
                    for (i, sg) in subgoals.iter().enumerate() {
                        c.push_str(&format!("\n  {}. {sg}", i + 1));
                    }
                    c
                };
                // Lead with the strict judge directive so a raw LLM
                // evaluator (`claude -p`, `hermes -p`) emits the
                // verdict instead of prose — otherwise parsing fails
                // and we'd fall back to "exit 0 = always met". The
                // conservative framing mirrors Hermes/Claude /goal.
                let eval_in = format!(
                    "You are a STRICT completion judge. Reply with ONLY a JSON object: \
                     {{\"done\": true|false, \"reason\": \"<one sentence>\"}}. Mark done:true \
                     ONLY if the WORKER OUTPUT shows the CONDITION is fully and verifiably met; \
                     when in doubt, done:false with the gap as the reason.\n\n\
                     CONDITION:\n{criteria}\n\nWORKER OUTPUT:\n{}",
                    work.stdout
                );
                let r = run_subprocess(ev, &eval_in, iter, &condition)
                    .map_err(|e| workflows_proto::WorkflowError::Backend(format!("eval: {e}")))?;
                // Prefer a structured verdict — the judge convention
                // Hermes / Claude / Codex `/goal` all share:
                // `{"done": bool, "reason": "..."}` on stdout. This is
                // the cross-agent interop contract. Fall back to the
                // exit code when the output isn't that JSON.
                parse_eval_verdict(&r.stdout, r.code)
            }
            None => {
                // No evaluator + a linked task: the condition is
                // "task is done". Checked synchronously below by the
                // outer loop via a flag; here we approximate using
                // the worker exit code as a hint and defer the
                // authoritative check to the post-run reconcile.
                EvalVerdict {
                    met: work.code == 0,
                    reason: "worker did not exit 0".to_owned(),
                }
            }
        };

        // Persist progress so `goal status` reflects this turn. Atomic
        // read-modify-write under the session lock so a concurrent
        // `goal update`/`subgoal` (live steering) can't lose-update
        // against this write.
        let activity = work.last_activity.clone();
        let _ = wf.store().mutate_goal(session_id, |g| {
            g.turns_used = iter + 1;
            g.last_reason = verdict.reason.clone();
            if let Some(a) = &activity {
                g.current_activity = a.clone();
            }
            g.updated_at = chrono::Utc::now();
        });

        if verdict.met {
            Ok(IterationOutcome::Done)
        } else {
            *last_reason.borrow_mut() = verdict.reason.clone();
            if verdict.reason.is_empty() {
                println!("  ◎ turn {iter}: not met yet");
            } else {
                println!("  ◎ turn {iter}: not met — {}", verdict.reason);
            }
            Ok(IterationOutcome::Continue)
        }
    })?;
    Ok(run)
}

/// The two ways to drive a goal session. Selected by `--delegate` /
/// `agent.json goal.mode`. This is the seam #161 deferred —
/// concretised now that a second real executor exists.
pub(crate) enum GoalExecutor<'a> {
    /// Default: our turn-by-turn loop — worker turn → judge → repeat,
    /// with live steering + the budget. Agent-agnostic.
    Subprocess {
        worker: &'a str,
        evaluator: Option<&'a str>,
    },
    /// Hand the whole goal to the agent's *own* loop in one shot (for
    /// agents that have a native goal loop — Hermes/Codex/Claude). One
    /// worker invocation with the full directive; its exit code is the
    /// verdict. No re-prompt, no separate judge.
    NativeDelegate { worker: &'a str },
}

impl GoalExecutor<'_> {
    pub(crate) fn drive(
        &self,
        wf: &workflows_orchestrator::CodingWorkflow,
        session_id: uuid::Uuid,
        agent: &workflows_proto::AgentRef,
        static_preamble: &str,
        budget: u32,
    ) -> eyre::Result<workflows_orchestrator::SessionRun> {
        match self {
            GoalExecutor::Subprocess { worker, evaluator } => drive_goal_loop(
                wf,
                session_id,
                agent,
                static_preamble,
                worker,
                *evaluator,
                budget,
            ),
            GoalExecutor::NativeDelegate { worker } => {
                drive_goal_delegate(wf, session_id, agent, static_preamble, worker)
            }
        }
    }
}

/// Native-delegate executor (#169): one worker invocation handed the
/// full goal (the agent runs its *own* loop internally), mapped onto a
/// single-turn `WorkSession`. Exit 0 = met → finish; nonzero → park.
pub(crate) fn drive_goal_delegate(
    wf: &workflows_orchestrator::CodingWorkflow,
    session_id: uuid::Uuid,
    agent: &workflows_proto::AgentRef,
    static_preamble: &str,
    worker: &str,
) -> eyre::Result<workflows_orchestrator::SessionRun> {
    use workflows_orchestrator::IterationOutcome;
    // Budget of 1: a single delegated invocation. The agent's native
    // loop does the iterating; we just record the one outcome.
    let run = wf.run_session(session_id, agent.clone(), 1, |_iter| {
        let (condition, subgoals) = wf
            .store()
            .goal(session_id)
            .map(|g| (g.condition, g.subgoals.0))
            .unwrap_or_default();
        let prompt = goal_prompt(static_preamble, &condition, &subgoals, "");
        println!("▶ delegating the whole goal to the agent's native loop…");
        let work = run_subprocess(worker, &prompt, 0, &condition)
            .map_err(|e| workflows_proto::WorkflowError::Backend(format!("worker: {e}")))?;
        let _ = wf.store().mutate_goal(session_id, |g| {
            g.turns_used = 1;
            if let Some(a) = &work.last_activity {
                g.current_activity = a.clone();
            }
            g.updated_at = chrono::Utc::now();
        });
        if work.code == 0 {
            Ok(IterationOutcome::Done)
        } else {
            Ok(IterationOutcome::Blocked {
                reason: format!("delegated agent exited {}", work.code),
                summary: "native-delegate run did not complete the goal".to_owned(),
            })
        }
    })?;
    Ok(run)
}

/// Report a finished goal run + reconcile side effects: drop the
/// goal row on completion and close the linked task. Shared by
/// `goal run` and `goal resume`.
async fn finalize_goal_run(
    run: &workflows_orchestrator::SessionRun,
    budget: u32,
    task_id: Option<uuid::Uuid>,
    url: &str,
    wf: &workflows_orchestrator::CodingWorkflow,
    session_id: uuid::Uuid,
) -> eyre::Result<()> {
    use workflows_orchestrator::RunEnd;
    match &run.end {
        RunEnd::Completed => {
            println!("✓ goal met after {} turn(s)", run.iterations);
            // The standing goal is satisfied — drop its row so it no
            // longer shows up in `goal status`.
            let _ = wf.store().remove_goal(session_id);
            if let Some(id) = task_id {
                let client = connect_task_client(url).await?;
                if let Ok(mut t) = client.get(id).await {
                    if task::Status::from_str(&t.status)
                        .is_none_or(|s| !matches!(s, task::Status::Done))
                    {
                        t.status = "done".into();
                        t.completed_date = Some(chrono::Utc::now().date_naive());
                        let _ = client.update(t).await;
                        println!("  closed linked task {}", short_uuid(&id));
                    }
                }
            }
        }
        RunEnd::Parked { reason } => {
            println!("⏸ goal parked after {} turn(s): {reason}", run.iterations);
        }
        RunEnd::MaxedOut => {
            println!(
                "⏹ hit the {budget}-turn ceiling without meeting the goal — session parked, resume to continue"
            );
        }
    }
    Ok(())
}

/// The org's standing goal session: the most recently touched
/// `Active`/`Parked` [`WorkSession`](workflows_proto::WorkSession)
/// that carries a [`GoalSession`](workflows_proto::GoalSession) row.
/// `None` when no goal loop is in flight. Backs `status` / `pause` /
/// `resume` / `clear`.
fn active_goal_session(
    wf: &workflows_orchestrator::CodingWorkflow,
) -> eyre::Result<Option<(workflows_proto::WorkSession, workflows_proto::GoalSession)>> {
    use workflows_proto::SessionStatus;
    let mut hits: Vec<(workflows_proto::WorkSession, workflows_proto::GoalSession)> = Vec::new();
    for g in wf.store().goals()? {
        if let Ok(s) = wf.store().session(g.session_id) {
            if matches!(s.status, SessionStatus::Active | SessionStatus::Parked) {
                hits.push((s, g));
            }
        }
    }
    hits.sort_by_key(|(s, _)| s.updated_at);
    Ok(hits.pop())
}

/// `task agent goal status` — report the active goal session.
async fn run_goal_status(org: Option<String>, _server: Option<String>) -> eyre::Result<()> {
    use workflows_orchestrator::{CodingWorkflow, WorkflowStore};
    use workflows_proto::{SessionStatus, SubjectRef};

    let slug = resolve_active_org(org)?;
    let wf = CodingWorkflow::new(WorkflowStore::open(org_workflows_dir(&slug)?));
    match active_goal_session(&wf)? {
        None => println!("no active goal session"),
        Some((s, g)) => {
            let status = match s.status {
                SessionStatus::Active => "active",
                SessionStatus::Parked => "parked",
                SessionStatus::Blocked => "blocked",
                SessionStatus::Finished => "finished",
                SessionStatus::Cancelled => "cancelled",
            };
            println!("goal session {}  [{status}]", short_uuid(&s.id));
            println!("  condition: {}", g.condition);
            println!("  turns:     {}/{}", g.turns_used, g.budget);
            if let SubjectRef::Task { id } = s.subject {
                println!("  task:      {}", short_uuid(&id));
            }
            if !g.subgoals.0.is_empty() {
                println!("  subgoals:");
                for (i, sg) in g.subgoals.0.iter().enumerate() {
                    println!("    {}. {sg}", i + 1);
                }
            }
            if g.last_reason.is_empty() {
                println!("  last eval: (none yet)");
            } else {
                println!("  last eval: {}", g.last_reason);
            }
            if !g.current_activity.is_empty() {
                println!("  doing:     {}", g.current_activity);
            }
            // Heartbeat + a peek at recent activity — so a running
            // loop's liveness ("how long since the last turn") and
            // what it's been doing are visible on demand.
            let recent = wf.store().activities_for(s.id).unwrap_or_default();
            if let Some(last) = recent.first() {
                println!("  heartbeat: last activity {} ago", human_ago(last.at));
            }
            if !recent.is_empty() {
                println!("  recent:");
                for a in recent.iter().take(5) {
                    let kind = serde_json::to_value(&a.kind)
                        .ok()
                        .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_owned))
                        .unwrap_or_else(|| "activity".into());
                    println!("    {} · {kind}", human_ago(a.at));
                }
            }
        }
    }
    Ok(())
}

/// Render a `DateTime` as a coarse "Ns / Nm / Nh ago" string for the
/// goal-status heartbeat.
fn human_ago(at: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - at).num_seconds().max(0);
    if secs < 90 {
        format!("{secs}s")
    } else if secs < 5400 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

/// `task agent goal pause` — park the active goal session without
/// dropping it. Idempotent if already parked.
async fn run_goal_pause(org: Option<String>, _server: Option<String>) -> eyre::Result<()> {
    use workflows_orchestrator::{CodingWorkflow, WorkflowStore};
    use workflows_proto::{HandoffReason, SessionStatus};

    let slug = resolve_active_org(org)?;
    let wf = CodingWorkflow::new(WorkflowStore::open(org_workflows_dir(&slug)?));
    match active_goal_session(&wf)? {
        None => println!("no active goal session to pause"),
        Some((s, _g)) => {
            if s.status == SessionStatus::Parked {
                println!("goal session {} is already parked", short_uuid(&s.id));
                return Ok(());
            }
            wf.park(
                s.id,
                s.current_actor.clone(),
                HandoffReason::EndOfChunk,
                "goal paused via `goal pause`",
                "",
                "- resume with `task agent goal resume`",
            )?;
            println!("⏸ paused goal session {}", short_uuid(&s.id));
        }
    }
    Ok(())
}

/// `task agent goal clear` — drop the active goal session: cancel it
/// and delete its goal row.
async fn run_goal_clear(org: Option<String>, _server: Option<String>) -> eyre::Result<()> {
    use workflows_orchestrator::{CodingWorkflow, WorkflowStore};

    let slug = resolve_active_org(org)?;
    let wf = CodingWorkflow::new(WorkflowStore::open(org_workflows_dir(&slug)?));
    match active_goal_session(&wf)? {
        None => println!("no active goal session to clear"),
        Some((s, _g)) => {
            wf.cancel(s.id, s.current_actor.clone())?;
            wf.store().remove_goal(s.id)?;
            println!("⏹ cleared goal session {}", short_uuid(&s.id));
        }
    }
    Ok(())
}

/// `task agent goal update --condition` — live-steer the active loop
/// by replacing its completion condition. The running loop re-reads
/// the `GoalSession` row at the top of each turn (#174), so the next
/// turn re-steers toward the new condition without a restart.
async fn run_goal_update(
    condition: String,
    org: Option<String>,
    _server: Option<String>,
) -> eyre::Result<()> {
    use workflows_orchestrator::{CodingWorkflow, WorkflowStore};

    let slug = resolve_active_org(org)?;
    let wf = CodingWorkflow::new(WorkflowStore::open(org_workflows_dir(&slug)?));
    match active_goal_session(&wf)? {
        None => println!("no active goal session to update"),
        Some((s, _g)) => {
            // Atomic RMW so we don't clobber the loop's concurrent
            // per-turn progress write.
            wf.store().mutate_goal(s.id, |g| {
                g.condition = condition.clone();
                g.updated_at = chrono::Utc::now();
            })?;
            println!("◎ updated goal session {} condition:", short_uuid(&s.id));
            println!("  {condition}");
            println!("  (a running loop re-steers on its next turn)");
        }
    }
    Ok(())
}

/// `task agent goal subgoal *` — manage the active session's subgoals
/// (extra acceptance criteria appended mid-run). Dispatches on the
/// first token: `remove <N>` / `clear` / `list`, else the joined text
/// is appended as a new criterion (bare = list). Mutations persist on
/// the `GoalSession` row, so a running loop folds them into its next
/// worker prompt + evaluator gate at the top of its next turn.
async fn run_goal_subgoal(
    args: Vec<String>,
    org: Option<String>,
    _server: Option<String>,
) -> eyre::Result<()> {
    use workflows_orchestrator::{CodingWorkflow, WorkflowStore};

    let slug = resolve_active_org(org)?;
    let wf = CodingWorkflow::new(WorkflowStore::open(org_workflows_dir(&slug)?));
    // `g` is a read-only snapshot for validation/display; every write
    // goes through the store's atomic `mutate_goal` (locked RMW) so a
    // concurrent loop turn or another steering command can't lose it.
    let Some((s, g)) = active_goal_session(&wf)? else {
        println!("no active goal session");
        return Ok(());
    };

    // Dispatch on the first token. `remove`/`clear`/`list` are
    // subverbs; anything else is the criterion text to append.
    match args.first().map(String::as_str) {
        None => {
            print_subgoals(&s.id, &g.subgoals.0);
        }
        Some("list") if args.len() == 1 => {
            print_subgoals(&s.id, &g.subgoals.0);
        }
        Some("clear") if args.len() == 1 => {
            wf.store().mutate_goal(s.id, |g| {
                g.subgoals.0.clear();
                g.updated_at = chrono::Utc::now();
            })?;
            println!("⌫ cleared subgoal(s) on {}", short_uuid(&s.id));
        }
        Some("remove") => {
            // `remove <N>` — N is 1-based, matching the `list` display.
            let n: usize = match args.get(1).and_then(|a| a.parse().ok()) {
                Some(n) if n >= 1 && n <= g.subgoals.0.len() => n,
                _ => {
                    println!(
                        "remove takes a number 1..={} (the index shown by `goal subgoal`)",
                        g.subgoals.0.len()
                    );
                    return Ok(());
                }
            };
            let updated = wf.store().mutate_goal(s.id, |g| {
                if n - 1 < g.subgoals.0.len() {
                    g.subgoals.0.remove(n - 1);
                    g.updated_at = chrono::Utc::now();
                }
            })?;
            println!("⌫ removed subgoal {n} on {}", short_uuid(&s.id));
            print_subgoals(&s.id, &updated.subgoals.0);
        }
        Some(_) => {
            // No subverb matched: treat the whole arg list as the
            // criterion text (so unquoted multi-word input still works).
            let text = args.join(" ");
            let text = text.trim();
            if text.is_empty() {
                print_subgoals(&s.id, &g.subgoals.0);
                return Ok(());
            }
            let updated = wf.store().mutate_goal(s.id, |g| {
                g.subgoals.0.push(text.to_owned());
                g.updated_at = chrono::Utc::now();
            })?;
            println!(
                "＋ added subgoal {} on {}: {text}",
                updated.subgoals.0.len(),
                short_uuid(&s.id)
            );
        }
    }
    Ok(())
}

/// Print the numbered subgoal list (1-based, matching `remove <N>`).
fn print_subgoals(session_id: &uuid::Uuid, subgoals: &[String]) {
    if subgoals.is_empty() {
        println!("no subgoals on goal session {}", short_uuid(session_id));
        return;
    }
    println!("subgoals on goal session {}:", short_uuid(session_id));
    for (i, sg) in subgoals.iter().enumerate() {
        println!("  {}. {sg}", i + 1);
    }
}

/// `task agent goal resume` — reset the parked session's turn counter
/// to 0 and continue the loop toward its stored condition.
async fn run_goal_resume(
    cmd: Option<String>,
    eval_cmd: Option<String>,
    as_agent: String,
    max_iters: Option<u32>,
    org: Option<String>,
    server: Option<String>,
) -> eyre::Result<()> {
    use workflows_orchestrator::{CodingWorkflow, WorkflowStore};
    use workflows_proto::SubjectRef;

    let slug = resolve_active_org(org)?;
    let url = resolve_org_vox_url(server, &slug);
    let agent = parse_agent_ref(&format!("agent:{as_agent}"))?;
    let wf = CodingWorkflow::new(WorkflowStore::open(org_workflows_dir(&slug)?));

    let Some((session, mut goal)) = active_goal_session(&wf)? else {
        println!("no goal session to resume");
        return Ok(());
    };

    // Re-resolve the worker / evaluator commands (flag → env → config),
    // matching `goal run`'s precedence — the loop doesn't persist them.
    let cfg = load_goal_config(&slug);
    let worker = cmd
        .or_else(|| std::env::var("TASK_AGENT_CMD").ok())
        .or(cfg.worker_cmd)
        .ok_or_else(|| {
            eyre::eyre!(
                "no worker command: pass --cmd, set TASK_AGENT_CMD, or set goal.worker_cmd in ~/.task/orgs/{slug}/agent.json"
            )
        })?;
    let evaluator = eval_cmd
        .or_else(|| std::env::var("TASK_GOAL_EVAL_CMD").ok())
        .or(cfg.eval_cmd);

    let task_id = match session.subject {
        SubjectRef::Task { id } => Some(id),
        _ => None,
    };
    if evaluator.is_none() && task_id.is_none() {
        return Err(eyre::eyre!(
            "no evaluator: pass --eval-cmd or set TASK_GOAL_EVAL_CMD / goal.eval_cmd"
        ));
    }

    let static_preamble = build_goal_preamble(&url, task_id).await?;
    let budget = max_iters.unwrap_or(goal.budget);

    // Resume the session (→ Active) and reset the turn counter — the
    // defining behaviour of `resume` vs a fresh `run`.
    wf.resume(session.id, agent.clone())?;
    goal.turns_used = 0;
    goal.budget = budget;
    goal.last_reason = String::new();
    goal.updated_at = chrono::Utc::now();
    wf.store().put_goal(&goal)?;

    println!(
        "▶ resuming goal session {} — “{}” (max {budget} turns, counter reset)",
        short_uuid(&session.id),
        goal.condition
    );

    // Honor the configured executor mode on resume too.
    let executor = if cfg.mode.as_deref() == Some("delegate") {
        GoalExecutor::NativeDelegate { worker: &worker }
    } else {
        GoalExecutor::Subprocess {
            worker: &worker,
            evaluator: evaluator.as_deref(),
        }
    };
    let run = executor.drive(&wf, session.id, &agent, &static_preamble, budget)?;
    finalize_goal_run(&run, budget, task_id, &url, &wf, session.id).await
}

/// Rebuild the loop's static preamble on resume: the rendered task
/// PRD + subtask checklist when the session is task-linked, else
/// empty. The condition is read live from the store per turn, so it's
/// deliberately not part of the preamble. Mirrors `goal run`.
async fn build_goal_preamble(url: &str, task_id: Option<uuid::Uuid>) -> eyre::Result<String> {
    if let Some(id) = task_id {
        let client = connect_task_client(url).await?;
        if let Ok(t) = client.get(id).await {
            let parent = match t.workflow.as_ref().and_then(|w| w.parent) {
                Some(pid) => client.get(pid).await.ok(),
                None => None,
            };
            let children = subtasks_of(&client, t.id).await.unwrap_or_default();
            let preamble = render_task_prompt(&t, parent.as_ref());
            let subtasks_md = render_subtask_checklist(&children);
            return Ok(format!("{preamble}{subtasks_md}"));
        }
    }
    Ok(String::new())
}

/// Verdict from one evaluator pass.
pub(crate) struct EvalVerdict {
    met: bool,
    reason: String,
}

/// Per-org `agent.json` `[goal]` defaults for `task agent goal` —
/// the executor seam's config layer. JSON to match the CLI's other
/// per-org stores (`labels.json`, `handoffs.json`). All optional.
#[derive(Default, serde::Deserialize)]
struct GoalConfig {
    worker_cmd: Option<String>,
    eval_cmd: Option<String>,
    #[allow(dead_code)] // reserved: config-driven turn budget
    max_turns: Option<u32>,
    /// `"delegate"` selects the native-delegate executor by default
    /// (for agents with their own goal loop). Anything else / unset =
    /// the turn-by-turn subprocess loop.
    mode: Option<String>,
}

#[derive(Default, serde::Deserialize)]
struct AgentConfig {
    #[serde(default)]
    goal: GoalConfig,
}

/// Load `~/.task/orgs/<slug>/agent.json` — missing / unparseable
/// file yields defaults (the feature degrades to flags + env).
fn load_goal_config(org_slug: &str) -> GoalConfig {
    let Some(home) = std::env::var_os("HOME") else {
        return GoalConfig::default();
    };
    let p = std::path::Path::new(&home)
        .join(".task")
        .join("orgs")
        .join(org_slug)
        .join("agent.json");
    std::fs::read(&p)
        .ok()
        .and_then(|b| serde_json::from_slice::<AgentConfig>(&b).ok())
        .unwrap_or_default()
        .goal
}

/// Interpret an evaluator's output. The cross-agent judge convention
/// (Hermes / Claude / Codex `/goal`) is a JSON object
/// `{"done": bool, "reason": "..."}` on stdout — preferred when
/// present (anywhere in the output, so a chatty judge still works).
/// Otherwise fall back to the exit code: `0` = met, nonzero = not,
/// with the trimmed stdout as the reason.
pub(crate) fn parse_eval_verdict(stdout: &str, code: i32) -> EvalVerdict {
    #[derive(serde::Deserialize)]
    struct Judged {
        done: bool,
        #[serde(default)]
        reason: String,
    }
    // Scan for the first `{...}` that parses as the judge shape, so
    // surrounding prose (common with LLM judges) doesn't defeat it.
    if let (Some(start), Some(end)) = (stdout.find('{'), stdout.rfind('}')) {
        if end > start {
            if let Ok(j) = serde_json::from_str::<Judged>(&stdout[start..=end]) {
                return EvalVerdict {
                    met: j.done,
                    reason: j.reason.trim().to_owned(),
                };
            }
        }
    }
    EvalVerdict {
        met: code == 0,
        reason: stdout.trim().to_owned(),
    }
}

/// Captured result of a worker / evaluator subprocess.
struct SubprocOut {
    code: i32,
    stdout: String,
    /// The most recent normalized step parsed from a stream-json
    /// worker (e.g. `Edit src/foo.rs`), if any — what it was last
    /// doing. `None` for opaque (non-stream-json) workers.
    last_activity: Option<String>,
}

/// Turn one line of a Claude `--output-format stream-json` worker into
/// a short, human-readable "current step" — a tool call (`Edit
/// <file>`, `Bash: <cmd>`) or an assistant message. Returns `None` for
/// lines that aren't a recognized event (the caller then streams them
/// raw), so plain workers are unaffected. Mirrors how t3code
/// normalizes agent events into a content-stream.
fn parse_stream_event(line: &str) -> Option<String> {
    fn clip(s: &str, n: usize) -> String {
        let s = s.trim();
        if s.chars().count() > n {
            format!("{}…", s.chars().take(n).collect::<String>())
        } else {
            s.to_owned()
        }
    }
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    match v.get("type")?.as_str()? {
        "assistant" => {
            let content = v.get("message")?.get("content")?.as_array()?;
            // Prefer a tool call (the concrete action).
            for b in content {
                if b.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                    let name = b
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("tool");
                    let arg = b.get("input").and_then(|i| {
                        ["file_path", "path", "command", "pattern", "query"]
                            .iter()
                            .find_map(|k| i.get(*k).and_then(serde_json::Value::as_str))
                    });
                    return Some(match arg {
                        Some(a) => format!("{name}: {}", clip(a, 80)),
                        None => name.to_owned(),
                    });
                }
            }
            // Else the assistant's message text.
            for b in content {
                if b.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                    if let Some(t) = b.get("text").and_then(serde_json::Value::as_str) {
                        let first = t.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                        if !first.is_empty() {
                            return Some(format!("\u{1f4ac} {}", clip(first, 80)));
                        }
                    }
                }
            }
            None
        }
        "result" => Some("✓ turn result".to_owned()),
        _ => None,
    }
}

/// Run `command` via `sh -c`, piping `prompt` to its stdin and
/// exposing `TASK_GOAL` / `TASK_GOAL_ITER` in its env.
///
/// Streams the child's stdout **live**, line by line, while also
/// capturing it for the caller — so a multi-minute agent turn isn't a
/// black box (the whole reason the loop looked "stuck"). A heartbeat
/// thread prints elapsed time every ~15s while the child runs quietly,
/// so even a worker that buffers its output (e.g. `claude -p`) shows
/// signs of life. stderr passes straight through.
fn run_subprocess(
    command: &str,
    prompt: &str,
    iter: u32,
    condition: &str,
) -> eyre::Result<SubprocOut> {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let started = std::time::Instant::now();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("TASK_GOAL", condition)
        .env("TASK_GOAL_ITER", iter.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| eyre::eyre!("spawn `{command}`: {e}"))?;

    // Feed the prompt and close stdin (drop) so the child sees EOF and
    // starts working.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| eyre::eyre!("write stdin: {e}"))?;
    }

    // Heartbeat: every 15s of quiet, remind the user it's alive.
    let alive = Arc::new(AtomicBool::new(true));
    let beat = {
        let alive = Arc::clone(&alive);
        std::thread::spawn(move || {
            let mut waited = 0u64;
            while alive.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_secs(1));
                waited += 1;
                if waited.is_multiple_of(15) && alive.load(Ordering::Relaxed) {
                    eprintln!("    …still working ({waited}s)");
                }
            }
        })
    };

    // Stream + capture stdout line by line. Stream-json events are
    // rendered as normalized steps (`→ Edit foo.rs`); everything else
    // streams raw (`│ <line>`).
    let mut captured = String::new();
    let mut last_activity: Option<String> = None;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines() {
            let line = line.map_err(|e| eyre::eyre!("read stdout: {e}"))?;
            if let Some(step) = parse_stream_event(&line) {
                println!("    → {step}");
                last_activity = Some(step);
            } else {
                println!("    │ {line}");
            }
            captured.push_str(&line);
            captured.push('\n');
        }
    }

    let status = child.wait().map_err(|e| eyre::eyre!("wait: {e}"))?;
    alive.store(false, Ordering::Relaxed);
    let _ = beat.join();
    eprintln!("    └ done in {}s", started.elapsed().as_secs());
    Ok(SubprocOut {
        code: status.code().unwrap_or(-1),
        stdout: captured,
        last_activity,
    })
}

/// Render the agent-facing prompt for a task: its own PRD plus the
/// parent issue's PRD when it's a subtask. The one built-in template
/// (concrete-first; a pluggable template system is deferred). Shared
/// by `task issue prompt` and `task agent goal --task` so the loop
/// and the standalone preview never drift.
pub(crate) fn render_task_prompt(t: &task::TaskInfo, parent: Option<&task::TaskInfo>) -> String {
    let mut s = String::new();
    if let Some(p) = parent {
        s.push_str(&format!("# Parent issue (PRD): {}\n\n", p.title));
        let body = p.details.trim();
        if body.is_empty() {
            s.push_str("(no description)\n\n");
        } else {
            s.push_str(body);
            s.push_str("\n\n");
        }
        s.push_str("---\n\n");
    }
    s.push_str(&format!("# Task: {}  [{}]\n\n", t.title, t.priority));
    let body = t.details.trim();
    if body.is_empty() {
        s.push_str("(no description)\n");
    } else {
        s.push_str(body);
        s.push('\n');
    }
    if !t.tags.0.is_empty() {
        s.push_str(&format!("\ntags: {}\n", t.tags.0.join(", ")));
    }
    s
}

/// Open subtasks of `parent_id` — tasks whose `workflow.parent`
/// points at it — oldest first by title (stable enough for a list).
async fn subtasks_of(
    client: &task::TaskServiceClient,
    parent_id: uuid::Uuid,
) -> eyre::Result<Vec<task::TaskInfo>> {
    let rows = client
        .list()
        .await
        .map_err(|e| eyre::eyre!("list: {e:?}"))?;
    let mut kids: Vec<task::TaskInfo> = rows
        .into_iter()
        .filter(|t| t.workflow.as_ref().and_then(|w| w.parent) == Some(parent_id))
        .collect();
    kids.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(kids)
}

/// Create one subtask `title` under `parent`, mirroring `issue
/// triage`'s row shape (parent link, `subtask` tag, inherited
/// project). Returns `false` (rather than erroring) when a task with
/// the same title-slug already exists — generated titles can collide,
/// and a collision shouldn't abort the whole triage.
async fn create_subtask(
    client: &task::TaskServiceClient,
    parent: &task::TaskInfo,
    title: &str,
) -> eyre::Result<bool> {
    let sub = task::TaskInfo {
        id: uuid::Uuid::nil(),
        path: String::new(),
        title: title.to_owned(),
        status: "open".into(),
        priority: parent.priority.clone(),
        due: None,
        scheduled: None,
        tags: task::model::StringList(vec!["task".into(), "subtask".into()]),
        contexts: task::model::StringList::default(),
        projects: task::model::StringList::default(),
        project_id: parent.project_id,
        milestone_id: None,
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
        details: String::new(),
        workflow: Some(task::model::WorkflowAttrs {
            parent: Some(parent.id),
            ..Default::default()
        }),
    };
    match client.create(sub).await {
        Ok(_) => Ok(true),
        // Title-slug already taken — skip rather than abort the run.
        // Matched on the message since the error is wrapped in
        // VoxError<TaskError> across the wire.
        Err(e) if format!("{e:?}").contains("AlreadyExists") => Ok(false),
        Err(e) => Err(eyre::eyre!("create subtask: {e:?}")),
    }
}

/// The decompose-turn prompt. "Decompose, don't execute" (per
/// Hermes's kanban-orchestrator): the worker breaks the PRD into
/// agent-sized titles or declares it a one-shot.
fn decompose_prompt(task_prompt: &str) -> String {
    format!(
        "{task_prompt}\n\n---\n\nYou are TRIAGING this task — do NOT implement anything. \
         Break it into 2–6 agent-sized subtasks, each a single focused PR's worth of work, \
         ideally independent. Output ONLY the subtask titles, one per line, no numbering or \
         prose. If the task is small enough to do in one shot (no fan-out needed), output \
         the single line: ONE-SHOT"
    )
}

/// Parse subtask titles from a decompose turn's output. Strips
/// bullets / numbering, drops blanks and the `ONE-SHOT` sentinel,
/// and ignores obvious prose (lines ending in `:` or very long).
fn parse_subtask_titles(out: &str) -> Vec<String> {
    out.lines()
        .map(|l| {
            l.trim()
                .trim_start_matches(['-', '*', '•'])
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')')
                .trim()
                .to_owned()
        })
        .filter(|l| {
            !l.is_empty()
                && !l.eq_ignore_ascii_case("ONE-SHOT")
                && !l.ends_with(':')
                && l.len() <= 140
        })
        .collect()
}

/// Render the subtask checklist appended to the goal prompt.
fn render_subtask_checklist(children: &[task::TaskInfo]) -> String {
    if children.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\n## Subtasks\n");
    for c in children {
        let done = matches!(task::Status::from_str(&c.status), Some(task::Status::Done));
        s.push_str(&format!(
            "- [{}] {}\n",
            if done { "x" } else { " " },
            c.title
        ));
    }
    s
}

/// `~/.task/orgs/<slug>/workflows` — the orchestrator store dir.
///
/// DELIBERATELY machine-local, not vox: this is the goal-loop
/// runtime's own state (sessions, subgoals, per-turn heartbeats),
/// and the loop it belongs to runs HERE — `goal run` spawns
/// worker / evaluator subprocesses on this machine, against this
/// machine's checkouts. No workflows service exists on the org
/// router (workflows-proto is types-only; the mounted agent
/// services are the Codex/Hermes chat + task-queue surfaces), so
/// there is nothing to route these through — and steering a
/// subprocess loop on ANOTHER box through org data would be wrong
/// anyway. If goal sessions ever need remote visibility, that's a
/// new `#[architect::rpc]` surface over this store, served by
/// whichever machine hosts the loop.
fn org_workflows_dir(org_slug: &str) -> eyre::Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| eyre::eyre!("HOME not set"))?;
    Ok(std::path::Path::new(&home)
        .join(".task")
        .join("orgs")
        .join(org_slug)
        .join("workflows"))
}

pub(crate) async fn run_agent_queue(cmd: AgentQueueCmd) -> eyre::Result<()> {
    use agent_proto::service::tasks::AgentTaskQueueClient;
    use agent_proto::tasks::QueueFilter;

    async fn connect_queue(url: String) -> eyre::Result<AgentTaskQueueClient> {
        establish_for_url(&url).await
    }
    let connect = |url: String| connect_queue(url);
    let default_handle = || {
        format!(
            "{}@{}",
            std::env::var("USER").unwrap_or_else(|_| "anon".into()),
            std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("HOST"))
                .unwrap_or_else(|_| "host".into())
        )
    };
    let body = |s: String| -> eyre::Result<String> {
        if s == "-" {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            Ok(buf)
        } else {
            Ok(s)
        }
    };

    match cmd {
        AgentQueueCmd::Read {
            queue,
            only_handle,
            include_archived,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect(url.clone()).await?;
            let queue_id = queue.unwrap_or_else(|| slug.clone());
            let filter = QueueFilter {
                assignee: String::new(),
                include_archived,
                only_handle: only_handle.unwrap_or_default(),
                linked_session_id: String::new(),
                agent_profile: String::new(),
            };
            let snap = client
                .read_queue(queue_id, filter)
                .await
                .map_err(|e| eyre::eyre!("read_queue: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&snap).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!(
                "queue {}  ({} tasks, watermark={})",
                snap.queue.id,
                snap.tasks.len(),
                snap.latest_event_id
            );
            for t in &snap.tasks {
                println!("  {:<10}  {:<32}  {}", t.status, t.title, t.id);
            }
        }
        AgentQueueCmd::Claim {
            task_id,
            handle,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect(url.clone()).await?;
            let h = handle.unwrap_or_else(default_handle);
            let t = client
                .claim_agent_task(task_id, h.clone())
                .await
                .map_err(|e| eyre::eyre!("claim: {e:?}"))?;
            println!("claimed {} as {h} → [{}]", t.title, t.status);
        }
        AgentQueueCmd::SetStatus {
            task_id,
            new_status,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect(url.clone()).await?;
            let t = client
                .set_agent_task_status(task_id, new_status)
                .await
                .map_err(|e| eyre::eyre!("set_status: {e:?}"))?;
            println!("{} → [{}]", t.title, t.status);
        }
        AgentQueueCmd::Complete {
            task_id,
            result,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect(url.clone()).await?;
            let result_blob = body(result)?;
            let t = client
                .complete_agent_task(task_id, result_blob)
                .await
                .map_err(|e| eyre::eyre!("complete: {e:?}"))?;
            println!("completed {} → [{}]", t.title, t.status);
        }
        AgentQueueCmd::Link {
            task_id,
            session_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect(url.clone()).await?;
            let t = client
                .link_agent_task_to_session(task_id, session_id.clone())
                .await
                .map_err(|e| eyre::eyre!("link: {e:?}"))?;
            println!("linked {} → session {session_id}", t.title);
        }
        AgentQueueCmd::Links {
            queue,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect(url.clone()).await?;
            let queue_id = queue.unwrap_or_else(|| slug.clone());
            let links = client
                .list_agent_task_links(queue_id)
                .await
                .map_err(|e| eyre::eyre!("links: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&links).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            for l in &links {
                println!("{}  →  {}  ({})", l.from_task, l.to_task, l.kind);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_eval_verdict_reads_the_judges_json() {
        let v = parse_eval_verdict(r#"{"done": true, "reason": "tests pass"}"#, 0);
        assert!(v.met);
        assert_eq!(v.reason, "tests pass");

        // A judge that says "not done" overrides a zero exit code —
        // the whole point of running an evaluator.
        let v = parse_eval_verdict(r#"{"done": false, "reason": "3 failures"}"#, 0);
        assert!(!v.met);
        assert_eq!(v.reason, "3 failures");
    }

    #[test]
    fn parse_eval_verdict_digs_the_json_out_of_surrounding_prose() {
        // LLM judges routinely wrap the object in commentary; scanning
        // from the first `{` to the last `}` is what keeps that from
        // silently falling through to the exit-code path.
        let v = parse_eval_verdict(
            "Here is my assessment:\n{\"done\": true, \"reason\": \"looks good\"}\nHope that helps!",
            1,
        );
        assert!(v.met, "prose around the object must not defeat parsing");
        assert_eq!(v.reason, "looks good");
    }

    #[test]
    fn parse_eval_verdict_falls_back_to_the_exit_code() {
        let v = parse_eval_verdict("no json here", 0);
        assert!(v.met);
        assert_eq!(v.reason, "no json here");

        let v = parse_eval_verdict("  boom  ", 1);
        assert!(!v.met);
        assert_eq!(v.reason, "boom");

        // Malformed JSON is not a verdict — fall through, don't panic.
        let v = parse_eval_verdict("{not valid json}", 1);
        assert!(!v.met);

        // `reason` is optional in the judge shape.
        let v = parse_eval_verdict(r#"{"done": true}"#, 1);
        assert!(v.met);
        assert_eq!(v.reason, "");
    }

    #[test]
    fn parse_stream_event_prefers_the_tool_call() {
        let line = r#"{"type":"assistant","message":{"content":[
            {"type":"text","text":"Let me edit that file."},
            {"type":"tool_use","name":"Edit","input":{"file_path":"src/foo.rs"}}
        ]}}"#;
        assert_eq!(
            parse_stream_event(line).as_deref(),
            Some("Edit: src/foo.rs"),
            "the concrete action beats the narration"
        );
    }

    #[test]
    fn parse_stream_event_falls_back_to_message_text() {
        let line = r#"{"type":"assistant","message":{"content":[
            {"type":"text","text":"\n\nRunning the suite now.\nSecond line."}
        ]}}"#;
        let got = parse_stream_event(line).expect("text event");
        assert!(got.contains("Running the suite now."), "{got}");
        assert!(
            !got.contains("Second line."),
            "only the first non-empty line is the step: {got}"
        );
    }

    #[test]
    fn parse_stream_event_ignores_lines_it_does_not_understand() {
        // Unrecognized lines return None so the caller streams them
        // raw — this is what keeps plain (non-stream-json) workers
        // working unchanged.
        assert_eq!(parse_stream_event("not json at all"), None);
        assert_eq!(parse_stream_event(r#"{"type":"system"}"#), None);
        assert_eq!(parse_stream_event(""), None);
        assert_eq!(
            parse_stream_event(r#"{"type":"result"}"#).as_deref(),
            Some("✓ turn result")
        );
    }

    #[test]
    fn parse_stream_event_clips_long_arguments() {
        let long = "x".repeat(200);
        let line = format!(
            r#"{{"type":"assistant","message":{{"content":[
                {{"type":"tool_use","name":"Bash","input":{{"command":"{long}"}}}}
            ]}}}}"#
        );
        let got = parse_stream_event(&line).expect("tool event");
        assert!(got.ends_with('…'), "{got}");
        assert!(got.chars().count() < 100, "clipped to a status line: {got}");
    }

    #[test]
    fn parse_subtask_titles_strips_list_markers_and_drops_noise() {
        let out = "\
- Wire the store
* Add the route
3. Write tests

ONE-SHOT
Headings are not subtasks:
";
        assert_eq!(
            parse_subtask_titles(out),
            vec!["Wire the store", "Add the route", "Write tests"]
        );
    }

    #[test]
    fn parse_subtask_titles_rejects_prose_paragraphs() {
        // A model that answers in prose instead of a list would
        // otherwise turn a whole paragraph into a "subtask".
        let long = format!("- {}", "a".repeat(200));
        assert!(parse_subtask_titles(&long).is_empty());
    }
}
