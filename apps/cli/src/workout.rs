//! `task workout …` — routines + sessions.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::exercise::connect_exercises_client;
use crate::exercise::resolve_exercise_target;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;

// ── Workouts (routines + sessions) ───────────────────────────────────

#[derive(Subcommand)]
pub(crate) enum WorkoutCmd {
    /// Routines (the program — push/pull/legs, etc).
    #[command(subcommand)]
    Routine(WorkoutRoutineCmd),
    /// Sessions (one workout instance).
    #[command(subcommand)]
    Session(WorkoutSessionCmd),
}

#[derive(Subcommand)]
pub(crate) enum WorkoutRoutineCmd {
    List {
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

#[derive(Subcommand)]
pub(crate) enum WorkoutSessionCmd {
    List {
        #[arg(long)]
        date: Option<String>,
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
    /// Start a fresh session from a routine + day.
    StartFromRoutine {
        routine: String,
        day: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Log a working set against a session.
    LogSet {
        session: String,
        exercise: String,
        reps: u32,
        weight: f64,
        #[arg(long)]
        rpe: Option<f64>,
        #[arg(long)]
        note: Option<String>,
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

async fn connect_workouts_client(url: &str) -> eyre::Result<workouts::WorkoutsServiceClient> {
    establish_for_url(url).await
}

async fn resolve_routine_target(
    client: &workouts::WorkoutsServiceClient,
    target: &str,
) -> eyre::Result<workouts::Routine> {
    if uuid::Uuid::parse_str(target).is_ok() {
        return client
            .get_routine(target.to_owned())
            .await
            .map_err(|e| eyre::eyre!("get_routine: {e:?}"));
    }
    let rows = client
        .list_routines()
        .await
        .map_err(|e| eyre::eyre!("list_routines: {e:?}"))?;
    rows.into_iter()
        .find(|r| r.path == target || r.name.eq_ignore_ascii_case(target))
        .ok_or_else(|| {
            crate::errors::not_found("resolve target", target)
                .cause("no path or name match")
                .report()
        })
}

async fn resolve_session_target(
    client: &workouts::WorkoutsServiceClient,
    target: &str,
) -> eyre::Result<workouts::WorkoutSession> {
    if uuid::Uuid::parse_str(target).is_ok() {
        return client
            .get_session(target.to_owned())
            .await
            .map_err(|e| eyre::eyre!("get_session: {e:?}"));
    }
    let rows = client
        .list_sessions()
        .await
        .map_err(|e| eyre::eyre!("list_sessions: {e:?}"))?;
    rows.into_iter().find(|s| s.path == target).ok_or_else(|| {
        crate::errors::not_found("resolve target", target)
            .cause("no path or name match")
            .report()
    })
}

pub(crate) async fn run_workout(cmd: WorkoutCmd) -> eyre::Result<()> {
    match cmd {
        WorkoutCmd::Routine(rc) => run_workout_routine(rc).await,
        WorkoutCmd::Session(sc) => run_workout_session(sc).await,
    }
}

async fn run_workout_routine(cmd: WorkoutRoutineCmd) -> eyre::Result<()> {
    match cmd {
        WorkoutRoutineCmd::List { org, server, json } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_workouts_client(&u).await?;
            let rows = client
                .list_routines()
                .await
                .map_err(|e| eyre::eyre!("list_routines: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            for r in &rows {
                println!("{:<32}  {} days    {}", r.name, r.days.0.len(), r.path);
            }
        }
        WorkoutRoutineCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_workouts_client(&u).await?;
            let r = resolve_routine_target(&client, &target).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&r).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{}\n", r.name);
            println!("  id:    {}", r.id);
            println!("  path:  {}", r.path);
            for d in &r.days.0 {
                println!("  day:   {}  ({} slots)", d.name, d.slots.len());
            }
        }
        WorkoutRoutineCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_workouts_client(&u).await?;
            let r = resolve_routine_target(&client, &target).await?;
            if !yes && !confirm(&format!("delete `{}` ({})?", r.name, r.path))? {
                println!("aborted");
                return Ok(());
            }
            client
                .delete_routine(r.id.to_string())
                .await
                .map_err(|e| eyre::eyre!("delete_routine: {e:?}"))?;
            println!("deleted {}", r.path);
        }
    }
    Ok(())
}

async fn run_workout_session(cmd: WorkoutSessionCmd) -> eyre::Result<()> {
    match cmd {
        WorkoutSessionCmd::List {
            date,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_workouts_client(&u).await?;
            let parsed = match date {
                None => None,
                Some(s) => Some(
                    chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                        .map_err(|e| eyre::eyre!("--date: {e}"))?,
                ),
            };
            let rows: Vec<_> = client
                .list_sessions()
                .await
                .map_err(|e| eyre::eyre!("list_sessions: {e:?}"))?
                .into_iter()
                .filter(|s| parsed.is_none_or(|d| s.date == d))
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            for s in &rows {
                println!(
                    "{}  {:<24}  {} sets    {}",
                    s.date,
                    s.name,
                    s.logged_sets.0.len(),
                    s.path
                );
            }
        }
        WorkoutSessionCmd::Get {
            target,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_workouts_client(&u).await?;
            let s = resolve_session_target(&client, &target).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&s).map_err(|e| eyre::eyre!("json: {e}"))?
                );
                return Ok(());
            }
            println!("{} ({})\n", s.name, s.date);
            println!("  id:    {}", s.id);
            println!("  path:  {}", s.path);
            for set in &s.logged_sets.0 {
                let rpe = set.rpe.map(|r| format!(" @ rpe {r}")).unwrap_or_default();
                println!(
                    "    [{}] {}: {}x{}kg{rpe}",
                    set.order, set.exercise_name, set.reps, set.weight_kg
                );
            }
        }
        WorkoutSessionCmd::StartFromRoutine {
            routine,
            day,
            date,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_workouts_client(&u).await?;
            let r = resolve_routine_target(&client, &routine).await?;
            let date_str = date.unwrap_or_else(|| chrono::Local::now().date_naive().to_string());
            let session = client
                .start_from_routine(r.id.to_string(), day, date_str)
                .await
                .map_err(|e| eyre::eyre!("start_from_routine: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&session).map_err(|e| eyre::eyre!("json: {e}"))?
                );
            } else {
                println!("started {} ({})", session.name, session.path);
                println!("  id: {}", session.id);
            }
        }
        WorkoutSessionCmd::LogSet {
            session,
            exercise,
            reps,
            weight,
            rpe,
            note,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_workouts_client(&u).await?;
            let s = resolve_session_target(&client, &session).await?;
            // Resolve the exercise to its id (auto-population
            // of `exercise_name` happens server-side; we still
            // pass a best-effort cache here).
            let ec = connect_exercises_client(&u).await?;
            let ex = resolve_exercise_target(&ec, &exercise).await?;
            let order = u32::try_from(s.logged_sets.0.len()).unwrap_or(0);
            let set = workouts::LoggedSet {
                id: uuid::Uuid::new_v4(),
                exercise_id: ex.id,
                exercise_name: ex.name,
                order,
                reps,
                weight_kg: weight,
                rir: None,
                rpe,
                completed: true,
                note,
            };
            let updated = client
                .log_set(s.id.to_string(), set)
                .await
                .map_err(|e| eyre::eyre!("log_set: {e:?}"))?;
            println!(
                "logged set #{order} on {} ({} total sets)",
                updated.name,
                updated.logged_sets.0.len()
            );
        }
        WorkoutSessionCmd::Delete {
            target,
            yes,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let client = connect_workouts_client(&u).await?;
            let s = resolve_session_target(&client, &target).await?;
            if !yes && !confirm(&format!("delete `{}` ({})?", s.name, s.path))? {
                println!("aborted");
                return Ok(());
            }
            client
                .delete_session(s.id.to_string())
                .await
                .map_err(|e| eyre::eyre!("delete_session: {e:?}"))?;
            println!("deleted {}", s.path);
        }
    }
    Ok(())
}
