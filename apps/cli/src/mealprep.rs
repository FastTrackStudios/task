//! Mealprep-loop verbs (issue 674dadfe) — recipe authoring +
//! fulfillment, the `task shopping` family, and meal → day-plan
//! scheduling.
//!
//! Lives outside `main.rs` so concurrent CLI work doesn't
//! collide: `main.rs` only carries thin enum variants that
//! delegate here (clap `Args` structs are defined in this
//! module and wrapped as tuple variants).
//!
//! Write paths used:
//! - recipes: `CookbookService::create/update` RPC (server
//!   writes `<wiki>/Cookbook/<slug>.cook` via `write_cook`) —
//!   no raw fs writes from the CLI.
//! - shopping: `ShoppingService` RPC (`<vault>/shopping/*.md`).
//! - day plans: `DayPlans` RPC (`Records/dayplans/<date>.md`).

use clap::{Args, Subcommand};
use cookbook::CookbookServiceClient;
use mealplan::MealplanServiceClient;
use mealplan::shopping::{ShoppingList, ShoppingServiceClient};
use scheduling_proto::{
    BlockAssignment, BlockCategory, DayPlan, DayPlansClient, PlannedBlock, TimeBlockId, TimeOfDay,
};

use crate::establish_for_url;
use crate::meal::resolve_meal_target;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;

// ── Shared flags ─────────────────────────────────────────────

/// `--org` / `--server` routing flags shared by every verb here.
#[derive(Args)]
pub struct Remote {
    #[arg(long)]
    pub org: Option<String>,
    #[arg(long)]
    pub server: Option<String>,
}

impl Remote {
    /// Resolve the per-org vox URL from the flags + session.
    pub(crate) fn url(&self) -> eyre::Result<String> {
        let slug = resolve_active_org(self.org.clone())?;
        Ok(resolve_org_vox_url(self.server.clone(), &slug))
    }
}

// ── Cooklang validation ──────────────────────────────────────

/// Parse `source` as cooklang (all extensions, bundled units —
/// the same parser config as `cookbook::parse`). `Ok(true)` when
/// the source carries a title; `Err` is the parser's rendered
/// report **with line context** (codesnake snippets).
pub(crate) fn validate_cooklang(file_label: &str, source: &str) -> Result<bool, String> {
    let parser =
        cooklang::CooklangParser::new(cooklang::Extensions::all(), cooklang::Converter::bundled());
    match parser.parse(source).into_result() {
        Ok((recipe, _warnings)) => Ok(recipe.metadata.title().is_some()),
        Err(report) => {
            let mut buf = Vec::new();
            if report.write(file_label, source, false, &mut buf).is_err() {
                return Err(format!("{report}"));
            }
            Err(String::from_utf8_lossy(&buf).into_owned())
        }
    }
}

/// Read the cooklang source: `--file <path>` or stdin.
fn read_source(file: Option<&std::path::Path>) -> eyre::Result<String> {
    if let Some(p) = file {
        return std::fs::read_to_string(p).map_err(|e| eyre::eyre!("read {}: {e}", p.display()));
    }
    use std::io::Read as _;
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| eyre::eyre!("read stdin: {e}"))?;
    if buf.trim().is_empty() {
        eyre::bail!("empty source — pass --file <path> or pipe cooklang on stdin");
    }
    Ok(buf)
}

/// `true` when `s` looks like a `.cook` file path rather than a
/// display name.
fn is_cook_path(s: &str) -> bool {
    std::path::Path::new(s)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cook"))
}

/// Validate, and bail (printing the rendered report) on parse
/// errors. Returns the source to persist — when the recipe has
/// no title metadata and no frontmatter, a `>> title:` line is
/// prepended so `<name>` survives as the display title.
pub(crate) fn validated_source(name: &str, source: String) -> eyre::Result<String> {
    let label = format!("{name}.cook");
    match validate_cooklang(&label, &source) {
        Err(report) => {
            eprintln!("{report}");
            eyre::bail!("cooklang validation failed — nothing was written")
        }
        Ok(true) => Ok(source),
        Ok(false) if source.starts_with("---") => Ok(source),
        Ok(false) => Ok(format!(">> title: {name}\n\n{source}")),
    }
}

