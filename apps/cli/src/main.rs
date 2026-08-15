//! `task` CLI — argument parsing, dispatch, and client plumbing.
//!
//! Every command lives in its own module (`project`, `wiki`,
//! `timer`, …); this file owns [`Cli`] / [`Commands`], the dispatch
//! match, and the vox client helpers the modules share. The module
//! for `task task …` is `task_cmd` — `mod task` at the crate root
//! would shadow the `task` crate itself.
//!
//! Commands reach their data through vox against `/org/<slug>/vox`
//! — a remote server when the session points at one, or the
//! in-process embedded backend (`TASK_EMBED=1`) — so permissions,
//! streams, and the per-org plugin gate apply uniformly. The
//! remaining direct-to-disk paths are deliberate, each documented at
//! its site: machine-local dev-loop state (`label`, `code`, `cycle`,
//! `setup` webhook config), the local LLM-runner wiki verbs
//! (`agent_wiki::bridge` drives a co-resident `WikiLive`), explicit
//! local-path escape hatches (`wiki --vault <existing dir>`,
//! `vault --fs`), and true presentation-only work (invoice PDF
//! shell-out). Direct-to-disk resolution goes through
//! `org_proto::DataRoot::from_env()` and only works against an org
//! hosted on this machine.
//!
//! Server endpoint resolution (first match wins):
//! 1. `--server <url>` flag.
//! 2. `TASK_VOX_URL` env var (loaded from `.env` if present).
//! 3. `ws://127.0.0.1:9090/vox` default.

mod admin;
mod agent;
mod api;
mod auth;
#[cfg(feature = "plugin-fitness")]
mod body;
mod brief;
mod bulk_journal;
mod code;
#[cfg(feature = "plugin-fasttrackstudio")]
mod collection;
mod cycle;
#[cfg(feature = "plugin-email")]
mod email;
mod errors;
#[cfg(feature = "plugin-fitness")]
mod exercise;
#[cfg(feature = "plugin-finance")]
mod finance;
mod forge;
mod goal;
mod inbox;
#[cfg(feature = "plugin-fitness")]
mod intake;
mod issue;
mod json_out;
mod label;
// pantry/intake reuse location's client helpers, so the module
// compiles whenever any of the three owners is in.
#[cfg(any(
    feature = "plugin-home",
    feature = "plugin-mealplan",
    feature = "plugin-fitness"
))]
mod location;
#[cfg(feature = "plugin-mealplan")]
mod meal;
#[cfg(feature = "plugin-mealplan")]
mod mealprep;
mod media;
mod milestone;
mod mount;
mod org;
mod org_ctx;
// `intake` (fitness) reuses pantry's client helpers, so the module
// compiles under either plugin.
mod files;
#[cfg(any(feature = "plugin-mealplan", feature = "plugin-fitness"))]
mod pantry;
mod plan;
mod project;
#[cfg(feature = "plugin-mealplan")]
mod recipe;
#[cfg(feature = "plugin-mealplan")]
mod recipe_import;
mod runner;
mod session_store;
mod setup;
mod shared;
mod skills;
mod task_cmd;
mod threads;
mod timer;
mod vault;
#[cfg(feature = "plugin-wiki")]
mod wiki;
#[cfg(feature = "plugin-fitness")]
mod workout;
mod workstream;

use crate::admin::{AdminCmd, run_admin};
#[cfg(feature = "plugin-agent")]
use crate::agent::{AgentCmd, run_agent};
use crate::api::{ApiArgs, run_api};
use crate::auth::{AuthCmd, run_auth};
#[cfg(feature = "plugin-fitness")]
use crate::body::{BodyCmd, run_body};
use crate::code::{CodeCmd, run_code};
use crate::cycle::{CycleCmd, run_cycle};
#[cfg(feature = "plugin-email")]
use crate::email::{EmailCmd, run_email};
#[cfg(feature = "plugin-fitness")]
use crate::exercise::{ExerciseCmd, run_exercise};
use crate::files::{FilesCmd, run_files};
#[cfg(feature = "plugin-finance")]
use crate::finance::{FinanceCmd, run_finance};
use crate::goal::{GoalCmd, run_goal};
use crate::inbox::{InboxCmd, run_inbox};
#[cfg(feature = "plugin-fitness")]
use crate::intake::{IntakeCmd, run_intake};
use crate::issue::{IssueCmd, run_issue};
use crate::label::{LabelCmd, run_label};
#[cfg(feature = "plugin-home")]
use crate::location::{LocationCmd, run_location};
#[cfg(feature = "plugin-mealplan")]
use crate::meal::{MealCmd, run_meal};
use crate::milestone::{MilestoneCmd, run_milestone};
use crate::mount::{MountCmd, run_mount};
use crate::org::{OrgCmd, run_org};
#[cfg(feature = "plugin-mealplan")]
use crate::pantry::{PantryCmd, run_pantry};
use crate::project::{ProjectCmd, run_project};
#[cfg(feature = "plugin-mealplan")]
use crate::recipe::{RecipeCmd, run_recipe};
use crate::setup::{SetupCmd, run_setup};
use crate::task_cmd::{TaskCmd, run_task};
use crate::threads::{ThreadsCmd, run_threads};
use crate::timer::{TimerCmd, run_timer};
use crate::vault::{VaultCmd, run_vault, run_vault_sync};
#[cfg(feature = "plugin-wiki")]
use crate::wiki::{WikiCmd, run_wiki};
#[cfg(feature = "plugin-fitness")]
use crate::workout::{WorkoutCmd, run_workout};
use clap::{Parser, Subcommand};
use shared::RemoteVoxConfig;

#[derive(Parser)]
#[command(name = "task", about = "Task management CLI", version)]
struct Cli {
    /// Vox WebSocket URL (e.g. <ws://127.0.0.1:9090/vox>). Falls back
    /// to `TASK_VOX_URL` (loaded from .env) then to the localhost
    /// default.
    #[arg(long, env = "TASK_VOX_URL", global = true)]
    server: Option<String>,

    /// Architect Auth session token for remote vox.
    #[arg(long, env = "TASK_SESSION_TOKEN", global = true)]
    session_token: Option<String>,

    /// Organization id to route remote vox requests.
    #[arg(long, env = "TASK_ORGANIZATION_ID", global = true)]
    organization_id: Option<String>,

