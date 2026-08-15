//! `task body …` — body metrics log.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;

// ── Body metrics (body::Store) ───────────────────────────────────────

#[derive(Subcommand)]
pub(crate) enum BodyCmd {
    List {
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
    Create {
        name: String,
        /// e.g. `weight`, `body_fat`, `waist`. Free-form;
        /// canonical set in `body::MetricKind`.
        #[arg(long)]
        kind: Option<String>,
        /// Default unit (`kg`, `%`, `cm`).
        #[arg(long)]
        unit: Option<String>,
        #[arg(long)]
        goal: Option<f64>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Append a measurement to a metric's time series.
    Log {
        target: String,
        value: f64,
        /// Date (`YYYY-MM-DD`). Defaults to today.
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        unit: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
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

async fn connect_body_client(url: &str) -> eyre::Result<body::BodyServiceClient> {
    establish_for_url(url).await
}

async fn resolve_body_target(
    client: &body::BodyServiceClient,
    target: &str,
) -> eyre::Result<body::BodyMetric> {
    if uuid::Uuid::parse_str(target).is_ok() {
        return client
            .get(target.to_owned())
            .await
            .map_err(|e| eyre::eyre!("get: {e:?}"));
    }
    let rows = client
        .list()
        .await
        .map_err(|e| eyre::eyre!("list: {e:?}"))?;
    rows.into_iter()
        .find(|m| m.path == target || m.name == target || m.kind == target)
        .ok_or_else(|| {
            crate::errors::not_found("resolve target", target)
                .cause("no path or name match")
                .report()
        })
}

pub(crate) async fn run_body(cmd: BodyCmd) -> eyre::Result<()> {
    match cmd {
        BodyCmd::List { org, server, json } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_body_client(&u).await?;
            let rows = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            for m in &rows {
                let goal = m.goal.map(|g| format!(" goal={g}")).unwrap_or_default();
                let latest = m
                    .entries
                    .0
                    .last()
                    .map(|e| format!("  last {}: {}{}", e.date, e.value, m.unit))
                    .unwrap_or_default();
                println!("{:<24}  {:<10}{goal}{latest}", m.name, m.kind);
            }
        }
        BodyCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_body_client(&u).await?;
            let m = resolve_body_target(&client, &target).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&m).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{} [{}]\n", m.name, m.kind);
            println!("  id:    {}", m.id);
            println!("  path:  {}", m.path);
            println!("  unit:  {}", m.unit);
            if let Some(g) = m.goal {
                println!("  goal:  {g}");
            }
            println!("  entries: {} (last 10)", m.entries.0.len());
            for e in m.entries.0.iter().rev().take(10) {
                println!("    {}  {}{}", e.date, e.value, m.unit);
            }
        }
        BodyCmd::Create {
            name,
            kind,
            unit,
            goal,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_body_client(&u).await?;
            let new_metric = body::BodyMetric {
                path: String::new(),
                id: uuid::Uuid::nil(),
                name,
                kind: kind.unwrap_or_else(|| "other".into()),
                unit: unit.unwrap_or_default(),
                goal,
                tags: body::model::Tags(Vec::new()),
                entries: body::model::Entries(Vec::new()),
                date_created: None,
                date_modified: None,
                details: String::new(),
            };
            let created = client
                .create(new_metric)
                .await
                .map_err(|e| eyre::eyre!("create: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&created).map_err(|e| eyre::eyre!("json: {e}"))?
                );
            } else {
                println!("created {} ({})", created.name, created.path);
                println!("  id: {}", created.id);
            }
        }
        BodyCmd::Log {
            target,
            value,
            date,
            unit,
            note,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_body_client(&u).await?;
            let m = resolve_body_target(&client, &target).await?;
            let day = match date {
                Some(s) => chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .map_err(|e| eyre::eyre!("--date: {e}"))?,
                None => chrono::Local::now().date_naive(),
            };
            let entry = body::model::BodyEntry {
                id: uuid::Uuid::new_v4(),
                date: day,
                value,
                unit,
                note,
            };
            let updated = client
                .log_entry(m.id.to_string(), entry)
                .await
                .map_err(|e| eyre::eyre!("log_entry: {e:?}"))?;
            println!(
                "logged {} {} on {} for {}",
                value, updated.unit, day, updated.name
            );
        }
        BodyCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_body_client(&u).await?;
            let m = resolve_body_target(&client, &target).await?;
            if !yes && !confirm(&format!("delete `{}` ({})?", m.name, m.path))? {
                println!("aborted");
                return Ok(());
            }
            client
                .delete(m.id.to_string())
                .await
                .map_err(|e| eyre::eyre!("delete: {e:?}"))?;
            println!("deleted {}", m.path);
        }
    }
    Ok(())
}