// ── Recipe resolution ────────────────────────────────────────

/// Resolve a recipe by vault path (`Cookbook/x.cook`), display
/// name (case-insensitive), or file stem.
pub(crate) async fn resolve_recipe(
    client: &CookbookServiceClient,
    target: &str,
) -> eyre::Result<cookbook::Recipe> {
    if is_cook_path(target) {
        if let Ok(r) = client.get(target.to_string()).await {
            return Ok(r);
        }
    }
    let rows = client
        .list()
        .await
        .map_err(|e| eyre::eyre!("list: {e:?}"))?;
    let t = target.to_lowercase();
    rows.into_iter()
        .find(|r| {
            r.path == target
                || r.name.to_lowercase() == t
                || std::path::Path::new(&r.path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.to_lowercase() == t)
        })
        .ok_or_else(|| {
            crate::errors::not_found("resolve recipe", target)
                .cause("no path or name match")
                .hint("try `task recipe list`")
                .report()
        })
}

/// Map mixed recipe refs (paths and/or display names) to vault
/// paths. Used by `meal create --recipe <name>` so an agent can
/// plan a week without knowing file paths.
pub(crate) async fn resolve_recipe_refs(url: &str, refs: Vec<String>) -> eyre::Result<Vec<String>> {
    if refs.iter().all(|r| is_cook_path(r)) {
        return Ok(refs);
    }
    let client: CookbookServiceClient = establish_for_url(url).await?;
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        if is_cook_path(&r) {
            out.push(r);
        } else {
            out.push(resolve_recipe(&client, &r).await?.path);
        }
    }
    Ok(out)
}

// ── recipe create / update / show / can-cook ─────────────────

#[derive(Args)]
pub struct RecipeCreateArgs {
    /// Display name; the file lands at `Cookbook/<slug>.cook`.
    pub name: String,
    /// Read cooklang source from this file (default: stdin).
    #[arg(long)]
    pub file: Option<std::path::PathBuf>,
    #[command(flatten)]
    pub remote: Remote,
    #[arg(long)]
    pub json: bool,
}

pub async fn recipe_create(a: RecipeCreateArgs) -> eyre::Result<()> {
    let source = read_source(a.file.as_deref())?;
    let source = validated_source(&a.name, source)?;
    let path = cookbook::default_recipe_path(&a.name, None);
    let recipe = cookbook::parse_cook(&path, &source).map_err(|e| eyre::eyre!("parse: {e}"))?;
    let client: CookbookServiceClient = establish_for_url(&a.remote.url()?).await?;
    let created = client
        .create(recipe)
        .await
        .map_err(|e| eyre::eyre!("create: {e:?}"))?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&created)?);
    } else {
        println!("created {}  ({})", created.name, created.path);
    }
    Ok(())
}

#[derive(Args)]
pub struct RecipeUpdateArgs {
    /// Recipe path, display name, or file stem.
    pub name: String,
    /// Read the replacement cooklang source from this file
    /// (default: stdin).
    #[arg(long)]
    pub file: Option<std::path::PathBuf>,
    #[command(flatten)]
    pub remote: Remote,
    #[arg(long)]
    pub json: bool,
}

pub async fn recipe_update(a: RecipeUpdateArgs) -> eyre::Result<()> {
    let source = read_source(a.file.as_deref())?;
    let client: CookbookServiceClient = establish_for_url(&a.remote.url()?).await?;
    let existing = resolve_recipe(&client, &a.name).await?;
    let source = validated_source(&existing.name, source)?;
    let recipe =
        cookbook::parse_cook(&existing.path, &source).map_err(|e| eyre::eyre!("parse: {e}"))?;
    let updated = client
        .update(recipe)
        .await
        .map_err(|e| eyre::eyre!("update: {e:?}"))?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&updated)?);
    } else {
        println!("updated {}  ({})", updated.name, updated.path);
    }
    Ok(())
}