    /// Override the active org for this invocation only.
    /// Slug must match a dir under `<data_root>/orgs/`.
    /// Precedence: this flag > `session.json` active >
    /// single-org disambiguation > auto-bootstrap `default`.
    #[arg(long, global = true)]
    org: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Probe the configured vox endpoint.
    Doctor,
    /// The API reference, rendered from the live registry
    /// (`task_server::permits::mounts()`): every mounted service,
    /// its methods + arg names, the permit action/resource per
    /// method, stream vs rpc, and the schema stamp. `task api
    /// <service>` for one service; `--markdown` regenerates
    /// `apps/task/docs/api-reference.md`; `--json` mirrors
    /// `GET /org/{slug}/api`.
    Api(ApiArgs),
    /// Vault queries + edits. An existing local `<path>` (or
    /// `--fs`) works the directory on disk; otherwise the active
    /// org's vault is mirrored over vox and edits are pushed back.
    Vault {
        /// Force the direct-filesystem path: treat `<path>` as a
        /// plain on-disk vault, exactly as before the vox
        /// unification. Recovery / offline-inspection hatch —
        /// bypasses the org router (no permissions, no plugins,
        /// no CRDT presence).
        #[arg(long)]
        fs: bool,
        #[command(subcommand)]
        cmd: VaultCmd,
    },
    /// First-party task management. Tasks are markdown pages
    /// with TaskNotes-shape frontmatter (mirrors
    /// callumalpass/tasknotes). Files live at
    /// `<vault>/tasks/<slug>.md` by default.
    #[command(subcommand)]
    Task(TaskCmd),
    /// LLM-agent integration. Codex backend drives `chat`
    /// (one-shot) + `wiki ingest` (two-step `CoT` against a
    /// vault's `Wiki/raw/sources/`).
    #[cfg(feature = "plugin-agent")]
    #[command(subcommand)]
    Agent(AgentCmd),
    #[cfg(not(feature = "plugin-agent"))]
    #[command(hide = true)]
    Agent(NotCompiled),
    /// Runners — machines that execute agent work. Register this
    /// box, list the registry, heartbeat, deregister.
    #[cfg(feature = "plugin-agent")]
    #[command(subcommand)]
    Runner(runner::RunnerCmd),
    #[cfg(not(feature = "plugin-agent"))]
    #[command(hide = true)]
    Runner(NotCompiled),
    /// The agent-lane skills — install them into a working copy.
    #[command(subcommand)]
    Skills(skills::SkillsCmd),
    /// Mail accounts for this org (add / list / remove / test).
    #[cfg(feature = "plugin-email")]
    #[command(subcommand)]
    Email(EmailCmd),
    #[cfg(not(feature = "plugin-email"))]
    #[command(hide = true)]
    Email(NotCompiled),
    /// `Wiki/` operations — currently the LLM-driven
    /// ingest pipeline. Sister surface to `agent`; the
    /// command itself routes through `agent-wiki::bridge`.
    #[cfg(feature = "plugin-wiki")]
    #[command(subcommand)]
    Wiki(WikiCmd),
    #[cfg(not(feature = "plugin-wiki"))]
    #[command(hide = true)]
    Wiki(NotCompiled),
    /// Billable time tracking. Local SQLite backed (no
    /// server needed); same `timer::Store` the server
    /// mounts. Project lookup reads `Projects/*.md` for the
    /// rate cascade.
    #[command(subcommand)]
    Timer(TimerCmd),
    /// Files RPC surface (issue #259/#261, ADR 0001): turn a folder
    /// into a File Root, browse it, read a file's version chain,
    /// checkpoint on demand, and curate Named / Project Versions.
    #[command(subcommand)]
    Files(FilesCmd),
    /// Finance — reports + invoice generation from billable
    /// sessions, PDF rendering via fulgur.
    #[cfg(feature = "plugin-finance")]
    #[command(subcommand)]
    Finance(FinanceCmd),
    #[cfg(not(feature = "plugin-finance"))]
    #[command(hide = true)]
    Finance(NotCompiled),
    /// Architect-auth flows — local sign-in, session
    /// management, org selection. Writes the persistent
    /// session file consumed by `timer` / `finance`.
    #[command(subcommand)]
    Auth(AuthCmd),
    /// Federated org-root layout — scaffold, list, and (later)
    /// export/claim on-disk org directories under the data
    /// root. Distinct from `auth org`, which is about
    /// membership in architect-auth orgs. See
    /// `plans/federated-task-platform.md` Phase 1.
    #[command(subcommand)]
    Org(OrgCmd),
    /// Server administration — server-native git snapshots of the
    /// data root (snapshot / log / branch / restore). Talks to
    /// `<server>/server/vox` (`SnapshotService`), like `task org
    /// create`.
    #[command(subcommand)]
    Admin(AdminCmd),
    /// Per-machine project content mounts — register where each
    /// project's bytes live on this box. Reads/writes
    /// `$XDG_CONFIG_HOME/task/mounts.toml` (override via
    /// `TASK_MOUNTS_TOML`). See
    /// `plans/federated-task-platform.md` Phase 2.
    #[command(subcommand)]
    Mount(MountCmd),
    /// Cyclic life-calendar — show / list the 4-week cycles
    /// that anchor long-term planning. See
    /// `plans/cyclic-life-calendar.md`.
    #[command(subcommand)]
    Cycle(CycleCmd),
    /// Projects served by the active org. Talks to
    /// `/org/<slug>/vox` via the architect-generated
    /// `ProjectServiceClient`.
    #[command(subcommand)]
    Project(ProjectCmd),
    /// Linear-style issue surface over TaskInfo's
    /// `WorkflowAttrs` (workspace / cycle / project / estimate /
    /// assignees / blockers). The data still lives in TaskInfo —
    /// `task issue *` is just the workflow-aware view of it.
    /// `task work *` is an alias for ergonomic typing.
    #[command(subcommand, alias = "work")]
    Issue(IssueCmd),
    /// One-shot integration setup — connect a forge repo to a
    /// workspace: generate the webhook secret, register the
    /// webhook on the forge, record the repo binding.
    #[command(subcommand)]
    Setup(SetupCmd),
    /// The agent dev loop — git operations wrapped around the
    /// issue lifecycle. `start` branches + claims, `commit`
    /// stamps attribution trailers, `push` opens a linked PR,
    /// `finish` merges + closes. Infers the forge repo from the
    /// git remote, so it works on third-party repos too.
    #[command(subcommand)]
    Code(CodeCmd),
    /// Org-scoped labels — colored tags for triage + filtering.
    /// Persisted per-org as `labels.json`.
    #[command(subcommand)]
    Label(LabelCmd),
    /// Goals (with cycle anchoring) served by the active
    /// org. Talks to `/org/<slug>/vox` via the architect-
    /// generated `GoalServiceClient`.
    #[command(subcommand)]
    Goal(GoalCmd),
    /// Project milestones — GitHub-Projects-style checkpoints.
    /// Tasks roll up via `milestoneId`; milestones can ladder
    /// up to life-goals via `goalId`. Designed to sync 1:1
    /// with Forgejo / GitHub milestones in the future.
    #[command(subcommand)]
    Milestone(MilestoneCmd),
    /// Workstreams — the parent-with-swarm construct (lead +
    /// members + status + dates) that replaces the 'epic' tag.
    /// Tasks attach via `workflow.workstream`; progress is a
    /// derived rollup (`task workstream rollup`).
    #[command(subcommand)]
    Workstream(workstream::WorkstreamCmd),
    /// Ordered collections — libraries, setlists, shows, playlists.
    /// All the same primitive: an ordered list of `NodeRef` items
    /// over `CollectionService`. Create, populate, reorder, and
    /// inspect headlessly (the entry point for library/setlist
    /// seeding).
    #[cfg(feature = "plugin-fasttrackstudio")]
    #[command(subcommand)]
    Collection(collection::CollectionCmd),
    #[cfg(not(feature = "plugin-fasttrackstudio"))]
    #[command(hide = true)]
    Collection(NotCompiled),
    /// Songs — build a durable Song folder (via the `song` crate)
    /// and add it to a target collection as a `song:` node.
    #[cfg(feature = "plugin-fasttrackstudio")]
    #[command(subcommand)]
    Song(collection::SongCmd),
    #[cfg(not(feature = "plugin-fasttrackstudio"))]
    #[command(hide = true)]
    Song(NotCompiled),
    /// Media — content-addressed blobs streamed over vox (stat /
    /// get / verify-song). The no-browser audio-streaming E2E.
    #[command(subcommand)]
    Media(media::MediaCmd),
    /// Physical places — studios, rooms, venues, storage.
    /// Pantry + inventory reference these by id.
    #[cfg(feature = "plugin-home")]
    #[command(subcommand)]
    Location(LocationCmd),
    #[cfg(not(feature = "plugin-home"))]
    #[command(hide = true)]
    Location(NotCompiled),
    /// Inbox — capture fleeting notes and triage the daily queue.
    #[command(subcommand)]
    Inbox(InboxCmd),
    /// Threads — log conversations & topics on a task or project.
    #[command(subcommand)]
    Threads(ThreadsCmd),
    /// Cookbook recipes (cooklang `.cook` files under
    /// `Wiki/Cookbook/`).
    #[cfg(feature = "plugin-mealplan")]
    #[command(subcommand)]
    Recipe(RecipeCmd),
    #[cfg(not(feature = "plugin-mealplan"))]
    #[command(hide = true)]
    Recipe(NotCompiled),
    /// Scheduled meals + cooking lifecycle (planned →
    /// cooked → pantry deductions).
    #[cfg(feature = "plugin-mealplan")]
    #[command(subcommand)]
    Meal(MealCmd),
    #[cfg(not(feature = "plugin-mealplan"))]
    #[command(hide = true)]
    Meal(NotCompiled),
    /// Pantry — stocked food items, qty + unit tracking,
    /// barcode resolution.
    #[cfg(feature = "plugin-mealplan")]
    #[command(subcommand)]
    Pantry(PantryCmd),
    #[cfg(not(feature = "plugin-mealplan"))]
    #[command(hide = true)]
    Pantry(NotCompiled),
    /// Shopping lists — auto-populate from recipe shortages /
    /// low stock / expiry; mark-purchased restocks the pantry.
    #[cfg(feature = "plugin-mealplan")]
    #[command(subcommand)]
    Shopping(mealprep::ShoppingCmd),
    #[cfg(not(feature = "plugin-mealplan"))]
    #[command(hide = true)]
    Shopping(NotCompiled),
    /// Body metrics — weight / body-fat / measurements log.
    #[cfg(feature = "plugin-fitness")]
    #[command(subcommand)]
    Body(BodyCmd),
    #[cfg(not(feature = "plugin-fitness"))]
    #[command(hide = true)]
    Body(NotCompiled),
    /// Exercise library — movement definitions referenced
    /// by routines + sessions.
    #[cfg(feature = "plugin-fitness")]
    #[command(subcommand)]
    Exercise(ExerciseCmd),
    #[cfg(not(feature = "plugin-fitness"))]
    #[command(hide = true)]
    Exercise(NotCompiled),
    /// Workout routines + sessions.
    #[cfg(feature = "plugin-fitness")]
    #[command(subcommand)]
    Workout(WorkoutCmd),
    #[cfg(not(feature = "plugin-fitness"))]
    #[command(hide = true)]
    Workout(NotCompiled),
    /// Food intake log — daily calorie + macro tracking.
    #[cfg(feature = "plugin-fitness")]
    #[command(subcommand)]
    Intake(IntakeCmd),
    #[cfg(not(feature = "plugin-fitness"))]
    #[command(hide = true)]
    Intake(NotCompiled),
    /// Day-plan schedule surface — show / edit blocks, assign
    /// tasks, materialize from templates, plan-vs-actual diff.
    /// All logic in `plan.rs`.
    #[cfg(feature = "plugin-scheduling")]
    #[command(subcommand)]
    Plan(plan::PlanCmd),
    #[cfg(not(feature = "plugin-scheduling"))]
    #[command(hide = true)]
    Plan(NotCompiled),
    /// What should I be doing right now — current block + time
    /// remaining + next block (falls back to the next due task).
    #[cfg(feature = "plugin-scheduling")]
    Next(plan::NextArgs),
    #[cfg(not(feature = "plugin-scheduling"))]
    #[command(hide = true)]
    Next(NotCompiled),
    /// Morning digest — today's blocks + events, due/overdue +
    /// in-progress tasks, active timer, blocked agent tasks, open
    /// inbox, meals + bookings. All logic in `brief.rs`.
    Brief(brief::BriefArgs),
}

