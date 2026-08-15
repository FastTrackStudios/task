//! `task recipe …` — cooklang cookbook recipes.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;

// ── Recipe (cookbook::Store) — read + delete only ─────────────────────

#[derive(Subcommand)]
pub(crate) enum RecipeCmd {
    /// List every recipe in the active org's cookbook.
    List {
        /// Substring filter on title.
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Get {
        /// Recipe path (e.g. `Wiki/Cookbook/Oatmeal.cook`).
        path: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Author a new `.cook` recipe (validates the cooklang by
    /// parsing before anything is written).
    Create(crate::mealprep::RecipeCreateArgs),
    /// Import a recipe from a webpage (schema.org/Recipe → cooklang;
    /// LLM-synthesized, `--offline` for the deterministic converter,
    /// `--from-file` for bot-protected sites).
    Import(crate::recipe_import::RecipeImportArgs),
    /// Replace an existing recipe's cooklang source (validates
    /// by parsing first).
    Update(crate::mealprep::RecipeUpdateArgs),
    /// Rendered view — ingredients / cookware / steps /
    /// servings (`--json` for the wire shape).
    Show(crate::mealprep::RecipeShowArgs),
    /// Fulfillment check against the pantry: have / missing /
    /// substitution suggestions.
    CanCook(crate::mealprep::CanCookArgs),
    /// Attach a picture to a recipe. Names it by the cooklang
    /// convention — `<recipe>.jpg` is the dish, `<recipe>.<n>.jpg`
    /// belongs to step `n` — so the cookbook stays portable to the
    /// other tools that read it.
    Image {
        /// Recipe path, display name, or file stem.
        recipe: String,
        /// Local image file to upload.
        file: std::path::PathBuf,
        /// Attach to this step instead of the dish itself (0-based).
        #[arg(long)]
        step: Option<u32>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    Delete {
        path: String,
        #[arg(long, short = 'y')]
        yes: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

async fn connect_cookbook_client(url: &str) -> eyre::Result<cookbook::CookbookServiceClient> {
    establish_for_url(url).await
}

pub(crate) async fn run_recipe(cmd: RecipeCmd) -> eyre::Result<()> {
    match cmd {
        RecipeCmd::List {
            query,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_cookbook_client(&u).await?;
            let rows: Vec<_> = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?
                .into_iter()
                .filter(|r| {
                    query
                        .as_deref()
                        .is_none_or(|q| r.name.to_lowercase().contains(&q.to_lowercase()))
                })
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{} recipes\n", rows.len());
            for r in &rows {
                let s = r
                    .servings
                    .map(|n| format!("  ({n} srv)"))
                    .unwrap_or_default();
                println!("{:<40}{s}    {}", r.name, r.path);
            }
        }
        RecipeCmd::Get {
            path,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_cookbook_client(&u).await?;
            let r = client
                .get(path)
                .await
                .map_err(|e| eyre::eyre!("get: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&r).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{}\n", r.name);
            println!("  path:     {}", r.path);
            if let Some(s) = r.servings {
                println!("  servings: {s}");
            }
            if !r.ingredients.0.is_empty() {
                println!("  ingredients ({} items):", r.ingredients.0.len());
                for i in r.ingredients.0.iter().take(20) {
                    println!("    - {} {} {}", i.qty.unwrap_or(0.0), i.unit, i.name);
                }
            }
        }
        RecipeCmd::Create(a) => return crate::mealprep::recipe_create(a).await,
        RecipeCmd::Import(a) => return crate::recipe_import::recipe_import(a).await,
        RecipeCmd::Update(a) => return crate::mealprep::recipe_update(a).await,
        RecipeCmd::Show(a) => return crate::mealprep::recipe_show(a).await,
        RecipeCmd::CanCook(a) => return crate::mealprep::recipe_can_cook(a).await,
        RecipeCmd::Image {
            recipe,
            file,
            step,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_cookbook_client(&u).await?;
            let target = crate::mealprep::resolve_recipe(&client, &recipe)
                .await?
                .path;

            let ext = file
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_else(|| "jpg".into());
            let stem = target.trim_end_matches(".cook");
            let dest = match step {
                Some(n) => format!("{stem}.{n}.{ext}"),
                None => format!("{stem}.{ext}"),
            };

            let bytes =
                std::fs::read(&file).map_err(|e| eyre::eyre!("read {}: {e}", file.display()))?;
            let len = bytes.len();
            client
                .put_image(dest.clone(), bytes)
                .await
                .map_err(|e| eyre::eyre!("put image: {e:?}"))?;
            println!("attached {dest} ({len} bytes)");
        }
        RecipeCmd::Delete {
            path,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_cookbook_client(&u).await?;
            if !yes && !confirm(&format!("delete `{path}`?"))? {
                println!("aborted");
                return Ok(());
            }
            client
                .delete(path.clone())
                .await
                .map_err(|e| eyre::eyre!("delete: {e:?}"))?;
            println!("deleted {path}");
        }
    }
    Ok(())
}
