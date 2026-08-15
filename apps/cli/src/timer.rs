//! `task timer …` — billable time tracking.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::resolve_org_vox_url;

#[derive(Subcommand)]
pub(crate) enum TimerCmd {
    /// Start the timer for the configured user. Fails if a
    /// session is already open.
    Start {
        /// Free-text description. Quoted to allow spaces.
        /// Optional when `--task` is given (defaults to the
        /// task's title).
        #[arg(required_unless_present = "task")]
        description: Option<String>,
        /// Task to track against — full UUID, unique id
        /// prefix, or vault-relative path. Validates the
        /// task exists and fills description (title),
        /// project (the task's project), and task-note
        /// (the task's path); explicit flags still win.
        #[arg(long)]
        task: Option<String>,
        /// Project the session is logged against — uuid,
        /// title, vault path, or a unique prefix of either.
        /// Empty = uncategorized.
        #[arg(long)]
        project: Option<String>,
        /// Vault-relative path to the task note this
        /// session is for.
        #[arg(long, default_value = "")]
        task_note: String,
        /// Tag names to attach to the session. Tags are
        /// auto-created in the calling user's org if they
        /// don't already exist. Pass `--tag focus --tag review`
        /// to attach two.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Emit the started session as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop the current session. Snapshots `rate_cents` +
    /// `currency` via the rate cascade and writes the closed
    /// row.
    Stop {
        /// Emit the closed session as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show the active session, if any.
    Active {
        /// Emit the session as JSON (plus derived
        /// `seconds_elapsed` and joined task / project
        /// titles where resolvable). `null` when idle.
        #[arg(long)]
        json: bool,
    },
    /// Atomic stop-then-start. Same args as `start`.
    Switch {
        #[arg(required_unless_present = "task")]
        description: Option<String>,
        /// Task to track against (id / prefix / path) —
        /// same semantics as `start --task`.
        #[arg(long)]
        task: Option<String>,
        /// Project — uuid, title, path, or unique prefix.
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = "")]
        task_note: String,
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Emit `{stopped, started}` sessions as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Retro-log a past session: `--from` / `--to` ISO 8601
    /// timestamps + description. Skips the active-timer
    /// invariant.
    Log {
        #[arg(required_unless_present = "task")]
        description: Option<String>,
        #[arg(long)]
        from: chrono::DateTime<chrono::Utc>,
        #[arg(long)]
        to: chrono::DateTime<chrono::Utc>,
        /// Task to log against (id / prefix / path) — same
        /// semantics as `start --task`.
        #[arg(long)]
        task: Option<String>,
        /// Project — uuid, title, path, or unique prefix.
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = "")]
        task_note: String,
        /// `true` / `false` to override the project default.
        /// Omit to inherit.
        #[arg(long)]
        billable: Option<bool>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Emit the logged session as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set an org-level member hourly rate (cascade level 3) for a
    /// user. New sessions logged for that user snapshot this rate at
    /// close. Upserts. Use `--org` to target the org's timer DB.
    SetRate {
        /// The member's user id (uuid).
        #[arg(long)]
        user_id: uuid::Uuid,
        /// Hourly rate in cents (e.g. 3000 = $30/hr).
        #[arg(long)]
        cents: i64,
        #[arg(long, default_value = "USD")]
        currency: String,
        /// Emit the stored rate as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Edit an existing session. Only the flags you pass change; the
    /// billable rate is re-snapshotted from the cascade afterward
    /// (so reassigning `--user-id` or `--project` re-rates it).
    Edit {
        /// Session id (uuid).
        id: uuid::Uuid,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        from: Option<chrono::DateTime<chrono::Utc>>,
        #[arg(long)]
        to: Option<chrono::DateTime<chrono::Utc>>,
        /// Reassign to a project — uuid, title, path, or a
        /// unique prefix of either.
        #[arg(long)]
        project: Option<String>,
        /// Reassign to a different member.
        #[arg(long)]
        user_id: Option<uuid::Uuid>,
        #[arg(long)]
        billable: Option<bool>,
        #[arg(long)]
        task_note: Option<String>,
        /// Emit the updated session as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Delete a session by id. Permanent.
    Delete {
        /// Session id (uuid).
        id: uuid::Uuid,
        /// Emit `{"deleted": <id>}` as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List sessions. Defaults to the last 7 days, all
    /// users (matching the `finance project` rollup —
    /// the per-org DB is already the scope).
    List {
        /// Only sessions on this project — uuid, title,
        /// path, or a unique prefix of either.
        #[arg(long)]
        project: Option<String>,
        /// Only sessions logged by this user id. Omit for
        /// all users in the org.
        #[arg(long)]
        user: Option<uuid::Uuid>,
        /// Inclusive since-date. Defaults to 7 days ago.
        #[arg(long)]
        since: Option<chrono::DateTime<chrono::Utc>>,
        /// Exclusive until-date. Defaults to "now".
        #[arg(long)]
        until: Option<chrono::DateTime<chrono::Utc>>,
        /// Filter open / closed sessions; omit for both.
        #[arg(long)]
        open: Option<bool>,
        /// Filter billable / non-billable; omit for both.
        #[arg(long)]
        billable: Option<bool>,
        /// Emit the sessions as a JSON array.
        #[arg(long)]
        json: bool,
    },
    /// Resolve the rate cascade for the configured user +
    /// project. Useful to preview "what will this session
    /// bill at" before stopping.
    Resolve {
        /// Project — uuid, title, path, or unique prefix.
        #[arg(long)]
        project: Option<String>,
        /// Emit the resolution as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Audit which user_ids appear on sessions, with name
    /// resolution from the org's `auth.sqlite`. Useful for
    /// spotting detached / mis-attributed ids before
    /// invoicing.
    Users {
        /// Emit the per-user aggregates as a JSON array.
        #[arg(long)]
        json: bool,
    },
    /// Bulk-swap every matching session's `user_id`.
    /// Optional filters narrow the swap to a project /
    /// date window — without them, ALL sessions for `from`
    /// in the org are moved.
    ReassignUser {
        /// Source user_id (current owner of the sessions).
        #[arg(long)]
        from: uuid::Uuid,
        /// Destination user_id (new owner).
        #[arg(long)]
        to: uuid::Uuid,
        /// Limit to one project — uuid, title, path, or a
        /// unique prefix of either.
        #[arg(long)]
        project: Option<String>,
        /// Inclusive lower bound on `start_time`.
        #[arg(long)]
        since: Option<chrono::DateTime<chrono::Utc>>,
        /// Exclusive upper bound on `start_time`.
        #[arg(long)]
        until: Option<chrono::DateTime<chrono::Utc>>,
        /// Limit to sessions whose description matches this
        /// substring (case-insensitive). Useful for
        /// untangling "video editing" vs "PNG tracking"
        /// rows that share a user_id.
        #[arg(long)]
        description_contains: Option<String>,
        /// Re-snapshot `rate_cents` + `currency` from the
        /// rate cascade for the *new* user. Off by default
        /// so already-billed amounts don't shift; pass when
        /// you're correcting a fresh mistake.
        #[arg(long, default_value_t = false)]
        rerate: bool,
        /// Show what would change without writing.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Emit the match/update summary as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Tag CRUD + attach to existing sessions.
    #[command(subcommand)]
    Tag(TimerTagCmd),
}

#[derive(Subcommand)]
pub(crate) enum TimerTagCmd {
    /// List tags in the calling user's org.
    List {
        /// Emit the tags as a JSON array.
        #[arg(long)]
        json: bool,
    },
    /// Create a tag. Idempotent — no-op if a tag with that
    /// name already exists.
    Create {
        name: String,
        /// Hex `#RRGGBB` (UI hint). Empty = auto-pick.
        #[arg(long, default_value = "")]
        color: String,
        /// Emit the tag as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Delete a tag by name. Removes the join rows on every
    /// session via FK cascade.
    Rm {
        name: String,
        /// Emit the deleted tag as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Attach tags to an existing session.
    Attach {
        session_id: uuid::Uuid,
        #[arg(long = "tag", required = true)]
        tags: Vec<String>,
        /// Emit `{session_id, attached}` as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Detach tags from a session. `--tag <name>` removes
    /// that tag; `--all` removes every tag.
    Detach {
        session_id: uuid::Uuid,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        all: bool,
        /// Emit `{session_id, detached}` as JSON.
        #[arg(long)]
        json: bool,
    },
}

/// Deterministic local-owner user id for an org, so the CLI and the
/// `/timer` page resolve the same user and therefore see the same
/// sessions. One definition, in `timer_proto::owner` — this used to be
/// a hand-copied `new_v5` that only a doc comment kept in step with the
/// web UI's and the watch bridge's copies.
pub(crate) use timer_proto::local_owner_id as timer_owner_id;

pub(crate) async fn run_timer(cmd: TimerCmd, org_override: Option<&str>) -> eyre::Result<()> {
    use timer_proto::service::{LogSessionRequest, StartTimerRequest};

    // The timer DATA lives behind the org's `TimerService` — remote
    // server or embedded in-process backend alike (the localhost
    // default falls back to embedded when no server is running; see
    // `establish_for_url`). The CLI no longer opens timer.sqlite.
    let slug = crate::resolve_slug(org_override)?;
    forward_timer_db_override();
    // Local org tree — OPTIONAL now. When present (co-resident org)
    // it supplies the manifest org-id, the vault root for
    // project-path display scans, and the best-effort auth.sqlite
    // name join; when absent (remote-only session) the id comes from
    // the server's well-known doc and the local joins degrade
    // gracefully. `TASK_VAULT_ROOT` stays a fixture override.
    let data_root = org_proto::DataRoot::from_env().ok();
    let local = data_root.as_ref().and_then(|r| r.load_org(&slug).ok());
    let vault_root = std::env::var("TASK_VAULT_ROOT").map_or_else(
        |_| {
            data_root.as_ref().map_or_else(
                || std::path::PathBuf::from("."),
                |r| r.org(&slug).vault_dir(),
            )
        },
        std::path::PathBuf::from,
    );
    // Unified identity. The org id is the org's *manifest* id (the
    // same value the web UI gets from `.well-known` → `OrgMeta.id`),
    // and the default user is the deterministic "local owner" derived
    // from it — matching `task_ui::chrome::owner_id`. This is what makes
    // CLI- and UI-logged sessions land in the same `(org_id, user_id)`
    // keyspace so both surfaces see the same data. `TASK_ORG_ID` /
    // `TASK_USER_ID` still override (e.g. logging a contractor's time
    // under a distinct user id).
    let org_id = match std::env::var("TASK_ORG_ID")
        .ok()
        .and_then(|s| s.parse::<uuid::Uuid>().ok())
        .or_else(|| local.as_ref().map(|(_, m)| m.id))
    {
        Some(id) => id,
        None => crate::remote_org_id(&slug).await.unwrap_or_else(|| {
            uuid::Uuid::parse_str("00000000-0000-0000-0000-00000000000a").unwrap()
        }),
    };
    let user_id = std::env::var("TASK_USER_ID")
        .ok()
        .and_then(|s| s.parse::<uuid::Uuid>().ok())
        .unwrap_or_else(|| timer_owner_id(org_id));

    let vox_url = resolve_org_vox_url(None, &slug);
    // One typed client for every session/rate/tag operation below —
    // named `store` to keep the call sites reading the same as the
    // old direct-store code.
    let store: timer_proto::TimerServiceClient = establish_for_url(&vox_url).await?;
    // `--task <id|prefix|path>` → TaskInfo, used to default the
    // description / project / task-note on start | switch | log.
    let resolve_task_flag = |flag: Option<String>| {
        let vox_url = vox_url.clone();
        async move {
            match flag {
                None => Ok::<_, eyre::Report>(None),
                Some(t) => {
                    let tc: task::TaskServiceClient = establish_for_url(&vox_url).await?;
                    Ok(Some(crate::json_out::resolve_task_flexible(&tc, &t).await?))
                }
            }
        }
    };
    // `--project <uuid|title|path|prefix>` → (id, known-path).
    let resolve_project_flag = |flag: Option<String>| {
        let vox_url = vox_url.clone();
        async move {
            crate::json_out::resolve_project_arg(flag.as_deref(), || async {
                establish_for_url::<project::ProjectServiceClient>(&vox_url).await
            })
            .await
        }
    };

    match cmd {
        TimerCmd::Start {
            description,
            task,
            project,
            task_note,
            tags,
            json,
        } => {
            let task_info = resolve_task_flag(task).await?;
            let (mut project_id, resolved_path) = resolve_project_flag(project).await?;
            // --task fills the gaps; explicit flags win.
            if project_id.is_none() {
                project_id = task_info.as_ref().and_then(|t| t.project_id);
            }
            let description = description
                .or_else(|| task_info.as_ref().map(|t| t.title.clone()))
                .unwrap_or_default();
            let task_note = if task_note.is_empty() {
                task_info
                    .as_ref()
                    .map(|t| t.path.clone())
                    .unwrap_or_default()
            } else {
                task_note
            };
            let project_path =
                resolved_path.unwrap_or_else(|| project_path_for(&vault_root, project_id));
            let session = store
                .start_timer(StartTimerRequest {
                    user_id,
                    org_id,
                    project_id,
                    project_path,
                    task_note_path: task_note,
                    description,
                })
                .await
                .map_err(|e| eyre::eyre!("start: {e}"))?;
            if !tags.is_empty() {
                store
                    .attach_tags(session.id, org_id, tags.clone())
                    .await
                    .map_err(|e| eyre::eyre!("attach tags: {e:?}"))?;
            }
            if json {
                crate::json_out::print_json(&crate::json_out::session_json(&session))?;
            } else {
                println!("Started {} at {}", session.id, session.start_time);
                println!("  description: {}", session.description);
                if !session.project_path.is_empty() {
                    println!("  project:     {}", session.project_path);
                }
                if !session.task_note_path.is_empty() {
                    println!("  task:        {}", session.task_note_path);
                }
                println!("  billable:    {}", session.billable);
                if !tags.is_empty() {
                    println!("  tags:        {}", tags.join(", "));
                }
            }
        }
        TimerCmd::Stop { json } => {
            let session = store
                .stop_timer(user_id)
                .await
                .map_err(|e| eyre::eyre!("stop: {e}"))?;
            if json {
                crate::json_out::print_json(&crate::json_out::session_json(&session))?;
            } else {
                let elapsed = session
                    .end_time
                    .unwrap_or_else(chrono::Utc::now)
                    .signed_duration_since(session.start_time);
                println!("Stopped {}", session.id);
                println!("  description: {}", session.description);
                println!("  elapsed:     {}", fmt_duration(elapsed));
                if session.billable {
                    println!(
                        "  billed:      {} {} (rate: {} {}/h)",
                        fmt_money(billed_cents(&session, elapsed)),
                        session.currency,
                        fmt_money(session.rate_cents),
                        session.currency,
                    );
                }
            }
        }
        TimerCmd::Active { json } => {
            match store
                .active_timer(user_id)
                .await
                .map_err(|e| eyre::eyre!("{e}"))?
            {
                Some(s) => {
                    if json {
                        // Joined titles are best-effort: vox being
                        // down shouldn't break `active --json` —
                        // the entity + derived seconds still print.
                        let task_title = if s.task_note_path.is_empty() {
                            None
                        } else {
                            match establish_for_url::<task::TaskServiceClient>(&vox_url).await {
                                Ok(tc) => tc
                                    .get_by_path(s.task_note_path.clone())
                                    .await
                                    .ok()
                                    .map(|t| t.title),
                                Err(_) => None,
                            }
                        };
                        let project_title = match s.project_id {
                            None => None,
                            Some(pid) => {
                                match establish_for_url::<project::ProjectServiceClient>(&vox_url)
                                    .await
                                {
                                    Ok(pc) => pc.get(pid).await.ok().map(|p| p.title),
                                    Err(_) => None,
                                }
                            }
                        };
                        crate::json_out::print_json(&crate::json_out::session_json_joined(
                            &s,
                            task_title,
                            project_title,
                        ))?;
                    } else {
                        let elapsed = chrono::Utc::now().signed_duration_since(s.start_time);
                        println!("Running for {} ({})", fmt_duration(elapsed), s.id);
                        if !s.description.is_empty() {
                            println!("  description: {}", s.description);
                        }
                        if !s.project_path.is_empty() {
                            println!("  project:     {}", s.project_path);
                        }
                        if !s.task_note_path.is_empty() {
                            println!("  task:        {}", s.task_note_path);
                        }
                    }
                }
                None => {
                    if json {
                        println!("null");
                    } else {
                        println!("No active timer.");
                    }
                }
            }
        }
        TimerCmd::Switch {
            description,
            task,
            project,
            task_note,
            tags,
            json,
        } => {
            let task_info = resolve_task_flag(task).await?;
            let (mut project_id, resolved_path) = resolve_project_flag(project).await?;
            if project_id.is_none() {
                project_id = task_info.as_ref().and_then(|t| t.project_id);
            }
            let description = description
                .or_else(|| task_info.as_ref().map(|t| t.title.clone()))
                .unwrap_or_default();
            let task_note = if task_note.is_empty() {
                task_info
                    .as_ref()
                    .map(|t| t.path.clone())
                    .unwrap_or_default()
            } else {
                task_note
            };
            let project_path =
                resolved_path.unwrap_or_else(|| project_path_for(&vault_root, project_id));
            let (closed, started) = store
                .switch_timer(StartTimerRequest {
                    user_id,
                    org_id,
                    project_id,
                    project_path,
                    task_note_path: task_note,
                    description,
                })
                .await
                .map_err(|e| eyre::eyre!("switch: {e}"))?;
            if !tags.is_empty() {
                store
                    .attach_tags(started.id, org_id, tags.clone())
                    .await
                    .map_err(|e| eyre::eyre!("attach tags: {e:?}"))?;
            }
            if json {
                crate::json_out::print_json(&serde_json::json!({
                    "stopped": closed.as_ref().map(crate::json_out::session_json),
                    "started": crate::json_out::session_json(&started),
                }))?;
            } else {
                if let Some(prev) = closed {
                    let elapsed = prev
                        .end_time
                        .unwrap_or_else(chrono::Utc::now)
                        .signed_duration_since(prev.start_time);
                    println!("Stopped {} after {}", prev.id, fmt_duration(elapsed));
                }
                println!("Started {} at {}", started.id, started.start_time);
                if !tags.is_empty() {
                    println!("  tags: {}", tags.join(", "));
                }
            }
        }
        TimerCmd::Log {
            description,
            from,
            to,
            task,
            project,
            task_note,
            billable,
            tags,
            json,
        } => {
            let task_info = resolve_task_flag(task).await?;
            let (mut project_id, resolved_path) = resolve_project_flag(project).await?;
            if project_id.is_none() {
                project_id = task_info.as_ref().and_then(|t| t.project_id);
            }
            let description = description
                .or_else(|| task_info.as_ref().map(|t| t.title.clone()))
                .unwrap_or_default();
            let task_note = if task_note.is_empty() {
                task_info
                    .as_ref()
                    .map(|t| t.path.clone())
                    .unwrap_or_default()
            } else {
                task_note
            };
            let project_path =
                resolved_path.unwrap_or_else(|| project_path_for(&vault_root, project_id));
            let session = store
                .log_session(LogSessionRequest {
                    user_id,
                    org_id,
                    project_id,
                    project_path,
                    task_note_path: task_note,
                    description,
                    start_time: from,
                    end_time: to,
                    billable_override: billable,
                })
                .await
                .map_err(|e| eyre::eyre!("log: {e}"))?;
            if !tags.is_empty() {
                store
                    .attach_tags(session.id, org_id, tags.clone())
                    .await
                    .map_err(|e| eyre::eyre!("attach tags: {e:?}"))?;
            }
            if json {
                crate::json_out::print_json(&crate::json_out::session_json(&session))?;
            } else {
                println!("Logged {} ({})", session.id, fmt_duration(to - from));
            }
        }
        TimerCmd::SetRate {
            user_id,
            cents,
            currency,
            json,
        } => {
            store
                .set_org_member_rate(org_id, user_id, cents, currency.clone())
                .await
                .map_err(|e| eyre::eyre!("set rate: {e}"))?;
            if json {
                crate::json_out::print_json(&serde_json::json!({
                    "org_id": org_id,
                    "user_id": user_id,
                    "hourly_cents": cents,
                    "currency": currency,
                }))?;
            } else {
                println!(
                    "Set org rate for {user_id}: {} {currency}/hr",
                    fmt_money(cents)
                );
            }
        }
        TimerCmd::Edit {
            id,
            description,
            from,
            to,
            project,
            user_id: edit_user,
            billable,
            task_note,
            json,
        } => {
            let (project_id, resolved_path) = resolve_project_flag(project).await?;
            // Reassigning the project also refreshes the cached
            // path (resolver-known path first, vault scan second).
            let project_path = project_id.map(|pid| {
                resolved_path.unwrap_or_else(|| project_path_for(&vault_root, Some(pid)))
            });
            let session = store
                .update_session(timer_proto::service::UpdateSessionRequest {
                    id,
                    user_id: edit_user,
                    project_id,
                    project_path,
                    task_note_path: task_note,
                    description,
                    start_time: from,
                    end_time: to,
                    billable,
                    preserve_rate: false,
                })
                .await
                .map_err(|e| eyre::eyre!("edit: {e}"))?;
            if json {
                crate::json_out::print_json(&crate::json_out::session_json(&session))?;
            } else {
                println!(
                    "Updated {} — \"{}\" [{}] {}/hr",
                    session.id,
                    session.description,
                    if session.billable {
                        "billable"
                    } else {
                        "non-billable"
                    },
                    fmt_money(session.rate_cents),
                );
            }
        }
        TimerCmd::Delete { id, json } => {
            store
                .delete_session(id)
                .await
                .map_err(|e| eyre::eyre!("delete: {e}"))?;
            if json {
                crate::json_out::print_json(&serde_json::json!({ "deleted": id }))?;
            } else {
                println!("Deleted {id}");
            }
        }
        TimerCmd::List {
            project,
            user,
            since,
            until,
            open,
            billable,
            json,
        } => {
            let (project_id, _) = resolve_project_flag(project).await?;
            // No default user filter: sessions land in this
            // DB from several surfaces (CLI, web UI) whose
            // identity derivations have drifted, and a
            // silent owner filter made `list` undercount vs
            // the finance rollup (which has always been
            // org-wide).
            let filter = timer_proto::WorkSessionFilter {
                user_id: user,
                project_id,
                since: Some(
                    since.unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::days(7)),
                ),
                until,
                billable,
                open,
            };
            let rows = store
                .list_sessions(filter.clone())
                .await
                .map_err(|e| eyre::eyre!("list: {e}"))?;
            if json {
                let out: Vec<serde_json::Value> =
                    rows.iter().map(crate::json_out::session_json).collect();
                crate::json_out::print_json(&out)?;
                return Ok(());
            }
            if rows.is_empty() {
                println!("(no sessions)");
            }
            for s in rows {
                let end = s
                    .end_time
                    .map_or_else(|| "open".to_string(), |t| t.to_rfc3339());
                let elapsed = s
                    .end_time
                    .unwrap_or_else(chrono::Utc::now)
                    .signed_duration_since(s.start_time);
                println!(
                    "{}  {:>8}  {}  {} {}",
                    s.start_time.format("%Y-%m-%d %H:%M"),
                    fmt_duration(elapsed),
                    if s.billable { "billable" } else { "        " },
                    s.description,
                    if end == "open" {
                        "[OPEN]".to_string()
                    } else {
                        String::new()
                    },
                );
            }
        }
        TimerCmd::Users { json } => {
            // All sessions in scope; aggregate per user_id.
            let rows = store
                .list_sessions(timer_proto::WorkSessionFilter::default())
                .await
                .map_err(|e| eyre::eyre!("list: {e}"))?;
            let mut agg: std::collections::BTreeMap<uuid::Uuid, (usize, i64, i64)> =
                std::collections::BTreeMap::new();
            for s in &rows {
                let e = agg.entry(s.user_id).or_default();
                e.0 += 1;
                let secs = s
                    .end_time
                    .unwrap_or(s.start_time)
                    .signed_duration_since(s.start_time)
                    .num_seconds()
                    .max(0);
                e.1 += secs;
                e.2 +=
                    i64::try_from(i128::from(secs) * i128::from(s.rate_cents) / 3600).unwrap_or(0);
            }
            // Resolve names from the LOCAL org's auth.sqlite — same
            // lookup the invoice path uses. Best-effort presentation
            // join: with no co-resident org dir (remote-only
            // session) names simply come back unresolved.
            let names = {
                use architect_auth::db::{AuthUserColumn, AuthUserEntity};
                use sea_orm::{ColumnTrait, Database, EntityTrait, QueryFilter};
                let mut map: std::collections::HashMap<uuid::Uuid, String> =
                    std::collections::HashMap::new();
                let auth_path = local.as_ref().map(|(o, _)| o.auth_db());
                if let Some(auth_path) = auth_path.filter(|p| p.exists()) {
                    let url = format!("sqlite://{}?mode=ro", auth_path.display());
                    if let Ok(db) = Database::connect(&url).await {
                        let ids: Vec<uuid::Uuid> = agg.keys().copied().collect();
                        if let Ok(users) = AuthUserEntity::find()
                            .filter(AuthUserColumn::Id.is_in(ids))
                            .all(&db)
                            .await
                        {
                            for u in users {
                                let lbl = u
                                    .name
                                    .filter(|s| !s.is_empty())
                                    .or(u.email)
                                    .unwrap_or_default();
                                map.insert(u.id, lbl);
                            }
                        }
                    }
                }
                map
            };
            if json {
                let out: Vec<serde_json::Value> = agg
                    .iter()
                    .map(|(uid, (count, secs, cents))| {
                        serde_json::json!({
                            "user_id": uid,
                            "sessions": count,
                            "seconds": secs,
                            "cents": cents,
                            "name": names.get(uid),
                        })
                    })
                    .collect();
                crate::json_out::print_json(&out)?;
                return Ok(());
            }
            if agg.is_empty() {
                println!("(no sessions)");
            }
            println!(
                "{:<38}  {:>6}  {:>9}  {:>10}  name",
                "user_id", "count", "hours", "cents"
            );
            for (uid, (count, secs, cents)) in agg {
                let hours = secs as f64 / 3600.0;
                let name = names
                    .get(&uid)
                    .cloned()
                    .unwrap_or_else(|| "(not in auth_users)".into());
                println!("{uid:<38}  {count:>6}  {hours:>9.2}  {cents:>10}  {name}");
            }
        }
        TimerCmd::ReassignUser {
            from,
            to,
            project,
            since,
            until,
            description_contains,
            rerate,
            dry_run,
            json,
        } => {
            let (project_id, _) = resolve_project_flag(project).await?;
            let filter = timer_proto::WorkSessionFilter {
                user_id: Some(from),
                project_id,
                since,
                until,
                billable: None,
                open: None,
            };
            let rows = store
                .list_sessions(filter.clone())
                .await
                .map_err(|e| eyre::eyre!("list: {e}"))?;
            let needle = description_contains.map(|s| s.to_lowercase());
            let matched: Vec<_> = rows
                .into_iter()
                .filter(|s| {
                    needle
                        .as_ref()
                        .is_none_or(|n| s.description.to_lowercase().contains(n.as_str()))
                })
                .collect();
            if !json {
                println!(
                    "{} session(s) match (from={from}, to={to}, rerate={rerate}, dry_run={dry_run})",
                    matched.len()
                );
                for s in &matched {
                    println!(
                        "  {}  {}  {}",
                        s.start_time.format("%Y-%m-%d %H:%M"),
                        s.id,
                        s.description
                    );
                }
            }
            if dry_run || matched.is_empty() {
                if json {
                    crate::json_out::print_json(&serde_json::json!({
                        "from": from,
                        "to": to,
                        "rerate": rerate,
                        "dry_run": dry_run,
                        "matched": matched.len(),
                        "updated": 0,
                        "session_ids": matched.iter().map(|s| s.id).collect::<Vec<_>>(),
                    }))?;
                }
                return Ok(());
            }
            let mut updated = 0_usize;
            for s in &matched {
                if rerate {
                    // Goes through `update_session`, which
                    // re-snapshots `rate_cents` + `currency`
                    // from the cascade for the new user.
                    store
                        .update_session(timer_proto::service::UpdateSessionRequest {
                            id: s.id,
                            user_id: Some(to),
                            ..Default::default()
                        })
                        .await
                        .map_err(|e| eyre::eyre!("reassign {}: {e}", s.id))?;
                } else {
                    // Preserve the historical rate snapshot — only
                    // swap user_id. `preserve_rate` tells the server
                    // to skip the cascade re-resolution.
                    store
                        .update_session(timer_proto::service::UpdateSessionRequest {
                            id: s.id,
                            user_id: Some(to),
                            preserve_rate: true,
                            ..Default::default()
                        })
                        .await
                        .map_err(|e| eyre::eyre!("reassign {}: {e}", s.id))?;
                }
                updated += 1;
            }
            if json {
                crate::json_out::print_json(&serde_json::json!({
                    "from": from,
                    "to": to,
                    "rerate": rerate,
                    "dry_run": false,
                    "matched": matched.len(),
                    "updated": updated,
                    "session_ids": matched.iter().map(|s| s.id).collect::<Vec<_>>(),
                }))?;
            } else {
                println!("Updated {updated} session(s).");
            }
        }
        TimerCmd::Resolve { project, json } => {
            let (project_id, _) = resolve_project_flag(project).await?;
            let resolved = store
                .resolve_rate(user_id, project_id)
                .await
                .map_err(|e| eyre::eyre!("resolve: {e}"))?;
            if json {
                crate::json_out::print_json(&resolved)?;
            } else {
                println!(
                    "rate: {} {}/h  source: {:?}",
                    fmt_money(resolved.hourly_cents),
                    if resolved.currency.is_empty() {
                        "(none)".to_string()
                    } else {
                        resolved.currency
                    },
                    resolved.source,
                );
            }
        }
        TimerCmd::Tag(sub) => match sub {
            TimerTagCmd::List { json } => {
                let rows = store
                    .list_tags(org_id)
                    .await
                    .map_err(|e| eyre::eyre!("list tags: {e}"))?;
                if json {
                    let out: Vec<serde_json::Value> =
                        rows.iter().map(crate::json_out::tag_json).collect();
                    crate::json_out::print_json(&out)?;
                    return Ok(());
                }
                if rows.is_empty() {
                    println!("(no tags)");
                }
                for t in rows {
                    let color = if t.color.is_empty() {
                        "(auto)"
                    } else {
                        t.color.as_str()
                    };
                    println!("{}  {}  {}", t.id, t.name, color);
                }
            }
            TimerTagCmd::Create { name, color, json } => {
                let tag = store
                    .create_tag(org_id, name, color)
                    .await
                    .map_err(|e| eyre::eyre!("create tag: {e}"))?;
                if json {
                    crate::json_out::print_json(&crate::json_out::tag_json(&tag))?;
                } else {
                    println!("{}  {}", tag.id, tag.name);
                }
            }
            TimerTagCmd::Rm { name, json } => {
                let tag = store.delete_tag(org_id, name.clone()).await.map_err(|e| {
                    if matches!(&e, vox::VoxError::User(inner)
                        if matches!(**inner, timer_proto::TimerError::TagNotFound(_)))
                    {
                        eyre::eyre!("no such tag: {name}")
                    } else {
                        eyre::eyre!("delete tag: {e}")
                    }
                })?;
                if json {
                    crate::json_out::print_json(&serde_json::json!({
                        "deleted": crate::json_out::tag_json(&tag),
                    }))?;
                } else {
                    println!("Deleted tag {} ({})", tag.name, tag.id);
                }
            }
            TimerTagCmd::Attach {
                session_id,
                tags,
                json,
            } => {
                store
                    .attach_tags(session_id, org_id, tags.clone())
                    .await
                    .map_err(|e| eyre::eyre!("attach tags: {e}"))?;
                if json {
                    crate::json_out::print_json(&serde_json::json!({
                        "session_id": session_id,
                        "attached": tags,
                    }))?;
                } else {
                    println!("Attached {} to {session_id}", tags.join(", "));
                }
            }
            TimerTagCmd::Detach {
                session_id,
                tags,
                all,
                json,
            } => {
                if all {
                    store
                        .detach_tags(session_id, org_id, Vec::new(), true)
                        .await
                        .map_err(|e| eyre::eyre!("detach all: {e}"))?;
                    if json {
                        crate::json_out::print_json(&serde_json::json!({
                            "session_id": session_id,
                            "detached": "all",
                        }))?;
                    } else {
                        println!("Detached all tags from {session_id}");
                    }
                } else if tags.is_empty() {
                    return Err(eyre::eyre!("pass --tag <name> or --all"));
                } else {
                    let matched = store
                        .detach_tags(session_id, org_id, tags.clone(), false)
                        .await
                        .map_err(|e| eyre::eyre!("detach: {e}"))?;
                    if matched == 0 {
                        return Err(eyre::eyre!("no matching tags"));
                    }
                    if json {
                        crate::json_out::print_json(&serde_json::json!({
                            "session_id": session_id,
                            "detached": tags,
                        }))?;
                    } else {
                        println!("Detached {} from {session_id}", tags.join(", "));
                    }
                }
            }
        },
    }
    Ok(())
}

/// Fixture compat: `TASK_TIMER_DB` used to point the CLI's direct
/// sqlite open at a scratch db. The CLI no longer opens the db —
/// forward the override to the embedded server's own knob
/// (`TASK_SERVER_TIMER_URL`) so scratch-db flows keep working when
/// the backend boots in-process. Against a running (remote) server
/// it has, and had, no effect on the server's storage.
pub(crate) fn forward_timer_db_override() {
    if let Ok(url) = std::env::var("TASK_TIMER_DB") {
        if !url.is_empty() && std::env::var_os("TASK_SERVER_TIMER_URL").is_none() {
            // SAFETY: single process-config write, performed before
            // the embedded backend boots (first `establish`) and
            // before anything reads the forwarded variable.
            unsafe { std::env::set_var("TASK_SERVER_TIMER_URL", url) };
        }
    }
}

/// Resolve the project markdown path from its frontmatter
/// id by scanning `Projects/**/*.md` recursively (projects
/// conventionally live in their own folder, e.g.
/// `Projects/<Name>/<Name>.md` — a flat scan misses them and
/// every session then stores an empty `project_path`).
/// `None` project_id → empty.
pub(crate) fn project_path_for(
    vault_root: &std::path::Path,
    project_id: Option<uuid::Uuid>,
) -> String {
    let Some(pid) = project_id else {
        return String::new();
    };
    let mut dirs = vec![vault_root.join("Projects")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let rel = path
                .strip_prefix(vault_root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let basename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let Ok(p) = project::parse_str(&rel, basename, &raw) else {
                continue;
            };
            if p.id == pid {
                return rel;
            }
        }
    }
    String::new()
}

fn fmt_duration(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

fn fmt_money(cents: i64) -> String {
    let neg = cents < 0;
    let abs = cents.unsigned_abs();
    let dollars = abs / 100;
    let frac = abs % 100;
    format!("{}{dollars}.{frac:02}", if neg { "-" } else { "" })
}

fn billed_cents(s: &timer_proto::WorkSession, elapsed: chrono::Duration) -> i64 {
    let secs = elapsed.num_seconds().max(0);
    // rate_cents is per hour; convert seconds → hours via i128 to dodge overflow.
    let cents = (secs as i128) * (s.rate_cents as i128) / 3600_i128;
    cents.try_into().unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CLI, the `/timer` page and the watch bridge all resolve the
    /// same user, or a session started on one surface is invisible on
    /// the others. They now share one definition; this asserts the CLI
    /// alias still points at it.
    #[test]
    fn timer_owner_id_matches_the_shared_definition() {
        let org = uuid::Uuid::new_v4();
        assert_eq!(timer_owner_id(org), timer_proto::local_owner_id(org));
    }

    #[test]
    fn fmt_duration_drops_empty_leading_units() {
        use chrono::Duration;
        assert_eq!(fmt_duration(Duration::seconds(0)), "0s");
        assert_eq!(fmt_duration(Duration::seconds(9)), "9s");
        assert_eq!(fmt_duration(Duration::seconds(65)), "1m05s");
        assert_eq!(fmt_duration(Duration::seconds(3600)), "1h00m00s");
        assert_eq!(fmt_duration(Duration::seconds(3661)), "1h01m01s");
        // A clock that ran backwards clamps rather than rendering
        // a negative elapsed time.
        assert_eq!(fmt_duration(Duration::seconds(-30)), "0s");
    }

    #[test]
    fn fmt_money_keeps_the_sign_and_pads_cents() {
        assert_eq!(fmt_money(0), "0.00");
        assert_eq!(fmt_money(5), "0.05");
        assert_eq!(fmt_money(100), "1.00");
        assert_eq!(fmt_money(123_456), "1234.56");
        // A credit must not lose its sign, and a sub-dollar credit
        // must not render as a positive amount — the failure mode
        // that `unsigned_abs` + an explicit sign prefix exists to
        // prevent (`-1/100 == 0` in integer division).
        assert_eq!(fmt_money(-5), "-0.05");
        assert_eq!(fmt_money(-100), "-1.00");
    }

    /// A session carrying nothing but the hourly rate — the only
    /// field `billed_cents` reads.
    fn session_at(rate_cents: i64) -> timer_proto::WorkSession {
        let now = chrono::Utc::now();
        timer_proto::WorkSession {
            id: uuid::Uuid::nil(),
            org_id: uuid::Uuid::nil(),
            user_id: uuid::Uuid::nil(),
            project_id: None,
            project_path: String::new(),
            description: String::new(),
            start_time: now,
            end_time: None,
            billable: true,
            rate_cents,
            currency: "USD".into(),
            task_note_path: String::new(),
            invoice_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn billed_cents_prorates_by_the_second() {
        use chrono::Duration;
        let s = session_at(10_000); // $100/hr
        assert_eq!(billed_cents(&s, Duration::hours(1)), 10_000);
        assert_eq!(billed_cents(&s, Duration::minutes(30)), 5_000);
        assert_eq!(billed_cents(&s, Duration::minutes(6)), 1_000);
        assert_eq!(billed_cents(&s, Duration::seconds(0)), 0);
        // Partial cents truncate toward zero rather than rounding up,
        // so we never over-bill.
        assert_eq!(billed_cents(&s, Duration::seconds(1)), 2);
        // Negative elapsed (clock skew) bills nothing.
        assert_eq!(billed_cents(&s, Duration::seconds(-600)), 0);
    }

    #[test]
    fn billed_cents_does_not_overflow_on_absurd_rates() {
        use chrono::Duration;
        let s = session_at(i64::MAX);
        // The i128 intermediate is what keeps this from wrapping into
        // a negative charge.
        assert!(billed_cents(&s, Duration::hours(24)) > 0);
    }
}