/// Placeholder arguments for a plugin command that is compiled OUT of
/// this build. Swallows whatever the user typed (so clap still parses
/// the invocation) and the dispatch arm fails with "not compiled into
/// this build" instead of clap's "unrecognized subcommand".
#[allow(dead_code)] // referenced only when at least one plugin is compiled out
#[derive(clap::Args)]
struct NotCompiled {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true, num_args = 0..)]
    _rest: Vec<String>,
}

/// The error a compiled-out plugin command fails with.
#[allow(dead_code)] // referenced only when at least one plugin is compiled out
fn not_compiled(plugin: &str) -> eyre::Report {
    eyre::eyre!(
        "the `{plugin}` plugin is not compiled into this build of task-cli \
         (rebuild with `--features plugin-{plugin}`)"
    )
}

/// Global `--org` / `--server` flags, captured once before dispatch.
/// Subcommands that still declare local duplicates shadow these; the
/// shared resolvers ([`resolve_active_org`], [`resolve_org_vox_url`],
/// `org_ctx::resolve_active`) fall back here when a handler passes
/// `None`, so `task --org foo <any subcommand>` works even where the
/// local flag was removed (issue / threads) or never existed.
static GLOBAL_ORG: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
static GLOBAL_SERVER: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

pub(crate) fn global_org() -> Option<String> {
    GLOBAL_ORG.get().cloned().flatten()
}

fn global_server() -> Option<String> {
    GLOBAL_SERVER.get().cloned().flatten()
}

#[tokio::main]
async fn main() {
    // wss:// (vox-websocket TLS) needs a process-level rustls
    // CryptoProvider; this binary unifies both `ring` and
    // `aws-lc-rs` in its graph, so rustls cannot infer one.
    // Install ring once, before anything can open a TLS socket.
    // Err just means a provider is already installed — fine.
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Best-effort .env load before clap reads env. Missing file is
    // not an error — we just fall through to the hard-coded default.
    let _ = dotenvy::dotenv();
    let cli = Cli::parse();
    GLOBAL_ORG.set(cli.org.clone()).ok();
    GLOBAL_SERVER.set(cli.server.clone()).ok();
    // Error boundary: render the taxonomy line + hint and exit with
    // the stable code (4 not-found / 5 conflict / 6 connection / 1).
    if let Err(report) = run(cli).await {
        errors::exit_with(&report);
    }
}

