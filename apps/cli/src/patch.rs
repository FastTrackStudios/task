//! `task patch …` — the Signal patch lane of the Resource Library from
//! the shell.
//!
//! A patch is Signal's document, and Task is where it lives between
//! rigs: `<org>/resources/patches/<slug>/patch.json` holds the
//! definition verbatim, `patch.md` the manifest a wiki or a `Library`
//! collection can see. These verbs are the same four RPCs Signal itself
//! calls — nothing here is a private lane (ADR 0003).
//!
//! ```text
//! task patch get warm-analog-pad | task patch save warm-analog-pad --title 'Warm Analog Pad' --from -
//! ```
//!
//! `--content-root` / `--content-path` bind the patch to bytes in a
//! File Root. Nothing is transferred by this command: the manifest
//! records where the content is and the Files lane owns it.

use clap::Subcommand;
use resources_proto::{PatchDoc, ResourcesServiceClient};

use crate::asset::{client, content_cell, content_ref, read_body};

#[derive(Subcommand, Debug)]
pub enum PatchCmd {
    /// Create or update a patch. The slug is the identity: pass one to
    /// update, omit it (naming a definition file instead) to create.
    Save {
        /// Patch slug (`warm-analog-pad`) — or the definition file,
        /// whose stem becomes the slug when `--from` is absent.
        slug_or_file: String,
        /// Patch title. Required: it is what a library lists.
        #[arg(long)]
        title: String,
        /// The rig the patch is for (`helix`, `kemper`, `serum`).
        #[arg(long)]
        rig: Option<String>,
        /// Free tag, repeatable (`--tag pad --tag ambient`).
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// File Root holding the patch's bytes, when any are bound.
        #[arg(long)]
        content_root: Option<String>,
        /// Root-relative path of those bytes.
        #[arg(long)]
        content_path: Option<String>,
        /// Read the definition from this file, or `-` for stdin.
        /// Without it, `slug_or_file` is read as the file.
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print a patch's definition (or the whole document with `--json`).
    Get {
        slug: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Every patch the org holds.
    List {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Delete a patch — its whole directory. References to it from
    /// collections are left alone; a dangling reference reads as
    /// unresolved, which is a legible state. Content in a File Root is
    /// untouched: this lane never owned it.
    Rm {
        slug: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

pub async fn run_patch(cmd: PatchCmd, global_org: Option<&str>) -> eyre::Result<()> {
    let org_of = |org: Option<String>| org.or_else(|| global_org.map(str::to_owned));
    match cmd {
        PatchCmd::Save {
            slug_or_file,
            title,
            rig,
            tags,
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
                .upsert_patch(PatchDoc {
                    slug,
                    title,
                    rig: rig.unwrap_or_default(),
                    tags,
                    body,
                    content: content_ref(content_root, content_path),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                })
                .await
                .map_err(|e| eyre::eyre!("upsert_patch: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                let verb = if out.created { "created" } else { "updated" };
                println!("{verb} patch:{} → {}", out.slug, out.rel_path);
            }
            Ok(())
        }
        PatchCmd::Get {
            slug,
            org,
            server,
            json,
        } => {
            let client: ResourcesServiceClient = client(org_of(org), server).await?;
            let doc = client
                .patch(slug.clone())
                .await
                .map_err(|e| eyre::eyre!("patch `{slug}`: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&doc)?);
            } else {
                print!("{}", doc.body);
            }
            Ok(())
        }
        PatchCmd::List { org, server, json } => {
            let client: ResourcesServiceClient = client(org_of(org), server).await?;
            let list = client
                .list_patches()
                .await
                .map_err(|e| eyre::eyre!("list_patches: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }
            if list.is_empty() {
                println!("no patches yet");
                return Ok(());
            }
            for p in &list {
                println!(
                    "{:<32} {:<10} {:<24} {:<28} {}",
                    p.slug,
                    p.rig,
                    p.tags.join(","),
                    content_cell(&p.content),
                    p.title
                );
            }
            Ok(())
        }
        PatchCmd::Rm { slug, org, server } => {
            let client: ResourcesServiceClient = client(org_of(org), server).await?;
            let gone = client
                .delete_patch(slug.clone())
                .await
                .map_err(|e| eyre::eyre!("delete_patch `{slug}`: {e:?}"))?;
            if gone {
                println!("deleted patch:{slug}");
            } else {
                println!("no patch `{slug}`");
            }
            Ok(())
        }
    }
}
