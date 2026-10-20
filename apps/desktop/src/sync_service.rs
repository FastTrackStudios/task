//! Making sure the sync agent is installed, from inside the app.
//!
//! The agent (`fts-files-daemon`) is a separate process on purpose: it
//! has to keep syncing when this window is closed, when the app is
//! quit, and when nobody is logged into the desktop session at all. A
//! window that owns the sync engine can only sync while it is open,
//! which is the one property a person notices the absence of — they
//! close the laptop lid, and the mix they bounced is not on the studio
//! machine in the morning.
//!
//! So the app's job is not to *be* the agent. It is to make sure the
//! agent is installed, and then to be an ordinary client of its control
//! socket. This module is the first half.
//!
//! # It shells out rather than linking the daemon in
//!
//! `fts-files-daemon install` already knows how to register itself with
//! launchd or systemd, and it is shipped beside this binary — inside the
//! `.app` on macOS, next to it on Linux. Linking `files-daemon` into the
//! desktop app to call the same function would pull the whole files
//! backend (jj-lib, the CAS store) into a UI process that only needs to
//! ask a question, and would leave two code paths that must agree about
//! what a correct installation is.
//!
//! # It is quiet, and it is opt-out
//!
//! Installing a login agent is a real change to someone's machine, so:
//! it happens once (an already-installed agent is left alone, including
//! one the person configured by hand), it never prompts, it logs what it
//! did, and `TASK_SYNC_AUTOSTART=0` turns it off entirely.

use std::path::PathBuf;
use std::process::Command;

/// The agent binary, as shipped beside this app.
///
/// `Contents/MacOS/fts-files-daemon` in a bundle, a sibling file on
/// Linux — both are "next to the executable", which is why one lookup
/// covers them.
fn agent_binary() -> Option<PathBuf> {
    let here = std::env::current_exe().ok()?;
    let candidate = here.parent()?.join("fts-files-daemon");
    candidate.exists().then_some(candidate)
}

/// Install the sync agent if it is not already installed, and reinstall
/// it (keeping its settings) when this bundle carries a different build.
///
/// Returns what happened, for the log line. Never panics and never
/// blocks the UI thread's start-up path — the caller spawns it.
pub fn ensure_installed() -> String {
    if std::env::var("TASK_SYNC_AUTOSTART").is_ok_and(|v| v == "0") {
        return "sync agent: autostart disabled by TASK_SYNC_AUTOSTART=0".into();
    }
    let Some(agent) = agent_binary() else {
        return "sync agent: not shipped beside this build — background sync is off".into();
    };

    // Ask the agent, not the filesystem: where the unit belongs is its
    // decision, and duplicating that here is how the two drift.
    let stamp = build_stamp(&agent);
    match Command::new(&agent).arg("service-status").output() {
        Ok(out) if String::from_utf8_lossy(&out.stdout).starts_with("installed") => {
            // Installed — but by this build? After an update the unit
            // still runs the agent the old bundle started; re-running
            // `install` keeps its settings and restarts it on this one.
            if stamp.is_none() || read_stamp() == stamp {
                return "sync agent: already installed".into();
            }
        }
        Ok(_) => {}
        Err(e) => return format!("sync agent: could not ask {} ({e})", agent.display()),
    }

    let mut install = Command::new(&agent);
    install.arg("install");
    // The org to sync with, when the environment already says. Without
    // it the agent still installs and still serves this machine's own
    // content — it just pulls nothing until it is given a coordinator,
    // which is a working state rather than a broken one.
    if let Ok(coordinator) = std::env::var("TASK_SYNC_COORDINATOR") {
        if !coordinator.trim().is_empty() {
            install.args(["--coordinator", coordinator.trim()]);
        }
    }
    match install.output() {
        Ok(out) if out.status.success() => {
            if let Some(stamp) = &stamp {
                write_stamp(stamp);
            }
            "sync agent: installed and started on this build".into()
        }
        Ok(out) => format!(
            "sync agent: install failed — {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => format!("sync agent: could not run the installer ({e})"),
    }
}

/// Which build of the agent this bundle carries: its size and mtime,
/// which every build (and so every update) changes.
fn build_stamp(agent: &std::path::Path) -> Option<String> {
    let meta = std::fs::metadata(agent).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!("{} {mtime}", meta.len()))
}

fn stamp_file() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_vendor = "apple") {
                home.join("Library/Application Support")
            } else {
                home.join(".local/share")
            }
        });
    Some(base.join("task").join("agent-build"))
}

fn read_stamp() -> Option<String> {
    std::fs::read_to_string(stamp_file()?)
        .ok()
        .map(|s| s.trim().to_owned())
}

fn write_stamp(stamp: &str) {
    if let Some(file) = stamp_file() {
        let _ = file.parent().map(std::fs::create_dir_all);
        let _ = std::fs::write(file, stamp);
    }
}

/// Hand the app's sign-in to the sync agent, so the mount shows every
/// org the account can reach.
///
/// The app signs in through the central issuer and holds the session;
/// the agent is a separate process with none of that. This is the seam:
/// when the app has a bearer, the agent gets it over the control socket
/// (`sign_in`), enrols this machine with every org the account is in,
/// and composes them into the tree. Called once at startup and again
/// from the tray; a second call with the same token is harmless.
///
/// Says what happened in a sentence, for the log and the tray.
pub async fn sign_agent_in() -> String {
    let Some(token) = task_ui_core::vox_session::bearer() else {
        return "sync agent: nobody is signed in to Task yet".into();
    };
    let server = task_ui_core::vox_session::vox_url();
    let client = match agent().await {
        Ok(client) => client,
        Err(e) => return format!("sync agent: {e}"),
    };
    if let Err(e) = client.sign_in(server, token).await {
        return format!("sync agent: sign-in refused — {e}");
    }
    match client.enroll_now().await {
        Ok(orgs) => {
            let named: Vec<String> = orgs.iter().map(|o| o.slug.clone()).collect();
            let troubled = orgs.iter().filter(|o| o.error.is_some()).count();
            format!(
                "sync agent: signed in — {} org(s): {}{}",
                orgs.len(),
                named.join(" "),
                if troubled > 0 {
                    format!(" ({troubled} with trouble — see the sync page)")
                } else {
                    String::new()
                }
            )
        }
        Err(e) => format!("sync agent: signed in, but enrolment failed — {e}"),
    }
}

/// Tell the agent to forget the account. The orgs it already holds keep
/// syncing until forgotten one by one; what stops is discovering new
/// ones as this account.
pub async fn sign_agent_out() -> String {
    match agent().await {
        Ok(client) => match client.sign_out().await {
            Ok(_) => "sync agent: signed out".into(),
            Err(e) => format!("sync agent: sign-out failed — {e}"),
        },
        Err(e) => format!("sync agent: {e}"),
    }
}

pub(crate) async fn agent()
-> Result<files_daemon_proto::service::DaemonControlServiceClient, String> {
    let bind = std::env::var("FTS_FILES_DAEMON_BIND").unwrap_or_else(|_| "127.0.0.1:4055".into());
    vox::connect_lane(&format!("ws://{bind}/vox"))
        .establish()
        .await
        .map_err(|e| format!("no agent answering on {bind} ({e})"))
}
