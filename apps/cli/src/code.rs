//! `task code …` — the agent dev loop (git + issue lifecycle).
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::forge::forge_backend_for;
use crate::forge::forge_link_store;
use crate::issue::ClaimOutcome;
use crate::issue::parse_agent_ref;
use crate::issue::resolve_issue_id;
use crate::issue::try_claim;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::git;
use crate::shared::short_uuid;
use crate::task_cmd::connect_task_client;

/// `task code *` — the agent dev loop over git + issues.
///
/// Branch convention: `task/<short-id>-<slug>`. The short id is
/// the first 8 chars of the task UUID; `commit`/`push`/`status`/
/// `finish` parse it back out of the current branch name, so the
/// branch is the only state these verbs need.
// ── git helpers for `task code` ──────────────────────────────

#[derive(Subcommand)]
pub(crate) enum CodeCmd {
    /// Claim a task, flip it to in-progress, and create a work
    /// branch off the current HEAD. With `--worktree`, the
    /// branch gets its own git worktree (separate directory) so
    /// multiple agents can work different subtasks of one issue
    /// in parallel without colliding on HEAD / the index.
    Start {
        /// Task id (UUID or 8-char prefix).
        id: String,
        /// Claim as this agent (`name[@version]`).
        #[arg(long = "as-agent")]
        as_agent: Option<String>,
        /// Branch prefix. Default `task`.
        #[arg(long, default_value = "task")]
        prefix: String,
        /// Create an isolated git worktree for the branch (under
        /// `.task-worktrees/`) instead of switching the current
        /// checkout. The key to parallel agents on one issue.
        #[arg(long)]
        worktree: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// List active `task code` worktrees (parallel work dirs).
    Worktrees {
        /// Emit `{branch, path}` rows as a JSON array.
        #[arg(long)]
        json: bool,
    },
    /// Remove the worktree for a task branch once it's merged
    /// (or to abandon it). Accepts the task short-id or branch.
    Cleanup {
        /// Task short-id (8 chars) or full branch name.
        id: String,
    },
    /// `git commit` with attribution trailers (Task-Id,
    /// Task-Agent, Co-Authored-By) derived from the current
    /// branch's task.
    Commit {
        #[arg(short = 'm', long)]
        message: String,
        /// Attribute to this agent (`name[@version]`).
        #[arg(long = "as-agent")]
        as_agent: Option<String>,
        /// Stage everything first (`git add -A`).
        #[arg(long)]
        all: bool,
    },
    /// Push the current branch and open a linked PR that closes
    /// the branch's task's forge issue on merge.
    Push {
        /// Target GitHub instead of Forgejo.
        #[arg(long)]
        github: bool,
        /// Forgejo base URL (falls back to `TASK_FORGEJO_BASE_URL`).
        #[arg(long)]
        base_url: Option<String>,
        /// PR target branch. Default `main`.
        #[arg(long, default_value = "main")]
        base: String,
        /// Open as a draft.
        #[arg(long)]
        draft: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Show the current branch's task + its linked issue/PR.
    Status {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit `{branch, task, links}` as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Park the current branch's task — record a "where I left
    /// off" handoff, release the claim so another agent can pick
    /// it up. The branch + commits stay; resume picks up there.
    Park {
        /// Summary of where things stand (markdown).
        summary: String,
        /// Why parking: blocked / needs-input / context-limit /
        /// out-of-scope / end-of-chunk. Free-form.
        #[arg(long, default_value = "end-of-chunk")]
        reason: String,
        /// Open questions for the next agent (markdown bullets).
        #[arg(long)]
        open: Option<String>,
        /// Attribute the handoff to this agent.
        #[arg(long = "as-agent")]
        as_agent: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Resume a parked task: atomically claim it, print the
    /// handoff context (summary + open questions + recent
    /// commits), and switch to its branch.
    Resume {
        /// Task id (UUID or 8-char prefix). Omit to resume the
        /// current branch's task.
        id: Option<String>,
        /// Claim as this agent.
        #[arg(long = "as-agent")]
        as_agent: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// List parked tasks (open handoffs) available to pick up —
    /// the cross-agent work queue.
    Inbox {
        /// Only show handoffs addressed to (or open to) this agent.
        #[arg(long = "as-agent")]
        as_agent: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the open handoffs as a JSON array.
        #[arg(long)]
        json: bool,
    },
}

fn current_branch() -> eyre::Result<String> {
    git(&["rev-parse", "--abbrev-ref", "HEAD"])
}

/// Parse the 8-char task prefix out of a `task/<short>-<slug>`
/// branch name.
fn task_short_from_branch(branch: &str) -> Option<String> {
    let after = branch.split_once('/').map_or(branch, |(_, r)| r);
    let short = after.split('-').next()?;
    if short.len() == 8 && short.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(short.to_string())
    } else {
        None
    }
}

/// Derive a `RepoId` from `git remote get-url origin`. Handles
/// both SSH (`forgejo@host:owner/repo.git`,
/// `git@github.com:owner/repo.git`) and HTTPS forms.
fn repo_id_from_git_remote() -> eyre::Result<git_proto::RepoId> {
    let url = git(&["remote", "get-url", "origin"])?;
    let (host, owner, repo) =
        parse_remote_url(&url).ok_or_else(|| eyre::eyre!("can't parse origin remote `{url}`"))?;
    let forge = if host.contains("github.com") {
        git_proto::Forge::Github
    } else {
        git_proto::Forge::Forgejo {
            base_url: format!("https://{host}"),
        }
    };
    Ok(git_proto::RepoId { forge, owner, repo })
}

/// `(host, owner, repo)` from a git remote URL.
fn parse_remote_url(url: &str) -> Option<(String, String, String)> {
    let url = url.trim();
    // scp-like: user@host:owner/repo(.git)
    let rest = if let Some(idx) = url.find('@') {
        let after_at = &url[idx + 1..];
        if let Some((host, path)) = after_at.split_once(':') {
            let path = path.trim_end_matches(".git");
            let (owner, repo) = path.split_once('/')?;
            return Some((host.to_string(), owner.to_string(), repo.to_string()));
        }
        after_at.to_string()
    } else {
        url.to_string()
    };
    // https://host/owner/repo(.git)
    let rest = rest
        .strip_prefix("https://")
        .or_else(|| rest.strip_prefix("http://"))
        .unwrap_or(&rest);
    let (host, path) = rest.split_once('/')?;
    let path = path.trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    Some((host.to_string(), owner.to_string(), repo.to_string()))
}

pub(crate) async fn run_code(cmd: CodeCmd) -> eyre::Result<()> {
    use git_config::BindingStore as _;
    match cmd {
        CodeCmd::Start {
            id,
            as_agent,
            prefix,
            worktree,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_task_client(&url).await?;
            let mut t = resolve_issue_id(&client, &id).await?;
            let short = t.id.simple().to_string()[..8].to_string();
            let title_slug: String = t
                .title
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
                .split('-')
                .filter(|s| !s.is_empty())
                .take(6)
                .collect::<Vec<_>>()
                .join("-");
            let branch = format!("{prefix}/{short}-{title_slug}");

            if worktree {
                // Isolated worktree → parallel agents don't collide.
                // CRITICAL: place it as a SIBLING of the main repo,
                // not nested inside it. The workspace has relative
                // path-deps that point outside the repo (`../architect`,
                // `../FastTrackStudio/...`); a sibling worktree resolves
                // `../` to the same parent, so those deps still work. A
                // nested worktree would break them.
                let repo_root = git(&["rev-parse", "--show-toplevel"])?;
                let root = std::path::Path::new(&repo_root);
                let parent = root
                    .parent()
                    .ok_or_else(|| eyre::eyre!("repo has no parent dir"))?;
                let repo_name = root
                    .file_name()
                    .map_or_else(|| "repo".to_string(), |s| s.to_string_lossy().to_string());
                let wt_path = parent.join(format!("{repo_name}-wt-{short}-{title_slug}"));
                if wt_path.exists() {
                    return Err(eyre::eyre!(
                        "worktree already exists at {} — `task code cleanup {short}` to remove it",
                        wt_path.display()
                    ));
                }
                git(&["worktree", "add", "-b", &branch, &wt_path.to_string_lossy()])?;
                println!("started {short} in worktree (branch {branch})");
                println!("  work in: {}", wt_path.display());
                println!("  then: cd into it and run `task code commit` / `task code push` there");
                // Share the main repo's build cache so cargo in the
                // worktree doesn't compile from scratch. The git
                // hooks set this automatically; print it for the
                // agent's own `cargo` invocations.
                println!("  for fast builds: export CARGO_TARGET_DIR={repo_root}/target");
            } else {
                git(&["switch", "-c", &branch])?;
                println!("started {short} on branch {branch}");
            }

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
            client
                .update(t)
                .await
                .map_err(|e| eyre::eyre!("update: {e:?}"))?;
        }
        CodeCmd::Worktrees { json } => {
            // `git worktree list --porcelain` → show the task ones.
            let out = git(&["worktree", "list", "--porcelain"])?;
            let mut path = String::new();
            let mut rows: Vec<(String, String)> = Vec::new();
            for line in out.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = p.to_string();
                } else if let Some(b) = line.strip_prefix("branch ") {
                    let b = b.trim_start_matches("refs/heads/");
                    if b.starts_with("task/") {
                        rows.push((b.to_string(), path.clone()));
                    }
                }
            }
            if json {
                let out: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|(branch, path)| serde_json::json!({ "branch": branch, "path": path }))
                    .collect();
                crate::json_out::print_json(&out)?;
                return Ok(());
            }
            if rows.is_empty() {
                println!("(no task worktrees)");
            }
            for (branch, path) in rows {
                println!("{branch}\n  {path}");
            }
        }
        CodeCmd::Cleanup { id } => {
            // Resolve the worktree dir from the short-id or branch.
            let out = git(&["worktree", "list", "--porcelain"])?;
            let mut path = String::new();
            let mut target: Option<String> = None;
            for line in out.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = p.to_string();
                } else if let Some(b) = line.strip_prefix("branch ") {
                    let b = b.trim_start_matches("refs/heads/");
                    let matches =
                        b == id || b.starts_with(&format!("task/{id}-")) || b.contains(&id);
                    if matches && b.starts_with("task/") {
                        target = Some(path.clone());
                    }
                }
            }
            let Some(dir) = target else {
                return Err(eyre::eyre!("no task worktree matching `{id}`"));
            };
            git(&["worktree", "remove", "--force", &dir])?;
            println!("removed worktree {dir}");
        }
        CodeCmd::Commit {
            message,
            as_agent,
            all,
        } => {
            let branch = current_branch()?;
            let short = task_short_from_branch(&branch);
            let mut trailers = String::new();
            if let Some(s) = &short {
                trailers.push_str(&format!("\n\nTask-Id: {s}"));
            }
            let agent = as_agent.unwrap_or_else(|| "claude".to_string());
            trailers.push_str(&format!("\nTask-Agent: {agent}"));
            trailers.push_str("\nCo-Authored-By: Claude <noreply@anthropic.com>");
            let full = format!("{message}{trailers}");
            if all {
                git(&["add", "-A"])?;
            }
            git(&["commit", "-m", &full])?;
            let sha = git(&["rev-parse", "--short", "HEAD"])?;
            println!("committed {sha} on {branch}");
            if short.is_none() {
                eprintln!("  note: branch isn't a `task/<id>-…` branch — no Task-Id trailer");
            }
        }
        CodeCmd::Push {
            github,
            base_url,
            base,
            draft,
            org,
            server,
        } => {
            let branch = current_branch()?;
            let short = task_short_from_branch(&branch).ok_or_else(|| {
                eyre::eyre!(
                    "current branch `{branch}` isn't a `task/<id>-…` branch; can't link a PR"
                )
            })?;
            let slug = resolve_active_org(org)?;
            let vox = resolve_org_vox_url(server, &slug);
            let client = connect_task_client(&vox).await?;
            let t = resolve_issue_id(&client, &short).await?;

            // Forge repo inferred from the git remote (works on
            // third-party repos). --github / --base-url are
            // accepted for parity but the remote is authoritative.
            let _ = (github, &base_url);
            let repo_id = repo_id_from_git_remote()?;
            let repo_slug = format!("{}/{}", repo_id.owner, repo_id.repo);

            // Push the branch.
            git(&["push", "-u", "origin", &branch])?;
            println!("pushed {branch} → {repo_slug}");

            // Find the linked forge issue → inject Closes #N.
            let store = forge_link_store(&slug)?;
            let links = store
                .issues_for_task(&t.id.to_string())
                .map_err(|e| eyre::eyre!("link store: {e}"))?;
            let closes = links
                .iter()
                .find(|l| l.repo == repo_id && l.kind == git_config::LinkKind::Issue)
                .map(|l| l.number);
            let mut body = format!("Work for task {short}.");
            if let Some(n) = closes {
                body.push_str(&format!("\n\nCloses #{n}"));
            }

            let repo_c = repo_id.clone();
            let title = t.title.clone();
            let head = branch.clone();
            let pr = tokio::task::spawn_blocking(move || {
                let backend = forge_backend_for(&repo_c)?;
                backend
                    .create_pull_request(
                        &repo_c,
                        git_proto::reviews::NewPullRequest {
                            title,
                            body,
                            base,
                            head,
                            draft,
                        },
                    )
                    .map_err(|e| eyre::eyre!("create_pull_request: {e:?}"))
            })
            .await
            .map_err(|e| eyre::eyre!("join: {e}"))??;

            // Record a PR link on the task.
            store
                .add_issue_link(git_config::IssueLink {
                    task_id: t.id.to_string(),
                    repo: repo_id.clone(),
                    number: pr.id.0,
                    kind: git_config::LinkKind::Pull,
                })
                .map_err(|e| eyre::eyre!("link store: {e}"))?;
            println!("opened PR #{} ({repo_slug})", pr.id.0);
            if let Some(n) = closes {
                println!("  closes #{n} on merge");
            } else {
                eprintln!("  note: task has no linked forge issue — PR won't auto-close one");
            }
        }
        CodeCmd::Status { org, server, json } => {
            let branch = current_branch()?;
            if !json {
                println!("branch:  {branch}");
            }
            let Some(short) = task_short_from_branch(&branch) else {
                if json {
                    crate::json_out::print_json(&serde_json::json!({
                        "branch": branch,
                        "task": null,
                        "links": [],
                    }))?;
                } else {
                    println!("task:    (branch isn't a task/<id>-… branch)");
                }
                return Ok(());
            };
            let slug = resolve_active_org(org)?;
            let vox = resolve_org_vox_url(server, &slug);
            let client = connect_task_client(&vox).await?;
            let t = resolve_issue_id(&client, &short).await?;
            let store = forge_link_store(&slug)?;
            let links = store
                .issues_for_task(&t.id.to_string())
                .map_err(|e| eyre::eyre!("link store: {e}"))?;
            if json {
                let link_rows: Vec<serde_json::Value> = links
                    .iter()
                    .map(|l| {
                        let kind = match l.kind {
                            git_config::LinkKind::Issue => "issue",
                            git_config::LinkKind::Pull => "pr",
                        };
                        serde_json::json!({
                            "kind": kind,
                            "owner": l.repo.owner,
                            "repo": l.repo.repo,
                            "number": l.number,
                        })
                    })
                    .collect();
                crate::json_out::print_json(&serde_json::json!({
                    "branch": branch,
                    "task": {
                        "id": t.id,
                        "short": short,
                        "status": t.status,
                        "title": t.title,
                        "path": t.path,
                    },
                    "links": link_rows,
                }))?;
                return Ok(());
            }
            println!("task:    {} [{}]  {}", short, t.status, t.title);
            for l in links {
                let kind = match l.kind {
                    git_config::LinkKind::Issue => "issue",
                    git_config::LinkKind::Pull => "pr",
                };
                println!("  {kind:<5} {}/{}#{}", l.repo.owner, l.repo.repo, l.number);
            }
        }
        CodeCmd::Park {
            summary,
            reason,
            open,
            as_agent,
            org,
            server,
        } => {
            let branch = current_branch()?;
            let short = task_short_from_branch(&branch).ok_or_else(|| {
                eyre::eyre!("current branch `{branch}` isn't a task/<id>-… branch")
            })?;
            let slug = resolve_active_org(org)?;
            let vox = resolve_org_vox_url(server, &slug);
            let client = connect_task_client(&vox).await?;
            let mut t = resolve_issue_id(&client, &short).await?;
            let agent = parse_agent_ref(&format!(
                "agent:{}",
                as_agent.as_deref().unwrap_or("claude")
            ))?;

            // Record the handoff (one active per task; supersede prior open ones).
            let mut hs = load_handoffs(&slug)?;
            for h in hs.iter_mut().filter(|h| {
                h.session_id == t.id && h.status == workflows_proto::HandoffStatus::Open
            }) {
                h.status = workflows_proto::HandoffStatus::Cancelled;
                h.resolved_at = Some(chrono::Utc::now());
            }
            let mut handoff = workflows_proto::Handoff::post(
                t.id, // session_id repurposed as the task id (no separate WorkSession yet)
                agent.clone(),
                workflows_proto::HandoffReason::Custom {
                    tag: reason.clone(),
                },
                summary,
            );
            handoff.open_questions = open.unwrap_or_default();
            hs.push(handoff);
            save_handoffs(&slug, &hs)?;

            // Release the claim + return to the ready queue.
            if let Some(w) = t.workflow.as_mut() {
                w.assignees = task::model::AgentRefList(vec![]);
            }
            t.status = "open".into();
            client
                .update(t)
                .await
                .map_err(|e| eyre::eyre!("update: {e:?}"))?;
            println!("parked {short} (reason: {reason}) — claim released, branch {branch} kept");
            println!("another agent: `task code resume {short} --as-agent <name>`");
        }
        CodeCmd::Resume {
            id,
            as_agent,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let vox = resolve_org_vox_url(server, &slug);
            let client = connect_task_client(&vox).await?;
            let target = match id {
                Some(i) => i,
                None => task_short_from_branch(&current_branch()?).ok_or_else(|| {
                    eyre::eyre!("no task id given and current branch isn't a task branch")
                })?,
            };
            let t = resolve_issue_id(&client, &target).await?;
            let agent = parse_agent_ref(&format!("agent:{as_agent}"))?;
            // Atomically claim it.
            if let ClaimOutcome::Lost(holder) = try_claim(&client, &t.id, &agent, false).await? {
                return Err(eyre::eyre!(
                    "{} is held by {holder} — can't resume",
                    short_uuid(&t.id)
                ));
            }
            // Flip to in-progress.
            let mut t2 = client
                .get(t.id)
                .await
                .map_err(|e| eyre::eyre!("get: {e:?}"))?;
            t2.status = "in-progress".into();
            client
                .update(t2)
                .await
                .map_err(|e| eyre::eyre!("update: {e:?}"))?;

            // Surface the latest open handoff context + mark it claimed.
            let mut hs = load_handoffs(&slug)?;
            let short = &t.id.simple().to_string()[..8];
            println!("resumed {short}: {}\n", t.title);
            if let Some(h) = hs
                .iter_mut()
                .filter(|h| {
                    h.session_id == t.id && h.status == workflows_proto::HandoffStatus::Open
                })
                .max_by_key(|h| h.created_at)
            {
                println!(
                    "── handoff from {} ({:?}) ──",
                    h.from_actor.short_label(),
                    h.reason
                );
                println!("{}", h.summary);
                if !h.open_questions.trim().is_empty() {
                    println!("\nopen questions:\n{}", h.open_questions);
                }
                h.status = workflows_proto::HandoffStatus::Claimed;
                save_handoffs(&slug, &hs)?;
            } else {
                println!("(no handoff note recorded)");
            }
            // Switch to the work branch if it exists locally.
            let want = format!("task/{short}-");
            let branches = git(&["branch", "--list", &format!("{want}*")]).unwrap_or_default();
            if let Some(line) = branches.lines().next() {
                let b = line.trim_start_matches('*').trim();
                if !b.is_empty() {
                    let _ = git(&["switch", b]);
                    println!("\nswitched to {b}");
                    if let Ok(log) = git(&["log", "--oneline", "-5"]) {
                        println!("recent commits:\n{log}");
                    }
                }
            } else {
                println!(
                    "\n(no local branch {want}* — `git fetch` then switch, or `task code start` to recreate)"
                );
            }
        }
        CodeCmd::Inbox {
            as_agent,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let vox = resolve_org_vox_url(server, &slug);
            let client = connect_task_client(&vox).await?;
            let me = as_agent
                .as_deref()
                .map(|s| parse_agent_ref(&format!("agent:{s}")))
                .transpose()?;
            let hs = load_handoffs(&slug)?;
            let open: Vec<&workflows_proto::Handoff> = hs
                .iter()
                .filter(|h| h.status == workflows_proto::HandoffStatus::Open)
                .filter(|h| match (&me, &h.to_actor) {
                    (Some(m), Some(to)) => to == m, // addressed to me
                    (_, None) => true,              // open to anyone
                    _ => true,
                })
                .collect();
            if open.is_empty() && !json {
                println!("(no parked tasks)");
                return Ok(());
            }
            if json {
                // Handoff entities + the joined task title.
                let mut rows: Vec<serde_json::Value> = Vec::with_capacity(open.len());
                for h in open {
                    let title = client.get(h.session_id).await.ok().map(|t| t.title);
                    let mut v = serde_json::to_value(h).unwrap_or(serde_json::Value::Null);
                    if let serde_json::Value::Object(map) = &mut v {
                        map.insert(
                            "task_short".into(),
                            h.session_id.simple().to_string()[..8].into(),
                        );
                        if let Some(t) = title {
                            map.insert("task_title".into(), t.into());
                        }
                    }
                    rows.push(v);
                }
                crate::json_out::print_json(&rows)?;
                return Ok(());
            }
            println!("{} parked task(s):", open.len());
            for h in open {
                let title = client
                    .get(h.session_id)
                    .await
                    .map_or_else(|_| "(task?)".into(), |t| t.title);
                println!(
                    "  {}  from {:<16} {:?}  {}",
                    &h.session_id.simple().to_string()[..8],
                    h.from_actor.short_label(),
                    h.reason,
                    title
                );
            }
        }
    }
    Ok(())
}

