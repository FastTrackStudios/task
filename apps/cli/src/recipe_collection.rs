//! `task recipe import-collection <listing-url>` — a whole listing
//! page into the cookbook as the *resources* tier.
//!
//! The loop the CLI owns (the pure parts live in
//! `recipe_import::collection`):
//!
//! 1. Enumerate — fetch the listing (or read `--from-file`, a saved
//!    HTML page or a one-URL-per-line text file) and keep the links
//!    that point at recipe pages.
//! 2. Skip what the cookbook already holds — matched on the canonical
//!    `source:` URL of every recipe the service lists — and what the
//!    `--since-file` ledger says a previous run handled.
//! 3. Import each remaining page through the ordinary pipeline
//!    (fetch → extract → heuristic synthesis, `--llm` opts into the
//!    model), stamp it as a resource (`source_site`, `collection`,
//!    `author`, `imported`, `tags`, `curated: false`), render the
//!    metadata as `>> key: value` lines, and `create` it at
//!    `Cookbook/<folder>/<slug>.cook`.
//! 4. Regenerate `<Collection>.md` in the wiki — an index page listing
//!    every recipe in the folder with its source link.
//!
//! Cron-friendly by construction: idempotent (source URL is the
//! identity), one request at a time with a delay, a per-recipe failure
//! is a line in the summary rather than an abort, and a bot-protection
//! response stops the run with a clear message instead of hammering.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use clap::Args;
use cookbook::CookbookServiceClient;
use recipe_import::collection::{
    IndexEntry, ResourceStamp, canonical_url, enumerate, find_present, frontmatter_to_arrows,
    index_page, is_curated, page_file_name, schema_author, site_of,
};
use recipe_import::{
    AnthropicClient, ImportError, NormalizedRecipe, extract, fetch_html, synthesize_heuristic,
    synthesize_llm,
};

use crate::establish_for_url;
use crate::mealprep::{Remote, validated_source};

#[derive(Args)]
pub struct RecipeImportCollectionArgs {
    /// Listing page URL (optional with --from-file; then it only
    /// resolves relative links and stamps the index page's source).
    pub url: Option<String>,
    /// Read the listing from a saved HTML page or a text file of one
    /// recipe URL per line, instead of fetching it.
    #[arg(long, value_name = "LISTING")]
    pub from_file: Option<PathBuf>,
    /// Directory of saved recipe pages (`<slug>.html`, the last path
    /// segment of the recipe URL) used instead of fetching each page —
    /// the escape hatch for bot-protected sites, and what the e2e
    /// test drives.
    #[arg(long, value_name = "DIR")]
    pub pages_dir: Option<PathBuf>,
    /// Sub-folder of `Cookbook/` the recipes land in.
    #[arg(long, default_value = "Food Wishes")]
    pub folder: String,
    /// Collection name for the `collection:` key and the index page
    /// (defaults to the folder name).
    #[arg(long)]
    pub collection: Option<String>,
    /// Author to stamp on every recipe, overriding the page's own
    /// (Allrecipes credits Chef John as "John Mitzewich"; a collection
    /// is one author's, so name them the way the cookbook does).
    /// Without it the page's schema.org author is used.
    #[arg(long)]
    pub author: Option<String>,
    /// Extra tags every imported recipe gets, comma-separated.
    #[arg(long, default_value = "resource")]
    pub tags: String,
    /// Wiki whose `<Collection>.md` index page is regenerated.
    #[arg(long, default_value = "cooking")]
    pub wiki: String,
    /// Don't touch the index page.
    #[arg(long)]
    pub no_index: bool,
    /// Deterministic synthesis (the default; kept for symmetry with
    /// `task recipe import`).
    #[arg(long)]
    pub offline: bool,
    /// Synthesize with the LLM (needs ANTHROPIC_API_KEY); falls back
    /// to the heuristic per recipe when the model's output fails to
    /// validate.
    #[arg(long, conflicts_with = "offline")]
    pub llm: bool,
    /// Import at most N new recipes this run.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Enumerate and plan; write nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Re-import recipes already present, updating the file when the
    /// synthesized source changed. Curated recipes are never touched.
    #[arg(long)]
    pub refresh: bool,
    /// JSON ledger of URLs handled by earlier runs; read to skip them
    /// without a request, rewritten at the end.
    #[arg(long, value_name = "PATH")]
    pub since_file: Option<PathBuf>,
    /// Pause between network requests.
    #[arg(long, default_value_t = 1500)]
    pub delay_ms: u64,
    #[command(flatten)]
    pub remote: Remote,
    #[arg(long)]
    pub json: bool,
}

/// One line of the run summary.
#[derive(Debug, Clone, serde::Serialize)]
struct Outcome {
    url: String,
    status: &'static str,
    path: Option<String>,
    detail: Option<String>,
}

