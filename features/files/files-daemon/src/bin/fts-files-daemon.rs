//! `fts-files-daemon` — the standalone headless sync daemon (issue
//! #265): studio rigs, servers, and any machine whose files should keep
//! syncing while nobody is logged in run this as a system/user service.
//!
//! What it does, in the order it does it:
//!
//! 1. Opens its replica store and its **device identity** — a uuid and
//!    an endpoint key, both on disk in the data dir, so a restart comes
//!    back as the same machine at the same address rather than as a
//!    stranger.
//! 2. Binds that endpoint and **serves the replica lane** on it. This is
//!    what makes sync two-way: the engine has no push, so a machine that
//!    cannot be dialled has no way to hand over the work it did offline.
//! 3. Starts **watching** its roots, so local edits become checkpoints
//!    on cadence without anyone pressing anything.
//! 4. Dials its coordinator, admits it in return, and pulls on a timer.
//! 5. Serves the control surface on a local socket for the desktop app
//!    and the CLI.
//!
//! Config is environment-driven so it drops into a service unit:
//!
//! | var | meaning | default |
//! |---|---|---|
//! | `FTS_FILES_DAEMON_DATA` | data dir (store, vault, identity, key) | `~/.local/share/fts-files` |
//! | `FTS_FILES_DAEMON_BIND` | control socket | `127.0.0.1:4055` |
//! | `FTS_FILES_DAEMON_INTERVAL_SECS` | tick cadence | `30` |
//! | `FTS_FILES_DAEMON_COORDINATOR` | the peer to sync with: an iroh **endpoint id**, or a `ws://` URL for a local dev server | — |
//! | `FTS_FILES_DAEMON_ROOTS` | where newly adopted roots land | `<data>/roots` |
//! | `FTS_FILES_DAEMON_SYNC_ALL` | take everything the coordinator offers | `1` |
//!
//! # Why the coordinator is an endpoint id and not a URL
//!
//! Because an id is a credential and a URL is not. An iroh connection is
//! mutually authenticated, so dialling the org's id proves who this
//! machine is without a token to mint, store, expire or leak — which is
//! why this binary enrols nothing. A `ws://` coordinator is still
//! accepted for a dev server on localhost, where the transport proves
//! nothing and nothing is being protected.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use architect::LayerRouter;
use axum::extract::ws::WebSocketUpgrade;
use axum::response::IntoResponse;
use axum::routing::any;
use files::FilesBackend;
use files_daemon::{DaemonControl, SyncDaemon};

const VOX_SUBPROTOCOL: &str = "vox";

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

const USAGE: &str = "\
fts-files-daemon — the Task file sync agent

    fts-files-daemon                          run in the foreground
    fts-files-daemon install [options]        install as a background service (starts at login)
    fts-files-daemon uninstall                remove the background service
    fts-files-daemon service-status           is the service installed, and where
    fts-files-daemon id                       this machine's endpoint id (admit it on the server)
    fts-files-daemon status                   what the running agent is doing
    fts-files-daemon checkpoint <root-id>     force a save point now (before unplugging)
    fts-files-daemon share <dir> [--name N]   share a folder from this machine
    fts-files-daemon peer <endpoint-id>       admit a machine, and take what it shares
    fts-files-daemon forget <endpoint-id>     stop admitting a machine, and stop pulling it

install options:
    --coordinator <endpoint-id>   the org endpoint to sync with
    --data <dir>                  store, vault, device identity   [~/.local/share/fts-files]
    --roots <dir>                 where synced projects land      [~/Task]
    --bind <addr>                 control socket                  [127.0.0.1:4055]
    --interval <secs>             reconcile cadence               [30]
    --dry-run                     print what would happen, change nothing
";

/// The subcommands, before any async runtime exists — installing a
/// service starts no sockets and should not pay for a runtime.
fn run_command(args: &[String]) -> Option<Result<(), Box<dyn std::error::Error>>> {
    match args.first().map(String::as_str) {
        Some("install") => Some(install(&args[1..])),
        Some("uninstall") => Some(uninstall()),
        Some("service-status") => Some(service_status()),
        Some("id") => Some(print_endpoint_id()),
        // Handled in `main`: they dial something, so they need the async
        // runtime rather than avoiding it.
        Some("status" | "checkpoint" | "share" | "peer" | "forget") => None,
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Some(Ok(()))
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            Some(Err("unknown command".into()))
        }
        None => None,
    }
}