#[derive(Args)]
pub struct RecipeShowArgs {
    /// Recipe path, display name, or file stem.
    pub name: String,
    #[command(flatten)]
    pub remote: Remote,
    #[arg(long)]
    pub json: bool,
}

pub async fn recipe_show(a: RecipeShowArgs) -> eyre::Result<()> {
    let client: CookbookServiceClient = establish_for_url(&a.remote.url()?).await?;
    let r = resolve_recipe(&client, &a.name).await?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&r)?);
        return Ok(());
    }
    println!("{}  ({})", r.name, r.path);
    if let Some(d) = &r.description {
        println!("  {d}");
    }
    let mut meta = Vec::new();
    if let Some(s) = r.servings {
        meta.push(format!("servings: {s}"));
    }
    if let Some(m) = r.prep_minutes {
        meta.push(format!("prep: {m}m"));
    }
    if let Some(m) = r.cook_minutes {
        meta.push(format!("cook: {m}m"));
    }
    if let Some(c) = &r.course {
        meta.push(format!("course: {c}"));
    }
    if !meta.is_empty() {
        println!("  {}", meta.join("   "));
    }
    if !r.ingredients.is_empty() {
        println!("\n  ingredients:");
        for i in r.ingredients.iter() {
            let qty = i
                .qty_display
                .clone()
                .or_else(|| i.qty.map(|q| q.to_string()))
                .unwrap_or_default();
            let mut line = format!("{qty} {} {}", i.unit, i.name)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if let Some(alias) = &i.alias {
                line.push_str(&format!(" (as {alias})"));
            }
            if i.optional {
                line.push_str(" [optional]");
            }
            if let Some(n) = &i.note {
                line.push_str(&format!(" — {n}"));
            }
            if i.is_recipe_ref {
                line.push_str(" [recipe]");
            }
            println!("    - {line}");
        }
    }
    if !r.cookware.is_empty() {
        println!("\n  cookware:");
        for c in r.cookware.iter() {
            println!("    - {c}");
        }
    }
    if !r.steps.is_empty() {
        println!("\n  steps:");
        for (n, s) in r.steps.iter().enumerate() {
            println!("    {}. {s}", n + 1);
        }
    }
    Ok(())
}

#[derive(Args)]
pub struct CanCookArgs {
    /// Recipe path, display name, or file stem.
    pub name: String,
    /// Servings to scale to (default: the recipe's own yield).
    #[arg(long)]
    pub servings: Option<u32>,
    #[command(flatten)]
    pub remote: Remote,
    #[arg(long)]
    pub json: bool,
}

