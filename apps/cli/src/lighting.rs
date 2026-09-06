//! `task lighting …` — the Ignition lane of the Resource Library from
//! the shell.
//!
//! A lighting document covers a song, a setlist or a show, and lives at
//! `<org>/resources/lighting/<slug>/show.json` with its manifest beside
//! it as `show.md`. `--scope` is one of `song`, `setlist`, `show` and
//! nothing else; `--cue` declares the labels an anchor may address, so
//! `lighting:sunday-set#cue:12` resolves only if `12` was listed here —
//! the server does not parse lighting source, exactly as it does not
//! parse chart source.

use clap::Subcommand;
use resources_proto::{LightingDoc, ResourcesServiceClient};

use crate::asset::{client, content_cell, content_ref, read_body};

#[derive(Subcommand, Debug)]
pub enum LightingCmd {
    /// Create or update a lighting document. The slug is the identity.
    Save {
        /// Lighting slug (`sunday-set`) — or the cue-list file, whose
        /// stem becomes the slug when `--from` is absent.
        slug_or_file: String,
        /// Title. Required: it is what a library lists.
        #[arg(long)]
        title: String,
        /// What this covers: `song`, `setlist` or `show`. Anything else
        /// is refused rather than stored.
        #[arg(long, default_value = "show")]
        scope: String,
        /// Cue label, repeatable (`--cue 12 --cue 13`) — the anchors a
        /// `lighting:<slug>#cue:<label>` reference may address.
        #[arg(long = "cue")]
        cues: Vec<String>,
        /// File Root holding any rendered media the show needs.
        #[arg(long)]
        content_root: Option<String>,
        /// Root-relative path of that content.
        #[arg(long)]
        content_path: Option<String>,
        /// Read the cue list from this file, or `-` for stdin.
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print a lighting document's cue list (or the whole document with
    /// `--json`).
    Get {
        slug: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Every lighting document the org holds.
    List {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Delete a lighting document — its whole directory. References to
    /// it from collections are left dangling and legible.
    Rm {
        slug: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

pub async fn run_lighting(cmd: LightingCmd, global_org: Option<&str>) -> eyre::Result<()> {
    let org_of = |org: Option<String>| org.or_else(|| global_org.map(str::to_owned));
    match cmd {
        LightingCmd::Save {
            slug_or_file,
            title,
            scope,
            cues,
            content_root,
            content_path,
            from,
            org,
            server,
            json,
        } => {
            let (slug, body) = read_body(&slug_or_file, from.as_deref())?;
            let client: ResourcesServiceClient = client(org_of(org), server).await?;
            let out = client
                .upsert_lighting(LightingDoc {
                    slug,
                    title,
                    scope,
                    cues,
                    body,
                    content: content_ref(content_root, content_path),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                })
                .await
                .map_err(|e| eyre::eyre!("upsert_lighting: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                let verb = if out.created { "created" } else { "updated" };
                println!("{verb} lighting:{} → {}", out.slug, out.rel_path);
            }
            Ok(())
        }
        LightingCmd::Get {
            slug,
            org,
            server,
            json,
        } => {
            let client: ResourcesServiceClient = client(org_of(org), server).await?;
            let doc = client
                .lighting(slug.clone())
                .await
                .map_err(|e| eyre::eyre!("lighting `{slug}`: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&doc)?);
            } else {
                print!("{}", doc.body);
            }
            Ok(())
        }
        LightingCmd::List { org, server, json } => {
            let client: ResourcesServiceClient = client(org_of(org), server).await?;
            let list = client
                .list_lighting()
                .await
                .map_err(|e| eyre::eyre!("list_lighting: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }
            if list.is_empty() {
                println!("no lighting yet");
                return Ok(());
            }
            for l in &list {
                println!(
                    "{:<32} {:<8} {:>2} cues  {:<28} {}",
                    l.slug,
                    l.scope,
                    l.cues.len(),
                    content_cell(&l.content),
                    l.title
                );
            }
            Ok(())
        }
        LightingCmd::Rm { slug, org, server } => {
            let client: ResourcesServiceClient = client(org_of(org), server).await?;
            let gone = client
                .delete_lighting(slug.clone())
                .await
                .map_err(|e| eyre::eyre!("delete_lighting `{slug}`: {e:?}"))?;
            if gone {
                println!("deleted lighting:{slug}");
            } else {
                println!("no lighting `{slug}`");
            }
            Ok(())
        }
    }
}