fn install(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let home = home();
    let mut config = files_daemon::install::ServiceConfig::for_this_binary(&home)?;
    // The environment is the same configuration surface the running
    // daemon reads, so an operator who has already exported these gets
    // them baked in rather than having to repeat them as flags.
    config.coordinator = env("FTS_FILES_DAEMON_COORDINATOR");
    if let Some(v) = env("FTS_FILES_DAEMON_DATA") {
        config.data_dir = PathBuf::from(v);
    }
    if let Some(v) = env("FTS_FILES_DAEMON_ROOTS") {
        config.roots_dir = PathBuf::from(v);
    }

    let mut dry_run = false;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let mut value = || {
            rest.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--coordinator" => config.coordinator = Some(value()?),
            "--data" => config.data_dir = PathBuf::from(value()?),
            "--roots" => config.roots_dir = PathBuf::from(value()?),
            "--bind" => config.bind = value()?,
            "--interval" => config.interval_secs = value()?.parse()?,
            "--dry-run" => dry_run = true,
            other => return Err(format!("unknown option: {other}\n\n{USAGE}").into()),
        }
    }

    // Copy this binary somewhere stable and register *that*, unless it
    // already lives there or the caller named a program explicitly. A
    // unit pointing into `target/debug` works until the next
    // `cargo clean` and then fails at every login, silently.
    let installed = files_daemon::install::ServiceConfig::installed_binary(&home);
    let copy_binary = config.program != installed
        && !config.program.starts_with(home.join(".local/bin"))
        && !config.program.starts_with("/usr")
        && !config.program.starts_with("/nix/store")
        // Inside a macOS app bundle the binary is already where it
        // belongs, and copying it out would leave the copy unsigned.
        && !config.program.to_string_lossy().contains(".app/Contents/");
    if copy_binary {
        config.program = installed.clone();
    }

    let plan = files_daemon::install::install_plan(&home, &config)?;
    if copy_binary {
        println!("copy    {} → {}", std::env::current_exe()?.display(), installed.display());
    }
    print!("{}", plan.describe());
    if dry_run {
        return Ok(());
    }
    std::fs::create_dir_all(&config.roots_dir)?;
    if copy_binary {
        let from = std::env::current_exe()?;
        std::fs::create_dir_all(installed.parent().expect("a parent"))?;
        // Copy to a temp name and rename: overwriting a running binary
        // in place is what "text file busy" is, and the agent being
        // upgraded is usually running.
        let tmp = installed.with_extension("new");
        std::fs::copy(&from, &tmp)?;
        std::fs::rename(&tmp, &installed)?;
    }
    plan.apply()?;
    println!(
        "\ninstalled. the agent starts at login and restarts if it dies.\n\
         its endpoint id is logged at startup — admit it on the server to sync."
    );
    if config.coordinator.is_none() {
        println!(
            "no coordinator set: this machine will serve its own content but pull nothing.\n\
             re-run with --coordinator <endpoint-id> once you have the org's id."
        );
    }
    Ok(())
}

fn uninstall() -> Result<(), Box<dyn std::error::Error>> {
    let plan = files_daemon::install::uninstall_plan(&home())?;
    print!("{}", plan.describe());
    plan.apply()?;
    println!("\nremoved. synced content is untouched.");
    Ok(())
}

/// This machine's endpoint id, without starting anything.
///
/// The id is the public half of the device key, so it can be read (or
/// minted, on a first run) from the data dir alone — which is what makes
/// "paste this into the server" a step a person can take before the
/// agent has ever connected to anything.
fn print_endpoint_id() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = PathBuf::from(
        env("FTS_FILES_DAEMON_DATA")
            .unwrap_or_else(|| format!("{}/.local/share/fts-files", home().display())),
    );
    let key = files_daemon::identity::DeviceIdentity::endpoint_key(&data_dir.join("daemon"))?;
    println!("{}", key.public());
    Ok(())
}

fn service_status() -> Result<(), Box<dyn std::error::Error>> {
    let home = home();
    let path = files_daemon::install::unit_path(&home);
    if files_daemon::install::is_installed(&home) {
        println!("installed: {}", path.display());
    } else {
        println!("not installed (would be {})", path.display());
    }
    Ok(())
}

/// The control socket of the agent already running on this machine.
async fn control() -> Result<files_daemon::DaemonControlServiceClient, Box<dyn std::error::Error>> {
    let bind = env("FTS_FILES_DAEMON_BIND").unwrap_or_else(|| "127.0.0.1:4055".into());
    let url = format!("ws://{bind}/vox");
    Ok(vox::connect_lane(&url)
        .establish()
        .await
        .map_err(|e| format!("no agent answering on {url} ({e}) — is it running?"))?)
}