/// Per-org handoff store path.
///
/// Vox-unification judgment: DELIBERATELY machine-local. Handoffs
/// are the park/resume context of the `task code` git dev-loop,
/// which operates on THIS machine's checkouts and worktrees —
/// exactly like the agent goal-loop store (`org_workflows_dir`).
/// Cross-machine agent handoff would need a workflows service on
/// the org router first (none exists; workflows-proto is
/// types-only).
fn handoff_store_path(org_slug: &str) -> eyre::Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| eyre::eyre!("HOME not set"))?;
    Ok(std::path::Path::new(&home)
        .join(".task")
        .join("orgs")
        .join(org_slug)
        .join("handoffs.json"))
}

fn load_handoffs(org_slug: &str) -> eyre::Result<Vec<workflows_proto::Handoff>> {
    let p = handoff_store_path(org_slug)?;
    if !p.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(&p).map_err(|e| eyre::eyre!("read {}: {e}", p.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| eyre::eyre!("parse handoffs.json: {e}"))
}

fn save_handoffs(org_slug: &str, hs: &[workflows_proto::Handoff]) -> eyre::Result<()> {
    let p = handoff_store_path(org_slug)?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| eyre::eyre!("mkdir: {e}"))?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(hs)?).map_err(|e| eyre::eyre!("write: {e}"))?;
    std::fs::rename(&tmp, &p).map_err(|e| eyre::eyre!("rename: {e}"))?;
    Ok(())
}
