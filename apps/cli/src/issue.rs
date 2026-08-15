//! `task issue …` — the workflow-aware view of `TaskInfo`.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::agent::render_task_prompt;
use crate::establish_for_url;
use crate::forge::build_repo_id;
use crate::forge::forge_backend_for;
use crate::forge::forge_link_store;
use crate::forge::forgejo_base_url;
use crate::forge::parse_repo_slug;
use crate::goal::resolve_cycle_arg;
use crate::project::connect_project_client;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::resolve_body;
use crate::shared::short_uuid;
use crate::task_cmd::connect_task_client;

/// Linear-style issue surface over TaskInfo's WorkflowAttrs.
///
/// **Why this exists alongside `task task *`.** TaskInfo is the
/// canonical unit of work in Task — the same row underpins both
/// `task task *` (the TaskNotes-shape personal-task surface)
/// and `task issue *` (the Linear-shape work-tracking surface).
/// `issue` verbs operate through `WorkflowAttrs`: filter / show
/// / patch the workspace + cycle + project + estimate +
/// assignees + blockers triplet.
///
/// Org / server routing: this command group relies on the global
/// `--org` / `--server` flags (clap propagates them, so they can
/// still be passed after the subcommand) instead of re-declaring
/// per-variant duplicates like the older groups do.
#[derive(Subcommand)]
pub(crate) enum IssueCmd {
    /// List tasks filtered by their workflow attributes.
    List {
        /// Filter by cycle — UUID, `YYYY:Qn:Cm` / `YYYY-Qn-Cm`
        /// label, or `current` for today's cycle.
        #[arg(long)]
        cycle: Option<String>,
        /// Filter by project — UUID, id prefix, vault path, or
        /// name (exact / unique prefix).
        #[arg(long)]
        project: Option<String>,
        /// Filter by an `AgentRef` in `workflow.assignees`.
        /// Accepts `agent:name`, `agent:name@version`,
        /// `human:user_id`, or a bare name (defaults to `agent:`).
        #[arg(long)]
        assignee: Option<String>,
        /// Filter by `TaskInfo::status` (e.g. `open`, `in-progress`).
        #[arg(long)]
        status: Option<String>,
        /// Only show tasks with `workflow: Some(_)` set. Useful
        /// while migrating — keeps unmigrated personal tasks
        /// out of the issue view.
        #[arg(long)]
        has_workflow: bool,
        /// Emit JSON instead of the tabular default.
        #[arg(long)]
        json: bool,
    },

