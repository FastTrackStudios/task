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

/// Prove the agent can actually use its directories, before it needs to.
///
/// Every path here is one an operator chose, and the interesting failure
/// is not "permission denied" — that returns an error and says so. It is
/// the one this hit on a Mac with its store on an external SSD: a
/// launchd agent touching a volume macOS guards asks the system for
/// consent, no window can appear for a background job, and the `mkdir`
/// **never returns**. The service sits there, `launchctl` calls it
/// running, the log is empty, and nothing anywhere says why.
///
/// So the first filesystem work happens on a thread with a deadline, and
/// a deadline that expires names the path, the platform's reason, and
/// the fix. A background service is allowed to fail; it is not allowed
/// to hang and look healthy.
fn preflight(dirs: &[&std::path::Path]) -> Result<(), Box<dyn std::error::Error>> {
    const PATIENCE: Duration = Duration::from_secs(10);

    for dir in dirs {
        let dir = dir.to_path_buf();
        let probing = dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = probing;
            // A create *and* a write: consent can be granted for reading
            // a volume and withheld for writing it, and a store that
            // cannot be written to is not a store.
            let probe = std::fs::create_dir_all(&dir).and_then(|()| {
                let file = dir.join(".fts-writable");
                std::fs::write(&file, b"")?;
                std::fs::remove_file(&file)
            });
            let _ = tx.send(probe);
        });
        match rx.recv_timeout(PATIENCE) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(format!("{}: {e}", dir.display()).into()),
            Err(_) => {
                let secs = PATIENCE.as_secs();
                return Err(format!(
                    "{} did not answer in {secs}s.\n\
                     \n\
                     On macOS this is what a background agent looks like when the system \
                     is waiting for permission it cannot ask for: an external or removable \
                     volume needs consent, and no window can appear for a launchd job.\n\
                     Grant it in System Settings → Privacy & Security → Full Disk Access, \
                     adding:\n    {}\n\
                     then `launchctl kickstart -k gui/$(id -u)/{}`.\n\
                     \n\
                     Elsewhere: an unmounted volume, or a network filesystem that is not \
                     answering.",
                    dir.display(),
                    std::env::current_exe()
                        .unwrap_or_else(|_| PathBuf::from("this binary"))
                        .display(),
                    files_daemon::install::SERVICE_LABEL,
                )
                .into());
            }
        }
    }
    Ok(())
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
    fts-files-daemon share <dir> --later      register it now, read its bytes later
    fts-files-daemon capture                  capture what has not been captured yet
    fts-files-daemon shares                   every root this machine holds
    fts-files-daemon unshare <root>           stop holding one (the files stay)
    fts-files-daemon peer <endpoint-id>       admit a machine, and take what it shares
    fts-files-daemon forget <endpoint-id>     stop admitting a machine, and stop pulling it
    fts-files-daemon resolve <root> <path>    two machines changed it — keep both sides
    fts-files-daemon mount <root> <dir>       show it as a folder; opening fetches (Linux)
    fts-files-daemon unmount <root>           take the folder down (the files stay)
    fts-files-daemon mounts                   what is mounted, and where
    fts-files-daemon place <root> <path>      where it appears, e.g. org/Projects/Name
    fts-files-daemon mount-all <dir> [--flat] compose every root into one tree there
    fts-files-daemon evict <root> <path>      give its bytes back to the disk
    fts-files-daemon fetch <root> <path>      bring one file's bytes back now

