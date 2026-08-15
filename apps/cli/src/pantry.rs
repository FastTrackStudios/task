//! `task pantry …` — stocked food items.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::location::connect_locations_client;
use crate::location::resolve_location_target;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;
use crate::shared::resolve_body;

// ── Pantry (pantry::Store) ───────────────────────────────────────────

#[derive(Subcommand)]
pub(crate) enum PantryCmd {
    List {
        #[arg(long)]
        low_stock: bool,
        #[arg(long)]
        expired: bool,
        /// Only items expiring within N days (uses
        /// `best_before` stock entries).
        #[arg(long)]
        expiring_in: Option<i64>,
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
        #[arg(long)]
        qty: Option<f64>,
        /// Unit slug (`g` / `ml` / `each` / `cup` / `clove`).
        #[arg(long)]
        unit: Option<String>,
        /// Location id or path.
        #[arg(long)]
        location: Option<String>,
        /// Free-form food category.
        #[arg(long)]
        food_category: Option<String>,
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
    /// Decrement `qty` by `amount`.
    Consume {
        target: String,
        amount: f64,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Increment `qty` by `amount`.
    Restock {
        target: String,
        amount: f64,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Mark opened; stamps today onto `openedDate`.
    Open {
        target: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Find a pantry row by barcode. `--resolve` falls back
    /// to OpenFoodFacts if no local match.
    FindByBarcode {
        barcode: String,
        #[arg(long)]
        resolve: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
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

pub(crate) async fn connect_pantry_client(url: &str) -> eyre::Result<pantry::PantryServiceClient> {
    establish_for_url(url).await
}

pub(crate) async fn resolve_pantry_target(
    client: &pantry::PantryServiceClient,
    target: &str,
) -> eyre::Result<pantry::PantryItem> {
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
        .find(|p| p.path == target || p.name == target)
        .ok_or_else(|| {
            crate::errors::not_found("resolve target", target)
                .cause("no path or name match")
                .report()
        })
}

pub(crate) async fn run_pantry(cmd: PantryCmd) -> eyre::Result<()> {
    match cmd {
        PantryCmd::List {
            low_stock,
            expired,
            expiring_in,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_pantry_client(&u).await?;
            let today = chrono::Local::now().date_naive();
            let rows: Vec<_> = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?
                .into_iter()
                .filter(|p| !low_stock || p.qty.is_some_and(|q| q < 1.0))
                .filter(|p| {
                    !expired
                        || p.stock_entries
                            .iter()
                            .any(|e| e.best_before.is_some_and(|d| d < today))
                })
                .filter(|p| {
                    expiring_in.is_none_or(|n| {
                        let cutoff = today + chrono::Duration::days(n);
                        p.stock_entries
                            .iter()
                            .any(|e| e.best_before.is_some_and(|d| d <= cutoff && d >= today))
                    })
                })
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            for p in &rows {
                let q = p
                    .qty
                    .map_or_else(|| "?".into(), |n| format!("{n} {}", p.unit));
                println!("{:<32}  {:<12}    {}", p.name, q, p.path);
            }
        }
        PantryCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_pantry_client(&u).await?;
            let p = resolve_pantry_target(&client, &target).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&p).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{}\n", p.name);
            println!("  id:       {}", p.id);
            println!("  path:     {}", p.path);
            println!("  status:   {}", p.status);
            if let Some(q) = p.qty {
                println!("  qty:      {q} {}", p.unit);
            }
            if !p.food_category.is_empty() {
                println!("  food:     {}", p.food_category);
            }
            if let Some(l) = p.location_id {
                println!("  location: {l}");
            }
        }
        PantryCmd::Create {
            name,
            qty,
            unit,
            location,
            food_category,
            tags,
            details,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_pantry_client(&u).await?;
            let location_id = match location {
                None => None,
                Some(loc) => {
                    let lc = connect_locations_client(&u).await?;
                    Some(resolve_location_target(&lc, &loc).await?.id)
                }
            };
            // PantryItem has many fields; use the
            // `PantryItemDraft::into_item` helper to construct
            // a fully-defaulted item from a minimal draft.
            let draft = pantry::PantryItemDraft {
                barcode: String::new(),
                name,
                brand: None,
                food_category: food_category.unwrap_or_default(),
                unit: unit.unwrap_or_default(),
                nutrition_per_unit: None,
                nutrition_unit: None,
                image_url: None,
            };
            let mut new_item = draft.into_item(location_id);
            new_item.qty = qty;
            if !tags.is_empty() {
                new_item.tags = pantry::model::StringList(tags);
            }
            new_item.details = resolve_body(details)?;
            let created = client
                .create(new_item)
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
        PantryCmd::Consume {
            target,
            amount,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_pantry_client(&u).await?;
            let p = resolve_pantry_target(&client, &target).await?;
            let updated = client
                .consume(p.id.to_string(), amount)
                .await
                .map_err(|e| eyre::eyre!("consume: {e:?}"))?;
            let q = updated.qty.map_or_else(|| "?".into(), |n| n.to_string());
            println!("{}  qty={q} {}", updated.name, updated.unit);
        }
        PantryCmd::Restock {
            target,
            amount,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_pantry_client(&u).await?;
            let p = resolve_pantry_target(&client, &target).await?;
            let updated = client
                .restock(p.id.to_string(), amount)
                .await
                .map_err(|e| eyre::eyre!("restock: {e:?}"))?;
            let q = updated.qty.map_or_else(|| "?".into(), |n| n.to_string());
            println!("{}  qty={q} {}", updated.name, updated.unit);
        }
        PantryCmd::Open {
            target,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_pantry_client(&u).await?;
            let p = resolve_pantry_target(&client, &target).await?;
            let updated = client
                .open(p.id.to_string())
                .await
                .map_err(|e| eyre::eyre!("open: {e:?}"))?;
            println!("opened {}", updated.name);
        }
        PantryCmd::FindByBarcode {
            barcode,
            resolve,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_pantry_client(&u).await?;
            if resolve {
                let r = client
                    .resolve_barcode(barcode)
                    .await
                    .map_err(|e| eyre::eyre!("resolve_barcode: {e:?}"))?;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&r).map_err(|e| eyre::eyre!("json: {e}"))?
                    );
                } else {
                    println!("{r:#?}");
                }
            } else {
                let p = client
                    .find_by_barcode(barcode)
                    .await
                    .map_err(|e| eyre::eyre!("find_by_barcode: {e:?}"))?;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&p).map_err(|e| eyre::eyre!("json: {e}"))?
                    );
                } else {
                    println!("{} ({})", p.name, p.path);
                }
            }
        }
        PantryCmd::Rename {
            target,
            new_path,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_pantry_client(&u).await?;
            let p = resolve_pantry_target(&client, &target).await?;
            let renamed = client
                .rename(p.id.to_string(), new_path)
                .await
                .map_err(|e| eyre::eyre!("rename: {e:?}"))?;
            println!("renamed → {}", renamed.path);
        }
        PantryCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_pantry_client(&u).await?;
            let p = resolve_pantry_target(&client, &target).await?;
            if !yes && !confirm(&format!("delete `{}` ({})?", p.name, p.path))? {
                println!("aborted");
                return Ok(());
            }
            client
                .delete(p.id.to_string())
                .await
                .map_err(|e| eyre::eyre!("delete: {e:?}"))?;
            println!("deleted {}", p.path);
        }
    }
    Ok(())
}