    /// Show a single issue. Accepts a UUID, an id prefix, a vault
    /// path, or a title (exact / unique prefix).
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },

    /// Render the agent prompt for a task — its PRD plus the parent
    /// issue's PRD (when it's a subtask), formatted as the directive
    /// an agent receives. The same renderer `task agent goal --task`
    /// feeds the loop, exposed standalone so you can inspect exactly
    /// what an agent will be handed.
    Prompt { id: String },

    /// Patch the issue's `WorkflowAttrs` in place. Repeatable
    /// `--add-assignee` / `--add-blocker` for set operations.
    /// Pass `--clear` to drop the workflow block entirely (the
    /// task becomes a plain TaskNotes-shape task again).
    SetWorkflow {
        id: String,
        /// UUID, `YYYY:Qn:Cm`, `current`, or `"none"` / `""` to
        /// clear.
        #[arg(long)]
        cycle: Option<String>,
        /// Project reference (UUID, name, path, prefix), or
        /// `"none"` / `""` to clear.
        #[arg(long)]
        project: Option<String>,
        /// Workstream reference (UUID, name, path, prefix), or
        /// `"none"` / `""` to clear. Sets `workflow.workstream`.
        #[arg(long)]
        workstream: Option<String>,
        /// `xs`, `s`, `m`, `l`, `xl`, or a plain integer for
        /// `Estimate::Points`.
        #[arg(long)]
        estimate: Option<String>,
        #[arg(long = "add-assignee", value_name = "AGENT")]
        add_assignee: Vec<String>,
        #[arg(long = "remove-assignee", value_name = "AGENT")]
        remove_assignee: Vec<String>,
        /// Blocking issue (UUID, id prefix, path, or title).
        #[arg(long = "add-blocker", value_name = "TASK")]
        add_blocker: Vec<String>,
        /// Blocking issue (UUID, id prefix, path, or title).
        #[arg(long = "remove-blocker", value_name = "TASK")]
        remove_blocker: Vec<String>,
        /// Drop the workflow block entirely.
        #[arg(long)]
        clear: bool,
        /// Emit the resulting issue as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Atomically claim an issue for an agent — the core of the
    /// parallel-agent workflow. Fails if another agent already
    /// holds it (read → check-empty → write → re-read verify),
    /// so two agents racing for the same subtask can't both win.
    /// Pass `--force` to steal a claim.
    Claim {
        id: String,
        /// `name[@version]` — version omitted means "any version".
        #[arg(long = "as-agent")]
        as_agent: String,
        /// Steal the claim even if someone else holds it.
        #[arg(long)]
        force: bool,
        /// Emit the claimed issue as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Triage an issue (PRD) into agent-sized subtasks — the
    /// "it's time to start this" step. Creates one subtask per
    /// title under the parent, flips the parent to in-progress,
    /// and prints the board. Titles come from repeated
    /// `--subtask` flags and/or `--from` (one per line, `-` for
    /// stdin). After this, parallel agents `claim` + `code start`.
    Triage {
        /// Parent issue id (UUID or 8-char prefix).
        id: String,
        /// A subtask title. Repeatable.
        #[arg(long = "subtask", value_name = "TITLE")]
        subtasks: Vec<String>,
        /// Read additional subtask titles, one per line, from a
        /// file or `-` for stdin.
        #[arg(long)]
        from: Option<String>,
        /// Status to set on the parent after triage. Default
        /// `in-progress`.
        #[arg(long, default_value = "in-progress")]
        parent_status: String,
        /// Priority applied to every created subtask.
        #[arg(long, default_value = "normal")]
        priority: String,
    },

    /// List the subtasks of a parent task with their claim +
    /// status, so you can see who's working what at a glance.
    /// Header shows the derived rollup (done / in-progress /
    /// blocked / points), classified via state groups.
    Subtasks {
        /// Parent task id (UUID or 8-char prefix).
        id: String,
        #[arg(long)]
        json: bool,
    },

    /// Derived sub-issue rollup for one parent — done / total /
    /// in-progress / blocked / estimate points over its direct
    /// children (`workflow.parent`), classified via each child's
    /// project state registry. Same engine as the workstream
    /// rollup.
    Rollup {
        /// Parent issue id (UUID, prefix, path, or title).
        id: String,
        #[arg(long)]
        json: bool,
    },

    /// Add a typed relation between two issues:
    /// `task issue relate <a> <kind> <b>` records
    /// "`<a>` `<kind>`s `<b>`" (kind ∈ blocks | duplicate | implements
    /// | relates).
    /// Stored in `<a>`'s `workflow.relations`; the legacy
    /// blockers / relates_to lists keep working alongside.
    Relate {
        /// Source issue (UUID, prefix, path, or title).
        a: String,
        /// blocks | duplicate | implements | relates.
        kind: String,
        /// Target issue (UUID, prefix, path, or title).
        b: String,
        /// Remove the relation instead of adding it.
        #[arg(long)]
        remove: bool,
        /// Emit the resulting source issue as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Show an issue's relation graph — outgoing edges (typed
    /// relations + the legacy relates_to entries + "blocks"
    /// edges implied by other tasks' blockers lists) and
    /// incoming reverse edges ("what blocks / duplicates /
    /// implements THIS"), merged across both encodings.
    Relations {
        id: String,
        #[arg(long)]
        json: bool,
    },

    /// Sugar: every issue this one BLOCKS (typed `blocks`
    /// relations + other tasks listing it in `blockers`).
    Blocking {
        id: String,
        #[arg(long)]
        json: bool,
    },

    /// List the current assignees on an issue.
    Assignees {
        id: String,
        #[arg(long)]
        json: bool,
    },

    /// Create a new issue. Workflow attrs can be set inline.
    /// Equivalent to `task task create` + `task issue
    /// set-workflow` in one call.
    Create {
        /// Title (positional). Body can be passed via --body.
        title: String,
        /// Vault-relative path; defaults to `Task/<slug>.md`.
        #[arg(long)]
        path: Option<String>,
        /// Initial status. Default `open`.
        #[arg(long)]
        status: Option<String>,
        /// Initial priority. Default `normal`.
        #[arg(long)]
        priority: Option<String>,
        /// Cycle (UUID, `YYYY:Qn:Cm`, or `current`). Sets
        /// `workflow.cycle`.
        #[arg(long)]
        cycle: Option<String>,
        /// Project (UUID, name, path, prefix). Sets `project_id`.
        #[arg(long)]
        project: Option<String>,
        /// Parent issue (UUID, id prefix, path, or title) — makes
        /// this a subtask. Sets `workflow.parent`.
        #[arg(long)]
        parent: Option<String>,
        /// Workstream (UUID, name, path, prefix). Sets
        /// `workflow.workstream`.
        #[arg(long)]
        workstream: Option<String>,
        /// Estimate (`xs` / `s` / `m` / `l` / `xl` / integer).
        #[arg(long)]
        estimate: Option<String>,
        /// Repeatable assignee. `agent:name[@ver]` or
        /// `human:user_id`. Bare names default to agent.
        #[arg(long = "assignee", value_name = "AGENT")]
        assignees: Vec<String>,
        /// Repeatable blocker (UUID, id prefix, path, or title) —
        /// `task issue ready` won't surface this issue until each
        /// blocker closes.
        #[arg(long = "blocker", value_name = "TASK")]
        blockers: Vec<String>,
        /// Repeatable tag.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,

        /// Verify command for this ticket — the shell command whose
        /// exit code decides whether an agent's work is done.
        /// Omit to inherit the project's `verifyCommand`. A ticket
        /// that resolves to none cannot be tagged `ready-for-agent`.
        #[arg(long = "verify", value_name = "COMMAND")]
        verify: Option<String>,

        /// Repeatable capability a runner must have to take this
        /// ticket: `records`, `shell`, `build`, `repo:<owner>/<name>`.
        /// Set during triage; empty means any runner will do.
        #[arg(long = "cap", value_name = "CAPABILITY")]
        caps: Vec<String>,

        /// Model this ticket should be worked with. Omit for the
        /// runner's default.
        #[arg(long = "model", value_name = "MODEL")]
        model: Option<String>,
        /// Body (markdown). Pass `-` for stdin, or a file path.
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Show issues ready to work — open, not done, with no
    /// unresolved blockers. The beads-equivalent of `bd ready`.
    Ready {
        /// Cycle filter — UUID, `YYYY:Qn:Cm` label, or `current`.
        #[arg(long)]
        cycle: Option<String>,
        /// Project filter — UUID, id prefix, path, or name.
        #[arg(long)]
        project: Option<String>,
        /// Show only issues claimable by this agent (no
        /// assignee yet, OR this agent is already listed).
        #[arg(long)]
        as_agent: Option<String>,
        /// Max rows to show.
        #[arg(long, default_value = "20")]
        limit: usize,
        #[arg(long)]
        json: bool,
    },

    /// Claim an issue and flip its status to `in-progress`.
    /// The combined `bd update --claim` equivalent.
    Start {
        id: String,
        /// Agent to claim as — `name[@version]`. If omitted,
        /// only the status is changed (existing assignees
        /// are preserved).
        #[arg(long = "as-agent")]
        as_agent: Option<String>,
        /// Emit the resulting issue as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Close an issue — flips status to `done` and stamps
    /// `completedDate`. Pass `--undo` to reopen.
    Close {
        id: String,
        #[arg(long)]
        undo: bool,
        /// Emit the resulting issue as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Migrate beads issues into Task. Reads a `bd list --json`
    /// export and creates a TaskInfo per issue (status + priority
    /// mapped, tagged `from-beads`). The "replace beads" step.
    ImportBeads {
        /// Source: `bd` (shell `bd list --json`), a file path, or
        /// `-` for stdin. Default `bd`.
        #[arg(long, default_value = "bd")]
        from: String,
        /// Parse + report what would be created without writing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Project-level overview — counts grouped by status,
    /// priority, workspace, and assignee. Beads-equivalent of
    /// `bd stats`.
    Stats {
        /// Restrict to one project (UUID, id prefix, path, or
        /// name).
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Record an existing forge issue as the upstream of a
    /// local TaskInfo. Doesn't create either — just adds the
    /// IssueLink to the per-org link store. Use this when an
    /// issue already exists on both sides.
    LinkForge {
        /// Local TaskInfo id (UUID or prefix).
        id: String,
        /// `owner/repo` on the forge.
        repo: String,
        /// Forge-assigned issue number (or PR number with --kind pull).
        number: u64,
        /// Forge host base URL (e.g. `https://git.starcommand.live`).
        /// Defaults to the value in `TASK_FORGEJO_BASE_URL`.
        #[arg(long)]
        base_url: Option<String>,
        /// `issue` or `pull`. Default `issue`.
        #[arg(long, default_value = "issue")]
        kind: String,
    },

    /// Push a local TaskInfo upstream — creates a Forgejo issue
    /// from `title` + `details`, records the link, exits. If the
    /// task already has a link to this repo, prints the existing
    /// link and exits without re-creating.
    Push {
        id: String,
        /// `owner/repo` on the forge.
        #[arg(long)]
        repo: String,
        /// Target GitHub instead of Forgejo. Uses TASK_GITHUB_TOKEN.
        #[arg(long)]
        github: bool,
        /// Forgejo host base URL. Falls back to `TASK_FORGEJO_BASE_URL`.
        /// Ignored when --github is set.
        #[arg(long)]
        base_url: Option<String>,
    },

    /// On-demand bidirectional reconcile — no webhook needed.
    /// For every locally-linked issue in the repo, fetch its
    /// current forge state and apply it (forge wins for
    /// open/closed); then pull any new forge issues we don't
    /// track yet. Run manually or on a cron for third-party
    /// repos where you can't install a webhook.
    Sync {
        /// `owner/repo` on the forge.
        #[arg(long)]
        repo: String,
        /// Sync against GitHub instead of Forgejo.
        #[arg(long)]
        github: bool,
        /// Forgejo host base URL. Falls back to `TASK_FORGEJO_BASE_URL`.
        #[arg(long)]
        base_url: Option<String>,
        /// Optional project (UUID, name, path, prefix) to stamp
        /// on newly-pulled tasks.
        #[arg(long)]
        project: Option<String>,
        /// Don't create local tasks for forge issues we don't
        /// track — only reconcile state of already-linked ones.
        #[arg(long)]
        no_pull: bool,
    },

    /// Sync every linked repo in the org in one pass — one
    /// cron line keeps all your tracked repos fresh without
    /// webhooks.
    SyncAll {
        /// Optional project (UUID, name, path, prefix) to stamp
        /// on newly-pulled tasks.
        #[arg(long)]
        project: Option<String>,
        /// Only reconcile existing links; don't pull new issues.
        #[arg(long)]
        no_pull: bool,
    },

    /// List open pull requests on a repo.
    PrList {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        github: bool,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Open a pull request.
    PrCreate {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        github: bool,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        title: String,
        /// Source branch.
        #[arg(long)]
        head: String,
        /// Target branch. Default `main`.
        #[arg(long, default_value = "main")]
        base: String,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        draft: bool,
        /// Forge issue number this PR closes. Injects
        /// `Closes #N` into the body so the forge auto-closes
        /// the issue when the PR merges.
        #[arg(long)]
        closes: Option<u64>,
        /// Local task whose linked forge issue this PR closes.
        /// Resolves the issue number from the link store and
        /// injects `Closes #N` — and records a PR link on the
        /// task so `pr-merge`/sync can finish the loop.
        #[arg(long)]
        close_task: Option<String>,
    },

    /// Merge a pull request by number. With `--close-task`,
    /// closes the linked task afterward (which propagates the
    /// close back to its own forge issue) — the `task code
    /// merge` chain: merge PR → close task → done everywhere.
    PrMerge {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        github: bool,
        #[arg(long)]
        base_url: Option<String>,
        number: u64,
        /// `merge` (default), `squash`, or `rebase`.
        #[arg(long, default_value = "merge")]
        method: String,
        /// After merging, close this task (UUID or prefix). Its
        /// own linked forge issue gets closed too via the normal
        /// close-propagation path.
        #[arg(long)]
        close_task: Option<String>,
    },

    /// Serialize-merge a queue of open PRs (the parallel-agent
    /// landing strip). Merges in PR-number order, one at a time, so
    /// N worktree PRs from one issue land without racing on `main`.
    /// Each merged PR closes its linked task (and that task's forge
    /// issue). On a merge that the forge rejects (e.g. now-conflicting
    /// after an earlier merge) the queue stops — fix the conflict and
    /// re-run — unless `--keep-going`.
    MergeQueue {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        github: bool,
        #[arg(long)]
        base_url: Option<String>,
        /// `squash` (default), `merge`, or `rebase`.
        #[arg(long, default_value = "squash")]
        method: String,
        /// Only queue PRs linked to subtasks of this issue (UUID or
        /// 8-char prefix). Omit to queue every open PR on the repo.
        #[arg(long)]
        issue: Option<String>,
        /// Print the merge plan without merging anything.
        #[arg(long)]
        dry_run: bool,
        /// Keep merging the rest of the queue after a failed merge
        /// instead of stopping at the first conflict.
        #[arg(long)]
        keep_going: bool,
    },

    /// Fetch all issues from a Forgejo repo and create local
    /// TaskInfos for ones we don't already have linked. Existing
    /// linked issues are left alone (use `sync` to update).
    Pull {
        /// `owner/repo` on the forge.
        #[arg(long)]
        repo: String,
        /// Pull from GitHub instead of Forgejo. Uses TASK_GITHUB_TOKEN.
        #[arg(long)]
        github: bool,
        /// Forgejo host base URL. Falls back to `TASK_FORGEJO_BASE_URL`.
        /// Ignored when --github is set.
        #[arg(long)]
        base_url: Option<String>,
        /// Optional project (UUID, name, path, prefix) to stamp
        /// on pulled-in tasks.
        #[arg(long)]
        project: Option<String>,
        /// Filter by issue state: `open` (default), `closed`, or `all`.
        #[arg(long, default_value = "open")]
        state: String,
    },
}

/// Per-project state registries: project id → its optional
/// `states:` config. Best-effort (an unreachable project service
/// degrades to the default registry everywhere).
async fn project_states_map(
    url: &str,
) -> std::collections::HashMap<uuid::Uuid, Option<project::StatesConfig>> {
    match connect_project_client(url).await {
        Ok(pc) => pc
            .list()
            .await
            .map(|ps| ps.into_iter().map(|p| (p.id, p.states)).collect())
            .unwrap_or_default(),
        Err(_) => std::collections::HashMap::new(),
    }
}

/// Classify one task's status via its owning project's state
/// registry (default registry when project unknown / unset).
fn resolve_task_group(
    states: &std::collections::HashMap<uuid::Uuid, Option<project::StatesConfig>>,
    t: &task::TaskInfo,
) -> project::StateGroup {
    let cfg = t
        .project_id
        .and_then(|pid| states.get(&pid))
        .and_then(Option::as_ref);
    project::resolve_state_group(cfg, &t.status)
}

/// Parse an `AgentRef` from CLI input. Accepted forms:
/// `agent:name`, `agent:name@version`, `human:user_id`, or
/// a bare `name` (defaults to an unversioned agent).
pub(crate) fn parse_agent_ref(s: &str) -> eyre::Result<workflows_proto::AgentRef> {
    let s = s.trim();
    if s.is_empty() {
        return Err(eyre::eyre!("empty agent ref"));
    }
    if let Some(rest) = s.strip_prefix("human:") {
        let rest = rest.trim();
        if rest.is_empty() {
            return Err(eyre::eyre!("human: prefix requires a user id"));
        }
        return Ok(workflows_proto::AgentRef::human(rest));
    }
    let body = s.strip_prefix("agent:").unwrap_or(s);
    let body = body.trim();
    if body.is_empty() {
        return Err(eyre::eyre!("agent: prefix requires a name"));
    }
    if let Some((name, ver)) = body.split_once('@') {
        let name = name.trim();
        let ver = ver.trim();
        if name.is_empty() {
            return Err(eyre::eyre!("agent name is empty"));
        }
        if ver.is_empty() {
            return Ok(workflows_proto::AgentRef::agent(name));
        }
        return Ok(workflows_proto::AgentRef::agent_versioned(name, ver));
    }
    Ok(workflows_proto::AgentRef::agent(body))
}

/// Parse `xs|s|m|l|xl` or a numeric points value into an
/// [`task::model::Estimate`].
fn parse_estimate(s: &str) -> eyre::Result<task::model::Estimate> {
    use task::model::Estimate;
    match s.trim().to_ascii_lowercase().as_str() {
        "xs" => Ok(Estimate::XS),
        "s" => Ok(Estimate::S),
        "m" => Ok(Estimate::M),
        "l" => Ok(Estimate::L),
        "xl" => Ok(Estimate::XL),
        other => {
            let value: u8 = other
                .parse()
                .map_err(|e| eyre::eyre!("bad estimate `{other}`: {e}"))?;
            Ok(Estimate::Points { value })
        }
    }
}

/// Resolve an issue reference — uuid, id prefix, vault path, or
/// title (issues and tasks are the same `TaskInfo` row, so this is
/// the shared flexible task resolver).
pub(crate) async fn resolve_issue_id(
    client: &task::TaskServiceClient,
    id: &str,
) -> eyre::Result<task::TaskInfo> {
    crate::json_out::resolve_task_flexible(client, id).await
}

#[allow(clippy::ref_option)] // ergonomic: callers pass `&t.workflow` directly
fn workflow_summary(w: &Option<task::model::WorkflowAttrs>) -> String {
    let Some(w) = w else {
        return "—".into();
    };
    let cy = w.cycle.as_ref().map_or("—".into(), short_uuid);
    format!("cy={cy}")
}

fn print_workflow_block(w: &task::model::WorkflowAttrs) {
    use task::model::Estimate;
    println!("  workflow:");
    if let Some(cy) = w.cycle {
        println!("    cycle:     {cy}");
    }
    if let Some(ws) = w.workstream {
        println!("    workstream:{ws}");
    }
    if let Some(est) = &w.estimate {
        let rendered = match est {
            Estimate::XS => "xs".to_string(),
            Estimate::S => "s".to_string(),
            Estimate::M => "m".to_string(),
            Estimate::L => "l".to_string(),
            Estimate::XL => "xl".to_string(),
            Estimate::Points { value } => format!("{value} pts"),
        };
        println!("    estimate:  {rendered}");
    }
    if let Some(sid) = w.session {
        println!("    session:   {sid}");
    }
    if !w.assignees.is_empty() {
        println!("    assignees:");
        for a in w.assignees.iter() {
            println!("      - {}", a.short_label());
        }
    }
    if !w.blockers.is_empty() {
        println!("    blockers:");
        for b in w.blockers.iter() {
            println!("      - {b}");
        }
    }
    if !w.relates_to.is_empty() {
        println!("    relates_to:");
        for r in w.relates_to.iter() {
            println!("      - {r}");
        }
    }
}

/// Result of an atomic claim attempt.
pub(crate) enum ClaimOutcome {
    /// This agent now holds the claim.
    Won,
    /// This agent already held it (idempotent).
    AlreadyMine,
    /// Another actor holds it; carries their label.
    Lost(String),
}

/// Atomic claim via the server-side `try_claim` RPC. The backend
/// serializes the read-check-write under a process lock, so two
/// agents racing for the same task can't both win — no TOCTOU
/// window (unlike the old client-side optimistic version). The
/// agent is sent as a JSON-encoded `AgentRef`.
pub(crate) async fn try_claim(
    client: &task::TaskServiceClient,
    task_id: &uuid::Uuid,
    agent: &workflows_proto::AgentRef,
    force: bool,
) -> eyre::Result<ClaimOutcome> {
    let agent_json = serde_json::to_string(agent).map_err(|e| eyre::eyre!("encode agent: {e}"))?;
    let res = client
        .try_claim(*task_id, agent_json, force)
        .await
        .map_err(|e| eyre::eyre!("try_claim: {e:?}"))?;
    Ok(match res {
        task::service::ClaimResult::Won => ClaimOutcome::Won,
        task::service::ClaimResult::AlreadyMine => ClaimOutcome::AlreadyMine,
        task::service::ClaimResult::Lost { holder } => ClaimOutcome::Lost(holder),
    })
}

/// Apply `set-workflow` style edits to a `TaskInfo` in-place. The
/// cycle / project / blocker references arrive pre-resolved (the
/// caller ran the flexible resolvers): outer `None` = leave alone,
/// `Some(None)` = clear, `Some(Some(id))` = set.
#[allow(clippy::too_many_arguments)]
// Option<Option<_>> is exactly the tri-state these patch fields
// need (untouched / cleared / set) — a custom enum adds noise for
// one private helper.
#[allow(clippy::option_option)]
fn apply_workflow_patch(
    t: &mut task::TaskInfo,
    cycle: Option<Option<uuid::Uuid>>,
    project: Option<Option<uuid::Uuid>>,
    workstream: Option<Option<uuid::Uuid>>,
    estimate: Option<String>,
    add_assignee: Vec<workflows_proto::AgentRef>,
    remove_assignee: Vec<workflows_proto::AgentRef>,
    add_blocker: Vec<uuid::Uuid>,
    remove_blocker: Vec<uuid::Uuid>,
) -> eyre::Result<()> {
    // Project membership lives on TaskInfo.project_id (the
    // canonical Project link), not in WorkflowAttrs.
    if let Some(v) = project {
        t.project_id = v;
    }

    let w = t
        .workflow
        .get_or_insert_with(task::model::WorkflowAttrs::default);

    if let Some(v) = cycle {
        w.cycle = v;
    }
    if let Some(v) = workstream {
        w.workstream = v;
    }
    if let Some(v) = estimate {
        w.estimate = Some(parse_estimate(&v)?);
    }
    for a in remove_assignee {
        w.assignees.0.retain(|x| x != &a);
    }
    for a in add_assignee {
        if !w.assignees.iter().any(|x| x == &a) {
            w.assignees.0.push(a);
        }
    }
    for b in remove_blocker {
        w.blockers.0.retain(|x| x != &b);
    }
    for b in add_blocker {
        if !w.blockers.iter().any(|x| x == &b) {
            w.blockers.0.push(b);
        }
    }
    Ok(())
}

/// Resolve an optional `--project` filter (uuid, id prefix, path,
/// or name) into the project id, dialing the project service only
/// when the flag is present.
/// Refuse `ready-for-agent` on a ticket that resolves to no verify
/// command.
///
/// The resolution walks the ticket's own override, then the owning
/// project and its ancestors — so most tickets satisfy the gate
/// through their project's default and pass no flag at all.
///
/// Only dials the project service when the tag is actually present,
/// so ordinary `issue create` calls pay nothing for this.
async fn gate_agent_ready(
    url: &str,
    tags: &[String],
    verify: Option<&str>,
    project: Option<uuid::Uuid>,
) -> eyre::Result<()> {
    let wants_agent = tags
        .iter()
        .any(|t| task::TriageLabel::parse(t) == Some(task::TriageLabel::ReadyForAgent));
    if !wants_agent {
        return Ok(());
    }

    let projects = match connect_project_client(url).await {
        Ok(pc) => pc.list().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let resolved = project::verify::resolve(verify, project, &projects);

    task::agent_lane::check_agent_ready(resolved.as_deref()).map_err(|e| {
        eyre::eyre!(
            "refusing `{}`: {e}",
            task::TriageLabel::ReadyForAgent.as_str()
        )
    })
}

async fn resolve_project_filter(
    url: &str,
    project: Option<String>,
) -> eyre::Result<Option<uuid::Uuid>> {
    match project {
        None => Ok(None),
        Some(p) => {
            let pc = connect_project_client(url).await?;
            Ok(Some(
                crate::json_out::resolve_project_flexible(&pc, &p).await?.id,
            ))
        }
    }
}

/// Resolve an optional `--workstream` filter (uuid, id prefix,
/// path, or name) into the workstream id, dialing the workstream
/// service only when the flag is present.
async fn resolve_workstream_filter(
    url: &str,
    workstream: Option<String>,
) -> eyre::Result<Option<uuid::Uuid>> {
    match workstream {
        None => Ok(None),
        Some(w) => {
            let wc: ::workstream::WorkstreamServiceClient = establish_for_url(url).await?;
            Ok(Some(
                crate::json_out::resolve_workstream_flexible(&wc, &w)
                    .await?
                    .id,
            ))
        }
    }
}

/// Org slug + per-org vox URL from the global `--org` / `--server`
/// flags (the `issue` group dropped its per-variant duplicates).
/// Called per-arm so org-free verbs (`pr-list`, dry runs) keep
/// working without a session.
/// Refuse to send an agent-lane artifact outward.
///
/// Task may cite a GitHub issue; GitHub never cites Task. The issue
/// list stays human-authored, so anything carrying an agent-lane
/// triage label — or belonging to a workstream, which makes it part
/// of a map rather than a report someone filed — does not leave.
///
/// This is a *constraint on the existing sync*, not its removal:
/// pushing and syncing human tickets keeps working exactly as before.
/// Automatic mirroring is what produced the issue-list pollution the
/// agent lane exists to end, so the boundary is enforced rather than
/// documented.
pub(crate) fn refuse_if_agent_artifact(t: &task::TaskInfo) -> eyre::Result<()> {
    if let Some(label) = task::triage_labels(t).first() {
        return Err(eyre::eyre!(
            "refusing to push {}: it carries `{}`, an agent-lane label. \
             Agent artifacts live only in Task — remove the label first, \
             or write the GitHub issue by hand.",
            short_uuid(&t.id),
            label.as_str()
        ));
    }
    if t.workflow.as_ref().and_then(|w| w.workstream).is_some() {
        return Err(eyre::eyre!(
            "refusing to push {}: it belongs to a workstream, so it is part \
             of a wayfinding map rather than something a person reported.",
            short_uuid(&t.id)
        ));
    }
    Ok(())
}

fn issue_ctx() -> eyre::Result<(String, String)> {
    let slug = resolve_active_org(None)?;
    let url = resolve_org_vox_url(None, &slug);
    Ok((slug, url))
}

pub(crate) async fn run_issue(cmd: IssueCmd) -> eyre::Result<()> {
    match cmd {
        IssueCmd::List {
            cycle,
            project,
            assignee,
            status,
            has_workflow,
            json,
        } => {
            let (_slug, url) = issue_ctx()?;
            let cycle = resolve_cycle_arg(cycle, false)?;
            let project = resolve_project_filter(&url, project).await?;
            let client = connect_task_client(&url).await?;
            let rows = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;
            let assignee_ref = assignee.as_deref().map(parse_agent_ref).transpose()?;

            let mut rows: Vec<task::TaskInfo> = rows
                .into_iter()
                .filter(|t| {
                    status
                        .as_deref()
                        .is_none_or(|s| t.status.eq_ignore_ascii_case(s))
                })
                .filter(|t| !has_workflow || t.workflow.is_some())
                .filter(|t| match cycle {
                    None => true,
                    Some(c) => t.workflow.as_ref().and_then(|x| x.cycle) == Some(c),
                })
                .filter(|t| match project {
                    None => true,
                    Some(p) => t.project_id == Some(p),
                })
                .filter(|t| match &assignee_ref {
                    None => true,
                    Some(a) => t
                        .workflow
                        .as_ref()
                        .is_some_and(|w| w.assignees.iter().any(|x| x == a)),
                })
                .collect();
            rows.sort_by(|a, b| {
                let a_done = task::Status::from_str(&a.status).is_some_and(task::Status::is_done);
                let b_done = task::Status::from_str(&b.status).is_some_and(task::Status::is_done);
                a_done.cmp(&b_done).then_with(|| a.title.cmp(&b.title))
            });

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            if rows.is_empty() {
                println!("(no issues)");
                return Ok(());
            }
            for t in &rows {
                println!(
                    "{}  {:<10}  {:<8}  {}  {}",
                    short_uuid(&t.id),
                    t.status,
                    t.priority,
                    workflow_summary(&t.workflow),
                    t.title,
                );
            }
        }
        IssueCmd::Show { id, json } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let t = resolve_issue_id(&client, &id).await?;
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
            if let Some(p) = t.project_id {
                println!("  project:  {p}");
            }
            if let Some(m) = t.milestone_id {
                println!("  milestone:{m}");
            }
            match &t.workflow {
                Some(w) => print_workflow_block(w),
                None => println!("  workflow: (none)"),
            }
        }
        IssueCmd::Prompt { id } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let t = resolve_issue_id(&client, &id).await?;
            let parent = match t.workflow.as_ref().and_then(|w| w.parent) {
                Some(pid) => client.get(pid).await.ok(),
                None => None,
            };
            print!("{}", render_task_prompt(&t, parent.as_ref()));
        }
        IssueCmd::SetWorkflow {
            id,
            cycle,
            project,
            workstream,
            estimate,
            add_assignee,
            remove_assignee,
            add_blocker,
            remove_blocker,
            clear,
            json,
        } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let mut t = resolve_issue_id(&client, &id).await?;
            if clear {
                t.workflow = None;
            } else {
                let add: Vec<_> = add_assignee
                    .iter()
                    .map(|s| parse_agent_ref(s))
                    .collect::<eyre::Result<_>>()?;
                let rm: Vec<_> = remove_assignee
                    .iter()
                    .map(|s| parse_agent_ref(s))
                    .collect::<eyre::Result<_>>()?;
                // Resolve the entity references up-front: cycle
                // accepts uuid / label / `current` / `none`; project
                // accepts uuid / name / path / prefix / `none`;
                // blockers accept any issue reference.
                let cycle = match cycle {
                    None => None,
                    Some(c) => Some(resolve_cycle_arg(Some(c), false)?),
                };
                let project = match project.as_deref() {
                    None => None,
                    Some("" | "none" | "null") => Some(None),
                    Some(p) => Some(resolve_project_filter(&url, Some(p.to_owned())).await?),
                };
                let workstream = match workstream.as_deref() {
                    None => None,
                    Some("" | "none" | "null") => Some(None),
                    Some(w) => Some(resolve_workstream_filter(&url, Some(w.to_owned())).await?),
                };
                let mut add_b = Vec::with_capacity(add_blocker.len());
                for b in &add_blocker {
                    add_b.push(resolve_issue_id(&client, b).await?.id);
                }
                let mut rm_b = Vec::with_capacity(remove_blocker.len());
                for b in &remove_blocker {
                    rm_b.push(resolve_issue_id(&client, b).await?.id);
                }
                apply_workflow_patch(
                    &mut t, cycle, project, workstream, estimate, add, rm, add_b, rm_b,
                )?;
            }
            let updated = client
                .update(t)
                .await
                .map_err(|e| eyre::eyre!("update: {e:?}"))?;
            if json {
                crate::json_out::print_json(&updated)?;
                return Ok(());
            }
            println!("{}  [{}]  {}", updated.title, updated.status, updated.path);
            if let Some(w) = &updated.workflow {
                print_workflow_block(w);
            } else {
                println!("  workflow: (none)");
            }
        }
        IssueCmd::Claim {
            id,
            as_agent,
            force,
            json,
        } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let agent = parse_agent_ref(&format!("agent:{as_agent}"))?;
            let t = resolve_issue_id(&client, &id).await?;
            match try_claim(&client, &t.id, &agent, force).await? {
                ClaimOutcome::Won => {
                    if !json {
                        println!("claimed {} by {}", short_uuid(&t.id), agent.short_label());
                    }
                }
                ClaimOutcome::AlreadyMine => {
                    if !json {
                        println!(
                            "{} already claimed by {}",
                            short_uuid(&t.id),
                            agent.short_label()
                        );
                    }
                }
                ClaimOutcome::Lost(holder) => {
                    return Err(crate::errors::conflict("claim issue", short_uuid(&t.id))
                        .cause(format!("already claimed by {holder}"))
                        .hint("pass --force to steal the claim")
                        .report());
                }
            }
            if json {
                // Re-read so the emitted entity reflects the claim.
                let after = client
                    .get(t.id)
                    .await
                    .map_err(|e| eyre::eyre!("re-read after claim: {e:?}"))?;
                crate::json_out::print_json(&after)?;
            }
        }
        IssueCmd::Triage {
            id,
            subtasks,
            from,
            parent_status,
            priority,
        } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let mut parent = resolve_issue_id(&client, &id).await?;

            // Collect subtask titles: --subtask flags + --from lines.
            let mut titles: Vec<String> = subtasks;
            if let Some(src) = from {
                let raw = if src == "-" {
                    use std::io::Read as _;
                    let mut s = String::new();
                    std::io::stdin()
                        .read_to_string(&mut s)
                        .map_err(|e| eyre::eyre!("stdin: {e}"))?;
                    s
                } else {
                    std::fs::read_to_string(&src).map_err(|e| eyre::eyre!("read {src}: {e}"))?
                };
                titles.extend(
                    raw.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(String::from),
                );
            }
            if titles.is_empty() {
                return Err(eyre::eyre!(
                    "no subtasks — pass --subtask <title> (repeatable) and/or --from <file|->"
                ));
            }

            // Create each subtask under the parent.
            for title in &titles {
                let sub = task::TaskInfo {
                    id: uuid::Uuid::nil(),
                    path: String::new(),
                    title: title.clone(),
                    status: "open".into(),
                    priority: priority.clone(),
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
                client
                    .create(sub)
                    .await
                    .map_err(|e| eyre::eyre!("create subtask: {e:?}"))?;
            }

            // Flip the parent into the working state.
            parent.status = parent_status.clone();
            parent.completed_date = None;
            let parent_id = parent.id;
            client
                .update(parent)
                .await
                .map_err(|e| eyre::eyre!("update parent: {e:?}"))?;

            println!(
                "triaged {} into {} subtask(s) [parent → {parent_status}]\n",
                short_uuid(&parent_id),
                titles.len()
            );
            // Show the resulting board.
            let all = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;
            for t in all
                .iter()
                .filter(|t| t.workflow.as_ref().and_then(|w| w.parent) == Some(parent_id))
            {
                println!(
                    "  {}  {:<10} unclaimed   {}",
                    short_uuid(&t.id),
                    t.status,
                    t.title
                );
            }
            println!(
                "\nparallel agents now: `task issue ready --as-agent <name>` → `task issue claim <id> --as-agent <name>`"
            );
        }
        IssueCmd::Subtasks { id, json } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let parent = resolve_issue_id(&client, &id).await?;
            let all = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;
            // Derived rollup over the children — shared engine,
            // classified via each task's project state registry.
            let states = project_states_map(&url).await;
            let rollup =
                ::workstream::subtask_rollup(parent.id, &all, |t| resolve_task_group(&states, t));
            let mut subs: Vec<&task::TaskInfo> = all
                .iter()
                .filter(|t| t.workflow.as_ref().and_then(|w| w.parent) == Some(parent.id))
                .collect();
            subs.sort_by(|a, b| a.status.cmp(&b.status).then_with(|| a.title.cmp(&b.title)));
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "parent": parent.id,
                        "rollup": rollup,
                        "subtasks": subs,
                    }))
                    .map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!(
                "{} [{}]  {}",
                short_uuid(&parent.id),
                parent.status,
                parent.title
            );
            println!(
                "  {}/{} done · {} in-progress · {} blocked · {} pts\n",
                rollup.done,
                rollup.total,
                rollup.in_progress,
                rollup.blocked,
                rollup.estimate_points_sum
            );
            for t in &subs {
                let claim = t
                    .workflow
                    .as_ref()
                    .and_then(|w| w.assignees.0.first())
                    .map_or_else(
                        || "unclaimed".to_string(),
                        workflows_proto::AgentRef::short_label,
                    );
                println!(
                    "  {}  {:<12} {:<22} {}",
                    short_uuid(&t.id),
                    t.status,
                    claim,
                    t.title
                );
            }
        }
        IssueCmd::Rollup { id, json } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let parent = resolve_issue_id(&client, &id).await?;
            let all = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;
            let states = project_states_map(&url).await;
            let rollup =
                ::workstream::subtask_rollup(parent.id, &all, |t| resolve_task_group(&states, t));
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "parent": parent.id,
                        "rollup": rollup,
                    }))
                    .map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!(
                "{} [{}]  {}",
                short_uuid(&parent.id),
                parent.status,
                parent.title
            );
            println!("  done:        {}/{}", rollup.done, rollup.total);
            println!("  in-progress: {}", rollup.in_progress);
            println!("  blocked:     {}", rollup.blocked);
            println!("  points:      {}", rollup.estimate_points_sum);
            let g = &rollup.groups;
            println!(
                "  groups:      backlog {} / unstarted {} / started {} / completed {} / cancelled {}",
                g.backlog, g.unstarted, g.started, g.completed, g.cancelled
            );
            if rollup.total > 0 {
                println!(
                    "  progress:    {:.0}%",
                    f64::from(rollup.done) * 100.0 / f64::from(rollup.total)
                );
            }
        }
        IssueCmd::Relate {
            a,
            kind,
            b,
            remove,
            json,
        } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let kind = task::RelationKind::from_str(&kind).ok_or_else(|| {
                eyre::eyre!(
                    "unknown relation kind `{kind}` — one of blocks / duplicate / \
                     implements / relates"
                )
            })?;
            let mut src = resolve_issue_id(&client, &a).await?;
            let dst = resolve_issue_id(&client, &b).await?;
            if src.id == dst.id {
                return Err(eyre::eyre!("an issue can't relate to itself"));
            }
            let rel = task::Relation {
                kind,
                target: dst.id,
            };
            let w = src
                .workflow
                .get_or_insert_with(task::model::WorkflowAttrs::default);
            let already = w.relations.0.contains(&rel);
            if remove {
                if !already {
                    return Err(eyre::eyre!(
                        "no `{}` relation from {} to {}",
                        kind.as_str(),
                        short_uuid(&src.id),
                        short_uuid(&dst.id)
                    ));
                }
                w.relations.0.retain(|r| r != &rel);
            } else if !already {
                w.relations.0.push(rel);
            }
            // Relation changes ride the normal update path, so the
            // backend publishes TaskEvent::Upserted to subscribers.
            let updated = client
                .update(src)
                .await
                .map_err(|e| eyre::eyre!("update: {e:?}"))?;
            if json {
                crate::json_out::print_json(&updated)?;
                return Ok(());
            }
            let verb = if remove { "unrelated" } else { "related" };
            println!(
                "{verb}: {} ({}) —{}→ {} ({})",
                updated.title,
                short_uuid(&updated.id),
                kind.as_str(),
                dst.title,
                short_uuid(&dst.id)
            );
        }
        IssueCmd::Relations { id, json } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let t = resolve_issue_id(&client, &id).await?;
            let all = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;
            let by_id: std::collections::HashMap<uuid::Uuid, &task::TaskInfo> =
                all.iter().map(|x| (x.id, x)).collect();
            let label = |id: &uuid::Uuid| {
                by_id
                    .get(id)
                    .map_or_else(|| "(unknown)".to_string(), |x| x.title.clone())
            };
            // Outgoing from the merged local view; incoming via
            // the server's reverse index.
            let outgoing = task::relations::outgoing(t.id, &all);
            let incoming = client
                .reverse_relations(t.id)
                .await
                .map_err(|e| eyre::eyre!("reverse_relations: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "id": t.id,
                        "outgoing": outgoing,
                        "incoming": incoming,
                    }))
                    .map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{} [{}]  {}\n", short_uuid(&t.id), t.status, t.title);
            if outgoing.is_empty() && incoming.is_empty() {
                println!("  (no relations)");
                return Ok(());
            }
            if !outgoing.is_empty() {
                println!("  outgoing (this issue → other):");
                for r in &outgoing {
                    println!(
                        "    {:<11} {}  {}",
                        r.kind.as_str(),
                        short_uuid(&r.target),
                        label(&r.target)
                    );
                }
            }
            if !incoming.is_empty() {
                println!("  incoming (other → this issue):");
                for r in &incoming {
                    println!(
                        "    {:<11} {}  {}",
                        r.kind.as_str(),
                        short_uuid(&r.source),
                        label(&r.source)
                    );
                }
            }
        }
        IssueCmd::Blocking { id, json } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let t = resolve_issue_id(&client, &id).await?;
            let all = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;
            let blocked_ids = task::relations::blocking(t.id, &all);
            let by_id: std::collections::HashMap<uuid::Uuid, &task::TaskInfo> =
                all.iter().map(|x| (x.id, x)).collect();
            if json {
                let rows: Vec<&task::TaskInfo> = blocked_ids
                    .iter()
                    .filter_map(|bid| by_id.get(bid).copied())
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            if blocked_ids.is_empty() {
                println!("{} blocks nothing", short_uuid(&t.id));
                return Ok(());
            }
            println!("{} blocks:", short_uuid(&t.id));
            for bid in &blocked_ids {
                match by_id.get(bid) {
                    Some(b) => println!("  {}  {:<12} {}", short_uuid(bid), b.status, b.title),
                    None => println!("  {bid}  (unknown)"),
                }
            }
        }
        IssueCmd::Assignees { id, json } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let t = resolve_issue_id(&client, &id).await?;
            let assignees: Vec<workflows_proto::AgentRef> = t
                .workflow
                .as_ref()
                .map(|w| w.assignees.0.clone())
                .unwrap_or_default();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&assignees)
                        .map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            if assignees.is_empty() {
                println!("(no assignees)");
                return Ok(());
            }
            for a in &assignees {
                let kind = if a.is_agent() { "agent" } else { "human" };
                println!("{kind:<6}  {}", a.short_label());
            }
        }
        IssueCmd::Create {
            title,
            path,
            status,
            priority,
            cycle,
            project,
            parent,
            workstream,
            estimate,
            assignees,
            blockers,
            tags,
            verify,
            caps,
            model,
            body,
            json,
        } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let body = resolve_body(body)?;

            // Resolve entity references: cycle label / `current`,
            // project name / path / prefix, parent + blockers by
            // any issue reference.
            let cycle = resolve_cycle_arg(cycle, false)?;
            let project = resolve_project_filter(&url, project).await?;
            let workstream = resolve_workstream_filter(&url, workstream).await?;
            let parent = match parent {
                None => None,
                Some(p) => Some(resolve_issue_id(&client, &p).await?.id),
            };
            let mut blocker_ids = Vec::with_capacity(blockers.len());
            for b in &blockers {
                blocker_ids.push(resolve_issue_id(&client, b).await?.id);
            }
            let blockers = blocker_ids;

            // Build the WorkflowAttrs from inline flags. Skip if
            // nothing was set — leaves `workflow: None`, preserving
            // the TaskNotes-shape round-trip for plain tasks.
            let assignee_refs: Vec<workflows_proto::AgentRef> = assignees
                .iter()
                .map(|s| parse_agent_ref(s))
                .collect::<eyre::Result<_>>()?;
            // The agent-ready gate: a ticket tagged `ready-for-agent`
            // must resolve to a verify command, or nobody can tell
            // when an agent is done with it. Checked before the
            // create call so a refused ticket never lands.
            gate_agent_ready(&url, &tags, verify.as_deref(), project).await?;

            // A capability token that is not in the closed
            // vocabulary is a bad ticket, so reject it here rather
            // than writing work that can never route.
            agent_proto::runner::parse_capabilities(&caps)?;

            let any_workflow = cycle.is_some()
                || parent.is_some()
                || workstream.is_some()
                || estimate.is_some()
                || verify.is_some()
                || model.is_some()
                || !caps.is_empty()
                || !assignee_refs.is_empty()
                || !blockers.is_empty();
            let workflow = if any_workflow {
                let estimate = match estimate {
                    Some(e) => Some(parse_estimate(&e)?),
                    None => None,
                };
                Some(task::model::WorkflowAttrs {
                    cycle,
                    parent,
                    workstream,
                    estimate,
                    verify_command: verify,
                    capabilities: task::model::StringList(caps),
                    model,
                    assignees: task::model::AgentRefList(assignee_refs),
                    blockers: task::model::UuidList(blockers),
                    ..Default::default()
                })
            } else {
                None
            };

            let new_task = task::TaskInfo {
                id: uuid::Uuid::nil(),
                path: path.unwrap_or_default(),
                title,
                status: status.unwrap_or_else(|| "open".into()),
                priority: priority.unwrap_or_else(|| "normal".into()),
                due: None,
                scheduled: None,
                tags: task::model::StringList(tags),
                contexts: task::model::StringList::default(),
                projects: task::model::StringList::default(),
                project_id: project,
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
                details: body,
                workflow,
            };
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
                if let Some(w) = &created.workflow {
                    print_workflow_block(w);
                }
            }
        }
        IssueCmd::Ready {
            cycle,
            project,
            as_agent,
            limit,
            json,
        } => {
            let (_slug, url) = issue_ctx()?;
            let cycle = resolve_cycle_arg(cycle, false)?;
            let project = resolve_project_filter(&url, project).await?;
            let client = connect_task_client(&url).await?;
            let rows = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;

            // Index id → status so we can resolve blockers cheaply.
            let by_id: std::collections::HashMap<uuid::Uuid, &task::TaskInfo> =
                rows.iter().map(|t| (t.id, t)).collect();
            let agent_ref = as_agent
                .as_deref()
                .map(|s| parse_agent_ref(&format!("agent:{s}")))
                .transpose()?;

            let mut ready: Vec<&task::TaskInfo> = rows
                .iter()
                .filter(|t| {
                    // Status check — not done / cancelled.
                    let s = task::Status::from_str(&t.status);
                    !matches!(s, Some(task::Status::Done | task::Status::Cancelled))
                })
                .filter(|t| match cycle {
                    None => true,
                    Some(c) => t.workflow.as_ref().and_then(|x| x.cycle) == Some(c),
                })
                .filter(|t| match project {
                    None => true,
                    Some(p) => t.project_id == Some(p),
                })
                .filter(|t| match &agent_ref {
                    None => true,
                    Some(a) => {
                        // Available to this agent: either no
                        // assignees yet, or this agent is in the list.
                        let assignees = t.workflow.as_ref().map_or(&[][..], |w| &w.assignees.0[..]);
                        assignees.is_empty() || assignees.iter().any(|x| x == a)
                    }
                })
                .filter(|t| {
                    // No unresolved blockers — every blocker task
                    // must exist AND be in `done` / `cancelled`.
                    let blockers = t.workflow.as_ref().map_or(&[][..], |w| &w.blockers.0[..]);
                    blockers.iter().all(|bid| {
                        by_id.get(bid).is_some_and(|b| {
                            matches!(
                                task::Status::from_str(&b.status),
                                Some(task::Status::Done | task::Status::Cancelled)
                            )
                        })
                    })
                })
                .collect();

            // Priority desc, then title.
            ready.sort_by(|a, b| {
                let prio = |t: &task::TaskInfo| match task::Priority::from_str(&t.priority) {
                    Some(task::Priority::Critical) => 0,
                    Some(task::Priority::High) => 1,
                    Some(task::Priority::Normal) => 2,
                    Some(task::Priority::Low) => 3,
                    _ => 4,
                };
                prio(a).cmp(&prio(b)).then_with(|| a.title.cmp(&b.title))
            });
            ready.truncate(limit);

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ready).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            if ready.is_empty() {
                println!("(no ready issues)");
                return Ok(());
            }
            for t in &ready {
                println!(
                    "{}  {:<10}  {:<8}  {}  {}",
                    short_uuid(&t.id),
                    t.status,
                    t.priority,
                    workflow_summary(&t.workflow),
                    t.title,
                );
            }
        }
        IssueCmd::Start { id, as_agent, json } => {
            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let mut t = resolve_issue_id(&client, &id).await?;
            // Flip status; preserve completedDate semantics: if
            // re-opening from done, clear the date.
            t.status = "in-progress".into();
            t.completed_date = None;
            if let Some(name) = as_agent {
                let agent = parse_agent_ref(&format!("agent:{name}"))?;
                let w = t
                    .workflow
                    .get_or_insert_with(task::model::WorkflowAttrs::default);
                if !w.assignees.0.iter().any(|a| a == &agent) {
                    w.assignees.0.push(agent);
                }
            }
            let updated = client
                .update(t)
                .await
                .map_err(|e| eyre::eyre!("update: {e:?}"))?;
            if json {
                crate::json_out::print_json(&updated)?;
                return Ok(());
            }
            println!(
                "started {}  [{}]  {}",
                short_uuid(&updated.id),
                updated.status,
                updated.title
            );
            if let Some(w) = &updated.workflow {
                print_workflow_block(w);
            }
        }
        IssueCmd::ImportBeads { from, dry_run } => {
            // 1. Get the beads JSON.
            let raw = match from.as_str() {
                "bd" => {
                    let out = std::process::Command::new("bd")
                        .args(["list", "--json"])
                        .output()
                        .map_err(|e| eyre::eyre!("run `bd list --json`: {e}"))?;
                    if !out.status.success() {
                        return Err(eyre::eyre!(
                            "bd list --json failed: {}",
                            String::from_utf8_lossy(&out.stderr).trim()
                        ));
                    }
                    String::from_utf8_lossy(&out.stdout).to_string()
                }
                "-" => {
                    use std::io::Read as _;
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s)?;
                    s
                }
                path => {
                    std::fs::read_to_string(path).map_err(|e| eyre::eyre!("read {path}: {e}"))?
                }
            };

            // 2. Parse — beads `list --json` is either an array of
            //    issues or `{ "issues": [...] }`. Be lenient.
            let val: serde_json::Value =
                serde_json::from_str(&raw).map_err(|e| eyre::eyre!("parse beads json: {e}"))?;
            let items = val
                .get("issues")
                .and_then(|v| v.as_array())
                .or_else(|| val.as_array())
                .cloned()
                .ok_or_else(|| eyre::eyre!("beads json: expected an array or {{issues:[…]}}"))?;

            let map_status = |s: &str| match s.to_ascii_lowercase().as_str() {
                "closed" | "done" | "completed" => "done",
                "in_progress" | "in-progress" | "doing" => "in-progress",
                "blocked" | "waiting" => "waiting",
                _ => "open",
            };
            let map_priority = |p: &serde_json::Value| -> String {
                // beads priority is 0..4 (0=critical) or a string.
                if let Some(n) = p.as_u64() {
                    match n {
                        0 => "critical",
                        1 => "high",
                        2 => "normal",
                        3 => "low",
                        _ => "none",
                    }
                    .to_string()
                } else {
                    p.as_str().unwrap_or("normal").to_string()
                }
            };

            println!("{} beads issue(s) to import", items.len());
            if dry_run {
                for it in &items {
                    let title = it
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(untitled)");
                    let st = it.get("status").and_then(|v| v.as_str()).unwrap_or("open");
                    println!("  [{}] {title}", map_status(st));
                }
                println!("\n(dry run — nothing written)");
                return Ok(());
            }

            let (_slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let mut created = 0usize;
            for it in &items {
                let title = match it.get("title").and_then(|v| v.as_str()) {
                    Some(t) if !t.is_empty() => t.to_string(),
                    _ => continue,
                };
                let status =
                    map_status(it.get("status").and_then(|v| v.as_str()).unwrap_or("open"));
                let priority = it
                    .get("priority")
                    .map_or_else(|| "normal".to_string(), map_priority);
                let body = it
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let new_task = task::TaskInfo {
                    id: uuid::Uuid::nil(),
                    path: String::new(),
                    title,
                    status: status.into(),
                    priority,
                    due: None,
                    scheduled: None,
                    tags: task::model::StringList(vec!["task".into(), "from-beads".into()]),
                    contexts: task::model::StringList::default(),
                    projects: task::model::StringList::default(),
                    project_id: None,
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
                    details: body,
                    workflow: None,
                };
                client
                    .create(new_task)
                    .await
                    .map_err(|e| eyre::eyre!("create: {e:?}"))?;
                created += 1;
            }
            println!("imported {created} task(s) (tagged `from-beads`)");
            println!(
                "note: beads dependencies aren't mapped to blockers yet — \
                 the beads ids don't survive into TaskInfo uuids. Re-link by hand if needed."
            );
        }
        IssueCmd::Stats { project, json } => {
            use std::collections::BTreeMap;

            let (_slug, url) = issue_ctx()?;
            let project = resolve_project_filter(&url, project).await?;
            let client = connect_task_client(&url).await?;
            let rows = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;

            let mut filtered: Vec<&task::TaskInfo> = rows
                .iter()
                .filter(|t| match project {
                    None => true,
                    Some(p) => t.project_id == Some(p),
                })
                .collect();

            // Per-project state registries: status → group
            // classification routes through each task's owning
            // project (custom registries respected; tasks with
            // no project use the default registry).
            let states_by_project: std::collections::HashMap<
                uuid::Uuid,
                Option<project::StatesConfig>,
            > = match connect_project_client(&url).await {
                Ok(pc) => pc
                    .list()
                    .await
                    .map(|ps| ps.into_iter().map(|p| (p.id, p.states)).collect())
                    .unwrap_or_default(),
                Err(_) => std::collections::HashMap::new(),
            };
            let group_of = |t: &task::TaskInfo| -> project::StateGroup {
                let cfg = t
                    .project_id
                    .and_then(|pid| states_by_project.get(&pid))
                    .and_then(Option::as_ref);
                project::resolve_state_group(cfg, &t.status)
            };

            let total = filtered.len();
            let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
            let mut by_group: BTreeMap<String, usize> = BTreeMap::new();
            let mut by_priority: BTreeMap<String, usize> = BTreeMap::new();
            let mut by_project: BTreeMap<String, usize> = BTreeMap::new();
            let mut by_assignee: BTreeMap<String, usize> = BTreeMap::new();
            let mut blocked: usize = 0;
            let mut with_workflow: usize = 0;

            let by_id: std::collections::HashMap<uuid::Uuid, &task::TaskInfo> =
                rows.iter().map(|t| (t.id, t)).collect();

            for t in filtered.drain(..) {
                *by_status.entry(t.status.clone()).or_default() += 1;
                *by_group
                    .entry(group_of(t).as_str().to_string())
                    .or_default() += 1;
                *by_priority.entry(t.priority.clone()).or_default() += 1;
                let p_label = t
                    .project_id
                    .map_or_else(|| "—".to_string(), |id| short_uuid(&id));
                *by_project.entry(p_label).or_default() += 1;
                if let Some(wf) = &t.workflow {
                    with_workflow += 1;
                    for a in &wf.assignees.0 {
                        *by_assignee.entry(a.short_label()).or_default() += 1;
                    }
                    // Blocked = has at least one blocker whose
                    // state *group* isn't closed (completed /
                    // cancelled), or that we can't resolve.
                    let is_blocked = wf
                        .blockers
                        .0
                        .iter()
                        .any(|bid| by_id.get(bid).is_none_or(|b| !group_of(b).is_closed()));
                    if is_blocked {
                        blocked += 1;
                    }
                }
            }

            if json {
                let payload = serde_json::json!({
                    "total": total,
                    "with_workflow": with_workflow,
                    "blocked": blocked,
                    "by_status": by_status,
                    "by_group": by_group,
                    "by_priority": by_priority,
                    "by_project": by_project,
                    "by_assignee": by_assignee,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }

            println!("total:        {total}");
            println!("with workflow: {with_workflow}");
            println!("blocked:      {blocked}");
            println!();
            println!("by status:");
            for (k, v) in &by_status {
                println!("  {k:<14} {v}");
            }
            println!();
            println!("by group:");
            for (k, v) in &by_group {
                println!("  {k:<14} {v}");
            }
            println!();
            println!("by priority:");
            for (k, v) in &by_priority {
                println!("  {k:<14} {v}");
            }
            if !by_project.is_empty() {
                println!();
                println!("by project:");
                for (k, v) in &by_project {
                    println!("  {k:<14} {v}");
                }
            }
            if !by_assignee.is_empty() {
                println!();
                println!("by assignee:");
                for (k, v) in &by_assignee {
                    println!("  {k:<28} {v}");
                }
            }
        }
        IssueCmd::Close { id, undo, json } => {
            let (slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let mut t = resolve_issue_id(&client, &id).await?;
            if undo {
                t.status = "open".into();
                t.completed_date = None;
            } else {
                t.status = "done".into();
                t.completed_date = Some(chrono::Local::now().date_naive());
            }
            // Clear the active session pointer on close — work is
            // over; resume of this task starts a new session.
            if let Some(w) = t.workflow.as_mut() {
                w.session = None;
            }
            let updated = client
                .update(t)
                .await
                .map_err(|e| eyre::eyre!("update: {e:?}"))?;
            if json {
                crate::json_out::print_json(&updated)?;
            } else {
                let verb = if undo { "reopened" } else { "closed" };
                println!(
                    "{verb} {}  [{}]  {}",
                    short_uuid(&updated.id),
                    updated.status,
                    updated.title
                );
            }

            // Propagate to any linked forge issues. Best-effort:
            // a forge that's unreachable / unauthenticated logs
            // a warning but doesn't fail the local close. Under
            // --json the note goes to stderr so stdout stays a
            // single parseable entity.
            let new_state = if undo {
                git_proto::IssueState::Open
            } else {
                git_proto::IssueState::Closed
            };
            match propagate_state_to_forge(&slug, &updated.id, new_state).await {
                Ok(0) => {}
                Ok(n) if json => eprintln!("propagated to {n} linked forge issue(s)"),
                Ok(n) => println!("  propagated to {n} linked forge issue(s)"),
                Err(e) => eprintln!("  warning: forge propagation failed: {e}"),
            }
        }
        IssueCmd::LinkForge {
            id,
            repo,
            number,
            base_url,
            kind,
        } => {
            let (slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let t = resolve_issue_id(&client, &id).await?;
            let (owner, repo_name) = parse_repo_slug(&repo)?;
            let base = forgejo_base_url(base_url)?;
            let link_kind = match kind.to_ascii_lowercase().as_str() {
                "issue" => git_config::LinkKind::Issue,
                "pull" | "pr" => git_config::LinkKind::Pull,
                _ => return Err(eyre::eyre!("--kind must be `issue` or `pull`")),
            };
            let store = forge_link_store(&slug)?;
            use git_config::BindingStore as _;
            store
                .add_issue_link(git_config::IssueLink {
                    task_id: t.id.to_string(),
                    repo: git_proto::RepoId {
                        forge: git_proto::Forge::Forgejo { base_url: base },
                        owner,
                        repo: repo_name,
                    },
                    number,
                    kind: link_kind,
                })
                .map_err(|e| eyre::eyre!("link store: {e}"))?;
            println!(
                "linked {} -> {}#{} ({:?})",
                short_uuid(&t.id),
                repo,
                number,
                link_kind,
            );
        }
        IssueCmd::Push {
            id,
            repo,
            github,
            base_url,
        } => {
            let (slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let t = resolve_issue_id(&client, &id).await?;
            refuse_if_agent_artifact(&t)?;
            let repo_id = build_repo_id(&repo, github, base_url)?;

            // Skip if we already have a link to this repo.
            let store = forge_link_store(&slug)?;
            use git_config::BindingStore as _;
            let existing = store
                .issues_for_task(&t.id.to_string())
                .map_err(|e| eyre::eyre!("link store: {e}"))?;
            if let Some(l) = existing.iter().find(|l| l.repo == repo_id) {
                println!(
                    "already linked: {} -> {}/{}#{} ({:?})",
                    short_uuid(&t.id),
                    l.repo.owner,
                    l.repo.repo,
                    l.number,
                    l.kind,
                );
                return Ok(());
            }

            // `IssueTracker` methods are sync but internally `block_on`
            // their HTTP call — we're inside tokio::main, so push them
            // onto the blocking pool to avoid the runtime-in-runtime
            // panic. `forge_backend_for` picks Forgejo/GitHub + token.
            let repo_c = repo_id.clone();
            let title = t.title.clone();
            let body = t.details.clone();
            let created = tokio::task::spawn_blocking(move || {
                let backend = forge_backend_for(&repo_c)?;
                backend
                    .create_issue(&repo_c, title, body)
                    .map_err(|e| eyre::eyre!("create_issue: {e:?}"))
            })
            .await
            .map_err(|e| eyre::eyre!("join: {e}"))??;
            store
                .add_issue_link(git_config::IssueLink {
                    task_id: t.id.to_string(),
                    repo: repo_id.clone(),
                    number: created.id.0,
                    kind: git_config::LinkKind::Issue,
                })
                .map_err(|e| eyre::eyre!("link store: {e}"))?;
            println!(
                "pushed {} -> {}#{}: {}",
                short_uuid(&t.id),
                repo,
                created.id.0,
                created.title,
            );
        }
        IssueCmd::Pull {
            repo,
            github,
            base_url,
            project,
            state,
        } => {
            let (slug, url) = issue_ctx()?;
            let project = resolve_project_filter(&url, project).await?;
            let client = connect_task_client(&url).await?;
            let repo_id = build_repo_id(&repo, github, base_url)?;
            let filter_state = match state.to_ascii_lowercase().as_str() {
                "open" => Some(git_proto::IssueState::Open),
                "closed" => Some(git_proto::IssueState::Closed),
                "all" => None,
                _ => return Err(eyre::eyre!("--state must be `open`, `closed`, or `all`")),
            };

            let filter = git_proto::issues::IssueFilter {
                state: filter_state,
                ..Default::default()
            };
            let repo_c = repo_id.clone();
            let issues = tokio::task::spawn_blocking(move || {
                let backend = forge_backend_for(&repo_c)?;
                backend
                    .list_issues(&repo_c, filter)
                    .map_err(|e| eyre::eyre!("list_issues: {e:?}"))
            })
            .await
            .map_err(|e| eyre::eyre!("join: {e}"))??;

            let store = forge_link_store(&slug)?;
            use git_config::BindingStore as _;
            let mut created_n = 0usize;
            let mut skipped_n = 0usize;

            for ext in issues {
                let already = store
                    .tasks_for_issue(&repo_id, ext.id.0)
                    .map_err(|e| eyre::eyre!("link store: {e}"))?;
                if !already.is_empty() {
                    skipped_n += 1;
                    continue;
                }
                // Translate forge issue → TaskInfo (status derived
                // from the forge state inside build_pulled_task).
                let mut new_task = build_pulled_task(&ext, None::<task::model::WorkflowAttrs>);
                new_task.project_id = project;
                let created = client
                    .create(new_task)
                    .await
                    .map_err(|e| eyre::eyre!("create: {e:?}"))?;
                store
                    .add_issue_link(git_config::IssueLink {
                        task_id: created.id.to_string(),
                        repo: repo_id.clone(),
                        number: ext.id.0,
                        kind: git_config::LinkKind::Issue,
                    })
                    .map_err(|e| eyre::eyre!("link store: {e}"))?;
                created_n += 1;
                println!(
                    "pulled {}#{}: {}  -> {}",
                    repo,
                    ext.id.0,
                    ext.title,
                    short_uuid(&created.id),
                );
            }
            println!("\n{created_n} new, {skipped_n} already linked");
        }
        IssueCmd::Sync {
            repo,
            github,
            base_url,
            project,
            no_pull,
        } => {
            let (slug, url) = issue_ctx()?;
            let project = resolve_project_filter(&url, project).await?;
            let client = connect_task_client(&url).await?;
            let repo_id = build_repo_id(&repo, github, base_url)?;
            let store = forge_link_store(&slug)?;
            let (reconciled, pulled) =
                sync_repo(&client, &store, &repo_id, project, no_pull).await?;
            println!("\nsync: {reconciled} reconciled, {pulled} pulled");
        }
        IssueCmd::SyncAll { project, no_pull } => {
            let (slug, url) = issue_ctx()?;
            let project = resolve_project_filter(&url, project).await?;
            let client = connect_task_client(&url).await?;
            let store = forge_link_store(&slug)?;
            let repos = store
                .distinct_repos()
                .map_err(|e| eyre::eyre!("link store: {e}"))?;
            if repos.is_empty() {
                println!("(no linked repos — run `task issue push` or `task setup forge` first)");
                return Ok(());
            }
            let mut total_r = 0usize;
            let mut total_p = 0usize;
            for repo_id in &repos {
                let label = format!("{}/{}", repo_id.owner, repo_id.repo);
                println!("=== {label} ===");
                match sync_repo(&client, &store, repo_id, project, no_pull).await {
                    Ok((r, p)) => {
                        total_r += r;
                        total_p += p;
                    }
                    // One unreachable / unauthorized repo shouldn't
                    // abort the whole sweep.
                    Err(e) => eprintln!("  skipped {label}: {e}"),
                }
            }
            println!(
                "\nsync-all: {} repo(s), {total_r} reconciled, {total_p} pulled",
                repos.len()
            );
        }
        IssueCmd::PrList {
            repo,
            github,
            base_url,
            json,
        } => {
            let repo_id = build_repo_id(&repo, github, base_url)?;
            let repo_c = repo_id.clone();
            let prs = tokio::task::spawn_blocking(move || {
                let backend = forge_backend_for(&repo_c)?;
                backend
                    .list_pull_requests(&repo_c)
                    .map_err(|e| eyre::eyre!("list_pull_requests: {e:?}"))
            })
            .await
            .map_err(|e| eyre::eyre!("join: {e}"))??;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&prs).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            if prs.is_empty() {
                println!("(no open PRs)");
                return Ok(());
            }
            for pr in &prs {
                println!(
                    "#{:<5} [{:?}] {} ({} <- {})",
                    pr.id.0, pr.state, pr.title, pr.base, pr.head
                );
            }
        }
        IssueCmd::PrCreate {
            repo,
            github,
            base_url,
            title,
            head,
            base,
            body,
            draft,
            closes,
            close_task,
        } => {
            let repo_id = build_repo_id(&repo, github, base_url)?;
            let mut body = resolve_body(body)?;

            // Resolve the issue number this PR should close:
            // explicit --closes wins; else look up the linked
            // forge issue for --close-task. Capture the task id
            // so we can record a PR link afterward.
            let (slug, url) = issue_ctx()?;
            let store = forge_link_store(&slug)?;
            use git_config::BindingStore as _;
            let mut closes_number = closes;
            let mut linked_task: Option<uuid::Uuid> = None;
            if let Some(ref tid) = close_task {
                let client = connect_task_client(&url).await?;
                let t = resolve_issue_id(&client, tid).await?;
                linked_task = Some(t.id);
                let links = store
                    .issues_for_task(&t.id.to_string())
                    .map_err(|e| eyre::eyre!("link store: {e}"))?;
                match links
                    .iter()
                    .find(|l| l.repo == repo_id && l.kind == git_config::LinkKind::Issue)
                {
                    Some(l) => closes_number = Some(l.number),
                    None => {
                        return Err(eyre::eyre!(
                            "task {} has no linked issue on {}/{} — push it first (task issue push)",
                            short_uuid(&t.id),
                            repo_id.owner,
                            repo_id.repo
                        ));
                    }
                }
            }

            // Inject the forge's close-on-merge keyword if not
            // already present.
            if let Some(n) = closes_number {
                let kw = format!("Closes #{n}");
                if !body.contains(&kw) {
                    if body.is_empty() {
                        body = kw.clone();
                    } else {
                        body = format!("{body}\n\n{kw}");
                    }
                }
            }

            let repo_c = repo_id.clone();
            let pr = tokio::task::spawn_blocking(move || {
                let backend = forge_backend_for(&repo_c)?;
                let new = git_proto::reviews::NewPullRequest {
                    title,
                    body,
                    base,
                    head,
                    draft,
                };
                backend
                    .create_pull_request(&repo_c, new)
                    .map_err(|e| eyre::eyre!("create_pull_request: {e:?}"))
            })
            .await
            .map_err(|e| eyre::eyre!("join: {e}"))??;

            // Record a PR-kind link on the task so pr-merge/sync
            // can finish the loop.
            if let Some(tid) = linked_task {
                store
                    .add_issue_link(git_config::IssueLink {
                        task_id: tid.to_string(),
                        repo: repo_id.clone(),
                        number: pr.id.0,
                        kind: git_config::LinkKind::Pull,
                    })
                    .map_err(|e| eyre::eyre!("link store: {e}"))?;
            }

            println!(
                "opened PR #{}: {} ({} <- {})",
                pr.id.0, pr.title, pr.base, pr.head
            );
            if let Some(n) = closes_number {
                println!("  will close #{n} on merge (Closes #{n} in body)");
            }
        }
        IssueCmd::PrMerge {
            repo,
            github,
            base_url,
            number,
            method,
            close_task,
        } => {
            let repo_id = build_repo_id(&repo, github, base_url)?;
            let merge_method = match method.to_ascii_lowercase().as_str() {
                "merge" => git_proto::reviews::MergeMethod::Merge,
                "squash" => git_proto::reviews::MergeMethod::Squash,
                "rebase" => git_proto::reviews::MergeMethod::Rebase,
                _ => return Err(eyre::eyre!("--method must be merge, squash, or rebase")),
            };
            let repo_c = repo_id.clone();
            let sha = tokio::task::spawn_blocking(move || {
                let backend = forge_backend_for(&repo_c)?;
                backend
                    .merge_pull_request(&repo_c, git_proto::PullRequestId(number), merge_method)
                    .map_err(|e| eyre::eyre!("merge_pull_request: {e:?}"))
            })
            .await
            .map_err(|e| eyre::eyre!("join: {e}"))??;
            match sha {
                Some(s) => println!("merged PR #{number} ({s})"),
                None => println!("merged PR #{number}"),
            }

            // The `task code merge` chain: close the linked task,
            // which propagates the close to its own forge issue.
            if let Some(tid) = close_task {
                let (slug, url) = issue_ctx()?;
                let client = connect_task_client(&url).await?;
                let mut t = resolve_issue_id(&client, &tid).await?;
                t.status = "done".into();
                t.completed_date = Some(chrono::Local::now().date_naive());
                if let Some(w) = t.workflow.as_mut() {
                    w.session = None;
                }
                let updated = client
                    .update(t)
                    .await
                    .map_err(|e| eyre::eyre!("update: {e:?}"))?;
                println!(
                    "  closed task {} ({})",
                    short_uuid(&updated.id),
                    updated.title
                );
                match propagate_state_to_forge(&slug, &updated.id, git_proto::IssueState::Closed)
                    .await
                {
                    Ok(0) => {}
                    Ok(n) => println!("  propagated close to {n} linked forge issue(s)"),
                    Err(e) => eprintln!("  warning: forge propagation failed: {e}"),
                }
            }
        }
        IssueCmd::MergeQueue {
            repo,
            github,
            base_url,
            method,
            issue,
            dry_run,
            keep_going,
        } => {
            use git_config::BindingStore as _;
            let repo_id = build_repo_id(&repo, github, base_url)?;
            let merge_method = match method.to_ascii_lowercase().as_str() {
                "merge" => git_proto::reviews::MergeMethod::Merge,
                "squash" => git_proto::reviews::MergeMethod::Squash,
                "rebase" => git_proto::reviews::MergeMethod::Rebase,
                _ => return Err(eyre::eyre!("--method must be merge, squash, or rebase")),
            };
            let (slug, url) = issue_ctx()?;
            let client = connect_task_client(&url).await?;
            let store = forge_link_store(&slug)?;

            // Map PR number → task id for this repo, via the link
            // store (Pull-kind links). Lets us close each PR's task
            // as it lands, and scope the queue to one issue.
            let tasks = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;
            let mut pr_task: std::collections::HashMap<u64, uuid::Uuid> =
                std::collections::HashMap::new();
            for t in &tasks {
                let links = store
                    .issues_for_task(&t.id.to_string())
                    .map_err(|e| eyre::eyre!("link store: {e}"))?;
                for l in links {
                    if l.repo == repo_id && l.kind == git_config::LinkKind::Pull {
                        pr_task.insert(l.number, t.id);
                    }
                }
            }

            // When scoped to an issue, the eligible PRs are those
            // linked to its subtasks (tasks whose workflow.parent is
            // the issue) — plus the issue itself.
            let scope: Option<std::collections::HashSet<uuid::Uuid>> = match &issue {
                None => None,
                Some(r) => {
                    let parent = resolve_issue_id(&client, r).await?;
                    let mut set: std::collections::HashSet<uuid::Uuid> =
                        std::iter::once(parent.id).collect();
                    for t in &tasks {
                        if t.workflow.as_ref().and_then(|w| w.parent) == Some(parent.id) {
                            set.insert(t.id);
                        }
                    }
                    Some(set)
                }
            };

            // Open, non-draft PRs, oldest first (PR number order).
            let repo_c = repo_id.clone();
            let mut prs = tokio::task::spawn_blocking(move || {
                let backend = forge_backend_for(&repo_c)?;
                backend
                    .list_pull_requests(&repo_c)
                    .map_err(|e| eyre::eyre!("list_pull_requests: {e:?}"))
            })
            .await
            .map_err(|e| eyre::eyre!("join: {e}"))??;
            prs.retain(|pr| {
                matches!(pr.state, git_proto::PullRequestState::Open)
                    && !pr.draft
                    && match &scope {
                        None => true,
                        Some(set) => pr_task.get(&pr.id.0).is_some_and(|tid| set.contains(tid)),
                    }
            });
            prs.sort_by_key(|pr| pr.id.0);

            if prs.is_empty() {
                println!("(no mergeable PRs in the queue)");
                return Ok(());
            }
            println!("merge queue: {} PR(s) on {}", prs.len(), repo);
            for pr in &prs {
                let who = pr_task
                    .get(&pr.id.0)
                    .map(|t| format!("  → task {}", short_uuid(t)))
                    .unwrap_or_default();
                println!(
                    "  #{:<5} {} ({} <- {}){who}",
                    pr.id.0, pr.title, pr.base, pr.head
                );
            }
            if dry_run {
                println!("\n(dry run — nothing merged; method would be {method})");
                return Ok(());
            }

            let mut merged = 0usize;
            for pr in &prs {
                let number = pr.id.0;
                let repo_c = repo_id.clone();
                let res = tokio::task::spawn_blocking(move || {
                    let backend = forge_backend_for(&repo_c)?;
                    backend
                        .merge_pull_request(&repo_c, git_proto::PullRequestId(number), merge_method)
                        .map_err(|e| eyre::eyre!("merge #{number}: {e:?}"))
                })
                .await
                .map_err(|e| eyre::eyre!("join: {e}"))?;
                match res {
                    Ok(sha) => {
                        merged += 1;
                        match sha {
                            Some(s) => println!("✓ merged #{number} ({s})"),
                            None => println!("✓ merged #{number}"),
                        }
                        // Close the PR's linked task + propagate.
                        if let Some(tid) = pr_task.get(&number) {
                            if let Ok(mut t) = client.get(*tid).await {
                                t.status = "done".into();
                                t.completed_date = Some(chrono::Local::now().date_naive());
                                if let Some(w) = t.workflow.as_mut() {
                                    w.session = None;
                                }
                                if client.update(t).await.is_ok() {
                                    println!("    closed task {}", short_uuid(tid));
                                    let _ = propagate_state_to_forge(
                                        &slug,
                                        tid,
                                        git_proto::IssueState::Closed,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        println!("✗ #{number} did not merge: {e}");
                        if keep_going {
                            println!("    (--keep-going: continuing)");
                        } else {
                            println!(
                                "    stopping — rebase #{number} onto {} and re-run the queue",
                                pr.base
                            );
                            break;
                        }
                    }
                }
            }
            println!("\nmerged {merged}/{} queued PR(s)", prs.len());
        }
    }
    Ok(())
}

/// Reconcile one repo's linked tasks against the forge, then
/// pull new issues. Returns `(reconciled, pulled)`. Shared by
/// `task issue sync` and `sync-all`.
async fn sync_repo(
    client: &task::TaskServiceClient,
    store: &git_config::FileStore,
    repo_id: &git_proto::RepoId,
    project: Option<uuid::Uuid>,
    no_pull: bool,
) -> eyre::Result<(usize, usize)> {
    use git_config::BindingStore as _;

    // 1. Reconcile already-linked issues with the per-field resolver
    //    in `git_config::sync` over the scalar forge-owned projection
    //    (title / body / state). Provenance is a value-diff against
    //    the last-converged snapshot the store persists per link, so
    //    a forge edit lands locally while a local-only edit to a
    //    forge-owned field isn't clobbered, and Task-owned fields
    //    (priority/cycle/project/estimate/agent-attribution) are never
    //    in the projection so they always survive a forge edit.
    //
    //    Both directions are wired: forge→Task below, and Task→forge
    //    (pushing task-won forge-owned edits via `update_issue`) after
    //    the merge. FUTURE (issue #127 follow-up): extend the
    //    projection past the scalar fields to labels / assignees /
    //    milestone with their richer mapping.
    let local = client
        .list()
        .await
        .map_err(|e| eyre::eyre!("list: {e:?}"))?;
    let mut reconciled = 0usize;
    for t in &local {
        // The boundary, enforced where the sweep runs rather than
        // only at `push`: a sync over a whole repo must not carry
        // agent artifacts outward just because someone linked one by
        // hand. Skipping (not erroring) keeps the sweep useful for
        // every human ticket alongside it.
        if refuse_if_agent_artifact(t).is_err() {
            continue;
        }
        let links = store
            .issues_for_task(&t.id.to_string())
            .map_err(|e| eyre::eyre!("link store: {e}"))?;
        // Only reconcile *issue* links. A task also linked to its own
        // PR (via `task code push`) must not have its title/body/state
        // synced from that PR — the PR's title is a commit subject, not
        // the issue's.
        let Some(link) = links
            .iter()
            .find(|l| &l.repo == repo_id && l.kind == git_config::LinkKind::Issue)
        else {
            continue;
        };
        let number = link.number;
        let repo_c = repo_id.clone();
        let ext = tokio::task::spawn_blocking(move || {
            let backend = forge_backend_for(&repo_c)?;
            backend
                .get_issue(&repo_c, git_proto::IssueId(number))
                .map_err(|e| eyre::eyre!("get_issue #{number}: {e:?}"))
        })
        .await
        .map_err(|e| eyre::eyre!("join: {e}"))??;

        // Trim title/body on both sides: the task-note parser strips
        // leading/trailing whitespace from the markdown body, so a
        // freshly-pulled `t.details` is never byte-identical to the
        // forge body (which often carries a leading newline). Comparing
        // trimmed values keeps that cosmetic difference from looking
        // like a real edit and churning the sync every run.
        let task_proj = git_config::SyncedFields {
            title: t.title.trim().to_string(),
            body: t.details.trim().to_string(),
            closed: t.status == "done",
        };
        let forge_proj = git_config::SyncedFields {
            title: ext.title.trim().to_string(),
            body: ext.body.trim().to_string(),
            closed: matches!(ext.state, git_proto::IssueState::Closed),
        };
        // The forge issue's last-update time (parsed from the DTO's
        // RFC-3339 string) and the recorded snapshot.
        let forge_ts = ext
            .updated_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc));
        let snap = store
            .get_issue_snapshot(repo_id, number)
            .map_err(|e| eyre::eyre!("snapshot: {e}"))?;

        // Fast-path: when the forge's `updated_at` hasn't advanced past
        // the snapshot and the Task projection is unchanged, neither
        // side moved — skip the diff + writes for this issue entirely.
        if let (Some(s), Some(cur)) = (snap.as_ref(), forge_ts) {
            if s.forge_updated_at == Some(cur) && s.task == task_proj {
                continue;
            }
        }

        // Baseline: the recorded snapshot, or — on the first reconcile
        // of this link — the *task* projection on both sides. That way
        // a field the forge disagrees on registers as a forge-side
        // change (forge != baseline, task == baseline) and the
        // substrate wins for its owned fields, preserving the prior
        // "forge wins for state" behaviour; a freshly-pulled task
        // already equals the forge, so this is a no-op that just seeds
        // the snapshot.
        let (base_task, base_forge) = match &snap {
            Some(s) => (s.task.clone(), s.forge.clone()),
            None => (task_proj.clone(), task_proj.clone()),
        };
        let merged = git_config::reconcile_synced(&base_task, &base_forge, &task_proj, &forge_proj);

        if merged != task_proj {
            let mut t2 = t.clone();
            t2.title = merged.title.clone();
            t2.details = merged.body.clone();
            // State projection: only cross the done boundary, leaving
            // non-done statuses (in-progress/waiting/…) intact.
            let was_done = t2.status == "done";
            if merged.closed && !was_done {
                t2.status = "done".into();
                t2.completed_date = Some(chrono::Local::now().date_naive());
                if let Some(w) = t2.workflow.as_mut() {
                    w.session = None;
                }
            } else if !merged.closed && was_done {
                t2.status = "open".into();
                t2.completed_date = None;
            }
            client
                .update(t2)
                .await
                .map_err(|e| eyre::eyre!("update: {e:?}"))?;
            reconciled += 1;
            println!("  reconciled {} #{number}", short_uuid(&t.id));
        }

        // Push the Task→forge half: any forge-owned field the Task won
        // (a local edit the forge hadn't moved) is written back via
        // `update_issue` so both sides converge. On success the forge
        // baseline advances to *what the forge returned* (not what we
        // sent), so any server-side normalization doesn't ping-pong;
        // on failure we warn and leave the baseline at `forge_proj` so
        // the next sync retries.
        let fu = git_config::forge_update(&forge_proj, &merged);
        let (forge_base, forge_ts_after) = if fu.is_empty() {
            (forge_proj, forge_ts)
        } else {
            let repo_c = repo_id.clone();
            let update = git_proto::issues::IssueUpdate {
                title: fu.title,
                body: fu.body,
                state: fu.closed.map(|c| {
                    if c {
                        git_proto::IssueState::Closed
                    } else {
                        git_proto::IssueState::Open
                    }
                }),
                labels: None,
                assignees: None,
                milestone: None,
            };
            let pushed = tokio::task::spawn_blocking(move || {
                let backend = forge_backend_for(&repo_c)?;
                backend
                    .update_issue(&repo_c, git_proto::IssueId(number), update)
                    .map_err(|e| eyre::eyre!("update_issue #{number}: {e:?}"))
            })
            .await
            .map_err(|e| eyre::eyre!("join: {e}"))?;
            match pushed {
                Ok(issue) => {
                    println!("  pushed {} -> #{number}", short_uuid(&t.id));
                    // The forge bumped its `updated_at` on this write —
                    // record what it returned so the next sync's
                    // fast-path sees the new baseline, not a stale one.
                    let ts = issue
                        .updated_at
                        .as_deref()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&chrono::Utc));
                    let fields = git_config::SyncedFields {
                        title: issue.title,
                        body: issue.body,
                        closed: matches!(issue.state, git_proto::IssueState::Closed),
                    };
                    (fields, ts)
                }
                Err(e) => {
                    eprintln!("  warn: push to #{number} failed: {e}");
                    (forge_proj, forge_ts)
                }
            }
        };

        // Record the new baseline: Task holds `merged`, forge holds
        // `forge_base` (== `merged` after a successful push, else its
        // pre-push value pending a retry), with the forge's
        // `updated_at` so the next run's fast-path can short-circuit.
        store
            .set_issue_snapshot(
                repo_id,
                number,
                git_config::IssueSnapshot {
                    task: merged,
                    forge: forge_base,
                    forge_updated_at: forge_ts_after,
                },
            )
            .map_err(|e| eyre::eyre!("snapshot: {e}"))?;
    }

    // 2. Pull new forge issues (unless suppressed).
    let mut pulled = 0usize;
    if !no_pull {
        let repo_c = repo_id.clone();
        let issues = tokio::task::spawn_blocking(move || {
            let backend = forge_backend_for(&repo_c)?;
            let filter = git_proto::issues::IssueFilter {
                state: Some(git_proto::IssueState::Open),
                ..Default::default()
            };
            backend
                .list_issues(&repo_c, filter)
                .map_err(|e| eyre::eyre!("list_issues: {e:?}"))
        })
        .await
        .map_err(|e| eyre::eyre!("join: {e}"))??;
        for ext in issues {
            let already = store
                .tasks_for_issue(repo_id, ext.id.0)
                .map_err(|e| eyre::eyre!("link store: {e}"))?;
            if !already.is_empty() {
                continue;
            }
            let mut new_task = build_pulled_task(&ext, None::<task::model::WorkflowAttrs>);
            new_task.project_id = project;
            let created = client
                .create(new_task)
                .await
                .map_err(|e| eyre::eyre!("create: {e:?}"))?;
            store
                .add_issue_link(git_config::IssueLink {
                    task_id: created.id.to_string(),
                    repo: repo_id.clone(),
                    number: ext.id.0,
                    kind: git_config::LinkKind::Issue,
                })
                .map_err(|e| eyre::eyre!("link store: {e}"))?;
            pulled += 1;
            println!(
                "  pulled {} #{}: {}",
                short_uuid(&created.id),
                ext.id.0,
                ext.title
            );
        }
    }
    Ok((reconciled, pulled))
}

/// Build a `TaskInfo` from a forge issue (used by pull + sync).
fn build_pulled_task(
    ext: &git_proto::issues::Issue,
    workflow: Option<task::model::WorkflowAttrs>,
) -> task::TaskInfo {
    let status = match ext.state {
        git_proto::IssueState::Open => "open",
        git_proto::IssueState::Closed => "done",
    };
    task::TaskInfo {
        id: uuid::Uuid::nil(),
        path: String::new(),
        title: ext.title.clone(),
        status: status.into(),
        priority: "normal".into(),
        due: None,
        scheduled: None,
        tags: task::model::StringList(vec!["task".into(), "from-forge".into()]),
        contexts: task::model::StringList::default(),
        projects: task::model::StringList::default(),
        project_id: None,
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
        details: ext.body.clone(),
        workflow,
    }
}

/// Push an `Open`/`Closed` state change to every forge issue
/// linked to `task_id`. Returns the number of links touched.
///
/// Best-effort per-link: a forge that's unreachable or that we
/// have no token for is logged + skipped, not propagated as an
/// error. The local close already landed; one flaky forge
/// shouldn't break it.
async fn propagate_state_to_forge(
    org_slug: &str,
    task_id: &uuid::Uuid,
    new_state: git_proto::IssueState,
) -> eyre::Result<usize> {
    use git_config::BindingStore as _;
    let store = forge_link_store(org_slug)?;
    let links = store
        .issues_for_task(&task_id.to_string())
        .map_err(|e| eyre::eyre!("link store: {e}"))?;
    if links.is_empty() {
        return Ok(0);
    }
    let mut touched = 0usize;
    for link in links {
        // Only issue links get state-propagated. PR links exist
        // for traceability, but "closing" a PR is a different
        // operation than closing an issue (and a merged PR is
        // already closed) — never route a PR number through
        // update_issue.
        if link.kind != git_config::LinkKind::Issue {
            continue;
        }
        let repo_c = link.repo.clone();
        let number = link.number;
        // Best-effort: a missing token for this forge family is a
        // skip-with-warning, not a hard error.
        let result = tokio::task::spawn_blocking(move || {
            let backend = forge_backend_for(&repo_c)?;
            let update = git_proto::issues::IssueUpdate {
                state: Some(new_state),
                ..Default::default()
            };
            backend
                .update_issue(&repo_c, git_proto::IssueId(number), update)
                .map_err(|e| eyre::eyre!("update_issue: {e:?}"))
        })
        .await
        .map_err(|e| eyre::eyre!("join: {e}"))?;
        match result {
            Ok(_) => touched += 1,
            Err(e) => eprintln!(
                "  skipping {}/{}#{}: {e}",
                link.repo.owner, link.repo.repo, link.number
            ),
        }
    }
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_agent_ref_handles_the_three_forms() {
        // Bare name = an agent; `agent:` is the explicit spelling.
        assert_eq!(parse_agent_ref("claude").unwrap(), parse_agent_ref("agent:claude").unwrap());
        assert_eq!(
            parse_agent_ref("claude@2").unwrap(),
            workflows_proto::AgentRef::agent_versioned("claude", "2")
        );
        assert_eq!(
            parse_agent_ref("human:u-123").unwrap(),
            workflows_proto::AgentRef::human("u-123")
        );
    }

    #[test]
    fn parse_agent_ref_tolerates_surrounding_whitespace() {
        assert_eq!(
            parse_agent_ref("  agent: claude @ 2 ").unwrap(),
            workflows_proto::AgentRef::agent_versioned("claude", "2")
        );
        assert_eq!(
            parse_agent_ref("human:  u-123  ").unwrap(),
            workflows_proto::AgentRef::human("u-123")
        );
    }

    #[test]
    fn parse_agent_ref_drops_an_empty_version_rather_than_recording_one() {
        // `claude@` must not become a versioned ref pinned to "".
        assert_eq!(
            parse_agent_ref("claude@").unwrap(),
            workflows_proto::AgentRef::agent("claude")
        );
    }

    #[test]
    fn parse_agent_ref_rejects_refs_with_no_subject() {
        for bad in ["", "   ", "human:", "human:   ", "agent:", "@2"] {
            assert!(parse_agent_ref(bad).is_err(), "`{bad}` should not parse");
        }
    }

    #[test]
    fn parse_estimate_accepts_shirt_sizes_and_points() {
        use task::model::Estimate;
        assert!(matches!(parse_estimate("xs").unwrap(), Estimate::XS));
        assert!(matches!(parse_estimate("S").unwrap(), Estimate::S));
        assert!(matches!(parse_estimate(" m ").unwrap(), Estimate::M));
        assert!(matches!(parse_estimate("L").unwrap(), Estimate::L));
        assert!(matches!(parse_estimate("XL").unwrap(), Estimate::XL));
        assert!(matches!(
            parse_estimate("8").unwrap(),
            Estimate::Points { value: 8 }
        ));
        assert!(matches!(
            parse_estimate("0").unwrap(),
            Estimate::Points { value: 0 }
        ));
    }

    #[test]
    fn parse_estimate_rejects_values_it_cannot_represent() {
        // Points are a `u8`; anything else must surface as an error
        // rather than silently wrapping or defaulting.
        for bad in ["", "xxl", "-1", "256", "3.5", "medium"] {
            assert!(parse_estimate(bad).is_err(), "`{bad}` should not parse");
        }
    }

    #[test]
    fn workflow_summary_renders_a_dash_when_there_is_nothing_to_show() {
        assert_eq!(workflow_summary(&None), "—");
    }
}
