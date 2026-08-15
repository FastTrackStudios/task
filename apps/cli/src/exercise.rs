//! `task exercise …` — the movement library.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;

// ── Exercises (exercises::Store) ─────────────────────────────────────

#[derive(Subcommand)]
pub(crate) enum ExerciseCmd {
    List {
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
        kind: Option<String>,
        #[arg(long)]
        muscle: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
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

pub(crate) async fn connect_exercises_client(
    url: &str,
) -> eyre::Result<exercises::ExercisesServiceClient> {
    establish_for_url(url).await
}

pub(crate) async fn resolve_exercise_target(
    client: &exercises::ExercisesServiceClient,
    target: &str,
) -> eyre::Result<exercises::Exercise> {
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
        .find(|e| e.path == target || e.name.eq_ignore_ascii_case(target))
        .ok_or_else(|| {
            crate::errors::not_found("resolve target", target)
                .cause("no path or name match")
                .report()
        })
}

pub(crate) async fn run_exercise(cmd: ExerciseCmd) -> eyre::Result<()> {
    match cmd {
        ExerciseCmd::List {
            query,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_exercises_client(&u).await?;
            let rows: Vec<_> = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?
                .into_iter()
                .filter(|e| {
                    query
                        .as_deref()
                        .is_none_or(|q| e.name.to_lowercase().contains(&q.to_lowercase()))
                })
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            for e in &rows {
                println!("{:<32}  {:<12}    {}", e.name, e.category, e.path);
            }
        }
        ExerciseCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_exercises_client(&u).await?;
            let e = resolve_exercise_target(&client, &target).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&e).map_err(|err| eyre::eyre!("json: {err}"))?
                );
                return Ok(());
            }
            println!("{}\n", e.name);
            println!("  id:        {}", e.id);
            println!("  path:      {}", e.path);
            println!("  category:  {}", e.category);
            if !e.primary_muscles.is_empty() {
                println!("  muscles:   {}", e.primary_muscles.0.join(", "));
            }
            if !e.equipment.is_empty() {
                println!("  equipment: {}", e.equipment.0.join(", "));
            }
        }
        ExerciseCmd::Create {
            name,
            kind,
            muscle,
            tags,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_exercises_client(&u).await?;
            let primary_muscles = muscle
                .map(|m| exercises::model::StringList(vec![m]))
                .unwrap_or_default();
            let new_ex = exercises::Exercise {
                path: String::new(),
                id: uuid::Uuid::nil(),
                name,
                aliases: exercises::model::StringList::default(),
                description: None,
                category: kind.unwrap_or_else(|| "other".into()),
                primary_muscles,
                secondary_muscles: exercises::model::StringList::default(),
                equipment: exercises::model::StringList::default(),
                mechanics: None,
                force: None,
                instructions: exercises::model::StringList::default(),
                video_url: None,
                image_url: None,
                tags: exercises::model::StringList(tags),
                date_created: None,
                date_modified: None,
                details: String::new(),
            };
            let created = client
                .create(new_ex)
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
        ExerciseCmd::Rename {
            target,
            new_path,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_exercises_client(&u).await?;
            let e = resolve_exercise_target(&client, &target).await?;
            let renamed = client
                .rename(e.id.to_string(), new_path)
                .await
                .map_err(|e| eyre::eyre!("rename: {e:?}"))?;
            println!("renamed → {}", renamed.path);
        }
        ExerciseCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_exercises_client(&u).await?;
            let e = resolve_exercise_target(&client, &target).await?;
            if !yes && !confirm(&format!("delete `{}` ({})?", e.name, e.path))? {
                println!("aborted");
                return Ok(());
            }
            client
                .delete(e.id.to_string())
                .await
                .map_err(|e| eyre::eyre!("delete: {e:?}"))?;
            println!("deleted {}", e.path);
        }
    }
    Ok(())
}