#[derive(Debug, Default, serde::Serialize)]
struct Summary {
    enumerated: usize,
    imported: usize,
    updated: usize,
    skipped_present: usize,
    skipped_ledger: usize,
    not_attempted: usize,
    failed: Vec<Outcome>,
    stopped: Option<String>,
    index_page: Option<String>,
    outcomes: Vec<Outcome>,
}

pub async fn recipe_import_collection(a: RecipeImportCollectionArgs) -> eyre::Result<()> {
    let collection = a.collection.clone().unwrap_or_else(|| a.folder.clone());
    let folder_path = format!("{}/{}", cookbook::COOKBOOK_DIR, a.folder.trim_matches('/'));
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let base_url = a.url.clone().unwrap_or_default();

    // ── 1. enumerate ─────────────────────────────────────────
    let listing = match &a.from_file {
        Some(p) => {
            std::fs::read_to_string(p).map_err(|e| eyre::eyre!("read {}: {e}", p.display()))?
        }
        None => {
            let Some(u) = &a.url else {
                eyre::bail!("pass the collection <url>, or --from-file <listing.html|urls.txt>");
            };
            fetch_html(u).await.map_err(|e| eyre::eyre!("{e}"))?
        }
    };
    let urls = enumerate(&listing, &base_url);
    if urls.is_empty() {
        eyre::bail!(
            "no recipe links found in the listing — is it a recipe collection page? \
             (links must look like /recipe/<id>/<slug>/ or /<slug>-recipe-<id>)"
        );
    }
    let mut summary = Summary {
        enumerated: urls.len(),
        ..Default::default()
    };

    // ── 2. what's already there ──────────────────────────────
    let client: CookbookServiceClient = establish_for_url(&a.remote.url()?).await?;
    let existing = client
        .list()
        .await
        .map_err(|e| eyre::eyre!("list recipes: {e:?}"))?;
    let mut ledger: BTreeMap<String, String> = a
        .since_file
        .as_ref()
        .filter(|p| p.exists())
        .map(|p| -> eyre::Result<_> {
            let s = std::fs::read_to_string(p)?;
            Ok(serde_json::from_str(&s)?)
        })
        .transpose()?
        .unwrap_or_default();

    let site = a
        .url
        .as_deref()
        .or(urls.first().map(String::as_str))
        .map(site_of)
        .unwrap_or_else(|| "web".into());
    let stamp = ResourceStamp {
        collection: collection.clone(),
        site,
        author_fallback: None,
        imported: today.clone(),
        tags: a
            .tags
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .chain(std::iter::once(slug_tag(&collection)))
            .collect(),
    };
    let llm = if a.llm {
        Some(
            AnthropicClient::from_env()
                .ok_or_else(|| eyre::eyre!("--llm needs ANTHROPIC_API_KEY in the environment"))?,
        )
    } else {
        None
    };

    // ── 3. import ────────────────────────────────────────────
    let mut budget = a.limit.unwrap_or(usize::MAX);
    let mut taken: Vec<String> = existing.iter().map(|r| r.path.clone()).collect();
    let mut network_requests = 0usize;
    for (i, url) in urls.iter().enumerate() {
        let present = find_present(
            existing
                .iter()
                .map(|r| (r.path.as_str(), r.source_url.as_deref())),
            url,
        );
        if let Some(path) = &present {
            if !a.refresh {
                summary.skipped_present += 1;
                ledger.entry(url.clone()).or_insert_with(|| today.clone());
                summary.outcomes.push(Outcome {
                    url: url.clone(),
                    status: "present",
                    path: Some(path.clone()),
                    detail: None,
                });
                continue;
            }
            let curated = existing
                .iter()
                .find(|r| &r.path == path)
                .is_some_and(|r| is_curated(&r.source));
            if curated {
                summary.skipped_present += 1;
                summary.outcomes.push(Outcome {
                    url: url.clone(),
                    status: "curated",
                    path: Some(path.clone()),
                    detail: Some("curated recipes are never refreshed".into()),
                });
                continue;
            }
        } else if ledger.contains_key(url) && !a.refresh {
            summary.skipped_ledger += 1;
            summary.outcomes.push(Outcome {
                url: url.clone(),
                status: "ledger",
                path: None,
                detail: None,
            });
            continue;
        }
        if budget == 0 {
            summary.not_attempted = urls.len() - i;
            break;
        }
        budget -= 1;

        // Page HTML: saved copy, or one polite request.
        let html = match &a.pages_dir {
            Some(dir) => {
                let file = dir.join(page_file_name(url));
                match std::fs::read_to_string(&file) {
                    Ok(h) => h,
                    Err(e) => {
                        fail(
                            &mut summary,
                            url,
                            format!("no saved page {}: {e}", file.display()),
                        );
                        continue;
                    }
                }
            }
            None => {
                if network_requests > 0 || a.from_file.is_none() {
                    tokio::time::sleep(Duration::from_millis(a.delay_ms)).await;
                }
                network_requests += 1;
                match fetch_html(url).await {
                    Ok(h) => h,
                    Err(e @ ImportError::BotProtected { .. }) => {
                        let msg = format!("{e}");
                        fail(&mut summary, url, msg.clone());
                        summary.stopped = Some(format!(
                            "the site refused a request ({}); stopping so we don't hammer it — \
                             re-run later, or save the pages and use --pages-dir",
                            first_line(&msg)
                        ));
                        summary.not_attempted = urls.len() - i - 1;
                        break;
                    }
                    Err(e) => {
                        fail(&mut summary, url, e.to_string());
                        continue;
                    }
                }
            }
        };

        // Extract + stamp + synthesize.
        let mut normalized: NormalizedRecipe = match extract(&html, url) {
            Ok(n) => n,
            Err(e) => {
                fail(&mut summary, url, e.to_string());
                continue;
            }
        };
        normalized.source_url = Some(canonical_url(url));
        if let Some(author) = &a.author {
            normalized.metadata.insert("author".into(), author.clone());
        } else if !normalized.metadata.contains_key("author") {
            if let Some(author) = schema_author(&html) {
                normalized.metadata.insert("author".into(), author);
            }
        }
        recipe_import::collection::stamp_resource(&mut normalized, &stamp);
        let source = match synthesize(llm.as_ref(), &normalized).await {
            Ok(s) => s,
            Err(e) => {
                fail(&mut summary, url, e.to_string());
                continue;
            }
        };
        let source = match validated_source(&normalized.name, source) {
            Ok(s) => frontmatter_to_arrows(&s),
            Err(e) => {
                fail(&mut summary, url, e.to_string());
                continue;
            }
        };

        // Target path: the slug under the folder; disambiguate a
        // name clash with a different recipe by the URL's id.
        let path = match &present {
            Some(p) => p.clone(),
            None => {
                let mut p = cookbook::default_recipe_path(&normalized.name, Some(&folder_path));
                if taken.iter().any(|t| t == &p) {
                    let id = url_id(url);
                    p = p.replace(".cook", &format!("-{id}.cook"));
                }
                taken.push(p.clone());
                p
            }
        };

        if a.dry_run {
            println!(
                "would {} {}  ← {url}",
                if present.is_some() {
                    "update"
                } else {
                    "import"
                },
                path
            );
            summary.outcomes.push(Outcome {
                url: url.clone(),
                status: "planned",
                path: Some(path),
                detail: None,
            });
            continue;
        }

        let recipe = match cookbook::parse_cook(&path, &source) {
            Ok(r) => r,
            Err(e) => {
                fail(&mut summary, url, format!("parse: {e}"));
                continue;
            }
        };
        let result = if present.is_some() {
            let unchanged = existing
                .iter()
                .find(|r| r.path == path)
                .is_some_and(|r| r.source == source);
            if unchanged {
                summary.skipped_present += 1;
                summary.outcomes.push(Outcome {
                    url: url.clone(),
                    status: "unchanged",
                    path: Some(path.clone()),
                    detail: None,
                });
                continue;
            }
            client.update(recipe).await.map(|r| ("updated", r))
        } else {
            client.create(recipe).await.map(|r| ("imported", r))
        };
        match result {
            Ok((verb, r)) => {
                println!("{verb} {}  ({})", r.name, r.path);
                if verb == "updated" {
                    summary.updated += 1;
                } else {
                    summary.imported += 1;
                }
                ledger.insert(url.clone(), today.clone());
                summary.outcomes.push(Outcome {
                    url: url.clone(),
                    status: verb,
                    path: Some(r.path),
                    detail: None,
                });
            }
            Err(e) => fail(&mut summary, url, format!("save: {e:?}")),
        }
    }

    // ── 4. ledger + index ────────────────────────────────────
    if !a.dry_run {
        if let Some(p) = &a.since_file {
            std::fs::write(p, serde_json::to_string_pretty(&ledger)?)
                .map_err(|e| eyre::eyre!("write {}: {e}", p.display()))?;
        }
        if !a.no_index {
            match regenerate_index(&a, &client, &collection, &folder_path, &today).await {
                Ok(p) => summary.index_page = p,
                Err(e) => eprintln!("warning: index page not written: {e}"),
            }
        }
    }

    // ── summary ──────────────────────────────────────────────
    let mut line = format!(
        "imported {}, skipped {} present, {} failed",
        summary.imported,
        summary.skipped_present,
        summary.failed.len()
    );
    if summary.updated > 0 {
        line.push_str(&format!(", {} updated", summary.updated));
    }
    if summary.skipped_ledger > 0 {
        line.push_str(&format!(", {} in ledger", summary.skipped_ledger));
    }
    if summary.not_attempted > 0 {
        line.push_str(&format!(", {} not attempted", summary.not_attempted));
    }
    line.push_str(&format!(" (of {} enumerated)", summary.enumerated));
    if a.dry_run {
        line.push_str("; dry-run — nothing written");
    }
    println!("{line}");
    for f in &summary.failed {
        println!("  failed {}: {}", f.url, f.detail.as_deref().unwrap_or("?"));
    }
    if let Some(p) = &summary.index_page {
        println!("index page: {p}");
    }
    if a.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    }
    if let Some(why) = summary.stopped {
        eyre::bail!("{why}");
    }
    Ok(())
}

