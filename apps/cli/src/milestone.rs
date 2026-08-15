//! `task milestone …` — project milestones.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::goal::connect_goal_client;
use crate::goal::resolve_goal_target;
use crate::project::connect_project_client;
use crate::project::resolve_project_target;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;
use crate::shared::resolve_body;

#[derive(Subcommand)]
pub(crate) enum MilestoneCmd {
    /// List every milestone in the active org's vault.
    List {
        /// Restrict to one project (id or path).
        #[arg(long)]
        project: Option<String>,
        /// Restrict to one goal (id or path).
        #[arg(long)]
        goal: Option<String>,
        /// Only milestones whose status is not closed.
        #[arg(long)]
        open: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Get {
        target: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Create a milestone. `--project` is required.
    Create {
        title: String,
        /// Project id or path. Required.
        #[arg(long)]
        project: String,
        /// Optional life-goal link (id or path).
        #[arg(long)]
        goal: Option<String>,
        #[arg(long)]
        path: Option<String>,
        /// `open` / `closed`. Default `open`.
        #[arg(long)]
        status: Option<String>,
        /// YYYY-MM-DD.
        #[arg(long)]
        due: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// External reference for future Forgejo / GitHub
        /// sync (e.g. `forgejo:starcommand.live/foo/bar#7`).
        #[arg(long)]
        forge_ref: Option<String>,
        #[arg(long)]
        details: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// `open` or `closed`. Convenience over `update`.
    SetStatus {
        target: String,
        status: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting milestone as JSON.
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
        /// Emit the resulting milestone as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set or clear (`none`) the life-goal link.
    SetGoal {
        target: String,
        goal: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting milestone as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set or clear (`none`) the forge sync ref.
    SetForgeRef {
        target: String,
        forge_ref: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting milestone as JSON.
        #[arg(long)]
        json: bool,
    },
    /// `closed`. Just `set-status <target> closed`.
    Close {
        target: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting milestone as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reopen (status = open).
    Reopen {
        target: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting milestone as JSON.
        #[arg(long)]
        json: bool,
    },
    Rename {
        target: String,
        new_path: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the renamed milestone as JSON.
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

pub(crate) async fn run_milestone(cmd: MilestoneCmd) -> eyre::Result<()> {
    match cmd {
        MilestoneCmd::List {
            project,
            goal,
            open,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_milestone_client(&url).await?;
            let project_id = match project {
                Some(p) => {
                    let pc = connect_project_client(&url).await?;
                    Some(resolve_project_target(&pc, &p).await?.id)
                }
                None => None,
            };
            let goal_id = match goal {
                Some(g) => {
                    let gc = connect_goal_client(&url).await?;
                    Some(resolve_goal_target(&gc, &g).await?.id)
                }
                None => None,
            };
            let rows: Vec<_> = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?
                .into_iter()
                .filter(|m| project_id.is_none_or(|pid| m.project_id == pid))
                .filter(|m| goal_id.is_none_or(|gid| m.goal_id == Some(gid)))
                .filter(|m| !open || m.status != "closed")
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            if rows.is_empty() {
                println!("(no milestones)");
                return Ok(());
            }
            println!("{} milestones\n", rows.len());
            for m in &rows {
                let due = m
                    .due_date
                    .map(|d| format!("  (due {d})"))
                    .unwrap_or_default();
                let goal = m.goal_id.map(|_| "  →goal".to_string()).unwrap_or_default();
                println!("{:<32}  {:<8}{due}{goal}    {}", m.title, m.status, m.path);
            }
        }
        MilestoneCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_milestone_client(&url).await?;
            let m = resolve_milestone_target(&client, &target).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&m).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{} [{}]\n", m.title, m.status);
            println!("  id:       {}", m.id);
            println!("  path:     {}", m.path);
            println!("  project:  {}", m.project_id);
            if let Some(g) = m.goal_id {
                println!("  goal:     {g}");
            }
            if let Some(d) = m.due_date {
                println!("  due:      {d}");
            }
            if let Some(r) = &m.forge_ref {
                println!("  forge:    {r}");
            }
            if !m.tags.is_empty() {
                println!("  tags:     {}", m.tags.0.join(", "));
            }
            if !m.details.is_empty() {
                println!("\n{}", m.details);
            }
        }
        MilestoneCmd::Create {
            title,
            project,
            goal,
            path,
            status,
            due,
            tags,
            forge_ref,
            details,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let pc = connect_project_client(&url).await?;
            let project_id = resolve_project_target(&pc, &project).await?.id;
            let goal_id = match goal {
                None => None,
                Some(g) => {
                    let gc = connect_goal_client(&url).await?;
                    Some(resolve_goal_target(&gc, &g).await?.id)
                }
            };
            let due_date = match due {
                None => None,
                Some(s) => Some(
                    chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                        .map_err(|e| eyre::eyre!("--due: {e}"))?,
                ),
            };
            let details = resolve_body(details)?;
            let new_ms = milestone::Milestone {
                id: uuid::Uuid::nil(),
                path: path.unwrap_or_default(),
                title,
                project_id,
                goal_id,
                status: status.unwrap_or_else(|| "open".into()),
                due_date,
                tags: milestone::Tags(tags),
                forge_ref,
                date_created: None,
                date_modified: None,
                details,
            };
            let client = connect_milestone_client(&url).await?;
            let created = client
                .create(new_ms)
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
        MilestoneCmd::SetStatus {
            target,
            status,
            org,
            server,
            json,
        } => mutate_milestone(target, org, server, json, |m| m.status = status).await?,
        MilestoneCmd::SetDue {
            target,
            due,
            org,
            server,
            json,
        } => {
            let v = if matches!(due.as_str(), "none" | "null" | "") {
                None
            } else {
                Some(
                    chrono::NaiveDate::parse_from_str(&due, "%Y-%m-%d")
                        .map_err(|e| eyre::eyre!("--due: {e}"))?,
                )
            };
            mutate_milestone(target, org, server, json, |m| m.due_date = v).await?;
        }
        MilestoneCmd::SetGoal {
            target,
            goal,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org.clone())?;
            let url = resolve_org_vox_url(server.clone(), &slug);
            let new_goal = if matches!(goal.as_str(), "none" | "null" | "") {
                None
            } else {
                let gc = connect_goal_client(&url).await?;
                Some(resolve_goal_target(&gc, &goal).await?.id)
            };
            mutate_milestone(target, org, server, json, |m| m.goal_id = new_goal).await?;
        }
        MilestoneCmd::SetForgeRef {
            target,
            forge_ref,
            org,
            server,
            json,
        } => {
            let v = if matches!(forge_ref.as_str(), "none" | "null" | "") {
                None
            } else {
                Some(forge_ref)
            };
            mutate_milestone(target, org, server, json, |m| m.forge_ref = v).await?;
        }
        MilestoneCmd::Close {
            target,
            org,
            server,
            json,
        } => mutate_milestone(target, org, server, json, |m| m.status = "closed".into()).await?,
        MilestoneCmd::Reopen {
            target,
            org,
            server,
            json,
        } => mutate_milestone(target, org, server, json, |m| m.status = "open".into()).await?,
        MilestoneCmd::Rename {
            target,
            new_path,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_milestone_client(&url).await?;
            let m = resolve_milestone_target(&client, &target).await?;
            let renamed = client
                .rename(m.id, new_path)
                .await
                .map_err(|e| eyre::eyre!("rename: {e:?}"))?;
            if json {
                crate::json_out::print_json(&renamed)?;
            } else {
                println!("renamed → {}", renamed.path);
            }
        }
        MilestoneCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_milestone_client(&url).await?;
            let m = resolve_milestone_target(&client, &target).await?;
            if !yes && !confirm(&format!("delete `{}` ({})?", m.title, m.path))? {
                println!("aborted");
                return Ok(());
            }
            client
                .delete(m.id)
                .await
                .map_err(|e| eyre::eyre!("delete: {e:?}"))?;
            println!("deleted {}", m.path);
        }
    }
    Ok(())
}

pub(crate) async fn connect_milestone_client(
    url: &str,
) -> eyre::Result<milestone::MilestoneServiceClient> {
    establish_for_url(url).await
}

/// Resolve a milestone reference — uuid, vault path, title, or a
/// unique prefix of either (shared flexible resolver).
pub(crate) async fn resolve_milestone_target(
    client: &milestone::MilestoneServiceClient,
    target: &str,
) -> eyre::Result<milestone::Milestone> {
    crate::json_out::resolve_milestone_flexible(client, target).await
}

async fn mutate_milestone<F>(
    target: String,
    org: Option<String>,
    server: Option<String>,
    json: bool,
    apply: F,
) -> eyre::Result<()>
where
    F: FnOnce(&mut milestone::Milestone),
{
    let slug = resolve_active_org(org)?;
    let url = resolve_org_vox_url(server, &slug);
    let client = connect_milestone_client(&url).await?;
    let mut m = resolve_milestone_target(&client, &target).await?;
    apply(&mut m);
    let updated = client
        .update(m)
        .await
        .map_err(|e| eyre::eyre!("update: {e:?}"))?;
    if json {
        crate::json_out::print_json(&updated)?;
    } else {
        println!("{}  [{}]  {}", updated.title, updated.status, updated.path);
    }
    Ok(())
}
