//! `task recipe import <url>` (issue 44fe504f) — webpage → `.cook`.
//!
//! Thin CLI shell over the `recipe-import` crate: fetch → extract →
//! synthesize (LLM-primary, heuristic for `--offline` / fallback) →
//! validate → save through the SAME `CookbookServiceClient`
//! create/update path as `task recipe create`, so imported recipes
//! flow into pantry/shopping unchanged.
//!
//! Lives in its own module (like `mealprep.rs`) so concurrent CLI
//! work doesn't collide in `main.rs`.

use std::io::IsTerminal as _;

use clap::Args;
use cookbook::CookbookServiceClient;
use recipe_import::{
    AnthropicClient, ImportError, NormalizedRecipe, coverage, extract, fetch_html,
    synthesize_heuristic, synthesize_llm,
};

use crate::establish_for_url;
use crate::mealprep::{Remote, validated_source};

#[derive(Args)]
pub struct RecipeImportArgs {
    /// Recipe page URL (optional when --from-file is given; then it
    /// only stamps the `source:` frontmatter).
    pub url: Option<String>,
    /// Import from a saved HTML file instead of fetching — the escape
    /// hatch for bot-protected / paywalled sites.
    #[arg(long, value_name = "SAVED.HTML")]
    pub from_file: Option<std::path::PathBuf>,
    /// Skip the LLM: deterministic synthesis (qty/unit parse +
    /// first-occurrence matching). Output is marked
    /// `import: needs-review`.
    #[arg(long)]
    pub offline: bool,
    /// Print the synthesized cooklang + coverage; write nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Override the recipe display name (default: the page's title).
    #[arg(long)]
    pub name: Option<String>,
    /// Overwrite an existing recipe without prompting.
    #[arg(long, short = 'y')]
    pub yes: bool,
    #[command(flatten)]
    pub remote: Remote,
    #[arg(long)]
    pub json: bool,
}

pub async fn recipe_import(a: RecipeImportArgs) -> eyre::Result<()> {
    // ── stage 1+2: html → NormalizedRecipe ───────────────────
    let mut normalized = load_and_extract(&a).await?;
    if a.from_file.is_some() {
        // A URL passed alongside --from-file is the real provenance.
        if let Some(u) = &a.url {
            normalized.source_url = Some(u.clone());
        }
    }

    // ── stage 3: synthesize ──────────────────────────────────
    let (source, mode) = synthesize(&a, &normalized).await?;

    // Same gate as `task recipe create`: parse, render diagnostics,
    // inject a title if the synthesizer somehow lost it.
    let display_name = a.name.clone().unwrap_or_else(|| normalized.name.clone());
    let source = validated_source(&display_name, source)?;

    let cov = coverage(&normalized, &source);
    let todo_parked = source.contains("TODO (import review)");

    if a.dry_run {
        println!("{source}");
        eprintln!(
            "{}",
            summary(&mode, &cov, todo_parked, "dry-run — nothing written")
        );
        return Ok(());
    }

    // ── stage 4: save via the existing cookbook path ─────────
    let client: CookbookServiceClient = establish_for_url(&a.remote.url()?).await?;
    let path = cookbook::default_recipe_path(&display_name, None);

    // Re-import detection: same target path, same display name, or
    // same source URL.
    let rows = client
        .list()
        .await
        .map_err(|e| eyre::eyre!("list: {e:?}"))?;
    let existing = rows.into_iter().find(|r| {
        r.path == path
            || r.name.eq_ignore_ascii_case(&display_name)
            || (r.source_url.is_some() && r.source_url == normalized.source_url)
    });

    let saved = if let Some(existing) = existing {
        confirm_overwrite(&existing.name, &existing.path, a.yes)?;
        let recipe =
            cookbook::parse_cook(&existing.path, &source).map_err(|e| eyre::eyre!("parse: {e}"))?;
        let r = client
            .update(recipe)
            .await
            .map_err(|e| eyre::eyre!("update: {e:?}"))?;
        println!("updated {}  ({})", r.name, r.path);
        r
    } else {
        let recipe = cookbook::parse_cook(&path, &source).map_err(|e| eyre::eyre!("parse: {e}"))?;
        let r = client
            .create(recipe)
            .await
            .map_err(|e| eyre::eyre!("create: {e:?}"))?;
        println!("created {}  ({})", r.name, r.path);
        r
    };

    println!("{}", summary(&mode, &cov, todo_parked, ""));
    if a.json {
        println!("{}", serde_json::to_string_pretty(&saved)?);
    }
    Ok(())
}

