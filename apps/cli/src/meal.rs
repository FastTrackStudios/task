//! `task meal …` — scheduled meals + cooking lifecycle.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;

// ── Meal (mealplan::Store) ───────────────────────────────────────────

#[derive(Subcommand)]
pub(crate) enum MealCmd {
    /// List meals. Filters compose (AND).
    List {
        /// Only meals scheduled on this date (`YYYY-MM-DD`).
        #[arg(long)]
        date: Option<String>,
        /// `planned|cooked|skipped|eating-out`.
        #[arg(long)]
        status: Option<String>,
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
        /// `YYYY-MM-DD`. Required.
        #[arg(long)]
        date: String,
        /// `breakfast|lunch|dinner|snack`. Default `dinner`.
        #[arg(long)]
        slot: Option<String>,
        /// Recipe paths (repeatable or comma-separated).
        #[arg(long, value_delimiter = ',')]
        recipe: Vec<String>,
        #[arg(long, default_value_t = 1)]
        servings: u32,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
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
    },
    /// Mark cooked. Pantry deductions auto-computed from
    /// the recipe's `can_cook` check; pass `--no-deduct`
    /// to skip pantry adjustment (e.g. ate-out leftovers).
    Cook {
        target: String,
        #[arg(long)]
        no_deduct: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    Skip {
        target: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Mark the meal as eaten out — planned, not cooked, eaten anyway.
    /// Distinct from `skip`, which is a meal that didn't happen.
    #[command(name = "eat-out")]
    EatOut {
        target: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Put the meal on its date's day plan as a `Meal` block
    /// (`task meal schedule <meal> 17:30-18:30`). Overlapping
    /// blocks are rejected unless `--force`.
    Schedule(crate::mealprep::MealScheduleArgs),
    Rename {
        target: String,
        new_path: String,
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

async fn connect_mealplan_client(url: &str) -> eyre::Result<mealplan::MealplanServiceClient> {
    establish_for_url(url).await
}

pub(crate) async fn resolve_meal_target(
    client: &mealplan::MealplanServiceClient,
    target: &str,
) -> eyre::Result<mealplan::Meal> {
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
    rows.into_iter().find(|m| m.path == target).ok_or_else(|| {
        crate::errors::not_found("resolve target", target)
            .cause("no path or name match")
            .report()
    })
}

pub(crate) async fn run_meal(cmd: MealCmd) -> eyre::Result<()> {
    match cmd {
        MealCmd::List {
            date,
            status,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_mealplan_client(&u).await?;
            let parsed_date = match date {
                None => None,
                Some(s) => Some(
                    chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                        .map_err(|e| eyre::eyre!("--date: {e}"))?,
                ),
            };
            let rows: Vec<_> = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?
                .into_iter()
                .filter(|m| parsed_date.is_none_or(|d| m.scheduled_for == d))
                .filter(|m| status.as_deref().is_none_or(|s| m.status == s))
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            for m in &rows {
                println!(
                    "{}  {}  {:<10}  {:<10}    {}",
                    m.scheduled_for, m.slot, m.status, m.name, m.path
                );
            }
        }
        MealCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_mealplan_client(&u).await?;
            let m = resolve_meal_target(&client, &target).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&m).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{} [{}]\n", m.name, m.status);
            println!("  id:       {}", m.id);
            println!("  path:     {}", m.path);
            println!("  date:     {}", m.scheduled_for);
            println!("  slot:     {}", m.slot);
            println!("  servings: {}", m.servings);
            for r in m.recipe_paths.iter() {
                println!("  recipe:   {r}");
            }
        }
        MealCmd::Create {
            name,
            date,
            slot,
            recipe,
            servings,
            tags,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_mealplan_client(&u).await?;
            let scheduled_for = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                .map_err(|e| eyre::eyre!("--date: {e}"))?;
            // Accept recipe display names as well as `.cook` paths.
            let recipe = crate::mealprep::resolve_recipe_refs(&u, recipe).await?;
            let new_meal = mealplan::Meal {
                id: uuid::Uuid::nil(),
                path: String::new(),
                name,
                scheduled_for,
                slot: slot.unwrap_or_else(|| "dinner".into()),
                servings,
                recipe_paths: mealplan::model::StringList(recipe),
                status: "planned".into(),
                pantry_deductions: mealplan::model::PantryDeductions::default(),
                tags: mealplan::model::StringList(tags),
                date_created: None,
                date_modified: None,
                details: String::new(),
            };
            let created = client
                .create(new_meal)
                .await
                .map_err(|e| eyre::eyre!("create: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&created).map_err(|e| eyre::eyre!("json: {e}"))?
                );
            } else {
                println!(
                    "created {} for {} ({})",
                    created.name, created.scheduled_for, created.path
                );
                println!("  id: {}", created.id);
            }
        }
        MealCmd::SetStatus {
            target,
            status,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_mealplan_client(&u).await?;
            let mut m = resolve_meal_target(&client, &target).await?;
            m.status = status;
            let updated = client
                .update(m)
                .await
                .map_err(|e| eyre::eyre!("update: {e:?}"))?;
            println!("{}  [{}]  {}", updated.name, updated.status, updated.path);
        }
        MealCmd::Cook {
            target,
            no_deduct,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_mealplan_client(&u).await?;
            let m = resolve_meal_target(&client, &target).await?;
            // Auto-deduction logic lives server-side under
            // `can_cook` / `cook` — we pass an empty list and
            // let the server fill in from the recipes. The
            // `--no-deduct` flag is reserved for the future
            // ate-out-leftovers path; today both routes pass
            // the same empty list.
            let _ = no_deduct;
            let deductions = Vec::new();
            let cooked = client
                .cook(m.id.to_string(), deductions)
                .await
                .map_err(|e| eyre::eyre!("cook: {e:?}"))?;
            println!("cooked {}  ({})", cooked.name, cooked.path);
        }
        MealCmd::Skip {
            target,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_mealplan_client(&u).await?;
            let m = resolve_meal_target(&client, &target).await?;
            let skipped = client
                .skip(m.id.to_string())
                .await
                .map_err(|e| eyre::eyre!("skip: {e:?}"))?;
            println!("skipped {}  ({})", skipped.name, skipped.path);
        }
        MealCmd::EatOut {
            target,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_mealplan_client(&u).await?;
            let m = resolve_meal_target(&client, &target).await?;
            let out = client
                .eat_out(m.id.to_string())
                .await
                .map_err(|e| eyre::eyre!("eat-out: {e:?}"))?;
            println!("ate out {}  ({})", out.name, out.path);
        }
        MealCmd::Schedule(a) => return crate::mealprep::meal_schedule(a).await,
        MealCmd::Rename {
            target,
            new_path,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_mealplan_client(&u).await?;
            let m = resolve_meal_target(&client, &target).await?;
            let renamed = client
                .rename(m.id.to_string(), new_path)
                .await
                .map_err(|e| eyre::eyre!("rename: {e:?}"))?;
            println!("renamed → {}", renamed.path);
        }
        MealCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_mealplan_client(&u).await?;
            let m = resolve_meal_target(&client, &target).await?;
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