/// The proto/server skew guard half of `task doctor`: fetch the
/// server's `/.well-known/task-server.json`, compare its
/// `schema_stamps` (computed from the descriptors the *running*
/// binary mounts) against this CLI's own build (the CLI links
/// `task_server::schema_stamps()` directly, so both sides fold
/// the exact same descriptor list — no second list to drift).
///
/// A mismatch means the running task-server predates (or
/// postdates) a `*-proto` change relative to this CLI — the
/// state that otherwise surfaces as vox `structural mismatch` /
/// `InvalidPayload` / `Unknown method` errors with zero context.
/// Exits non-zero so dev scripts can gate on it.
async fn doctor_check_schema(ws_url: &str) -> eyre::Result<()> {
    let origin = http_origin(ws_url);
    let url = format!("{origin}/.well-known/task-server.json");

    let doc: serde_json::Value = match reqwest::get(&url).await {
        Ok(resp) => resp
            .json()
            .await
            .map_err(|e| eyre::eyre!("parse {url}: {e}"))?,
        Err(e) => {
            println!("Schema check: SKIPPED — could not fetch {url} ({e})");
            return Ok(());
        }
    };
    let Some(served) = doc.get("schema_stamps").and_then(|v| v.as_object()) else {
        println!(
            "Schema check: UNVERIFIED — the server exposes no `schema_stamps` \
             (it predates the skew guard). If you see `structural mismatch` / \
             `InvalidPayload` errors, rebuild + restart task-server."
        );
        return Ok(());
    };

    let local = task_server::schema_stamps();
    let mut stale: Vec<&str> = Vec::new();
    let mut unserved: Vec<&str> = Vec::new();
    for (name, stamp) in &local {
        match served.get(*name).and_then(|v| v.as_str()) {
            Some(s) if s == stamp => {}
            Some(_) => stale.push(name),
            None => unserved.push(name),
        }
    }

    if stale.is_empty() && unserved.is_empty() {
        println!(
            "Schema check: OK — {} service stamps match the running server",
            local.len()
        );
        return Ok(());
    }
    if !unserved.is_empty() {
        println!(
            "Schema check: {} service(s) not stamped by the server (added since \
             its build?): {}",
            unserved.len(),
            unserved.join(", ")
        );
    }
    if !stale.is_empty() {
        println!(
            "Schema check: STALE — stamp mismatch on: {}",
            stale.join(", ")
        );
        println!(
            "  The running task-server was built against different `*-proto` \
             shapes than this CLI."
        );
        println!(
            "  Fix: rebuild + restart it (`cargo run -p task-server`), or rebuild \
             this CLI if the server is newer."
        );
        return Err(eyre::eyre!(
            "proto/server schema skew on {} service(s)",
            stale.len()
        ));
    }
    Ok(())
}

/// ws(s)://host:port[/path] → http(s)://host:port — the HTTP origin of
/// the vox endpoint, where the server's plain-HTTP surfaces live.
fn http_origin(ws_url: &str) -> String {
    let http = ws_url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    let after_scheme = http.find("://").map_or(http.len(), |i| i + 3);
    let end = http[after_scheme..]
        .find('/')
        .map_or(http.len(), |i| after_scheme + i);
    http[..end].to_owned()
}