/// What the running agent is doing, as a person would ask it.
async fn status() -> Result<(), Box<dyn std::error::Error>> {

    let status = control().await?.status().await?;
    println!(
        "device     {}",
        status
            .device_id
            .map_or_else(|| "—".into(), |id| id.to_string())
    );
    println!("endpoint   {}", status.endpoint_id.as_deref().unwrap_or("—"));
    println!(
        "syncing    {}",
        if status.coordinator {
            "yes"
        } else {
            "no coordinator set"
        }
    );
    if status.paused {
        println!("paused     everything");
    }
    for peer in &status.peers {
        println!("admits     {peer}");
    }
    for root in &status.roots {
        println!(
            "root       {}  {:?}  {}%{}{}",
            root.name,
            root.state,
            root.percent(),
            // Where it comes from, shortened: a root that is not moving
            // is a question about its peer, and a full endpoint id
            // buries the state a person is scanning for.
            root.peer
                .as_deref()
                .map(|p| format!("  from {}", &p[..p.len().min(8)]))
                .unwrap_or_default(),
            root.last_error
                .as_deref()
                .map(|e| format!("  ({e})"))
                .unwrap_or_default()
        );
    }
    Ok(())
}

/// Force a save point on a root, which is what "before I unplug" means.
async fn checkpoint(root: &str) -> Result<(), Box<dyn std::error::Error>> {

    let client = control().await?;
    // By id, or by the name the status surface shows — a person reads
    // the name and should be able to type what they read.
    let id = match root.parse::<uuid::Uuid>() {
        Ok(id) => id,
        Err(_) => client
            .status()
            .await?
            .roots
            .iter()
            .find(|r| r.name == root)
            .ok_or_else(|| format!("no synced root called {root}"))?
            .root_id,
    };
    client.checkpoint_now(id).await?;
    println!("checkpointed {root}");
    Ok(())
}

/// Share a folder from this machine, through the running agent.
async fn share(dir: &str, name: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let path = std::fs::canonicalize(dir).map_err(|e| format!("{dir}: {e}"))?;
    let (id, name) = control()
        .await?
        .share(path.to_string_lossy().into_owned(), name)
        .await?;
    println!("sharing {name}  ({id})");
    println!("{}", path.display());
    println!();
    println!("on the other machine:");
    println!("    fts-files-daemon peer {}", endpoint_id()?);
    Ok(())
}