/// Stage 1+2 — html from the network or `--from-file`, then the
/// structured-data extractors.
async fn load_and_extract(a: &RecipeImportArgs) -> eyre::Result<NormalizedRecipe> {
    if let Some(p) = &a.from_file {
        let html =
            std::fs::read_to_string(p).map_err(|e| eyre::eyre!("read {}: {e}", p.display()))?;
        let label = a.url.clone().unwrap_or_else(|| p.display().to_string());
        return extract(&html, &label).map_err(|e| eyre::eyre!("{e}"));
    }
    let Some(url) = &a.url else {
        eyre::bail!("pass a recipe page <url>, or --from-file <saved.html>");
    };
    let html = fetch_html(url).await.map_err(|e| eyre::eyre!("{e}"))?;
    extract(&html, url).map_err(|e| eyre::eyre!("{e}"))
}

/// Stage 3 — LLM-primary with heuristic fallback; `--offline` goes
/// straight to the heuristic. Returns (cooklang source, mode label).
async fn synthesize(
    a: &RecipeImportArgs,
    normalized: &NormalizedRecipe,
) -> eyre::Result<(String, String)> {
    if a.offline {
        return Ok((synthesize_heuristic(normalized), "offline heuristic".into()));
    }
    let Some(client) = AnthropicClient::from_env() else {
        eyre::bail!(
            "no ANTHROPIC_API_KEY in the environment — export one for LLM synthesis, \
             or re-run with --offline for the deterministic converter"
        );
    };
    match synthesize_llm(&client, normalized).await {
        Ok(out) => {
            let label = if out.attempts == 1 {
                format!("llm ({})", client.model())
            } else {
                format!("llm ({}, after 1 retry)", client.model())
            };
            Ok((out.source, label))
        }
        Err(e @ ImportError::LlmValidation(_)) => {
            eprintln!("warning: {e}");
            eprintln!("falling back to the offline heuristic (marked import: needs-review)");
            Ok((
                synthesize_heuristic(normalized),
                "heuristic (llm fallback)".into(),
            ))
        }
        Err(e) => Err(eyre::eyre!("{e}")),
    }
}

/// Re-import guard: interactive y/N prompt on a TTY; otherwise
/// require `--yes`.
fn confirm_overwrite(name: &str, path: &str, yes: bool) -> eyre::Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        eyre::bail!(
            "a recipe matching this import already exists: {name} ({path}) — \
             re-run with --yes to overwrite"
        );
    }
    eprint!("recipe already exists: {name} ({path}) — overwrite? [y/N] ");
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| eyre::eyre!("read confirmation: {e}"))?;
    let answer = line.trim().to_lowercase();
    if answer == "y" || answer == "yes" {
        Ok(())
    } else {
        eyre::bail!("aborted — existing recipe left untouched")
    }
}

/// Coverage + mode line printed after every import.
fn summary(mode: &str, cov: &recipe_import::Coverage, todo_parked: bool, suffix: &str) -> String {
    let mut s = format!(
        "import: {mode}; ingredients annotated {}/{}",
        cov.matched, cov.total
    );
    if !cov.unmatched.is_empty() {
        s.push_str(&format!("; unmatched: {}", cov.unmatched.join(" | ")));
    }
    if todo_parked {
        s.push_str("; some ingredients parked in a TODO step — review the recipe");
    }
    if !suffix.is_empty() {
        s.push_str("; ");
        s.push_str(suffix);
    }
    s
}