/// The API-surface half of `task doctor`: fetch `GET /org/{slug}/api`
/// (the registry the running server serializes — see `task api`),
/// report what is mounted (services vs `#[subscribe]` streams), and
/// warn per service whose schema stamp differs from this CLI's build —
/// the same stamp logic as the schema check, surfaced per-service
/// instead of pass/fail. Warn-only: `doctor_check_schema` already
/// gates hard on skew.
async fn doctor_check_api(ws_url: &str) -> eyre::Result<()> {
    let origin = http_origin(ws_url);

    // Which org? Ask the server itself — the well-known document lists
    // every hosted slug; prefer the home org.
    let wk_url = format!("{origin}/.well-known/task-server.json");
    let slug = match reqwest::get(&wk_url).await {
        Ok(resp) => resp.json::<serde_json::Value>().await.ok().and_then(|doc| {
            let orgs = doc.get("orgs")?.as_array()?.clone();
            let pick = orgs
                .iter()
                .find(|o| o.get("is_home").and_then(|v| v.as_bool()).unwrap_or(false))
                .or_else(|| orgs.first())?;
            Some(pick.get("slug")?.as_str()?.to_owned())
        }),
        Err(e) => {
            println!("API check: SKIPPED — could not fetch {wk_url} ({e})");
            return Ok(());
        }
    };
    let Some(slug) = slug else {
        println!("API check: SKIPPED — the server hosts no orgs (nothing to describe)");
        return Ok(());
    };

    let api_url = format!("{origin}/org/{slug}/api");
    let doc: serde_json::Value = match reqwest::get(&api_url).await {
        Ok(resp) if resp.status().is_success() => resp
            .json()
            .await
            .map_err(|e| eyre::eyre!("parse {api_url}: {e}"))?,
        Ok(resp) => {
            println!(
                "API check: UNVERIFIED — {api_url} returned {} (the server \
                 predates the API reference endpoint)",
                resp.status()
            );
            return Ok(());
        }
        Err(e) => {
            println!("API check: SKIPPED — could not fetch {api_url} ({e})");
            return Ok(());
        }
    };

    let services = doc
        .get("services")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let streams = services
        .iter()
        .filter(|s| s.get("stream").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();
    println!(
        "API check: org `{slug}` mounts {} service(s) — {} rpc, {} stream(s) \
         ({api_url})",
        services.len(),
        services.len() - streams,
        streams,
    );

    // Per-service stamp diff against this build's registry.
    let local = task_server::schema_stamps();
    let mut stale: Vec<String> = Vec::new();
    let mut unserved: Vec<&str> = Vec::new();
    for (name, stamp) in &local {
        let served = services
            .iter()
            .find_map(|s| (s.get("name")?.as_str()? == *name).then(|| s.get("stamp"))?);
        match served.and_then(|v| v.as_str()) {
            Some(s) if s == stamp => {}
            Some(s) => stale.push(format!("{name} (server {s}, this build {stamp})")),
            None => unserved.push(name),
        }
    }
    if stale.is_empty() && unserved.is_empty() {
        println!("  all {} service stamps match this build", local.len());
    }
    if !unserved.is_empty() {
        println!(
            "  WARNING: {} service(s) in this build are not mounted by the \
             server: {}",
            unserved.len(),
            unserved.join(", ")
        );
    }
    for line in &stale {
        println!("  WARNING: stamp skew on {line}");
    }
    if !stale.is_empty() {
        println!(
            "  The running task-server was built against different `*-proto` \
             shapes than this CLI for the service(s) above — rebuild whichever \
             is older."
        );
    }
    Ok(())
}

/// The authorization half of `task doctor`: how much of the org lane the
/// permission gate actually covers, and what would break if enforcement
/// were switched on.
///
/// Static and offline — it folds the very same `permits` tables the server
/// installs (the CLI links `task_server`), so it answers for the build in
/// front of you without needing a running server or a token. For the
/// *runtime* half — what real clients have actually been refused — ask a
/// running server: `GET /server/permissions` with the
/// `TASK_BACKUP_GIT_TOKEN` bearer.
fn doctor_check_permissions() {
    println!("{}", task_server::permits::coverage_summary());
    if task_server::enforce_permissions() {
        println!("  TASK_ENFORCE_PERMISSIONS=1 in this environment — the gate ENFORCES.");
    } else {
        println!(
            "  Enforcement is OFF here (TASK_ENFORCE_PERMISSIONS != 1); the gate audits only."
        );
    }
}

async fn run(cli: Cli) -> eyre::Result<()> {
    match cli.command {
        Commands::Doctor => {
            let server = cli
                .server
                .unwrap_or_else(|| "ws://127.0.0.1:9090/vox".to_owned());
            let remote =
                RemoteVoxConfig::from_args(server.clone(), cli.session_token, cli.organization_id)?;
            println!("Vox endpoint: {}", remote.display_url);
            doctor_check_schema(&server).await?;
            doctor_check_permissions();
            doctor_check_api(&server).await?;
        }
        Commands::Api(args) => {
            return run_api(args);
        }
        Commands::Vault { fs, cmd } => match cmd {
            // Sync ops have their own vox orchestration.
            VaultCmd::Sync { .. } | VaultCmd::Pull { .. } | VaultCmd::Push { .. } => {
                return Box::pin(run_vault_sync(cmd)).await;
            }
            other => {
                return Box::pin(run_vault(other, fs)).await;
            }
        },
        Commands::Task(cmd) => {
            return Box::pin(run_task(cmd)).await;
        }
        #[cfg(feature = "plugin-agent")]
        Commands::Agent(cmd) => {
            return run_agent(cmd).await;
        }
        #[cfg(feature = "plugin-agent")]
        Commands::Runner(cmd) => {
            return runner::run_runner(cmd).await;
        }
        Commands::Skills(cmd) => {
            return skills::run_skills(cmd).await;
        }
        #[cfg(not(feature = "plugin-agent"))]
        Commands::Runner(_) => {
            return Err(not_compiled("runner"));
        }
        #[cfg(not(feature = "plugin-agent"))]
        Commands::Agent(_) => {
            return Err(not_compiled("agent"));
        }
        #[cfg(feature = "plugin-email")]
        Commands::Email(cmd) => {
            return run_email(cmd, cli.org.as_deref()).await;
        }
        #[cfg(not(feature = "plugin-email"))]
        Commands::Email(_) => {
            return Err(not_compiled("email"));
        }
        #[cfg(feature = "plugin-wiki")]
        Commands::Wiki(cmd) => {
            return run_wiki(cmd).await;
        }
        #[cfg(not(feature = "plugin-wiki"))]
        Commands::Wiki(_) => {
            return Err(not_compiled("wiki"));
        }
        Commands::Timer(cmd) => {
            return run_timer(cmd, cli.org.as_deref()).await;
        }
        Commands::Files(cmd) => {
            return run_files(cmd, cli.org.as_deref()).await;
        }
        #[cfg(feature = "plugin-finance")]
        Commands::Finance(cmd) => {
            return run_finance(cmd, cli.org.as_deref()).await;
        }
        #[cfg(not(feature = "plugin-finance"))]
        Commands::Finance(_) => {
            return Err(not_compiled("finance"));
        }
        Commands::Auth(cmd) => {
            return run_auth(cmd, cli.org.as_deref()).await;
        }
        Commands::Issue(cmd) => {
            return Box::pin(run_issue(cmd)).await;
        }
        Commands::Setup(cmd) => {
            return Box::pin(run_setup(cmd)).await;
        }
        Commands::Code(cmd) => {
            return Box::pin(run_code(cmd)).await;
        }
        Commands::Label(cmd) => {
            return run_label(cmd);
        }
        Commands::Org(cmd) => {
            return Box::pin(run_org(cmd)).await;
        }
        Commands::Admin(cmd) => {
            return Box::pin(run_admin(cmd)).await;
        }
        Commands::Mount(cmd) => {
            return run_mount(cmd);
        }
        Commands::Cycle(cmd) => {
            return Box::pin(run_cycle(cmd)).await;
        }
        Commands::Project(cmd) => {
            return Box::pin(run_project(cmd)).await;
        }
        Commands::Goal(cmd) => {
            return Box::pin(run_goal(cmd)).await;
        }
        Commands::Milestone(cmd) => {
            return Box::pin(run_milestone(cmd)).await;
        }
        Commands::Workstream(cmd) => {
            return Box::pin(workstream::run_workstream(cmd)).await;
        }
        #[cfg(feature = "plugin-fasttrackstudio")]
        Commands::Collection(cmd) => {
            return Box::pin(collection::run_collection(cmd)).await;
        }
        #[cfg(not(feature = "plugin-fasttrackstudio"))]
        Commands::Collection(_) => {
            return Err(not_compiled("fasttrackstudio"));
        }
        #[cfg(feature = "plugin-fasttrackstudio")]
        Commands::Song(cmd) => {
            return Box::pin(collection::run_song(cmd)).await;
        }
        #[cfg(not(feature = "plugin-fasttrackstudio"))]
        Commands::Song(_) => {
            return Err(not_compiled("fasttrackstudio"));
        }
        Commands::Media(cmd) => {
            return Box::pin(media::run_media(cmd)).await;
        }
        #[cfg(feature = "plugin-home")]
        Commands::Location(cmd) => {
            return Box::pin(run_location(cmd)).await;
        }
        #[cfg(not(feature = "plugin-home"))]
        Commands::Location(_) => {
            return Err(not_compiled("home"));
        }
        Commands::Inbox(cmd) => {
            return Box::pin(run_inbox(cmd)).await;
        }
        Commands::Threads(cmd) => {
            return Box::pin(run_threads(cmd)).await;
        }
        #[cfg(feature = "plugin-mealplan")]
        Commands::Recipe(cmd) => {
            return Box::pin(run_recipe(cmd)).await;
        }
        #[cfg(not(feature = "plugin-mealplan"))]
        Commands::Recipe(_) => {
            return Err(not_compiled("mealplan"));
        }
        #[cfg(feature = "plugin-mealplan")]
        Commands::Meal(cmd) => {
            return Box::pin(run_meal(cmd)).await;
        }
        #[cfg(not(feature = "plugin-mealplan"))]
        Commands::Meal(_) => {
            return Err(not_compiled("mealplan"));
        }
        #[cfg(feature = "plugin-mealplan")]
        Commands::Pantry(cmd) => {
            return Box::pin(run_pantry(cmd)).await;
        }
        #[cfg(not(feature = "plugin-mealplan"))]
        Commands::Pantry(_) => {
            return Err(not_compiled("mealplan"));
        }
        #[cfg(feature = "plugin-mealplan")]
        Commands::Shopping(cmd) => {
            return Box::pin(mealprep::run_shopping(cmd)).await;
        }
        #[cfg(not(feature = "plugin-mealplan"))]
        Commands::Shopping(_) => {
            return Err(not_compiled("mealplan"));
        }
        #[cfg(feature = "plugin-fitness")]
        Commands::Body(cmd) => {
            return Box::pin(run_body(cmd)).await;
        }
        #[cfg(not(feature = "plugin-fitness"))]
        Commands::Body(_) => {
            return Err(not_compiled("fitness"));
        }
        #[cfg(feature = "plugin-fitness")]
        Commands::Exercise(cmd) => {
            return Box::pin(run_exercise(cmd)).await;
        }
        #[cfg(not(feature = "plugin-fitness"))]
        Commands::Exercise(_) => {
            return Err(not_compiled("fitness"));
        }
        #[cfg(feature = "plugin-fitness")]
        Commands::Workout(cmd) => {
            return Box::pin(run_workout(cmd)).await;
        }
        #[cfg(not(feature = "plugin-fitness"))]
        Commands::Workout(_) => {
            return Err(not_compiled("fitness"));
        }
        #[cfg(feature = "plugin-fitness")]
        Commands::Intake(cmd) => {
            return Box::pin(run_intake(cmd)).await;
        }
        #[cfg(not(feature = "plugin-fitness"))]
        Commands::Intake(_) => {
            return Err(not_compiled("fitness"));
        }
        #[cfg(feature = "plugin-scheduling")]
        Commands::Plan(cmd) => {
            return Box::pin(plan::run_plan(cmd)).await;
        }
        #[cfg(not(feature = "plugin-scheduling"))]
        Commands::Plan(_) => {
            return Err(not_compiled("scheduling"));
        }
        #[cfg(feature = "plugin-scheduling")]
        Commands::Next(args) => {
            return Box::pin(plan::run_next(args)).await;
        }
        #[cfg(not(feature = "plugin-scheduling"))]
        Commands::Next(_) => {
            return Err(not_compiled("scheduling"));
        }
        Commands::Brief(args) => {
            return Box::pin(brief::run_brief(args)).await;
        }
    }
    Ok(())
}

/// Resolve the per-org vox URL from CLI flags + env + session.
/// Mirror of the helper inside `run_vault_sync`, lifted out
/// because project + goal share the same routing surface.
fn resolve_org_vox_url(server: Option<String>, org_slug: &str) -> String {
    let base = resolve_server_base(server.as_deref());
    format!("{base}/org/{org_slug}/vox")
}

/// Which server should this invocation talk to? Precedence:
///
/// 1. explicit `--server` (clap; the flag beats its env binding)
/// 2. `TASK_VOX_URL` env (folded into the global flag by clap)
/// 3. the active session's stored server URL (`task auth login`
///    against a remote records where it signed in, so subsequent
///    commands need nothing but the session)
/// 4. the localhost default
///
/// Returns a normalized vox base (`ws(s)://host[:port]`, no
/// trailing `/vox`).
fn resolve_server_base(explicit: Option<&str>) -> String {
    let flag_or_env = explicit
        .map(str::to_owned)
        .or_else(global_server)
        .or_else(|| std::env::var("TASK_VOX_URL").ok())
        .filter(|u| !u.trim().is_empty());
    // Only consult the session file when nothing explicit is set —
    // keeps the hot path off the filesystem.
    let session_url = if flag_or_env.is_some() {
        None
    } else {
        session_store::load()
            .ok()
            .flatten()
            .and_then(|s| s.active_server().map(|e| e.url.clone()))
    };
    pick_server_base(flag_or_env.as_deref(), session_url.as_deref())
}

/// Look up an org's manifest id from the resolved server's
/// `/.well-known/task-server.json` — the remote counterpart of
/// reading `<org>/org.toml` off the local data root. Best-effort:
/// `None` on any failure (offline, older server, unknown slug).
/// Meaningless in embedded mode (the org IS the local data root, so
/// the manifest read already answered).
pub(crate) async fn remote_org_id(slug: &str) -> Option<uuid::Uuid> {
    if embed_enabled() {
        return None;
    }
    let origin = resolve_server_http_base(None);
    let url = format!("{origin}/.well-known/task-server.json");
    let doc: serde_json::Value = reqwest::get(&url).await.ok()?.json().await.ok()?;
    doc.get("orgs")?.as_array()?.iter().find_map(|o| {
        if o.get("slug")?.as_str()? != slug {
            return None;
        }
        o.get("id")?.as_str()?.parse().ok()
    })
}

/// HTTP(S) base for the server's plain HTTP routes (`/blobs/*`),
/// derived from the resolved vox base (`ws→http`, `wss→https`).
fn resolve_server_http_base(explicit: Option<&str>) -> String {
    let base = resolve_server_base(explicit);
    if let Some(rest) = base.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = base.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        base
    }
}