pub async fn recipe_can_cook(a: CanCookArgs) -> eyre::Result<()> {
    let url = a.remote.url()?;
    let cookbook_client: CookbookServiceClient = establish_for_url(&url).await?;
    let r = resolve_recipe(&cookbook_client, &a.name).await?;
    let servings = a.servings.or(r.servings).unwrap_or(1);
    let mealplan_client: MealplanServiceClient = establish_for_url(&url).await?;
    // The service returns the full have/missing partition — no local
    // re-derivation; the UI renders the same `Fulfillment`.
    let f = mealplan_client
        .can_cook(r.path.clone(), servings)
        .await
        .map_err(|e| eyre::eyre!("can_cook: {e:?}"))?;
    if a.json {
        let out = serde_json::json!({
            "recipe": r.name,
            "path": r.path,
            "servings": servings,
            "canCook": f.can_cook,
            "have": f.have,
            "missing": f.missing,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!(
        "{} × {servings} servings — {}",
        r.name,
        if f.can_cook {
            "CAN cook"
        } else {
            "CANNOT cook"
        }
    );
    if !f.have.is_empty() {
        println!("\n  have:");
        for h in &f.have {
            match h.need {
                Some(n) => println!("    - {} ({n:.2} {})", h.name, h.unit),
                None => println!("    - {}", h.name),
            }
        }
    }
    if !f.missing.is_empty() {
        println!("\n  missing:");
        for s in &f.missing {
            println!(
                "    - {}: need {:.2} {} (have {:.2}) — {}",
                s.name,
                s.need,
                s.unit,
                s.have,
                s.reason.label()
            );
            for sub in &s.suggestions {
                println!(
                    "        substitute: {} ×{} (need {:.2}{})",
                    sub.name,
                    sub.ratio,
                    sub.need,
                    sub.have
                        .map(|h| format!(", have {h:.2}"))
                        .unwrap_or_default()
                );
            }
        }
    }
    Ok(())
}

// ── task shopping ────────────────────────────────────────────

#[derive(Subcommand)]
pub enum ShoppingCmd {
    /// Show the shopping list. The first list in the vault is
    /// the default; `--list` selects by name.
    List {
        #[arg(long)]
        list: Option<String>,
        #[command(flatten)]
        remote: Remote,
        #[arg(long)]
        json: bool,
    },
    /// Add a recipe's missing ingredients (vs current pantry
    /// stock) to the list.
    AddMissing {
        /// Recipe path, display name, or file stem.
        #[arg(long)]
        recipe: String,
        /// Servings to scale to (default: the recipe's yield).
        #[arg(long)]
        servings: Option<u32>,
        #[arg(long)]
        list: Option<String>,
        #[command(flatten)]
        remote: Remote,
        #[arg(long)]
        json: bool,
    },
    /// Add every pantry item at/below its reorder minimum.
    AddLowStock {
        #[arg(long)]
        list: Option<String>,
        #[command(flatten)]
        remote: Remote,
        #[arg(long)]
        json: bool,
    },
    /// Add every pantry item past its best-before date.
    AddExpired {
        #[arg(long)]
        list: Option<String>,
        #[command(flatten)]
        remote: Remote,
        #[arg(long)]
        json: bool,
    },
    /// Mark an entry purchased (by name). Entries linked to a
    /// pantry item restock it automatically.
    MarkPurchased {
        item: String,
        /// Override the purchased quantity before restocking.
        #[arg(long)]
        qty: Option<f64>,
        #[arg(long)]
        list: Option<String>,
        #[command(flatten)]
        remote: Remote,
        #[arg(long)]
        json: bool,
    },
    /// Remove entries matching `item` (case-insensitive name).
    Rm {
        item: String,
        #[arg(long)]
        list: Option<String>,
        #[command(flatten)]
        remote: Remote,
        #[arg(long)]
        json: bool,
    },
    /// Tick an entry off because it's already in the kitchen —
    /// the first pass, before leaving for the shop. Unlike
    /// `mark-purchased` this never touches pantry stock.
    MarkHave {
        item: String,
        /// Put it back on the list instead (after a miscount).
        #[arg(long)]
        undo: bool,
        #[arg(long)]
        list: Option<String>,
        #[command(flatten)]
        remote: Remote,
        #[arg(long)]
        json: bool,
    },
    /// Put every entry back to `needed`, keeping the rows — run
    /// the same list again next week without retyping it.
    Reset {
        #[arg(long)]
        list: Option<String>,
        #[command(flatten)]
        remote: Remote,
        #[arg(long)]
        json: bool,
    },
    /// Keep a list's rows as a reusable template.
    SaveTemplate {
        /// Name for the template.
        name: String,
        #[arg(long)]
        list: Option<String>,
        #[command(flatten)]
        remote: Remote,
        #[arg(long)]
        json: bool,
    },
    /// Start a fresh run from a template; the template is left
    /// untouched so it can be started again.
    Start {
        /// Template name.
        template: String,
        /// Name for the new run (default: the template's name).
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        remote: Remote,
        #[arg(long)]
        json: bool,
    },
}

/// First list in the vault (or by `--list` name); creates a
/// default "Shopping" list on first use.
async fn pick_list(
    client: &ShoppingServiceClient,
    name: Option<&str>,
) -> eyre::Result<ShoppingList> {
    let rows = client
        .list()
        .await
        .map_err(|e| eyre::eyre!("shopping list: {e:?}"))?;
    if let Some(n) = name {
        return rows
            .into_iter()
            .find(|l| l.name.eq_ignore_ascii_case(n) || l.id.to_string() == n)
            .ok_or_else(|| eyre::eyre!("shopping list not found: {n}"));
    }
    if let Some(first) = rows.into_iter().next() {
        return Ok(first);
    }
    client
        .create(ShoppingList {
            path: String::new(),
            id: uuid::Uuid::nil(),
            name: "Shopping".into(),
            store_location_id: None,
            entries: mealplan::shopping::ShoppingEntries::default(),
            is_template: false,
            from_template: None,
            date_created: None,
            date_modified: None,
            details: String::new(),
        })
        .await
        .map_err(|e| eyre::eyre!("create default list: {e:?}"))
}

fn print_list(l: &ShoppingList, json: bool) -> eyre::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(l)?);
        return Ok(());
    }
    let open = l.entries.iter().filter(|e| !e.is_settled()).count();
    let kind = if l.is_template { " [template]" } else { "" };
    println!(
        "{}{kind}  ({} open / {} total)",
        l.name,
        open,
        l.entries.len()
    );
    for e in l.entries.iter() {
        // `h` distinguishes "found it in the kitchen" from "bought it",
        // which is the whole point of the two-stage run.
        let mark = match e.status {
            mealplan::shopping::EntryStatus::Needed => " ",
            mealplan::shopping::EntryStatus::Have => "h",
            mealplan::shopping::EntryStatus::Purchased => "x",
        };
        let qty = e
            .qty
            .map(|q| format!("{q} {} ", e.unit).trim_end().to_string() + " ")
            .unwrap_or_default();
        let note = e
            .note
            .as_deref()
            .map(|n| format!("  — {n}"))
            .unwrap_or_default();
        println!("  [{mark}] {qty}{}{note}", e.name);
    }
    Ok(())
}

