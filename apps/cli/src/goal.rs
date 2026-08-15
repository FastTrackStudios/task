//! `task goal …` — life-goals with cycle anchoring.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_client;
use crate::establish_for_url;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;
use crate::shared::resolve_body;

#[derive(Subcommand)]
pub(crate) enum GoalCmd {
    /// List every goal the active org's vault carries.
    /// Output groups by lifetime root, shows the kind chip
    /// (lifetime / yearly / cycle / …) and cycle anchor when
    /// present.
    List {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Only goals scoped to the current cycle (per
        /// `cycle::cycle_for_date(today)`).
        #[arg(long)]
        current_cycle: bool,
        #[arg(long)]
        json: bool,
    },
    /// Fetch one goal by id or by vault-relative path.
    Get {
        target: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Create a new goal. Title is the only required arg.
    Create {
        title: String,
        /// `lifetime|yearly|quarterly|cycle|weekly`. Default
        /// `lifetime` for top-level, `cycle` when `--cycle`
        /// (or `--cycle-current`) is set, else `lifetime`.
        #[arg(long)]
        kind: Option<String>,
        /// Status slug. Default `aspiration`.
        #[arg(long)]
        status: Option<String>,
        /// Vault-relative path. Default `Goals/<slug>.md`.
        #[arg(long)]
        path: Option<String>,
        /// Parent goal id or path.
        #[arg(long)]
        parent: Option<String>,
        /// ISO date `YYYY-MM-DD`. Required for `yearly` goals
        /// by convention but not enforced.
        #[arg(long)]
        target_date: Option<String>,
        /// Cycle UUID. Mutually exclusive with
        /// `--cycle-current`.
        #[arg(long)]
        cycle: Option<String>,
        /// Anchor to today's cycle.
        #[arg(long)]
        cycle_current: bool,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long)]
        details: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Set the goal status.
    SetStatus {
        target: String,
        status: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting goal as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set or clear the parent goal (`none` clears).
    SetParent {
        target: String,
        parent: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting goal as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Anchor a goal to a specific cycle (by UUID, by
    /// `YYYY:Qn:Cm`, or `current` for today's cycle). Pass
    /// `none` / `null` to clear.
    SetCycle {
        target: String,
        cycle: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the resulting goal as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Move the backing markdown file. `id` is preserved.
    Rename {
        target: String,
        new_path: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Emit the renamed goal as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Delete the goal. Refuses if any other goal lists it as
    /// parent.
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

pub(crate) async fn run_goal(cmd: GoalCmd) -> eyre::Result<()> {
    use chrono::Weekday;
    use cycle::FirstWeekRule;
    use goal::GoalServiceClient;

    match cmd {
        GoalCmd::List {
            org,
            server,
            current_cycle,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let client: GoalServiceClient = establish_client(server, &slug).await?;
            let mut rows = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;

            if current_cycle {
                let today = chrono::Local::now().date_naive();
                let now = cycle::cycle_for_date(
                    today,
                    Weekday::Mon,
                    FirstWeekRule::AtLeastFourDaysInYear,
                );
                if let Some(c) = now {
                    rows.retain(|g| g.cycle_id == Some(c.id));
                } else {
                    println!("today is between cycles — nothing to show");
                    return Ok(());
                }
            }

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }

            // Resolve cycle id → label once, reused across rows.
            let cycle_label = |g: &goal::Goal| -> Option<String> {
                use chrono::Datelike;
                let id = g.cycle_id?;
                let base = chrono::Local::now().date_naive().year();
                for off in [-1, 0, 1, 2] {
                    let qs = cycle::generate_year(
                        base + off,
                        Weekday::Mon,
                        FirstWeekRule::AtLeastFourDaysInYear,
                    );
                    for q in qs {
                        for c in q.cycles.iter() {
                            if c.id == id {
                                return Some(format!("{} Q{} C{}", c.year, c.quarter, c.ordinal));
                            }
                        }
                    }
                }
                None
            };

            println!("{} goals\n", rows.len());
            let roots: Vec<&goal::Goal> = rows.iter().filter(|g| g.parent_id.is_none()).collect();
            for root in roots {
                print_goal_row(root, 0, cycle_label(root));
                for kid in rows.iter().filter(|g| g.parent_id == Some(root.id)) {
                    print_goal_row(kid, 2, cycle_label(kid));
                    for gc in rows.iter().filter(|g| g.parent_id == Some(kid.id)) {
                        print_goal_row(gc, 4, cycle_label(gc));
                    }
                }
            }
        }
        GoalCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let client: GoalServiceClient = establish_client(server, &slug).await?;
            let g = resolve_goal_target(&client, &target).await?;

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&g).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }

            println!("{} [{}]\n", g.title, g.status);
            println!("  id:       {}", g.id);
            println!("  path:     {}", g.path);
            println!("  kind:     {}", g.kind);
            if let Some(parent) = g.parent_id {
                println!("  parent:   {parent}");
            }
            if let Some(td) = g.target_date {
                println!("  target:   {td}");
            }
            if let Some(cid) = g.cycle_id {
                println!("  cycle:    {cid}");
            }
            if !g.tags.0.is_empty() {
                println!("  tags:     {}", g.tags.0.join(", "));
            }
            if !g.details.is_empty() {
                println!("\n{}", g.details);
            }
        }
        GoalCmd::Create {
            title,
            kind,
            status,
            path,
            parent,
            target_date,
            cycle,
            cycle_current,
            tags,
            details,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_goal_client(&url).await?;

            let parent_id = match parent {
                None => None,
                Some(s) => Some(resolve_goal_target(&client, &s).await?.id),
            };
            let cycle_id = resolve_cycle_arg(cycle, cycle_current)?;
            let kind_str = kind.unwrap_or_else(|| {
                if cycle_id.is_some() {
                    "cycle".into()
                } else {
                    "lifetime".into()
                }
            });
            let target_date = match target_date {
                None => None,
                Some(s) => Some(
                    chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                        .map_err(|e| eyre::eyre!("--target-date: {e}"))?,
                ),
            };
            let details = resolve_body(details)?;
            let new_goal = goal::Goal {
                id: uuid::Uuid::nil(),
                path: path.unwrap_or_default(),
                title,
                kind: kind_str,
                status: status.unwrap_or_else(|| "aspiration".into()),
                parent_id,
                target_date,
                cycle_id,
                tags: goal::Tags(tags),
                date_created: None,
                date_modified: None,
                details,
            };
            let created = client
                .create(new_goal)
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
        GoalCmd::SetStatus {
            target,
            status,
            org,
            server,
            json,
        } => {
            mutate_goal(target, org, server, json, |g| g.status = status).await?;
        }
        GoalCmd::SetParent {
            target,
            parent,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org.clone())?;
            let url = resolve_org_vox_url(server.clone(), &slug);
            let client = connect_goal_client(&url).await?;
            let new_parent = if matches!(parent.as_str(), "none" | "null" | "") {
                None
            } else {
                Some(resolve_goal_target(&client, &parent).await?.id)
            };
            mutate_goal(target, org, server, json, |g| g.parent_id = new_parent).await?;
        }
        GoalCmd::SetCycle {
            target,
            cycle,
            org,
            server,
            json,
        } => {
            let new_cycle = resolve_cycle_arg(Some(cycle), false)?;
            mutate_goal(target, org, server, json, |g| g.cycle_id = new_cycle).await?;
        }
        GoalCmd::Rename {
            target,
            new_path,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_goal_client(&url).await?;
            let g = resolve_goal_target(&client, &target).await?;
            let renamed = client
                .rename(g.id, new_path)
                .await
                .map_err(|e| eyre::eyre!("rename: {e:?}"))?;
            if json {
                crate::json_out::print_json(&renamed)?;
            } else {
                println!("renamed → {}", renamed.path);
            }
        }
        GoalCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let url = resolve_org_vox_url(server, &slug);
            let client = connect_goal_client(&url).await?;
            let g = resolve_goal_target(&client, &target).await?;
            if !yes && !confirm(&format!("delete `{}` ({})?", g.title, g.path))? {
                println!("aborted");
                return Ok(());
            }
            client
                .delete(g.id)
                .await
                .map_err(|e| eyre::eyre!("delete: {e:?}"))?;
            println!("deleted {}", g.path);
        }
    }
    Ok(())
}

pub(crate) async fn connect_goal_client(url: &str) -> eyre::Result<goal::GoalServiceClient> {
    establish_for_url(url).await
}

/// Resolve a goal reference — uuid, vault path, title, or a unique
/// prefix of either (shared flexible resolver).
pub(crate) async fn resolve_goal_target(
    client: &goal::GoalServiceClient,
    target: &str,
) -> eyre::Result<goal::Goal> {
    crate::json_out::resolve_goal_flexible(client, target).await
}

pub(crate) async fn mutate_goal<F>(
    target: String,
    org: Option<String>,
    server: Option<String>,
    json: bool,
    apply: F,
) -> eyre::Result<()>
where
    F: FnOnce(&mut goal::Goal),
{
    let slug = resolve_active_org(org)?;
    let url = resolve_org_vox_url(server, &slug);
    let client = connect_goal_client(&url).await?;
    let mut g = resolve_goal_target(&client, &target).await?;
    apply(&mut g);
    let updated = client
        .update(g)
        .await
        .map_err(|e| eyre::eyre!("update: {e:?}"))?;
    if json {
        crate::json_out::print_json(&updated)?;
    } else {
        println!("{}  [{}]  {}", updated.title, updated.status, updated.path);
    }
    Ok(())
}

/// Resolve a `--cycle` flag / argument into a concrete cycle
/// UUID. Accepts:
/// - a literal UUID
/// - `YYYY:Qn:Cm` (e.g. `2026:Q3:C1`)
/// - the cycle-current shortcut (when `current = true` or arg
///   is `current`)
/// - `none` / `null` / "" → clear
pub(crate) fn resolve_cycle_arg(
    arg: Option<String>,
    current: bool,
) -> eyre::Result<Option<uuid::Uuid>> {
    use chrono::{Datelike, Local, Weekday};
    use cycle::FirstWeekRule;

    if current || arg.as_deref() == Some("current") {
        let today = Local::now().date_naive();
        return Ok(cycle::cycle_for_date(
            today,
            Weekday::Mon,
            FirstWeekRule::AtLeastFourDaysInYear,
        )
        .map(|c| c.id));
    }
    let Some(s) = arg else {
        return Ok(None);
    };
    if matches!(s.as_str(), "none" | "null" | "") {
        return Ok(None);
    }
    if let Ok(id) = uuid::Uuid::parse_str(&s) {
        return Ok(Some(id));
    }
    // Parse `YYYY:Qn:Cm` (also accepted with `-` separators, the
    // form `cycle current` prints as its label: `2026-Q2-C3`).
    let sep = if s.contains(':') { ':' } else { '-' };
    let parts: Vec<&str> = s.split(sep).collect();
    if parts.len() == 3 {
        let year = parts[0].parse::<i32>().ok();
        let q = parts[1]
            .strip_prefix('Q')
            .and_then(|n| n.parse::<u8>().ok());
        let ord = parts[2]
            .strip_prefix('C')
            .and_then(|n| n.parse::<u8>().ok());
        if let (Some(year), Some(q), Some(ord)) = (year, q, ord) {
            let base = Local::now().date_naive().year();
            for off in [-1_i32, 0, 1, 2] {
                let qs = cycle::generate_year(
                    base + off,
                    Weekday::Mon,
                    FirstWeekRule::AtLeastFourDaysInYear,
                );
                for qq in qs {
                    if qq.year == year && qq.ordinal == q {
                        for c in qq.cycles.iter() {
                            if c.ordinal == ord {
                                return Ok(Some(c.id));
                            }
                        }
                    }
                }
            }
            return Err(crate::errors::not_found("resolve cycle", &s)
                .cause(format!(
                    "not found in surrounding years ({}..={})",
                    base - 1,
                    base + 2
                ))
                .report());
        }
    }
    Err(crate::errors::usage("parse --cycle")
        .cause(format!(
            "expected UUID, `YYYY:Qn:Cm`, `current`, or `none` (got `{s}`)"
        ))
        .report())
}

fn print_goal_row(g: &goal::Goal, indent: usize, cycle: Option<String>) {
    let pad = " ".repeat(indent);
    let cycle_str = cycle.map(|c| format!("  @{c}")).unwrap_or_default();
    let target = g
        .target_date
        .map(|d| format!("  (target {d})"))
        .unwrap_or_default();
    println!(
        "{pad}{:<32}  {:<10}  {:<10}{cycle_str}{target}",
        g.title, g.kind, g.status
    );
}