fn fail(summary: &mut Summary, url: &str, detail: String) {
    eprintln!("failed {url}: {}", first_line(&detail));
    let o = Outcome {
        url: url.to_string(),
        status: "failed",
        path: None,
        detail: Some(detail),
    };
    summary.failed.push(o.clone());
    summary.outcomes.push(o);
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

/// `food-wishes` from `Food Wishes` — the collection's tag.
fn slug_tag(collection: &str) -> String {
    vault_entity::slugify(collection, "collection")
}

/// The numeric id both Allrecipes URL shapes carry, or a short hash
/// of the URL for sites without one.
fn url_id(url: &str) -> String {
    let digits: Vec<&str> = url
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| s.len() >= 4)
        .collect();
    if let Some(d) = digits.last() {
        return (*d).to_string();
    }
    use sha2::Digest as _;
    let h = sha2::Sha256::digest(url.as_bytes());
    format!("{:x}", h)[..8].to_string()
}

async fn synthesize(
    llm: Option<&AnthropicClient>,
    normalized: &NormalizedRecipe,
) -> eyre::Result<String> {
    let Some(client) = llm else {
        return Ok(synthesize_heuristic(normalized));
    };
    match synthesize_llm(client, normalized).await {
        Ok(out) => Ok(out.source),
        Err(e @ ImportError::LlmValidation(_)) => {
            eprintln!("warning: {e}; using the heuristic for this one");
            Ok(synthesize_heuristic(normalized))
        }
        Err(e) => Err(eyre::eyre!("{e}")),
    }
}