/// Index of the entry named `item` — prefers a row still outstanding.
fn find_entry(l: &ShoppingList, item: &str) -> Option<usize> {
    let t = item.to_lowercase();
    l.entries
        .iter()
        .position(|e| !e.is_settled() && e.name.to_lowercase() == t)
        .or_else(|| l.entries.iter().position(|e| e.name.to_lowercase() == t))
}

pub async fn run_shopping(cmd: ShoppingCmd) -> eyre::Result<()> {
    match cmd {
        ShoppingCmd::List { list, remote, json } => {
            let client: ShoppingServiceClient = establish_for_url(&remote.url()?).await?;
            let l = pick_list(&client, list.as_deref()).await?;
            print_list(&l, json)?;
        }
        ShoppingCmd::AddMissing {
            recipe,
            servings,
            list,
            remote,
            json,
        } => {
            let url = remote.url()?;
            let cookbook_client: CookbookServiceClient = establish_for_url(&url).await?;
            let r = resolve_recipe(&cookbook_client, &recipe).await?;
            let servings = servings.or(r.servings).unwrap_or(1);
            let client: ShoppingServiceClient = establish_for_url(&url).await?;
            let l = pick_list(&client, list.as_deref()).await?;
            let updated = client
                .add_missing_for_recipe(l.id.to_string(), r.path.clone(), servings)
                .await
                .map_err(|e| eyre::eyre!("add-missing: {e:?}"))?;
            if !json {
                println!("added shortages for {} × {servings} servings", r.name);
            }
            print_list(&updated, json)?;
        }
        ShoppingCmd::AddLowStock { list, remote, json } => {
            let client: ShoppingServiceClient = establish_for_url(&remote.url()?).await?;
            let l = pick_list(&client, list.as_deref()).await?;
            let updated = client
                .add_low_stock(l.id.to_string())
                .await
                .map_err(|e| eyre::eyre!("add-low-stock: {e:?}"))?;
            print_list(&updated, json)?;
        }
        ShoppingCmd::AddExpired { list, remote, json } => {
            let client: ShoppingServiceClient = establish_for_url(&remote.url()?).await?;
            let l = pick_list(&client, list.as_deref()).await?;
            let updated = client
                .add_expired_or_overdue(l.id.to_string(), chrono::Utc::now().date_naive())
                .await
                .map_err(|e| eyre::eyre!("add-expired: {e:?}"))?;
            print_list(&updated, json)?;
        }
        ShoppingCmd::MarkPurchased {
            item,
            qty,
            list,
            remote,
            json,
        } => {
            let client: ShoppingServiceClient = establish_for_url(&remote.url()?).await?;
            let mut l = pick_list(&client, list.as_deref()).await?;
            let idx = find_entry(&l, &item)
                .ok_or_else(|| eyre::eyre!("no entry named `{item}` on `{}`", l.name))?;
            let entry_id = l.entries[idx].id;
            if let Some(q) = qty {
                l.entries[idx].qty = Some(q);
                l = client
                    .update(l)
                    .await
                    .map_err(|e| eyre::eyre!("update qty: {e:?}"))?;
            }
            let updated = client
                .mark_purchased(l.id.to_string(), entry_id.to_string())
                .await
                .map_err(|e| eyre::eyre!("mark-purchased: {e:?}"))?;
            if !json {
                println!("purchased {item} (pantry restocked when linked)");
            }
            print_list(&updated, json)?;
        }
        ShoppingCmd::Rm {
            item,
            list,
            remote,
            json,
        } => {
            let client: ShoppingServiceClient = establish_for_url(&remote.url()?).await?;
            let mut l = pick_list(&client, list.as_deref()).await?;
            let before = l.entries.len();
            let t = item.to_lowercase();
            l.entries.0.retain(|e| e.name.to_lowercase() != t);
            if l.entries.len() == before {
                eyre::bail!("no entry named `{item}` on `{}`", l.name);
            }
            let updated = client
                .update(l)
                .await
                .map_err(|e| eyre::eyre!("update: {e:?}"))?;
            print_list(&updated, json)?;
        }
        ShoppingCmd::MarkHave {
            item,
            undo,
            list,
            remote,
            json,
        } => {
            let client: ShoppingServiceClient = establish_for_url(&remote.url()?).await?;
            let l = pick_list(&client, list.as_deref()).await?;
            // When undoing, the row is already settled, so search the
            // whole list rather than preferring outstanding rows.
            let idx = if undo {
                let t = item.to_lowercase();
                l.entries.iter().position(|e| e.name.to_lowercase() == t)
            } else {
                find_entry(&l, &item)
            }
            .ok_or_else(|| eyre::eyre!("no entry named `{item}` on `{}`", l.name))?;
            let entry_id = l.entries[idx].id;
            let updated = client
                .mark_have(l.id.to_string(), entry_id.to_string(), !undo)
                .await
                .map_err(|e| eyre::eyre!("mark-have: {e:?}"))?;
            if !json {
                if undo {
                    println!("{item} back on the list");
                } else {
                    println!("{item} already in the kitchen — no pantry change");
                }
            }
            print_list(&updated, json)?;
        }
        ShoppingCmd::Reset { list, remote, json } => {
            let client: ShoppingServiceClient = establish_for_url(&remote.url()?).await?;
            let l = pick_list(&client, list.as_deref()).await?;
            let updated = client
                .reset(l.id.to_string())
                .await
                .map_err(|e| eyre::eyre!("reset: {e:?}"))?;
            if !json {
                println!("`{}` reset — every row outstanding again", updated.name);
            }
            print_list(&updated, json)?;
        }
        ShoppingCmd::SaveTemplate {
            name,
            list,
            remote,
            json,
        } => {
            let client: ShoppingServiceClient = establish_for_url(&remote.url()?).await?;
            let l = pick_list(&client, list.as_deref()).await?;
            let created = client
                .save_as_template(l.id.to_string(), name.clone())
                .await
                .map_err(|e| eyre::eyre!("save-template: {e:?}"))?;
            if !json {
                println!("saved template `{name}` from `{}`", l.name);
            }
            print_list(&created, json)?;
        }
        ShoppingCmd::Start {
            template,
            name,
            remote,
            json,
        } => {
            let client: ShoppingServiceClient = establish_for_url(&remote.url()?).await?;
            let rows = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;
            let t = template.to_lowercase();
            let tpl = rows
                .iter()
                .find(|l| l.is_template && l.name.to_lowercase() == t)
                .ok_or_else(|| eyre::eyre!("no template named `{template}`"))?;
            let run_name = name.unwrap_or_else(|| tpl.name.clone());
            let started = client
                .start_from_template(tpl.id.to_string(), run_name)
                .await
                .map_err(|e| eyre::eyre!("start: {e:?}"))?;
            if !json {
                println!("started `{}` from template `{}`", started.name, tpl.name);
            }
            print_list(&started, json)?;
        }
    }
    Ok(())
}