/// Pure core of [`resolve_server_base`] — unit-testable precedence
/// fold. `flag_or_env` is `--server`/`TASK_VOX_URL` (already
/// flag-over-env, courtesy of clap), `session_url` the active
/// session entry's stored server.
fn pick_server_base(flag_or_env: Option<&str>, session_url: Option<&str>) -> String {
    if let Some(u) = flag_or_env.filter(|u| !u.trim().is_empty()) {
        return session_store::normalize_server_base(u);
    }
    if let Some(u) = session_url.filter(|u| !u.trim().is_empty()) {
        return session_store::normalize_server_base(u);
    }
    session_store::DEFAULT_LOCAL_VOX.to_owned()
}

/// Embedded backend, built once per process: a full `AppState` plus the
/// construction `Scope` that keeps its in-process vox acceptor tasks
/// alive. Only initialized when embedded mode is active.
struct Embedded {
    state: task_server::AppState,
    scope: std::sync::Arc<architect::Scope>,
}

static EMBEDDED: tokio::sync::OnceCell<Embedded> = tokio::sync::OnceCell::const_new();

/// True when the CLI should host the backend in-process instead of
/// talking to a running `task-server`. Opt-in via `TASK_EMBED`.
pub(crate) fn embed_enabled() -> bool {
    std::env::var("TASK_EMBED").is_ok_and(|v| matches!(v.as_str(), "1" | "true" | "yes"))
}

/// Can `slug` be served in-process? True when the org exists under
/// the local data root — the precondition for the embedded fallback
/// in [`establish_for_url`].
fn org_on_disk(slug: &str) -> bool {
    org_proto::DataRoot::from_env().is_ok_and(|r| r.orgs_dir().join(slug).is_dir())
}

/// Lazily build (once) and return the embedded backend.
async fn embedded() -> eyre::Result<&'static Embedded> {
    EMBEDDED
        .get_or_try_init(|| async {
            let scope = architect::Scope::new();
            let state = task_server::AppState::new(None)
                .await
                .map_err(|e| eyre::eyre!("embedded backend boot: {e}"))?;
            Ok::<_, eyre::Report>(Embedded { state, scope })
        })
        .await
}

/// Establish a typed service client over the active transport: an
/// in-process `LocalServer` when embedded (`TASK_EMBED`), otherwise a
/// vox WebSocket to the resolved per-org URL. Same client type either
/// way — architect's "inject remote vs local, one client".
async fn establish_client<C>(server: Option<String>, slug: &str) -> eyre::Result<C>
where
    C: vox_core::FromVoxLane,
{
    let url = resolve_org_vox_url(server, slug);
    establish_for_url(&url).await
}

