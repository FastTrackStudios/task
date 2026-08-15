//! `task workstream …` — the parent-with-swarm surface.
//!
//! A workstream is the project-scoped construct that replaces
//! the `epic` tag: lead (the orchestrator), members (the swarm),
//! lifecycle status, start/target dates, and a derived rollup
//! over the tasks attached via `workflow.workstream`.
//!
//! Lives in its own module (like `plan` / `mealprep`) so
//! concurrent agents editing `main.rs` only collide on the
//! two-line dispatch arm. References resolve through the shared
//! flexible resolvers in [`crate::json_out`] — uuid, id prefix,
//! vault path, or title all work.

use clap::Subcommand;

use crate::json_out;

#[derive(Subcommand)]
pub enum WorkstreamCmd {
    /// List workstreams in the active org's vault.
    List {
        /// Restrict to one project (uuid, name, path, or prefix).
        #[arg(long)]
        project: Option<String>,
        /// Only workstreams that aren't done / cancelled.
        #[arg(long)]
        open: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show one workstream (uuid, id prefix, path, or title).
    Get {
        target: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Create a workstream. `--project` is required.
    Create {
        title: String,
        /// Owning project (uuid, name, path, or prefix). Required.
        #[arg(long)]
        project: String,
        /// The orchestrator — `agent:name[@ver]` or `human:user_id`.
        #[arg(long)]
        lead: Option<String>,
        /// Repeatable swarm member (`agent:…` / `human:…`).
        #[arg(long = "member", value_name = "AGENT")]
        members: Vec<String>,
        /// Initial status (`backlog` / `planned` / `in-progress` /
        /// `paused` / `done` / `cancelled`). Default `backlog`.
        #[arg(long)]
        status: Option<String>,
        /// Start date, YYYY-MM-DD.
        #[arg(long)]
        start: Option<String>,
        /// Target date, YYYY-MM-DD.
        #[arg(long)]
        target: Option<String>,
        /// Repeatable reference link (PRD, design doc, dashboard).
        #[arg(long = "link", value_name = "URL")]
        links: Vec<String>,
        /// Vault-relative path; defaults to
        /// `Projects/<slug>/workstreams/<ws-slug>.md`.
        #[arg(long)]
        path: Option<String>,
        /// Body (markdown charter). Pass `-` for stdin, or a file
        /// path.
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Set the lifecycle status (canonicalized server-side).
    SetStatus {
        target: String,
        status: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Set or clear (`none`) the lead.
    SetLead {
        target: String,
        /// `agent:name[@ver]`, `human:user_id`, or `none`.
        lead: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Add a swarm member (idempotent).
    AddMember {
        target: String,
        /// `agent:name[@ver]` or `human:user_id`.
        member: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Remove a swarm member.
    RmMember {
        target: String,
        /// `agent:name[@ver]` or `human:user_id`.
        member: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Attach one or more tasks to the workstream — sets
    /// `workflow.workstream` on each (clear it with
    /// `task issue set-workflow <id> --workstream none`).
    Assign {
        /// The workstream (uuid, id prefix, path, or title).
        target: String,
        /// Task references (uuid, id prefix, path, or title).
        #[arg(required = true)]
        tasks: Vec<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Derived progress: done / total / in-progress / blocked +
    /// estimate-point sum over the attached tasks.
    Rollup {
        target: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Delete. Refuses while tasks still point at it.
    Delete {
        target: String,
        /// Skip the y/N prompt.
        #[arg(long, short = 'y')]
        yes: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

pub async fn run_workstream(cmd: WorkstreamCmd) -> eyre::Result<()> {
    match cmd {
        WorkstreamCmd::List {
            project,
            open,
            org,
            server,
            json,
        } => {
            let url = ws_url(org, server)?;
            let client = connect(&url).await?;
            let project_id = match project {
                Some(p) => {
                    let pc = crate::project::connect_project_client(&url).await?;
                    Some(json_out::resolve_project_flexible(&pc, &p).await?.id)
                }
                None => None,
            };
            let rows: Vec<_> = client
                .list(project_id)
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?
                .into_iter()
                .filter(|w| {
                    !open
                        || !workstream::Status::from_str(&w.status)
                            .is_some_and(workstream::Status::is_terminal)
                })
                .collect();
            if json {
                return json_out::print_json(&rows);
            }
            if rows.is_empty() {
                println!("(no workstreams)");
                return Ok(());
            }
            println!("{} workstreams\n", rows.len());
            for w in &rows {
                let lead = w
                    .lead
                    .as_ref()
                    .map(|l| format!("  lead={}", l.short_label()))
                    .unwrap_or_default();
                let crew = if w.members.is_empty() {
                    String::new()
                } else {
                    format!("  crew={}", w.members.len())
                };
                let target = w
                    .target_date
                    .map(|d| format!("  (target {d})"))
                    .unwrap_or_default();
                println!(
                    "{:<32}  {:<12}{lead}{crew}{target}    {}",
                    w.title, w.status, w.path
                );
            }
        }
        WorkstreamCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let url = ws_url(org, server)?;
            let client = connect(&url).await?;
            let w = json_out::resolve_workstream_flexible(&client, &target).await?;
            if json {
                return json_out::print_json(&w);
            }
            print_workstream(&w);
        }
        WorkstreamCmd::Create {
            title,
            project,
            lead,
            members,
            status,
            start,
            target,
            links,
            path,
            body,
            org,
            server,
            json,
        } => {
            let url = ws_url(org, server)?;
            let pc = crate::project::connect_project_client(&url).await?;
            let project_id = json_out::resolve_project_flexible(&pc, &project).await?.id;
            let lead = lead
                .as_deref()
                .map(crate::issue::parse_agent_ref)
                .transpose()?;
            let members: Vec<_> = members
                .iter()
                .map(|m| crate::issue::parse_agent_ref(m))
                .collect::<eyre::Result<_>>()?;
            let status = match status.as_deref() {
                None => workstream::Status::Backlog.as_str().to_string(),
                Some(s) => canonical_status(s)?,
            };
            let details = crate::shared::resolve_body(body)?;
            let new_ws = workstream::Workstream {
                id: uuid::Uuid::nil(),
                path: path.unwrap_or_default(),
                title,
                project_id,
                status,
                lead,
                members: workstream::AgentRefList(members),
                start_date: parse_date(start.as_deref(), "--start")?,
                target_date: parse_date(target.as_deref(), "--target")?,
                links: workstream::Links(links),
                date_created: None,
                date_modified: None,
                details,
            };
            let client = connect(&url).await?;
            let created = client
                .create(new_ws)
                .await
                .map_err(|e| eyre::eyre!("create: {e:?}"))?;
            if json {
                return json_out::print_json(&created);
            }
            println!("created {} ({})", created.title, created.path);
            println!("  id: {}", created.id);
        }
        WorkstreamCmd::SetStatus {
            target,
            status,
            org,
            server,
            json,
        } => {
            let url = ws_url(org, server)?;
            let client = connect(&url).await?;
            let w = json_out::resolve_workstream_flexible(&client, &target).await?;
            let updated = client
                .set_status(w.id, status)
                .await
                .map_err(|e| eyre::eyre!("set-status: {e:?}"))?;
            emit(&updated, json)?;
        }
        WorkstreamCmd::SetLead {
            target,
            lead,
            org,
            server,
            json,
        } => {
            let new_lead = if matches!(lead.as_str(), "none" | "null" | "") {
                None
            } else {
                Some(crate::issue::parse_agent_ref(&lead)?)
            };
            mutate(target, org, server, json, |w| w.lead = new_lead).await?;
        }
        WorkstreamCmd::AddMember {
            target,
            member,
            org,
            server,
            json,
        } => {
            let m = crate::issue::parse_agent_ref(&member)?;
            mutate(target, org, server, json, move |w| {
                if !w.members.iter().any(|x| x == &m) {
                    w.members.0.push(m);
                }
            })
            .await?;
        }
        WorkstreamCmd::RmMember {
            target,
            member,
            org,
            server,
            json,
        } => {
            let m = crate::issue::parse_agent_ref(&member)?;
            mutate(target, org, server, json, move |w| {
                w.members.0.retain(|x| x != &m);
            })
            .await?;
        }
        WorkstreamCmd::Assign {
            target,
            tasks,
            org,
            server,
            json,
        } => {
            let url = ws_url(org, server)?;
            let client = connect(&url).await?;
            let w = json_out::resolve_workstream_flexible(&client, &target).await?;
            let tc = crate::task_cmd::connect_task_client(&url).await?;
            let mut updated = Vec::with_capacity(tasks.len());
            for tref in &tasks {
                let mut t = json_out::resolve_task_flexible(&tc, tref).await?;
                t.workflow
                    .get_or_insert_with(task::model::WorkflowAttrs::default)
                    .workstream = Some(w.id);
                let after = tc
                    .update(t)
                    .await
                    .map_err(|e| eyre::eyre!("update {tref}: {e:?}"))?;
                updated.push(after);
            }
            if json {
                return json_out::print_json(&updated);
            }
            for t in &updated {
                println!("{} → {}", t.title, w.title);
            }
        }
        WorkstreamCmd::Rollup {
            target,
            org,
            server,
            json,
        } => {
            let url = ws_url(org, server)?;
            let client = connect(&url).await?;
            let w = json_out::resolve_workstream_flexible(&client, &target).await?;
            let got = client
                .rollup(w.id)
                .await
                .map_err(|e| eyre::eyre!("rollup: {e:?}"))?;
            if json {
                return json_out::print_json(&got);
            }
            print_workstream(&got.workstream);
            let r = &got.rollup;
            println!("\n  rollup:");
            println!("    done:        {}/{}", r.done, r.total);
            println!("    in-progress: {}", r.in_progress);
            println!("    blocked:     {}", r.blocked);
            println!("    points:      {}", r.estimate_points_sum);
            let g = &r.groups;
            println!(
                "    groups:      backlog {} / unstarted {} / started {} / completed {} / cancelled {}",
                g.backlog, g.unstarted, g.started, g.completed, g.cancelled
            );
            if r.total > 0 {
                println!(
                    "    progress:    {:.0}%",
                    f64::from(r.done) * 100.0 / f64::from(r.total)
                );
            }
        }
        WorkstreamCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let url = ws_url(org, server)?;
            let client = connect(&url).await?;
            let w = json_out::resolve_workstream_flexible(&client, &target).await?;
            if !yes && !crate::shared::confirm(&format!("delete `{}` ({})?", w.title, w.path))? {
                println!("aborted");
                return Ok(());
            }
            client
                .delete(w.id)
                .await
                .map_err(|e| eyre::eyre!("delete: {e:?}"))?;
            println!("deleted {}", w.path);
        }
    }
    Ok(())
}

fn ws_url(org: Option<String>, server: Option<String>) -> eyre::Result<String> {
    let slug = crate::resolve_active_org(org)?;
    Ok(crate::resolve_org_vox_url(server, &slug))
}

async fn connect(url: &str) -> eyre::Result<workstream::WorkstreamServiceClient> {
    crate::establish_for_url(url).await
}

/// Canonicalize a user-typed status, erroring on unknown values
/// (the server would reject them anyway — fail before dialing).
fn canonical_status(s: &str) -> eyre::Result<String> {
    workstream::Status::from_str(s)
        .map(|c| c.as_str().to_string())
        .ok_or_else(|| {
            eyre::eyre!(
                "unknown status `{s}` — one of backlog / planned / in-progress / paused / \
                 done / cancelled"
            )
        })
}

fn parse_date(s: Option<&str>, flag: &str) -> eyre::Result<Option<chrono::NaiveDate>> {
    match s {
        None => Ok(None),
        Some(s) => chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(Some)
            .map_err(|e| eyre::eyre!("{flag}: {e}")),
    }
}

async fn mutate<F>(
    target: String,
    org: Option<String>,
    server: Option<String>,
    json: bool,
    apply: F,
) -> eyre::Result<()>
where
    F: FnOnce(&mut workstream::Workstream),
{
    let url = ws_url(org, server)?;
    let client = connect(&url).await?;
    let mut w = json_out::resolve_workstream_flexible(&client, &target).await?;
    apply(&mut w);
    let updated = client
        .update(w)
        .await
        .map_err(|e| eyre::eyre!("update: {e:?}"))?;
    emit(&updated, json)
}

fn emit(w: &workstream::Workstream, json: bool) -> eyre::Result<()> {
    if json {
        json_out::print_json(w)
    } else {
        println!("{}  [{}]  {}", w.title, w.status, w.path);
        Ok(())
    }
}

fn print_workstream(w: &workstream::Workstream) {
    println!("{} [{}]\n", w.title, w.status);
    println!("  id:       {}", w.id);
    println!("  path:     {}", w.path);
    println!("  project:  {}", w.project_id);
    if let Some(l) = &w.lead {
        println!("  lead:     {}", l.short_label());
    }
    if !w.members.is_empty() {
        println!("  members:");
        for m in w.members.iter() {
            println!("    - {}", m.short_label());
        }
    }
    if let Some(d) = w.start_date {
        println!("  start:    {d}");
    }
    if let Some(d) = w.target_date {
        println!("  target:   {d}");
    }
    if !w.links.is_empty() {
        println!("  links:");
        for l in &w.links.0 {
            println!("    - {l}");
        }
    }
    if !w.details.is_empty() {
        println!("\n{}", w.details);
    }
}
