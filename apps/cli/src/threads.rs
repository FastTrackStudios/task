//! `task threads …` — conversation logs on tasks / projects.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::project::connect_project_client;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::task_cmd::connect_task_client;
use crate::timer::timer_owner_id;

#[derive(Subcommand)]
/// Log conversations & topics against a task or project. `new` opens a
/// thread (topic); `post` adds a message; `list`/`show` read them.
/// Anchored by `(entity_type, entity_id)` so the same primitive works
/// for any entity later (forge issues, chats, ingested comms).
///
/// Org / server routing: uses the global `--org` / `--server` flags
/// (no per-variant duplicates).
pub(crate) enum ThreadsCmd {
    /// Open a new thread (topic) on a task or project.
    New {
        /// Host entity kind: `task` | `project`.
        #[arg(long)]
        entity_type: String,
        /// Host entity — UUID, id prefix, vault path, or title
        /// (resolved per `--entity-type`).
        #[arg(long)]
        entity_id: String,
        /// Topic / title. Quote multi-word.
        title: Vec<String>,
        /// Kind: `discussion` (default) | `question` | `decision` | `action` | `praise`.
        #[arg(long)]
        kind: Option<String>,
    },
    /// Post a message to a thread.
    Post {
        /// Target thread id.
        thread_id: uuid::Uuid,
        /// Message text. Quote multi-word.
        text: Vec<String>,
        /// Reply to another message in the thread.
        #[arg(long)]
        reply_to: Option<uuid::Uuid>,
        /// Source label: `native` (default) | `agent` | …
        #[arg(long)]
        source: Option<String>,
        /// Author display label. Defaults to `cli` (or `agent` when `--source agent`).
        #[arg(long)]
        author: Option<String>,
    },
    /// List threads on a task or project.
    List {
        #[arg(long)]
        entity_type: String,
        /// Host entity — UUID, id prefix, vault path, or title
        /// (resolved per `--entity-type`).
        #[arg(long)]
        entity_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Show a thread's messages.
    Show {
        thread_id: uuid::Uuid,
        #[arg(long)]
        json: bool,
    },
    /// Mark a thread resolved (or `--unresolve` to reopen).
    Resolve {
        thread_id: uuid::Uuid,
        #[arg(long)]
        unresolve: bool,
    },
    /// Delete a thread and its messages.
    Rm { id: uuid::Uuid },
}

async fn connect_threads_client(url: &str) -> eyre::Result<threads::ThreadsServiceClient> {
    establish_for_url(url).await
}

/// Resolve `(org_id, local_user_id)` for CLI-authored threads, matching
/// the timer CLI's identity derivation so UI + CLI share a keyspace.
fn threads_local_ids(org_override: Option<&str>) -> (uuid::Uuid, uuid::Uuid) {
    let org_id = crate::org_ctx::resolve_active(org_override)
        .ok()
        .and_then(|ctx| ctx.root.manifest().ok().map(|m| m.id))
        .unwrap_or_else(uuid::Uuid::nil);
    (org_id, timer_owner_id(org_id))
}

/// Resolve a threads `--entity-id` reference (uuid, id prefix, path,
/// or title) against the service named by `--entity-type`. Unknown
/// entity types still take a literal UUID.
async fn resolve_thread_entity(
    url: &str,
    entity_type: &str,
    target: &str,
) -> eyre::Result<uuid::Uuid> {
    if let Ok(id) = uuid::Uuid::parse_str(target) {
        return Ok(id);
    }
    match entity_type {
        "task" => {
            let tc = connect_task_client(url).await?;
            Ok(crate::json_out::resolve_task_flexible(&tc, target)
                .await?
                .id)
        }
        "project" => {
            let pc = connect_project_client(url).await?;
            Ok(crate::json_out::resolve_project_flexible(&pc, target)
                .await?
                .id)
        }
        other => Err(crate::errors::usage("resolve --entity-id")
            .cause(format!(
                "`{target}` is not a UUID and entity type `{other}` has no name resolver"
            ))
            .hint("pass a literal UUID, or use --entity-type task|project")
            .report()),
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_threads(cmd: ThreadsCmd) -> eyre::Result<()> {
    // Global --org / --server routing, shared by every arm.
    let slug = resolve_active_org(None)?;
    let url = resolve_org_vox_url(None, &slug);
    match cmd {
        ThreadsCmd::New {
            entity_type,
            entity_id,
            title,
            kind,
        } => {
            let title = title.join(" ");
            if title.trim().is_empty() {
                eyre::bail!("a thread needs a title — pass some text");
            }
            let entity_id = resolve_thread_entity(&url, &entity_type, &entity_id).await?;
            let (org_id, user_id) = threads_local_ids(None);
            let client = connect_threads_client(&url).await?;
            let t = client
                .create_thread(threads::CreateThreadRequest {
                    org_id,
                    entity_type,
                    entity_id,
                    title,
                    kind: kind.unwrap_or_default(),
                    created_by: user_id,
                    source_kind: "native".into(),
                    source_ref: None,
                    source_url: None,
                })
                .await
                .map_err(|e| eyre::eyre!("create_thread: {e:?}"))?;
            println!("created thread {}  {}", t.id, t.title);
        }
        ThreadsCmd::Post {
            thread_id,
            text,
            reply_to,
            source,
            author,
        } => {
            let body = text.join(" ");
            if body.trim().is_empty() {
                eyre::bail!("nothing to post — pass some message text");
            }
            let (org_id, user_id) = threads_local_ids(None);
            let client = connect_threads_client(&url).await?;
            let source_kind = source.unwrap_or_else(|| "native".into());
            let author_label = author.unwrap_or_else(|| {
                if source_kind == "agent" {
                    "agent".into()
                } else {
                    "cli".into()
                }
            });
            let m = client
                .post_message(threads::PostMessageRequest {
                    thread_id,
                    org_id,
                    author_id: Some(user_id),
                    author_label,
                    body,
                    reply_to,
                    source_kind,
                    external_id: None,
                    original_text: None,
                    source_url: None,
                    posted_at: None,
                })
                .await
                .map_err(|e| eyre::eyre!("post_message: {e:?}"))?;
            println!("posted {}", m.id);
        }
        ThreadsCmd::List {
            entity_type,
            entity_id,
            json,
        } => {
            let entity_id = resolve_thread_entity(&url, &entity_type, &entity_id).await?;
            let client = connect_threads_client(&url).await?;
            let rows = client
                .list_threads(entity_type, entity_id)
                .await
                .map_err(|e| eyre::eyre!("list_threads: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{} threads", rows.len());
            for t in rows {
                let r = if t.resolved { " (resolved)" } else { "" };
                println!("  {}  [{}]{}  {}", t.id, t.kind, r, t.title);
            }
        }
        ThreadsCmd::Show { thread_id, json } => {
            let client = connect_threads_client(&url).await?;
            let msgs = client
                .list_messages(thread_id)
                .await
                .map_err(|e| eyre::eyre!("list_messages: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&msgs).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{} messages", msgs.len());
            for m in msgs {
                println!(
                    "  [{}] {}: {}",
                    m.posted_at.format("%Y-%m-%d %H:%M"),
                    m.author_label,
                    m.body
                );
            }
        }
        ThreadsCmd::Resolve {
            thread_id,
            unresolve,
        } => {
            let (_org_id, user_id) = threads_local_ids(None);
            let client = connect_threads_client(&url).await?;
            let t = client
                .set_resolved(thread_id, !unresolve, Some(user_id))
                .await
                .map_err(|e| eyre::eyre!("set_resolved: {e:?}"))?;
            println!("thread {} resolved={}", t.id, t.resolved);
        }
        ThreadsCmd::Rm { id } => {
            let client = connect_threads_client(&url).await?;
            client
                .delete_thread(id)
                .await
                .map_err(|e| eyre::eyre!("delete_thread: {e:?}"))?;
            println!("deleted thread {id}");
        }
    }
    Ok(())
}