/// Admit a machine and take whatever it shares.
///
/// Both halves in one command, because they are one intention: a person
/// naming another machine means "sync with that", and admitting without
/// pulling (or pulling without admitting) is half of it — the half that
/// looks like nothing happening.
async fn peer(endpoint_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    client.admit_peer(endpoint_id.to_string()).await?;

    let roots = env("FTS_FILES_DAEMON_ROOTS").unwrap_or_else(|| {
        format!(
            "{}/.local/share/fts-files/roots",
            home().display()
        )
    });
    println!("admitted {endpoint_id}");

    // The pull is the second half, and it fails on a first run for a
    // reason that is not a failure: admission is symmetric, so until
    // that machine has admitted this one it refuses to be read. Saying
    // "permission denied" there would report the *successful* half as an
    // error and leave a person with no idea what to do next.
    match client.pull_all(endpoint_id.to_string(), roots.clone()).await {
        Ok(taken) if taken.is_empty() => {
            println!("it is sharing nothing yet — nothing to take.");
        }
        Ok(taken) => {
            for name in &taken {
                println!("syncing {name} → {roots}/{name}");
            }
        }
        Err(e) if is_not_admitted(&e) => {
            println!();
            println!("that machine has not admitted this one yet, so it will not be read.");
            println!("run this there, then re-run this command:");
            println!("    fts-files-daemon peer {}", endpoint_id_or_unknown());
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

/// Whether a pull failed because the far side has not admitted us.
///
/// Matched on the gate's own words rather than a typed variant: the
/// refusal crosses the wire as a `permission denied` payload, and the
/// alternative — a distinct error kind for it — would have to be
/// threaded through the replica lane, which serves peers that are not
/// supposed to learn why they were refused.
fn is_not_admitted(e: &impl std::fmt::Display) -> bool {
    let text = e.to_string();
    text.contains("permission denied") || text.contains("may not read")
}

/// Stop admitting a machine, and stop pulling from it.
///
/// The counterpart of `peer`, and its absence was a hole with a shape:
/// a person could add a machine to the one list that decides who may
/// read this one's whole history, and then had no way to take it off
/// again short of editing the store by hand.
///
/// Both halves, again for `peer`'s reason. Dismissing without dropping
/// the sync choices leaves an agent dialling a machine it no longer
/// trusts, every tick, forever.
async fn forget(endpoint_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    let before = client.status().await?;
    client.dismiss_peer(endpoint_id.to_string()).await?;

    // A root chosen against that peer has nowhere to pull from now.
    // Local content stays exactly where it is — this stops syncing it,
    // it does not delete it.
    let mut dropped = 0;
    for root in &before.roots {
        if root.peer.as_deref() == Some(endpoint_id) {
            client.remove_sync_choice(root.root_id).await?;
            println!("stopped syncing {} (its content stays)", root.name);
            dropped += 1;
        }
    }
    println!("forgot {endpoint_id}");
    if dropped == 0 {
        println!("nothing was being pulled from it.");
    }
    Ok(())
}

/// This machine's endpoint id, for printing in instructions.
fn endpoint_id() -> Result<String, Box<dyn std::error::Error>> {
    let data_dir = PathBuf::from(
        env("FTS_FILES_DAEMON_DATA")
            .unwrap_or_else(|| format!("{}/.local/share/fts-files", home().display())),
    );
    let key = files_daemon::identity::DeviceIdentity::endpoint_key(&data_dir.join("daemon"))?;
    Ok(key.public().to_string())
}

fn endpoint_id_or_unknown() -> String {
    endpoint_id().unwrap_or_else(|_| "<this machine's id>".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = run_command(&args) {
        return result;
    }
    // The two that talk to an agent already running, so they need the
    // runtime this function is already inside.
    match args.first().map(String::as_str) {
        Some("status") => return status().await,
        Some("checkpoint") => {
            let root = args
                .get(1)
                .ok_or("checkpoint needs a root id or name")?
                .clone();
            return checkpoint(&root).await;
        }
        Some("share") => {
            let dir = args.get(1).ok_or("share needs a directory")?.clone();
            let name = args
                .iter()
                .position(|a| a == "--name")
                .and_then(|i| args.get(i + 1))
                .cloned();
            return share(&dir, name).await;
        }
        Some("peer") => {
            let id = args.get(1).ok_or("peer needs an endpoint id")?.clone();
            return peer(&id).await;
        }
        Some("forget") => {
            let id = args.get(1).ok_or("forget needs an endpoint id")?.clone();
            return forget(&id).await;
        }
        _ => {}
    }

    // `info` unless RUST_LOG says otherwise. The default filter is
    // near-silent, which for a foreground run is tidy and for an
    // installed service is a hole: `journalctl -u task-sync` printed the
    // start line and nothing else, so "is it syncing?" had no answer
    // anywhere on the machine. iroh and jj are noisy at info and are
    // turned down rather than the rest turned off.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,iroh=warn,jj_lib=warn,netwatch=warn")
            }),
        )
        .init();

    let data_dir = PathBuf::from(env("FTS_FILES_DAEMON_DATA").unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        format!("{home}/.local/share/fts-files")
    }));
    let bind: SocketAddr = env("FTS_FILES_DAEMON_BIND")
        .unwrap_or_else(|| "127.0.0.1:4055".into())
        .parse()?;
    let interval = Duration::from_secs(
        env("FTS_FILES_DAEMON_INTERVAL_SECS")
            .and_then(|s| s.parse().ok())
            .unwrap_or(30),
    );
    let roots_under = env("FTS_FILES_DAEMON_ROOTS")
        .map_or_else(|| data_dir.join("roots"), PathBuf::from);

    // The daemon's own replica store lives under the data dir; the vault
    // is a sibling (curated version entities ride the vault, not the
    // replica content).
    let store = data_dir.join("store");
    std::fs::create_dir_all(&store)?;
    std::fs::create_dir_all(&roots_under)?;
    // The roots directory is declared as a permitted location, or every
    // adoption into it is refused as "outside the permitted boundary" —
    // see `peering::DeviceRoots` on why a device's boundary is not a
    // server's.
    // Cadence decides when local work becomes history, and therefore how
    // long it is before a peer can pull it: nothing syncs that has not
    // been captured. The product defaults (10-minute debounce, 30-minute
    // quiescence) are tuned for a session's *version history* being
    // legible — one save point per session, not one per keystroke — and
    // on a machine whose whole job is to get work to another machine
    // that is a long time to hold it. So both are settable, and neither
    // is silently changed.
    let cadence = {
        let mut config = files::CadenceConfig::default();
        if let Some(secs) = env("FTS_FILES_DAEMON_SNAPSHOT_SECS").and_then(|s| s.parse().ok()) {
            config.snapshot_debounce = chrono::TimeDelta::seconds(secs);
        }
        if let Some(secs) = env("FTS_FILES_DAEMON_QUIESCE_SECS").and_then(|s| s.parse().ok()) {
            config.quiescence = chrono::TimeDelta::seconds(secs);
        }
        config
    };
    // Where this machine may hold live trees: the replica directory,
    // plus every folder somebody has shared from here (persisted, so a
    // restart does not un-share them).
    let shared = std::sync::Arc::new(files_daemon::peering::DeviceRoots::open(
        &data_dir,
        &roots_under,
    )?);
    let backend = FilesBackend::with_cadence(
        &store,
        data_dir.join("vault"),
        cadence,
        std::sync::Arc::new(files::SystemClock),
    )?
    .with_location_boundaries(shared.clone());
    let daemon = SyncDaemon::open(backend, data_dir.join("daemon"))?;
    daemon.with_shared_dirs(shared);

    // Bind and serve before dialling anything: a device that pulls but
    // cannot be pulled from is the one-way arrangement this daemon
    // shipped with, and binding first means the coordinator can reach
    // back the moment it is told about us.
    let endpoint_id = daemon.bind_peering(None).await?;
    tracing::info!(
        device = %daemon.device_id(),
        %endpoint_id,
        %bind,
        "fts-files-daemon starting — admit this endpoint id on the server to sync"
    );

    // Local work becomes history on its own; without this the pull half
    // would be reconciling against a store nothing ever writes to.
    daemon.start_capture().await;

    // What this machine was syncing before it restarted. A peer that is
    // shut right now is skipped and retried on the next start — its
    // choice is still the person's.
    daemon.restore_choices().await;

    // The coordinator: the peer this daemon syncs with by default.
    // Optional, because a daemon with none is still useful — it serves
    // its own replica lane, so another machine can pull it.
    match env("FTS_FILES_DAEMON_COORDINATOR") {
        None => tracing::warn!(
            "no FTS_FILES_DAEMON_COORDINATOR set — this daemon serves its content but pulls nothing"
        ),
        Some(coordinator) if coordinator.starts_with("ws://") || coordinator.starts_with("wss://") => {
            // The dev path: a local server over a WebSocket, where the
            // transport proves nothing and nothing is being protected.
            let client: files_daemon::files_sync::SyncServiceClient =
                vox::connect_lane(&coordinator)
                    .establish()
                    .await
                    .map_err(|e| format!("connecting to coordinator {coordinator}: {e}"))?;
            daemon.set_coordinator(client);
            tracing::info!(%coordinator, "coordinator connected (websocket)");
        }
        Some(coordinator) => {
            daemon.set_coordinator_peer(&coordinator).await?;
            // Admit it in return. Syncing *with* a peer means each side
            // pulls the other, so a coordinator this machine will not
            // answer can never collect what this machine did offline.
            daemon.admit_peer(&coordinator);
            tracing::info!(%coordinator, "coordinator connected (iroh)");

            if env("FTS_FILES_DAEMON_SYNC_ALL").as_deref() != Some("0") {
                match daemon.peer_roots(&coordinator).await {
                    Ok(offered) => {
                        for root in offered {
                            match daemon
                                .sync_from_peer(&coordinator, root.id, vec![], &roots_under)
                                .await
                            {
                                Ok(_) => tracing::info!(root = %root.name, "syncing"),
                                Err(e) => {
                                    tracing::warn!(root = %root.name, error = %e, "could not take this root")
                                }
                            }
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "could not list the coordinator's roots"),
                }
            }
        }
    }

    // The reconcile tick loop — captures local work, then pulls every
    // chosen root.
    let ticker = daemon.clone();
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(interval);
        loop {
            timer.tick().await;
            ticker.tick().await;
        }
    });

    // The control surface on a local WebSocket.
    let control = DaemonControl::new(daemon);
    let app = axum::Router::new().route(
        "/vox",
        any(move |ws: WebSocketUpgrade| {
            let router = LayerRouter::new()
                .merge(files_daemon::service::layer(control.clone()))
                .merge(files_daemon::service::stream_layer(control.clone()));
            async move {
                ws.protocols([VOX_SUBPROTOCOL])
                    .on_upgrade(move |socket| architect::axum_ws::serve_router(socket, router))
                    .into_response()
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "fts-files-daemon control socket listening");
    axum::serve(listener, app).await?;
    Ok(())
}