install options:
    --coordinator <endpoint-id>   the org endpoint to sync with
    --data <dir>                  store, vault, device identity   [~/.local/share/fts-files]
    --roots <dir>                 where synced projects land      [~/Task]
    --bind <addr>                 control socket                  [127.0.0.1:4055]
    --interval <secs>             reconcile cadence               [30]
    --program <path>              register this binary where it is, do not copy it
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
        Some(
            "status" | "checkpoint" | "share" | "shares" | "unshare" | "peer" | "forget"
            | "resolve" | "mount" | "unmount" | "mounts" | "evict" | "fetch" | "place"
            | "mount-all" | "capture",
        ) => None,
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
    let mut keep_program = false;
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
            // Naming a program is also saying "leave it there": a
            // machine whose internal disk is nearly full wants the
            // binary on the volume it chose, not copied onto the one it
            // is trying to spare.
            "--program" => {
                config.program = PathBuf::from(value()?).canonicalize()?;
                keep_program = true;
            }
            "--dry-run" => dry_run = true,
            other => return Err(format!("unknown option: {other}\n\n{USAGE}").into()),
        }
    }

    // Copy this binary somewhere stable and register *that*, unless it
    // already lives there or the caller named a program explicitly. A
    // unit pointing into `target/debug` works until the next
    // `cargo clean` and then fails at every login, silently.
    let installed = files_daemon::install::ServiceConfig::installed_binary(&home);
    let copy_binary = !keep_program
        && config.program != installed
        && !config.program.starts_with(home.join(".local/bin"))
        && !config.program.starts_with("/usr")
        && !config.program.starts_with("/nix/store")
        // Inside a macOS app bundle the binary is already where it
        // belongs, and copying it out would leave the copy unsigned.
        && !config.program.to_string_lossy().contains(".app/Contents/");
    if copy_binary {
        config.program = installed.clone();
    }

    // Anything not named on this run keeps the value the installed unit
    // already had. Upgrading the binary is the usual reason to re-run
    // `install`, and rewriting the unit from defaults silently undid the
    // pairing done in the app.
    //
    // `--coordinator ""` is the exception, and deliberately: it is how a
    // person says "no org", which otherwise had no spelling at all — the
    // value would be kept forever because keeping is the default.
    let cleared = config.coordinator.as_deref() == Some("");
    if cleared {
        config.coordinator = None;
        // The agent also remembers a coordinator it was told over the
        // socket; clearing means clearing both, or the next start reads
        // the old one back.
        let remembered = config.data_dir.join("daemon").join("coordinator");
        let _ = std::fs::remove_file(remembered);
        println!("clear   coordinator");
    }
    for (key, slot) in [
        ("FTS_FILES_DAEMON_COORDINATOR", &mut config.coordinator),
    ] {
        if slot.is_none() && !cleared {
            *slot = files_daemon::install::configured(&home, key);
        }
    }
    if let Some(kept) = &config.coordinator {
        println!("keep    coordinator {kept}");
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
        // How long ago it last synced, which is the question "is this
        // thing actually running?" — and the one the status surface
        // could not answer. A root sitting at `Idle` looks identical
        // whether it reconciled four seconds ago or has not ticked
        // since the machine woke up.
        let ago = root
            .last_synced_at
            .map(|t| {
                let secs = (chrono::Utc::now() - t).num_seconds().max(0);
                match secs {
                    0..=90 => format!("  {secs}s ago"),
                    91..=5400 => format!("  {}m ago", secs / 60),
                    _ => format!("  {}h ago", secs / 3600),
                }
            })
            .unwrap_or_else(|| "  never".into());
        println!(
            "root       {}  {:?}  {}%{ago}{}{}",
            root.name,
            root.state,
            root.percent(),
            // Where it comes from, each id shortened *before* they are
            // joined: a full endpoint id buries the state a person is
            // scanning for, and shortening the joined list instead cut
            // every peer but the first off the end.
            if root.peers.is_empty() {
                String::new()
            } else {
                format!(
                    "  from {}",
                    root.peers
                        .iter()
                        .map(|p| p[..p.len().min(8)].to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            root.last_error
                .as_deref()
                .map(|e| format!("  ({e})"))
                .unwrap_or_default()
        );
        if let Some(at) = &root.mounted_at {
            println!("    showing as a folder at  {at}");
        }
        // The one thing here that needs a person. Named, not counted:
        // "2 divergent paths" tells somebody there is a problem and
        // nothing about which file, and the next thing they would have
        // to do is go looking.
        for path in &root.divergent {
            println!("    ⚠ two machines changed  {path}");
            println!("      settle it:  fts-files-daemon resolve {} {path}", root.name);
        }
    }
    Ok(())
}

/// Force a save point on a root, which is what "before I unplug" means.
async fn checkpoint(root: &str) -> Result<(), Box<dyn std::error::Error>> {

    let client = control().await?;
    // By id, or by the name the status surface shows — a person reads
    // the name and should be able to type what they read.
    // By id, or by name — and the name is looked up among every root
    // this machine holds, not only the ones it pulls. A folder shared
    // from here is pulled from nowhere, so checking the sync choices
    // alone answered "no synced root called sharetest" about a root the
    // agent was serving at that moment.
    let id = match root.parse::<uuid::Uuid>() {
        Ok(id) => id,
        Err(_) => {
            client
                .shares()
                .await?
                .into_iter()
                .find(|(_, name, _)| name == root)
                .ok_or_else(|| format!("this machine holds no root called {root}"))?
                .0
        }
    };
    client.checkpoint_now(id).await?;
    println!("checkpointed {root}");
    Ok(())
}

/// Share a folder from this machine, through the running agent.
async fn share(
    dir: &str,
    name: Option<String>,
    later: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = std::fs::canonicalize(dir).map_err(|e| format!("{dir}: {e}"))?;
    let client = control().await?;
    let here = path.to_string_lossy().into_owned();
    let root = if later {
        client.share_deferred(here, name).await?
    } else {
        let (id, name) = client.share(here, name).await?;
        println!("sharing {name}  ({id})");
        println!("{}", path.display());
        println!();
        println!("on the other machine:");
        println!("    fts-files-daemon peer {}", endpoint_id()?);
        return Ok(());
    };
    println!("registered {}  ({})", root.name, root.id);
    println!("{}", path.display());
    println!();
    println!("it is browsable now; nothing has read its bytes yet, so it would");
    println!("sync as an empty tree. `fts-files-daemon capture` fills that in.");
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
    let status = client.admit_peer(endpoint_id.to_string()).await?;

    // Where the agent puts adopted roots is the agent's business, and
    // asking beats guessing: the two defaults were the same string
    // written in two places, they drifted the moment `install` chose a
    // different one, and every adoption was then refused as "outside the
    // permitted boundary" while the CLI reported the other machine as
    // sharing nothing.
    let roots = status.roots_dir.clone();
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
            for root in &taken {
                match &root.error {
                    None => println!("syncing {} → {}", root.name, root.path),
                    // Reported, not swallowed. A root that could not be
                    // taken is the whole reason the person ran this.
                    Some(why) => println!("could NOT take {}: {why}", root.name),
                }
            }
        }
        Err(e) if is_not_admitted(&e) => {
            println!();
            println!("that machine has not admitted this one yet, so it will not be read.");
            println!("run this there, then re-run this command:");
            println!("    fts-files-daemon peer {}", endpoint_id_or_unknown());
        }
        // Asleep, or off, or on a network that cannot be reached — which
        // is the *usual* state of the machine somebody is asking to sync
        // with, because that is why they are asking. The intent is kept
        // and retried on the tick rather than thrown away with a
        // timeout.
        Err(e) if is_unreachable(&e) => {
            client.remember_peer(endpoint_id.to_string()).await?;
            println!();
            println!("it is not reachable right now (asleep, off, or on another network).");
            println!("this machine will take what it shares as soon as it answers —");
            println!("nothing more to run.");
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

/// Whether a dial failed because the machine is not there right now.
fn is_unreachable(e: &impl std::fmt::Display) -> bool {
    let text = e.to_string();
    text.contains("timed out") || text.contains("dialling") || text.contains("no route")
}

/// Stop holding a root. The files stay where they are.
async fn unshare(root: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    let (id, name, path) = client
        .shares()
        .await?
        .into_iter()
        .find(|(id, name, _)| name == root || id.to_string() == root)
        .ok_or_else(|| format!("this machine holds no root called {root}"))?;
    client.unshare(id).await?;
    println!("stopped holding {name}");
    if !path.is_empty() {
        println!("{path} is untouched — nothing was deleted.");
    }
    Ok(())
}

/// Settle a path two machines changed, keeping both sides.
async fn resolve(root: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    let id = match root.parse::<uuid::Uuid>() {
        Ok(id) => id,
        Err(_) => {
            client
                .shares()
                .await?
                .into_iter()
                .find(|(_, name, _)| name == root)
                .ok_or_else(|| format!("this machine holds no root called {root}"))?
                .0
        }
    };
    client.keep_both(id, path.to_string()).await?;
    println!("kept both sides of {path}");
    println!("the other side is beside it, named `<stem> (divergent 1).<ext>`.");
    Ok(())
}

/// A root id, from either an id or the name a person actually uses.
async fn root_id(
    client: &files_daemon::DaemonControlServiceClient,
    root: &str,
) -> Result<uuid::Uuid, Box<dyn std::error::Error>> {
    if let Ok(id) = root.parse::<uuid::Uuid>() {
        return Ok(id);
    }
    Ok(client
        .shares()
        .await?
        .into_iter()
        .find(|(_, name, _)| name == root)
        .ok_or_else(|| format!("this machine holds no root called {root}"))?
        .0)
}

/// Show a root as a folder whose files fetch when something opens them.
async fn mount(root: &str, dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    let id = root_id(&client, root).await?;
    client.mount(id, dir.to_string()).await?;
    println!("mounted {root} at {dir}");
    println!("everything is listed at its real size; opening what this machine");
    println!("does not hold fetches it first, so a program sees a slow disk, not a stub.");
    Ok(())
}

/// Take a mount down. The tree on disk is untouched.
async fn unmount(root: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    let id = root_id(&client, root).await?;
    client.unmount(id).await?;
    println!("unmounted {root} — the files are still on disk, in the root's own tree");
    Ok(())
}

/// Capture every root that has never been captured.
///
/// The drain for `share --later`: an archive is browsable the moment it
/// is registered, and this fills in the history behind it, smallest
/// project first so most of it is syncable early.
async fn capture() -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    let done = client.capture_pending().await?;
    if done.is_empty() {
        println!("every root has been captured");
        return Ok(());
    }
    let mut failed = 0;
    for (name, error) in &done {
        match error {
            None => println!("  captured  {name}"),
            Some(why) => {
                failed += 1;
                println!("  {name} — {why}");
            }
        }
    }
    println!("\n{} captured", done.len() - failed);
    if failed > 0 {
        println!("{failed} could not be");
    }
    Ok(())
}

/// Say where a root appears in the tree people are shown.
async fn place(root: &str, at: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    let id = root_id(&client, root).await?;
    client.set_place(id, at.to_string()).await?;
    println!("{root} appears at {at}");
    println!("nothing moved — that is where it shows up, not where it lives.");
    Ok(())
}

/// Compose every root into one tree.
async fn mount_all(under: &str, flat: bool) -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    let outcomes = client.mount_all(under.to_string(), flat).await?;
    if outcomes.is_empty() {
        println!("every root this machine holds is already mounted");
        return Ok(());
    }
    let mut failed = 0;
    for (place, error) in &outcomes {
        match error {
            None => println!("  {under}/{place}"),
            Some(why) => {
                failed += 1;
                println!("  {place} — {why}");
            }
        }
    }
    let mounted = outcomes.len() - failed;
    println!("\n{mounted} mounted under {under}");
    if failed > 0 {
        println!("{failed} could not be — the rest are up regardless");
    }
    Ok(())
}

/// Release one file's bytes, keeping the file.
async fn evict(root: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    let id = root_id(&client, root).await?;
    client.dehydrate(id, path.to_string()).await?;
    println!("evicted {path} — it still lists at its real size");
    println!("opening it through a mount fetches it back; so does `fetch`.");
    Ok(())
}

/// Bring one file's bytes back now, without waiting for an open.
async fn fetch(root: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    let id = root_id(&client, root).await?;
    client.hydrate(id, path.to_string()).await?;
    println!("{path} is resident");
    Ok(())
}

/// What is mounted right now.
async fn mounts() -> Result<(), Box<dyn std::error::Error>> {
    let client = control().await?;
    let at = client.mounts().await?;
    if at.is_empty() {
        println!("nothing is mounted");
        println!("  mount one:  fts-files-daemon mount <root> <dir>");
        return Ok(());
    }
    let names = client.shares().await?;
    for (id, dir) in at {
        let name = names
            .iter()
            .find(|(rid, _, _)| *rid == id)
            .map_or_else(|| id.to_string(), |(_, name, _)| name.clone());
        println!("{name}  →  {dir}");
    }
    Ok(())
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

    // Dropping the peer from every root is the daemon's job — it holds
    // the choices, and doing it here as well would mean two places that
    // have to agree about what "forget" does. This reports the
    // difference; content is untouched either way.
    let after = client.status().await?;
    let mut dropped = 0;
    for root in &before.roots {
        if !root.peers.iter().any(|p| p == endpoint_id) {
            continue;
        }
        dropped += 1;
        match after.roots.iter().find(|r| r.root_id == root.root_id) {
            // Still syncing, with the machines that remain.
            Some(now) => println!(
                "{} now syncs with {} machine(s) (its content stays)",
                root.name,
                now.peers.len()
            ),
            None => println!("stopped syncing {} (its content stays)", root.name),
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

/// Die quietly when the reader goes away, as a Unix tool does.
///
/// Rust ignores SIGPIPE and turns the failed write into a panic, so
/// `fts-files-daemon status | head -4` ends in a backtrace about broken
/// pipes — which looks like the agent crashed and is only `head`
/// closing the pipe it was done with.
#[cfg(unix)]
fn die_quietly_on_broken_pipe() {
    // SAFETY: restoring a signal to its default disposition takes no
    // handler and touches no state of ours.
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

#[cfg(unix)]
const SIGPIPE: i32 = 13;
#[cfg(unix)]
const SIG_DFL: usize = 0;

#[cfg(unix)]
unsafe extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    die_quietly_on_broken_pipe();

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
        Some("shares") => {
            for (id, name, path) in control().await?.shares().await? {
                println!("{id}  {name}");
                if !path.is_empty() {
                    println!("    {path}");
                }
            }
            return Ok(());
        }
        Some("share") => {
            let dir = args.get(1).ok_or("share needs a directory")?.clone();
            let name = args
                .iter()
                .position(|a| a == "--name")
                .and_then(|i| args.get(i + 1))
                .cloned();
            // `--later` registers the root and returns. On an archive
            // that matters: capturing a 210 GB project reads every byte,
            // so adopting thirty of them one after another shows the
            // last one a day after the first.
            let later = args.iter().any(|a| a == "--later");
            return share(&dir, name, later).await;
        }
        Some("capture") => return capture().await,
        Some("peer") => {
            let id = args.get(1).ok_or("peer needs an endpoint id")?.clone();
            return peer(&id).await;
        }
        Some("forget") => {
            let id = args.get(1).ok_or("forget needs an endpoint id")?.clone();
            return forget(&id).await;
        }
        Some("unshare") => {
            let root = args.get(1).ok_or("unshare needs a root id or name")?.clone();
            return unshare(&root).await;
        }
        Some("resolve") => {
            let root = args.get(1).ok_or("resolve needs a root id or name")?.clone();
            let path = args
                .get(2)
                .ok_or("resolve needs the path both machines changed")?
                .clone();
            return resolve(&root, &path).await;
        }
        Some("mount") => {
            let root = args.get(1).ok_or("mount needs a root id or name")?.clone();
            let dir = args
                .get(2)
                .ok_or("mount needs a directory to mount it at")?
                .clone();
            return mount(&root, &dir).await;
        }
        Some("unmount") => {
            let root = args.get(1).ok_or("unmount needs a root id or name")?.clone();
            return unmount(&root).await;
        }
        Some("mounts") => return mounts().await,
        Some("place") => {
            let root = args.get(1).ok_or("place needs a root id or name")?.clone();
            let at = args
                .get(2)
                .ok_or("place needs where it appears, e.g. org/Projects/Name")?
                .clone();
            return place(&root, &at).await;
        }
        Some("mount-all") => {
            let under = args
                .get(1)
                .ok_or("mount-all needs a directory to compose the tree under")?
                .clone();
            // `--flat` drops the org from every place, so the whole
            // studio is one Projects/ and one Assets/ instead of six
            // folders to look through.
            let flat = args.iter().any(|a| a == "--flat");
            return mount_all(&under, flat).await;
        }
        Some("evict") => {
            let root = args.get(1).ok_or("evict needs a root id or name")?.clone();
            let path = args.get(2).ok_or("evict needs a path in that root")?.clone();
            return evict(&root, &path).await;
        }
        Some("fetch") => {
            let root = args.get(1).ok_or("fetch needs a root id or name")?.clone();
            let path = args.get(2).ok_or("fetch needs a path in that root")?.clone();
            return fetch(&root, &path).await;
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
    // Before anything else touches them — see `preflight` on why this is
    // a deadline rather than a plain `create_dir_all`.
    preflight(&[&store, &roots_under]).inspect_err(|e| {
        tracing::error!("{e}");
    })?;
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

    // Before the control socket opens, because it is state the socket
    // hands out and acts on. The socket is bound early on purpose — an
    // agent that answers `status` in 75 seconds is an agent nobody
    // believes is running — but "answers early" must not mean "answers
    // wrong": a `mount-all` that arrived while this was still to come
    // read every root as having no place and mounted thirty-eight
    // projects flat, in the wrong tree, reporting success.
    //
    // This is a file read, so it costs nothing to do first. What stays
    // after the bind is the part that dials peers, where being slow is
    // the network's fault and a caller can see it in the status.
    daemon.restore_places();

    // And bring back the mounts, for the same reason and before the
    // same line. Mounting is local work — it needs the disk, not the
    // network — and a socket that answered "nothing is mounted" while
    // this was still to come told a caller the opposite of the truth
    // about its own machine.
    match daemon.restore_mounts().await {
        0 => {}
        n => tracing::info!(mounts = n, "re-mounted the roots that were mounted"),
    }
    daemon.with_shared_dirs(shared);
    daemon.set_roots_dir(&roots_under);

    // The control socket FIRST, before any of the network setup below.
    //
    // Everything after this line talks to the world — binding an iroh
    // endpoint waits on relay probes, restoring choices dials each
    // remembered peer — and on a real machine that added up to about
    // seventy-five seconds. With the socket bound last, `status` in that
    // window answered "no agent answering — is it running?" about an
    // agent that was running perfectly well, which is a lie told to the
    // person who is trying to find out what is happening. The point of a
    // control surface is to be reachable while the thing it describes is
    // still starting.
    let control = DaemonControl::new(daemon.clone());
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
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "the control socket stopped");
        }
    });

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
    // The unit's environment first, then what the agent was told over
    // its socket the last time somebody paired it from the app.
    match env("FTS_FILES_DAEMON_COORDINATOR").or_else(|| daemon.remembered_coordinator()) {
        None => tracing::warn!(
            "no coordinator — this daemon serves its content but pulls nothing"
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
            // Admit it whether or not it answers right now: admission is
            // this machine's own list, and it has to be in place before
            // the org can pull back.
            daemon.admit_peer(&coordinator);

            // An unreachable org must NOT be fatal. It was: the dial
            // error propagated out of `main`, the process exited 1, and
            // systemd restarted it — every twelve seconds, forever, on
            // a machine whose only problem was that its server was off.
            // A laptop that boots in a café is exactly this case, and
            // the right behaviour there is to sync with the machines it
            // *can* reach and keep trying the one it cannot.
            match daemon.set_coordinator_peer(&coordinator).await {
                Ok(()) => tracing::info!(%coordinator, "coordinator connected (iroh)"),
                Err(e) => {
                    tracing::warn!(
                        %coordinator,
                        error = %e,
                        "coordinator not reachable — will keep trying; other peers are unaffected"
                    );
                    daemon.remember_peer(&coordinator);
                }
            }

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
    // chosen root. This is what the process stays alive for; the control
    // socket has been answering since before any of the above ran.
    let mut timer = tokio::time::interval(interval);
    loop {
        timer.tick().await;
        daemon.tick().await;
    }
}
