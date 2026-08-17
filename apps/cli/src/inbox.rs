//! `task inbox …` — fleeting capture + daily triage.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::project::connect_project_client;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::task_cmd::connect_task_client;

#[derive(Subcommand)]
/// Capture + triage the inbox — the FLAP "capture" loop. Capture a
/// fleeting note with `add`, read the queue with `list` (open items,
/// oldest first), then `mark` / `snooze` / `rm` during the daily
/// review.
pub(crate) enum InboxCmd {
    /// Capture a note into the inbox (default kind `fleeting`).
    Add {
        /// The note text. Quote multi-word captures.
        text: Vec<String>,
        /// Note kind: `fleeting` (default), `literature`, `lecture`.
        #[arg(long)]
        kind: Option<String>,
        /// Capture source label. Defaults to `cli`.
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Stage an agent-proposed capture for one-tap review (status
    /// `suggested`). Producers (email ingestion, …) use this so
    /// suggestions don't flood the open queue until you accept them.
    Suggest {
        /// The summary text. Quote multi-word input.
        text: Vec<String>,
        /// Capture source label, e.g. `email`. Defaults to `agent`.
        #[arg(long)]
        source: Option<String>,
        /// Optional link back to the original (appended to the body).
        #[arg(long)]
        link: Option<String>,
        /// Note kind: `fleeting` (default), `literature`, `lecture`.
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// The AI daily processing pass: run ONE LLM turn over every
    /// open item and review the proposed promotions — `task`
    /// (created via TaskService), `note` (written to the org vault),
    /// or `skip` (offer archive). Drives `codex app-server` like
    /// `task wiki ingest`; `--heuristic` skips the LLM and proposes
    /// a task per item from `task::capture` parsing alone.
    Process {
        /// Model id (`gpt-5.4-mini`, `o3`, …). Default: daemon default.
        #[arg(long)]
        model: Option<String>,
        /// Print the proposals and stop — apply nothing.
        #[arg(long)]
        dry_run: bool,
        /// Accept every proposal without prompting.
        #[arg(long)]
        yes: bool,
        /// Deterministic proposals without an LLM: every item
        /// proposes a task, body first line = capture input.
        #[arg(long)]
        heuristic: bool,
        /// LLM turn timeout in seconds (the one turn covers the
        /// whole batch).
        #[arg(long, default_value_t = 300)]
        timeout_secs: u64,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// List the inbox. By default shows only `open` items, oldest
    /// first; `--all` includes processed + archived.
    List {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Set an item's triage status: `open` / `processed` / `archived`.
    Mark {
        id: String,
        /// `open` | `processed` | `archived`.
        status: String,
        /// For `processed`: id of the task / note it became.
        #[arg(long)]
        into: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Snooze an item until a date (`YYYY-MM-DD`); it's hidden from
    /// the daily queue until then.
    Snooze {
        id: String,
        until: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Permanently delete an item.
    Rm {
        id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

async fn connect_inbox_client(url: &str) -> eyre::Result<inbox_proto::InboxClient> {
    establish_for_url(url).await
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_inbox(cmd: InboxCmd) -> eyre::Result<()> {
    match cmd {
        InboxCmd::Add {
            text,
            kind,
            source,
            org,
            server,
        } => {
            let body = text.join(" ");
            if body.trim().is_empty() {
                eyre::bail!("nothing to capture — pass some note text");
            }
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_inbox_client(&u).await?;
            let id = uuid::Uuid::new_v4().to_string();
            let created = chrono::Utc::now().to_rfc3339();
            let mut item = inbox_proto::InboxItem::capture(
                id.clone(),
                body,
                source.unwrap_or_else(|| "cli".into()),
                created,
            );
            if let Some(k) = kind {
                item.kind = k;
            }
            client
                .upsert_inbox_item(item)
                .await
                .map_err(|e| eyre::eyre!("capture: {e:?}"))?;
            println!("captured {id}");
        }
        InboxCmd::Suggest {
            text,
            source,
            link,
            kind,
            org,
            server,
        } => {
            let mut body = text.join(" ");
            if body.trim().is_empty() {
                eyre::bail!("nothing to suggest — pass some text");
            }
            if let Some(l) = link {
                body.push_str(&format!("\n\n[open original]({l})"));
            }
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_inbox_client(&u).await?;
            let id = uuid::Uuid::new_v4().to_string();
            let created = chrono::Utc::now().to_rfc3339();
            let mut item = inbox_proto::InboxItem::capture(
                id.clone(),
                body,
                source.unwrap_or_else(|| "agent".into()),
                created,
            );
            item.status = inbox_proto::InboxItem::STATUS_SUGGESTED.to_string();
            if let Some(k) = kind {
                item.kind = k;
            }
            client
                .upsert_inbox_item(item)
                .await
                .map_err(|e| eyre::eyre!("suggest: {e:?}"))?;
            println!("suggested {id}");
        }
        InboxCmd::List {
            all,
            json,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_inbox_client(&u).await?;
            // The daily-review queue: open items whose snooze (if any)
            // has elapsed. `--all` bypasses both filters.
            let today = chrono::Utc::now().date_naive().to_string();
            let rows: Vec<_> = client
                .list_inbox()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?
                .into_iter()
                .filter(|it| {
                    all || (it.is_open()
                        && it
                            .resurface_on
                            .as_deref()
                            .is_none_or(|d| d <= today.as_str()))
                })
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            if rows.is_empty() {
                println!("inbox empty — nothing to review 🎉");
                return Ok(());
            }
            for it in &rows {
                let first_line = it.body.lines().next().unwrap_or("").trim();
                let date = it.created.get(..10).unwrap_or(&it.created);
                let snooze = it
                    .resurface_on
                    .as_deref()
                    .map(|d| format!("  💤 {d}"))
                    .unwrap_or_default();
                println!(
                    "{:<8}  {date}  {:<10}  {:<9}  {first_line}{snooze}",
                    it.id.get(..8).unwrap_or(&it.id),
                    it.kind,
                    it.status,
                );
            }
        }
        InboxCmd::Mark {
            id,
            status,
            into,
            org,
            server,
        } => {
            let allowed = [
                inbox_proto::InboxItem::STATUS_OPEN,
                inbox_proto::InboxItem::STATUS_PROCESSED,
                inbox_proto::InboxItem::STATUS_ARCHIVED,
            ];
            if !allowed.contains(&status.as_str()) {
                eyre::bail!("status must be one of: open, processed, archived");
            }
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_inbox_client(&u).await?;
            let mut item = client
                .get_inbox_item(id.clone())
                .await
                .map_err(|e| eyre::eyre!("get `{id}`: {e:?}"))?;
            item.status = status.clone();
            if into.is_some() {
                item.processed_into = into;
            }
            client
                .upsert_inbox_item(item)
                .await
                .map_err(|e| eyre::eyre!("mark: {e:?}"))?;
            println!("{id} → {status}");
        }
        InboxCmd::Snooze {
            id,
            until,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_inbox_client(&u).await?;
            let mut item = client
                .get_inbox_item(id.clone())
                .await
                .map_err(|e| eyre::eyre!("get `{id}`: {e:?}"))?;
            item.resurface_on = Some(until.clone());
            client
                .upsert_inbox_item(item)
                .await
                .map_err(|e| eyre::eyre!("snooze: {e:?}"))?;
            println!("{id} snoozed until {until}");
        }
        InboxCmd::Rm { id, org, server } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_inbox_client(&u).await?;
            client
                .delete_inbox_item(id.clone())
                .await
                .map_err(|e| eyre::eyre!("delete: {e:?}"))?;
            println!("deleted {id}");
        }
        InboxCmd::Process {
            model,
            dry_run,
            yes,
            heuristic,
            timeout_secs,
            org,
            server,
        } => {
            run_inbox_process(model, dry_run, yes, heuristic, timeout_secs, org, server).await?;
        }
    }
    Ok(())
}

/// What the user chose for one proposal.
enum ProcessDecision {
    Accept,
    /// Accept a task proposal with a replacement title.
    EditTitle(String),
    Decline,
    Quit,
}

fn prompt_process_decision(is_task: bool) -> eyre::Result<ProcessDecision> {
    use std::io::{BufRead as _, Write as _};
    let opts = if is_task {
        "[y]es / [e]dit title / [n]o / [q]uit"
    } else {
        "[y]es / [n]o / [q]uit"
    };
    loop {
        print!("  apply? {opts} > ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(ProcessDecision::Accept),
            "n" | "no" | "" => return Ok(ProcessDecision::Decline),
            "q" | "quit" => return Ok(ProcessDecision::Quit),
            "e" | "edit" if is_task => {
                print!("  new title > ");
                std::io::stdout().flush()?;
                let mut t = String::new();
                std::io::stdin().lock().read_line(&mut t)?;
                let t = t.trim().to_string();
                if t.is_empty() {
                    println!("  (empty title — keeping the proposal)");
                    return Ok(ProcessDecision::Accept);
                }
                return Ok(ProcessDecision::EditTitle(t));
            }
            other => println!("  unrecognized `{other}`"),
        }
    }
}

/// One-line human rendering of a proposal action.
fn describe_proposal(action: &agent_inbox::ProposalAction) -> String {
    match action {
        agent_inbox::ProposalAction::Task {
            title,
            project_title,
            contexts,
            due,
        } => {
            let mut extras = Vec::new();
            if let Some(p) = project_title {
                extras.push(format!("project {p}"));
            }
            if !contexts.is_empty() {
                extras.push(format!("contexts {}", contexts.join(" ")));
            }
            if let Some(d) = due {
                extras.push(format!("due {d}"));
            }
            let suffix = if extras.is_empty() {
                String::new()
            } else {
                format!("  ({})", extras.join(", "))
            };
            format!("task: \"{title}\"{suffix}")
        }
        agent_inbox::ProposalAction::Note { path, body } => {
            let first = body.lines().next().unwrap_or("").trim();
            format!("note: {path}  \"{first}\"")
        }
        agent_inbox::ProposalAction::Skip { reason } => format!("skip: {reason}"),
    }
}

/// Deterministic no-LLM proposal: everything becomes a task whose
/// capture input is the item body's first line (`task::capture`
/// extracts tags / contexts / dates from it at apply time).
fn heuristic_proposal(item: &inbox_proto::InboxItem) -> agent_inbox::Proposal {
    let first_line = item.body.lines().next().unwrap_or("").trim();
    agent_inbox::Proposal {
        item_id: item.id.clone(),
        action: agent_inbox::ProposalAction::Task {
            title: if first_line.is_empty() {
                "Untitled task".to_string()
            } else {
                first_line.to_string()
            },
            project_title: None,
            contexts: Vec::new(),
            due: None,
        },
    }
}

/// Set an inbox item's status (and optional provenance) via the
/// standard get-mutate-upsert cycle the `mark` verb uses.
async fn mark_inbox_item(
    client: &inbox_proto::InboxClient,
    item: &inbox_proto::InboxItem,
    status: &str,
    processed_into: Option<String>,
) -> eyre::Result<()> {
    let mut updated = item.clone();
    updated.status = status.to_string();
    if processed_into.is_some() {
        updated.processed_into = processed_into;
    }
    client
        .upsert_inbox_item(updated)
        .await
        .map_err(|e| eyre::eyre!("mark {}: {e:?}", item.id))?;
    Ok(())
}

// ── Inbox AI processing pass ─────────────────────────────────────────
//
// `task inbox process` — the "daily processing pass" from
// Relevancy/inbox doctrine: one LLM turn over every open
// fleeting item proposes a `task` / `note` / `skip` promotion per
// item; the user reviews each proposal (y / n / e-dit title, or
// `--yes` for all) and accepted ones are applied through the
// existing service surfaces:
//
//   task → `task::capture(title)` (tags/contexts/dates), project id
//          from the proposed title via direct match or
//          `task::infer_project_id`, `TaskService::create`, then the
//          inbox item is marked processed with `processed_into` =
//          the created task's vault path.
//   note → materialized into the org vault over the vault-sync
//          surface (`VaultSync::put_file`, `vault_id = "default"`,
//          `CreateOnly`) — remote server or embedded backend alike;
//          no local checkout required.
//   skip → offer `archived`.

#[allow(clippy::too_many_lines)]
async fn run_inbox_process(
    model: Option<String>,
    dry_run: bool,
    yes: bool,
    heuristic: bool,
    timeout_secs: u64,
    org: Option<String>,
    server: Option<String>,
) -> eyre::Result<()> {
    use std::io::IsTerminal as _;

    let slug = resolve_active_org(org)?;
    let url = resolve_org_vox_url(server, &slug);
    let inbox = connect_inbox_client(&url).await?;

    // The daily queue: open items whose snooze (if any) elapsed,
    // oldest first — same filter as `task inbox list`.
    let today = chrono::Utc::now().date_naive().to_string();
    let mut items: Vec<inbox_proto::InboxItem> = inbox
        .list_inbox()
        .await
        .map_err(|e| eyre::eyre!("list inbox: {e:?}"))?
        .into_iter()
        .filter(|it| {
            it.is_open()
                && it
                    .resurface_on
                    .as_deref()
                    .is_none_or(|d| d <= today.as_str())
        })
        .collect();
    items.sort_by(|a, b| a.created.cmp(&b.created));
    if items.is_empty() {
        println!("inbox empty — nothing to process 🎉");
        return Ok(());
    }

    if !dry_run && !yes && !std::io::stdin().is_terminal() {
        eyre::bail!(
            "stdin is not a terminal — the review loop is interactive; \
             rerun with --yes to accept all proposals or --dry-run to only print them"
        );
    }

    // Project list — both the prompt vocabulary and the apply-time
    // title → id resolver.
    let pc = connect_project_client(&url).await?;
    let known_projects: Vec<(uuid::Uuid, String)> = pc
        .list()
        .await
        .map_err(|e| eyre::eyre!("list projects: {e:?}"))?
        .into_iter()
        .map(|p| (p.id, p.title))
        .collect();

    // Local org checkout (when there is one): the LLM turn's
    // workspace root + the vault dir notes get written into.
    let local_org_root = crate::org_ctx::resolve_active(Some(slug.as_str()))
        .ok()
        .map(|ctx| ctx.root.path().to_path_buf());

    // ── Propose ────────────────────────────────────────────────
    let proposals: Vec<agent_inbox::Proposal> = if heuristic {
        items.iter().map(heuristic_proposal).collect()
    } else {
        let req = agent_inbox::bridge::ProcessRequest {
            items: items
                .iter()
                .map(|it| agent_inbox::bridge::ProcessItem {
                    id: it.id.clone(),
                    body: it.body.clone(),
                    source: it.source.clone(),
                    created: it.created.clone(),
                })
                .collect(),
            project_titles: known_projects.iter().map(|(_, t)| t.clone()).collect(),
            today: today.clone(),
            model: model.clone(),
            timeout: std::time::Duration::from_secs(timeout_secs),
        };
        let workspace = match &local_org_root {
            Some(r) => r.clone(),
            None => std::env::current_dir().map_err(|e| eyre::eyre!("cwd: {e}"))?,
        };
        eprintln!(
            "› inbox process@{}  {} item(s), {} project(s)",
            model.as_deref().unwrap_or("default"),
            items.len(),
            known_projects.len()
        );
        let backend = agent_codex::CodexBackend::new();
        match agent_inbox::bridge::run_process(&backend, &workspace, req).await {
            Ok(p) => p,
            Err(e) => {
                return Err(eyre::eyre!(
                    "inbox process: {e}\n\nThis verb drives `codex app-server` — the same \
                     backend as `task wiki ingest`.\n  - check the `codex` CLI is installed and \
                     on $PATH (and signed in)\n  - or rerun with --heuristic for deterministic \
                     no-LLM proposals"
                ));
            }
        }
    };

    // ── Review + apply ─────────────────────────────────────────
    let total = items.len();
    let mut created_tasks = 0usize;
    let mut notes_written = 0usize;
    let mut archived = 0usize;
    let mut left_open = 0usize;

    let mut task_client: Option<task::TaskServiceClient> = None;
    let mut vault_sync: Option<vault_proto::VaultSyncClient> = None;
    'items: for (idx, item) in items.iter().enumerate() {
        let first_line = item.body.lines().next().unwrap_or("").trim();
        println!(
            "\n[{}/{total}] {}  {}  \"{first_line}\"",
            idx + 1,
            item.id.get(..8).unwrap_or(&item.id),
            item.created.get(..10).unwrap_or(&item.created),
        );
        let Some(proposal) = proposals.iter().find(|p| p.item_id == item.id) else {
            println!("  → (no proposal returned for this item — left open)");
            left_open += 1;
            continue;
        };
        println!("  → {}", describe_proposal(&proposal.action));
        if dry_run {
            continue;
        }

        let is_task = matches!(proposal.action, agent_inbox::ProposalAction::Task { .. });
        let decision = if yes {
            ProcessDecision::Accept
        } else {
            prompt_process_decision(is_task)?
        };
        let edited_title = match decision {
            ProcessDecision::Quit => {
                println!("stopping — remaining items left open");
                left_open += total - idx;
                break 'items;
            }
            ProcessDecision::Decline => {
                left_open += 1;
                continue;
            }
            ProcessDecision::EditTitle(t) => Some(t),
            ProcessDecision::Accept => None,
        };

        match &proposal.action {
            agent_inbox::ProposalAction::Task {
                title,
                project_title,
                contexts,
                due,
            } => {
                let capture_input = edited_title.as_deref().unwrap_or(title);
                let mut info = task::capture(capture_input);
                info.path = task::write::default_task_path(&info.title, None);
                // Merge proposal contexts into whatever `capture`
                // extracted from inline `@…` tokens.
                for c in contexts {
                    if !info.contexts.0.iter().any(|x| x.eq_ignore_ascii_case(c)) {
                        info.contexts.0.push(c.clone());
                    }
                }
                if info.due.is_none() {
                    info.due.clone_from(due);
                }
                // Project: proposed title (direct, case-insensitive
                // match against the provided list only), else any
                // `[[wikilink]]` the capture parser extracted.
                info.project_id = project_title
                    .as_deref()
                    .and_then(|t| {
                        known_projects
                            .iter()
                            .find(|(_, kt)| kt.eq_ignore_ascii_case(t))
                            .map(|(id, _)| *id)
                    })
                    .or_else(|| task::infer_project_id(&info.projects.0, &known_projects));
                if task_client.is_none() {
                    task_client = Some(connect_task_client(&url).await?);
                }
                let created = task_client
                    .as_ref()
                    .expect("task client connected above")
                    .create(info)
                    .await
                    .map_err(|e| eyre::eyre!("create task: {e:?}"))?;
                println!("  created task {} ({})", created.title, created.path);
                mark_inbox_item(
                    &inbox,
                    item,
                    inbox_proto::InboxItem::STATUS_PROCESSED,
                    Some(created.path),
                )
                .await?;
                created_tasks += 1;
            }
            agent_inbox::ProposalAction::Note { path, body } => {
                // Fall back to the raw capture when the LLM sent an
                // empty BODY — never write an empty note.
                let content = if body.trim().is_empty() {
                    item.body.as_str()
                } else {
                    body.as_str()
                };
                // Materialize through the org's vault-sync surface
                // (`vault_id = "default"` is the org vault) — remote
                // server or embedded backend alike, no local
                // checkout needed. `CreateOnly` keeps the old
                // `create_page` never-clobber semantics. This
                // replaces both the direct `vault_obsidian` write
                // and the "no local vault — create it yourself"
                // manual flow.
                if vault_sync.is_none() {
                    vault_sync =
                        Some(establish_for_url::<vault_proto::VaultSyncClient>(&url).await?);
                }
                let vs = vault_sync.as_ref().expect("vault sync connected above");
                match vs
                    .put_file(
                        "default".to_owned(),
                        path.clone(),
                        content.as_bytes().to_vec(),
                        vault_proto::IfMatch::CreateOnly,
                    )
                    .await
                {
                    Ok(_) => {
                        println!("  wrote note {path}");
                        mark_inbox_item(
                            &inbox,
                            item,
                            inbox_proto::InboxItem::STATUS_PROCESSED,
                            Some(path.clone()),
                        )
                        .await?;
                        notes_written += 1;
                    }
                    Err(e) => {
                        println!("  could not write {path}: {e} — left open");
                        left_open += 1;
                    }
                }
            }
            agent_inbox::ProposalAction::Skip { .. } => {
                mark_inbox_item(&inbox, item, inbox_proto::InboxItem::STATUS_ARCHIVED, None)
                    .await?;
                println!("  archived");
                archived += 1;
            }
        }
    }

    if dry_run {
        println!("\n(dry run — nothing applied)");
    } else {
        println!(
            "\ndone: {created_tasks} task(s) created, {notes_written} note(s), \
             {archived} archived, {left_open} left open"
        );
    }
    Ok(())
}
