//! `task location …` — physical places.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use crate::establish_for_url;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;
use crate::shared::resolve_body;

// ── Location (locations::Store) ──────────────────────────────────────

#[derive(clap::Subcommand)]
pub(crate) enum LocationCmd {
    /// List every location in the active org's vault.
    List {
        #[arg(long)]
        kind: Option<String>,
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
        /// `studio|room|storage|venue|home|other`. Default
        /// `other`.
        #[arg(long)]
        kind: Option<String>,
        /// Parent location (id or path) for nested places.
        #[arg(long)]
        parent: Option<String>,
        #[arg(long)]
        address: Option<String>,
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

pub(crate) async fn connect_locations_client(
    url: &str,
) -> eyre::Result<locations::LocationsServiceClient> {
    establish_for_url(url).await
}

pub(crate) async fn resolve_location_target(
    client: &locations::LocationsServiceClient,
    target: &str,
) -> eyre::Result<locations::Location> {
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
        .find(|l| l.path == target || l.name == target)
        .ok_or_else(|| {
            crate::errors::not_found("resolve target", target)
                .cause("no path or name match")
                .report()
        })
}

pub(crate) async fn run_location(cmd: LocationCmd) -> eyre::Result<()> {
    match cmd {
        LocationCmd::List {
            kind,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_locations_client(&u).await?;
            let rows: Vec<_> = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?
                .into_iter()
                .filter(|l| kind.as_deref().is_none_or(|k| l.kind == k))
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            for l in &rows {
                let addr = l
                    .address
                    .as_deref()
                    .map(|a| format!("  ({a})"))
                    .unwrap_or_default();
                println!("{:<28}  {:<8}{addr}    {}", l.name, l.kind, l.path);
            }
        }
        LocationCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_locations_client(&u).await?;
            let l = resolve_location_target(&client, &target).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&l).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{} [{}]\n", l.name, l.kind);
            println!("  id:       {}", l.id);
            println!("  path:     {}", l.path);
            if let Some(a) = &l.address {
                println!("  address:  {a}");
            }
            if !l.tags.0.is_empty() {
                println!("  tags:     {}", l.tags.0.join(", "));
            }
            if !l.details.is_empty() {
                println!("\n{}", l.details);
            }
        }
        LocationCmd::Create {
            name,
            kind,
            parent,
            address,
            tags,
            details,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_locations_client(&u).await?;
            let parent_id = match parent {
                None => None,
                Some(p) => Some(resolve_location_target(&client, &p).await?.id),
            };
            let new_loc = locations::Location {
                id: uuid::Uuid::nil(),
                path: String::new(),
                name,
                kind: kind.unwrap_or_else(|| "other".into()),
                parent_id,
                address,
                tags: locations::model::Tags(tags),
                same_as: None,
                date_created: None,
                date_modified: None,
                details: resolve_body(details)?,
            };
            let created = client
                .create(new_loc)
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
        LocationCmd::Rename {
            target,
            new_path,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_locations_client(&u).await?;
            let l = resolve_location_target(&client, &target).await?;
            let renamed = client
                .rename(l.id.to_string(), new_path)
                .await
                .map_err(|e| eyre::eyre!("rename: {e:?}"))?;
            println!("renamed → {}", renamed.path);
        }
        LocationCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_locations_client(&u).await?;
            let l = resolve_location_target(&client, &target).await?;
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