/// `wss://host:port/anything` → `wss://host:port`. Everything past the
/// authority is per-org routing, not identity scope — two orgs on one
/// server share a session, two servers never do.
fn origin(u: &str) -> &str {
    let (scheme, rest) = u.split_once("://").unwrap_or(("", u));
    let prefix = if scheme.is_empty() {
        0
    } else {
        scheme.len() + 3
    };
    let authority_len = rest.find('/').unwrap_or(rest.len());
    &u[..prefix + authority_len]
}

/// The stored session token to present when dialing `url`, if any.
///
/// Scoped to the target twice over:
///
/// - **by server** — a token is only offered to the same scheme+authority
///   that issued it, so pointing the CLI at another host with `--server`
///   never hands that host the credential for the one we're signed into;
/// - **by org** — auth stores are per-org, so a token from `codywright`
///   is not a credential in `cbu`; it resolves to `anonymous` there. The
///   entry whose slug matches the URL's `/org/<slug>/vox` wins, and only
///   if none matches do we fall back to the active entry.
///
/// Without the org half, `task --org cbu …` would present whichever
/// session happened to be active — right host, wrong org, refused — and
/// the refusal reads identically to being signed out.
fn session_bearer_for(url: &str) -> Option<String> {
    let session = crate::session_store::load().ok().flatten()?;
    let same_server = |e: &crate::session_store::ServerEntry| {
        origin(&e.url) == origin(url) && !e.token.is_empty()
    };
    if let Some(slug) = url
        .rsplit_once("/org/")
        .and_then(|(_, rest)| rest.strip_suffix("/vox"))
        && let Some(entry) = session
            .servers
            .values()
            .find(|e| e.slug == slug && same_server(e))
    {
        return Some(entry.token.clone());
    }
    let entry = session.active_server()?;
    same_server(entry).then(|| entry.token.clone())
}

/// Dial `url` and establish `C`, presenting the stored session identity on
/// the handshake.
///
/// `vox::connect_lane` takes only a URL, and vox middleware is per typed
/// client (keyed to a service descriptor) rather than per connection — so
/// there is no choke point on the call path to hang a token on. The
/// identity therefore rides the WebSocket upgrade, as the web client does
/// it (`task_ui_core::vox_clients`), and the server applies it to every
/// call on the connection. Without this the CLI reaches the permission
/// gate as `principal=anonymous` on every RPC — fine while the gate is
/// observe-only, refused the moment `TASK_ENFORCE_PERMISSIONS=1`.
///
/// The token goes in `Authorization`, NOT the `vox.bearer.…` subprotocol
/// the browser uses: tungstenite fails the handshake outright when it
/// offers a subprotocol the peer doesn't echo, which would make the CLI
/// unable to reach an older server or anything behind a proxy that drops
/// the header. See `dial_ws_native` in task-ui-core.
async fn dial_authenticated<C>(url: &str) -> Result<C, vox_core::ConnectionError>
where
    C: vox_core::FromVoxLane,
{
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    // A tokenless dial stays on the stock path — identical behaviour to
    // before, including its error shapes.
    let Some(token) = session_bearer_for(url) else {
        return vox::connect_lane(url).establish().await;
    };
    let request = async {
        let mut request = url.into_client_request().ok()?;
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().ok()?);
        Some(request)
    }
    .await;
    // An unrepresentable URL or header is not an auth problem; let the
    // stock path produce its usual error for it.
    let Some(request) = request else {
        return vox::connect_lane(url).establish().await;
    };
    match tokio_tungstenite::connect_async(request).await {
        Ok((stream, _response)) => {
            vox_core::initiator_on(vox_websocket::WsLink::new(stream))
                .establish::<C>()
                .await
        }
        // Report through the stock path so the caller's `connect_error`
        // hint (and the embedded-server fallback above it) still applies.
        Err(_) => vox::connect_lane(url).establish().await,
    }
}

/// Tag a vox connect/establish failure with the `Connection` exit
/// class (6) and a "how do I point this somewhere else" hint.
fn connect_error<E: std::fmt::Debug>(url: &str, e: &E) -> eyre::Report {
    errors::connection(format!("connect `{url}`"))
        .cause(format!("{e:?}"))
        .hint("is task-server running? point the CLI elsewhere with --server or TASK_VOX_URL")
        .report()
}

/// Establish a typed client given an already-resolved per-org vox URL
/// (`…/org/<slug>/vox`). The choke point every per-org command goes
/// through. Transport resolution:
///
/// 1. `TASK_EMBED` set — serve the slug in-process, always.
/// 2. Otherwise dial the URL.
/// 3. Dial failed AND the target is the localhost default (nothing
///    remote was configured via `--server` / `TASK_VOX_URL` / a
///    remote session) AND the org exists under the local data root —
///    boot the embedded backend and serve in-process. This is what
///    keeps "no server running" workflows (timer, finance, wiki on a
///    laptop) working now that every command talks vox; an explicit
///    remote target still fails loud.
async fn establish_for_url<C>(url: &str) -> eyre::Result<C>
where
    C: vox_core::FromVoxLane,
{
    let slug = url
        .rsplit_once("/org/")
        .and_then(|(_, rest)| rest.strip_suffix("/vox"));
    if embed_enabled() {
        let slug = slug.ok_or_else(|| {
            eyre::eyre!("can't recover an org slug from `{url}` for embedded mode")
        })?;
        return establish_embedded(slug).await;
    }
    match Box::pin(dial_authenticated(url)).await {
        Ok(client) => Ok(client),
        Err(e) => {
            if let Some(slug) = slug {
                if url.starts_with(session_store::DEFAULT_LOCAL_VOX) && org_on_disk(slug) {
                    return establish_embedded(slug).await;
                }
            }
            Err(connect_error(url, &e))
        }
    }
}

/// Establish a typed client against the **server-management** endpoint
/// (`/server/vox` — `OrgManagementService` / `SnapshotService`). The
/// server-level counterpart of [`establish_client`]: no per-org slug.
/// Embedded (`TASK_EMBED`) serves the same router in-process via
/// [`task_server::AppState::server_local_server`]; otherwise it's a
/// WebSocket to the resolved server URL. Returns the client plus the
/// endpoint label for user-facing messages (`(embedded)` in-process).
async fn establish_server_client<C>(server: Option<&str>) -> eyre::Result<(C, String)>
where
    C: vox_core::FromVoxLane,
{
    if embed_enabled() {
        let emb = embedded().await?;
        let client = emb
            .state
            .server_local_server(&emb.scope)
            .establish()
            .await
            .map_err(|e| eyre::eyre!("embedded /server/vox establish: {e:?}"))?;
        Ok((client, "(embedded)".into()))
    } else {
        let url = resolve_server_vox_url(server)?;
        let client = Box::pin(vox::connect_lane(&url).establish())
            .await
            .map_err(|e| connect_error(&url, &e))?;
        Ok((client, url))
    }
}