// ── meal schedule — day-plan block ───────────────────────────

#[derive(Args)]
pub struct MealScheduleArgs {
    /// Meal id (uuid) or vault path.
    pub meal: String,
    /// Time window `HH:MM-HH:MM`, e.g. `17:30-18:30`.
    pub window: String,
    /// Schedule even when the window overlaps an existing
    /// block.
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub remote: Remote,
    #[arg(long)]
    pub json: bool,
}

/// `"HH:MM-HH:MM"` → (start, end). End must be after start.
pub(crate) fn parse_window(s: &str) -> Result<(TimeOfDay, TimeOfDay), String> {
    let (a, b) = s
        .split_once('-')
        .ok_or_else(|| format!("`{s}`: expected `HH:MM-HH:MM`"))?;
    let start = parse_hhmm(a.trim())?;
    let end = parse_hhmm(b.trim())?;
    if end.minutes_since_midnight <= start.minutes_since_midnight {
        return Err(format!("`{s}`: end must be after start"));
    }
    Ok((start, end))
}

fn parse_hhmm(s: &str) -> Result<TimeOfDay, String> {
    let (h, m) = s
        .split_once(':')
        .ok_or_else(|| format!("`{s}`: expected `HH:MM`"))?;
    let h: u16 = h.parse().map_err(|_| format!("`{s}`: bad hours"))?;
    let m: u16 = m.parse().map_err(|_| format!("`{s}`: bad minutes"))?;
    if h > 24 || m > 59 || h * 60 + m > 1440 {
        return Err(format!("`{s}`: out of range"));
    }
    Ok(TimeOfDay {
        minutes_since_midnight: h * 60 + m,
    })
}

