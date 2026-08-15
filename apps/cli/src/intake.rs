//! `task intake …` — food intake log.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::pantry::connect_pantry_client;
use crate::pantry::resolve_pantry_target;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;

// ── Intake (intake::Store) ───────────────────────────────────────────

#[derive(Subcommand)]
pub(crate) enum IntakeCmd {
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
    /// Get the intake log for `YYYY-MM-DD`. Creates empty
    /// if missing.
    ForDay {
        date: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    LogRecipe {
        date: String,
        recipe: String,
        servings: f64,
        #[arg(long, default_value = "snack")]
        slot: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    LogPantry {
        date: String,
        item: String,
        qty: f64,
        #[arg(long, default_value = "snack")]
        slot: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    LogFreeform {
        date: String,
        name: String,
        #[arg(long)]
        kcal: Option<f64>,
        #[arg(long, default_value = "snack")]
        slot: String,
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

async fn connect_intake_client(url: &str) -> eyre::Result<intake::IntakeServiceClient> {
    establish_for_url(url).await
}

async fn resolve_intake_target(
    client: &intake::IntakeServiceClient,
    target: &str,
) -> eyre::Result<intake::IntakeLog> {
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
        .find(|l| l.path == target || l.date.to_string() == target)
        .ok_or_else(|| {
            crate::errors::not_found("resolve target", target)
                .cause("no path or name match")
                .report()
        })
}

pub(crate) async fn run_intake(cmd: IntakeCmd) -> eyre::Result<()> {
    match cmd {
        IntakeCmd::List { org, server, json } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_intake_client(&u).await?;
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
            for l in &rows {
                println!(
                    "{}  {:<24}  {} entries    {}",
                    l.date,
                    l.name,
                    l.entries.0.len(),
                    l.path
                );
            }
        }
        IntakeCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_intake_client(&u).await?;
            let l = resolve_intake_target(&client, &target).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&l).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{} ({})\n", l.name, l.date);
            println!("  id:    {}", l.id);
            println!("  path:  {}", l.path);
            for e in &l.entries.0 {
                let slot = e.slot.as_deref().unwrap_or("?");
                println!("    [{slot}] {}", e.name);
            }
        }
        IntakeCmd::ForDay {
            date,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_intake_client(&u).await?;
            let l = client
                .for_day(date.clone())
                .await
                .map_err(|e| eyre::eyre!("for_day: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&l).map_err(|e| eyre::eyre!("json: {e}"))?
                );
            } else {
                println!("{} ({})", l.name, l.date);
                println!("  {} entries", l.entries.0.len());
            }
        }
        IntakeCmd::LogRecipe {
            date,
            recipe,
            servings,
            slot,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_intake_client(&u).await?;
            let updated = client
                .log_recipe(date, recipe, servings, slot)
                .await
                .map_err(|e| eyre::eyre!("log_recipe: {e:?}"))?;
            println!(
                "logged → {} entries on {}",
                updated.entries.0.len(),
                updated.date
            );
        }
        IntakeCmd::LogPantry {
            date,
            item,
            qty,
            slot,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_intake_client(&u).await?;
            // Resolve pantry path/name → id.
            let pc = connect_pantry_client(&u).await?;
            let p = resolve_pantry_target(&pc, &item).await?;
            let updated = client
                .log_pantry(date, p.id.to_string(), qty, slot)
                .await
                .map_err(|e| eyre::eyre!("log_pantry: {e:?}"))?;
            println!(
                "logged → {} entries on {}",
                updated.entries.0.len(),
                updated.date
            );
        }
        IntakeCmd::LogFreeform {
            date,
            name,
            kcal,
            slot,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_intake_client(&u).await?;
            let nutrition = cookbook::Nutrition {
                calories: kcal,
                protein_g: None,
                carbs_g: None,
                fat_g: None,
                fiber_g: None,
                sugar_g: None,
            };
            let updated = client
                .log_freeform(date, name, nutrition, slot)
                .await
                .map_err(|e| eyre::eyre!("log_freeform: {e:?}"))?;
            println!(
                "logged → {} entries on {}",
                updated.entries.0.len(),
                updated.date
            );
        }
        IntakeCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_intake_client(&u).await?;
            let l = resolve_intake_target(&client, &target).await?;
            if !yes && !confirm(&format!("delete `{}` ({})?", l.name, l.path))? {
                println!("aborted");
                return Ok(());
            }
            client
                .delete(l.id.to_string())
                .await
                .map_err(|e| eyre::eyre!("delete: {e:?}"))?;
            println!("deleted {}", l.path);
        }
    }
    Ok(())
}