/// Establish a typed client against the in-process [`LocalServer`] for
/// `slug`. Shared by [`establish_client`] and [`establish_for_url`].
async fn establish_embedded<C>(slug: &str) -> eyre::Result<C>
where
    C: vox_core::FromVoxLane,
{
    let emb = embedded().await?;
    emb.state
        .local_server(slug, &emb.scope)
        .ok_or_else(|| eyre::eyre!("org `{slug}` not hosted in embedded mode"))?
        .establish()
        .await
        .map_err(|e| eyre::eyre!("embedded establish for `{slug}`: {e:?}"))
}

/// Resolve the active org slug from `--org` flag or the
/// stored session. Returns a friendly error if neither
/// resolves.
///
/// Server-aware: when `--server` / `TASK_VOX_URL` targets a
/// specific server, the session entry FOR THAT SERVER supplies the
/// slug — switching the URL between the local dev server and a
/// remote deployment flips to the matching signed-in session
/// automatically, even though `active` still points elsewhere.
fn resolve_active_org(override_slug: Option<String>) -> eyre::Result<String> {
    if let Some(s) = override_slug.or_else(global_org) {
        return Ok(s);
    }
    let no_session = || {
        errors::usage("resolve active org")
            .cause("no org selected and no stored session")
            .hint("pass --org <slug> or run `task auth login` first")
            .report()
    };
    let sess = session_store::load()?.ok_or_else(no_session)?;
    if let Some(target) = global_server().or_else(|| std::env::var("TASK_VOX_URL").ok()) {
        if !target.trim().is_empty() {
            if let Some((_, entry)) = sess.entry_for_server(&target) {
                return Ok(entry.slug.clone());
            }
        }
    }
    let slug = sess.active_slug();
    if slug.is_empty() {
        return Err(no_session());
    }
    Ok(slug)
}

/// The org-slug resolution every RPC-only command module needs before
/// it can establish a client: `resolve_active_org` first (the
/// session/`--org` path — server-aware, see its own doc), falling back
/// to `org_ctx::resolve_active`'s local single-org disambiguation /
/// fresh-install auto-bootstrap of `default` when there's no `--org`
/// and no stored session, exactly as the pre-RPC direct-disk commands
/// used to behave. Shared so `timer`/`finance`/`cycle`/`files` (PR
/// #280 review: this exact match block had drifted into four verbatim
/// copies) don't grow a fifth.
pub(crate) fn resolve_slug(org_override: Option<&str>) -> eyre::Result<String> {
    match resolve_active_org(org_override.map(str::to_owned)) {
        Ok(s) => Ok(s),
        Err(_) => Ok(org_ctx::resolve_active(None)?.root.slug().to_owned()),
    }
}

/// Resolve the server-management vox URL:
/// - explicit `--server <ws://...>` flag wins
/// - else honor `TASK_SERVER_VOX_URL`
/// - else fall back to `ws://127.0.0.1:18080/server/vox`
fn resolve_server_vox_url(override_url: Option<&str>) -> eyre::Result<String> {
    if let Some(u) = override_url {
        return Ok(normalize_server_vox(u));
    }
    if let Ok(env) = std::env::var("TASK_SERVER_VOX_URL") {
        if !env.is_empty() {
            return Ok(normalize_server_vox(&env));
        }
    }
    Ok("ws://127.0.0.1:18080/server/vox".into())
}

fn normalize_server_vox(raw: &str) -> String {
    // Already pointed at the right endpoint.
    if raw.ends_with("/server/vox") {
        return raw.to_owned();
    }
    // Map http(s) → ws(s).
    let ws: String = if let Some(rest) = raw.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = raw.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if raw.starts_with("ws://") || raw.starts_with("wss://") {
        raw.to_owned()
    } else {
        format!("ws://{raw}")
    };
    // Strip legacy `/vox` suffix (the per-org URL hint that
    // `TASK_VOX_URL` sometimes points at) so we don't end up
    // with `…/vox/server/vox`. Then attach the canonical
    // server-mgmt path.
    let trimmed = ws.trim_end_matches('/').trim_end_matches("/vox");
    format!("{trimmed}/server/vox")
}

#[cfg(test)]
mod bearer_scope_tests {
    use super::origin;

    #[test]
    fn org_path_is_not_part_of_identity_scope() {
        // Every org on one server shares the session, so the per-org
        // routing suffix must not make the token look out-of-scope.
        assert_eq!(
            origin("wss://task.starcommand.live/org/codywright/vox"),
            "wss://task.starcommand.live"
        );
        assert_eq!(
            origin("wss://task.starcommand.live"),
            "wss://task.starcommand.live"
        );
    }

    #[test]
    fn a_different_server_is_a_different_scope() {
        // The point of the check: `--server elsewhere` must never hand
        // that host the credential we hold for this one.
        assert_ne!(
            origin("wss://task.starcommand.live/org/x/vox"),
            origin("wss://evil.example/org/x/vox"),
        );
        // Port and scheme are part of the authority, not decoration.
        assert_ne!(
            origin("ws://127.0.0.1:18080/vox"),
            origin("ws://127.0.0.1:9/vox")
        );
        assert_ne!(origin("ws://host/vox"), origin("wss://host/vox"));
    }
}

#[cfg(test)]
mod server_resolution_tests {
    use super::*;

    #[test]
    fn flag_or_env_beats_session() {
        assert_eq!(
            pick_server_base(
                Some("wss://task.starcommand.live/vox"),
                Some("ws://127.0.0.1:18080")
            ),
            "wss://task.starcommand.live"
        );
        // …and the flip: env pointing local wins over a stored
        // remote session — the URL switch IS the selector.
        assert_eq!(
            pick_server_base(
                Some("ws://127.0.0.1:18080/vox"),
                Some("wss://task.starcommand.live")
            ),
            "ws://127.0.0.1:18080"
        );
    }

    #[test]
    fn session_beats_default() {
        assert_eq!(
            pick_server_base(None, Some("wss://task.starcommand.live/vox")),
            "wss://task.starcommand.live"
        );
        // Legacy "local" session entries resolve to the default.
        assert_eq!(
            pick_server_base(None, Some("local")),
            session_store::DEFAULT_LOCAL_VOX
        );
    }

    #[test]
    fn default_when_nothing_set() {
        assert_eq!(
            pick_server_base(None, None),
            session_store::DEFAULT_LOCAL_VOX
        );
        // Blank values don't shadow lower-precedence sources.
        assert_eq!(
            pick_server_base(Some(""), Some(" ")),
            session_store::DEFAULT_LOCAL_VOX
        );
    }

    #[test]
    fn org_url_appends_per_org_path() {
        // resolve_org_vox_url rides the same fold; with an
        // explicit server the env/session never enter.
        assert_eq!(
            resolve_org_vox_url(Some("wss://task.starcommand.live/vox".into()), "codywright"),
            "wss://task.starcommand.live/org/codywright/vox"
        );
    }

    #[test]
    fn ws_http_derivation() {
        use crate::auth::ws_base_to_http;
        assert_eq!(
            ws_base_to_http("wss://task.starcommand.live"),
            "https://task.starcommand.live"
        );
        assert_eq!(
            ws_base_to_http("ws://127.0.0.1:18080"),
            "http://127.0.0.1:18080"
        );
    }
}