/// Rewrite `<Collection>.md` in the wiki from what the cookbook now
/// holds under the folder. Only writes when the page changed.
#[cfg(feature = "plugin-wiki")]
async fn regenerate_index(
    a: &RecipeImportCollectionArgs,
    client: &CookbookServiceClient,
    collection: &str,
    folder_path: &str,
    today: &str,
) -> eyre::Result<Option<String>> {
    use wiki_proto::service::pages::PagesClient;

    let rows = client
        .list()
        .await
        .map_err(|e| eyre::eyre!("list recipes: {e:?}"))?;
    let prefix = format!("{folder_path}/");
    let entries: Vec<IndexEntry> = rows
        .iter()
        .filter(|r| r.path.starts_with(&prefix))
        .map(|r| IndexEntry {
            name: r.name.clone(),
            path: r.path.clone(),
            source_url: r.source_url.clone(),
            curated: is_curated(&r.source),
        })
        .collect();
    let folder = folder_path
        .strip_prefix(&format!("{}/", cookbook::COOKBOOK_DIR))
        .unwrap_or(folder_path);
    let page_path = format!("{collection}.md");
    let pages: PagesClient = establish_for_url(&a.remote.url()?).await?;

    // The previous page's `generated:` date must not force a rewrite
    // when nothing else moved.
    let current = pages
        .read_page(a.wiki.clone(), page_path.clone())
        .await
        .ok();
    if let Some(doc) = &current {
        let prior_date = doc
            .markdown
            .lines()
            .find_map(|l| l.strip_prefix("generated: "))
            .unwrap_or(today)
            .to_string();
        let same = index_page(collection, a.url.as_deref(), folder, &entries, &prior_date);
        if same == doc.markdown {
            return Ok(Some(format!("{page_path} (unchanged)")));
        }
    }
    let markdown = index_page(collection, a.url.as_deref(), folder, &entries, today);
    pages
        .write_page(a.wiki.clone(), page_path.clone(), markdown, String::new())
        .await
        .map_err(|e| eyre::eyre!("write {} in wiki `{}`: {e:?}", page_path, a.wiki))?;
    Ok(Some(format!("{page_path} ({} recipes)", entries.len())))
}

#[cfg(not(feature = "plugin-wiki"))]
async fn regenerate_index(
    _a: &RecipeImportCollectionArgs,
    _client: &CookbookServiceClient,
    _collection: &str,
    _folder_path: &str,
    _today: &str,
) -> eyre::Result<Option<String>> {
    Ok(None)
}