/// First block overlapping [start, end). Touching edges do not
/// conflict.
///
/// NOTE: deliberately duplicated — the general `task plan`
/// verbs (separate issue/worktree) carry their own overlap
/// check. A tiny pure fn beats a cross-worktree dependency;
/// fold them together once both land.
pub(crate) fn find_overlap(
    blocks: &[PlannedBlock],
    start: TimeOfDay,
    end: TimeOfDay,
) -> Option<&PlannedBlock> {
    blocks.iter().find(|b| {
        b.start.minutes_since_midnight < end.minutes_since_midnight
            && start.minutes_since_midnight < b.end.minutes_since_midnight
    })
}

fn fmt_tod(t: TimeOfDay) -> String {
    format!("{:02}:{:02}", t.hours(), t.minutes())
}

pub async fn meal_schedule(a: MealScheduleArgs) -> eyre::Result<()> {
    let (start, end) = parse_window(&a.window).map_err(|e| eyre::eyre!(e))?;
    let url = a.remote.url()?;
    let meal_client: MealplanServiceClient = establish_for_url(&url).await?;
    let meal = resolve_meal_target(&meal_client, &a.meal).await?;
    let date = meal.scheduled_for.to_string();

    let plans: DayPlansClient = establish_for_url(&url).await?;
    let mut plan = plans
        .get_day_plan(date.clone())
        .await
        .map_err(|e| eyre::eyre!("get_day_plan: {e:?}"))?
        .unwrap_or_else(|| DayPlan {
            date: date.clone(),
            from_template: None,
            blocks: scheduling_proto::day_plan::PlannedBlocks::default(),
        });

    // Re-scheduling the same meal moves its block.
    let block_id = format!("meal-{}", meal.id);
    plan.blocks.0.retain(|b| b.id.0 != block_id);

    if let Some(conflict) = find_overlap(&plan.blocks, start, end) {
        if !a.force {
            eyre::bail!(
                "window {}–{} overlaps `{}` ({}–{}) on {date} — pass --force to schedule anyway",
                fmt_tod(start),
                fmt_tod(end),
                conflict.label,
                fmt_tod(conflict.start),
                fmt_tod(conflict.end),
            );
        }
    }

    let block = PlannedBlock {
        id: TimeBlockId(block_id),
        start,
        end,
        label: meal.name.clone(),
        category: BlockCategory::Meal,
        note: Some(meal.id.to_string()),
        assignment: Some(BlockAssignment {
            kind: "label".into(),
            title: meal.name.clone(),
            ref_id: None,
        }),
        fixed: false,
    };
    plan.blocks.0.push(block.clone());
    plan.blocks
        .0
        .sort_by_key(|b| b.start.minutes_since_midnight);

    plans
        .upsert_day_plan(plan)
        .await
        .map_err(|e| eyre::eyre!("upsert_day_plan: {e:?}"))?;

    if a.json {
        let out = serde_json::json!({ "date": date, "block": block });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "scheduled {} on {date} {}–{} (day-plan block {})",
            meal.name,
            fmt_tod(start),
            fmt_tod(end),
            block.id.0
        );
    }
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tod(h: u16, m: u16) -> TimeOfDay {
        TimeOfDay {
            minutes_since_midnight: h * 60 + m,
        }
    }

    fn block(label: &str, start: TimeOfDay, end: TimeOfDay) -> PlannedBlock {
        PlannedBlock {
            id: TimeBlockId(label.to_string()),
            start,
            end,
            label: label.to_string(),
            category: BlockCategory::Meal,
            fixed: false,
            note: None,
            assignment: None,
        }
    }

    #[test]
    fn valid_cooklang_passes() {
        let src = "Combine @rolled oats{1%cup} with @milk{1.5%cup} in a #jar{}.";
        assert_eq!(validate_cooklang("t.cook", src), Ok(false));
    }

    #[test]
    fn titled_cooklang_reports_title() {
        let src = ">> title: Oats\n\nCombine @oats{1%cup}.";
        assert_eq!(validate_cooklang("t.cook", src), Ok(true));
    }

    #[test]
    fn invalid_cooklang_fails_with_line_context() {
        // Empty quantity value before a unit — a hard parse
        // error ("Empty quantity value").
        let src = "Add @flour{%cups} and stir.";
        let err = validate_cooklang("bad.cook", src).unwrap_err();
        // The rendered report carries the file label and the
        // offending source line (codesnake snippet).
        assert!(err.contains("bad.cook"), "missing file label: {err}");
        assert!(err.contains("@flour{%cups}"), "missing line context: {err}");
    }

    #[test]
    fn parse_window_roundtrip() {
        let (s, e) = parse_window("17:30-18:30").unwrap();
        assert_eq!(s.minutes_since_midnight, 17 * 60 + 30);
        assert_eq!(e.minutes_since_midnight, 18 * 60 + 30);
    }

    #[test]
    fn parse_window_rejects_backwards_and_garbage() {
        assert!(parse_window("18:30-17:30").is_err());
        assert!(parse_window("17:30").is_err());
        assert!(parse_window("17:60-18:00").is_err());
        assert!(parse_window("25:00-26:00").is_err());
    }

    #[test]
    fn overlap_detected_and_named() {
        let blocks = vec![block("Gym", tod(17, 0), tod(18, 0))];
        let hit = find_overlap(&blocks, tod(17, 30), tod(18, 30)).expect("overlap");
        assert_eq!(hit.label, "Gym");
        // containment
        assert!(find_overlap(&blocks, tod(17, 15), tod(17, 45)).is_some());
    }

    #[test]
    fn touching_edges_do_not_conflict() {
        let blocks = vec![block("Gym", tod(17, 0), tod(18, 0))];
        assert!(find_overlap(&blocks, tod(18, 0), tod(19, 0)).is_none());
        assert!(find_overlap(&blocks, tod(16, 0), tod(17, 0)).is_none());
    }
}
