//! The arguments `task-server` answers before it is a server at all:
//! `--version`, `--help`, `service …` and `self-update …`.
//!
//! ## Why this runs first
//!
//! `task-server` with no arguments boots the hosted server: it reads the
//! environment, opens every org under `TASK_DATA_ROOT`, starts iroh
//! endpoints and binds `TASK_SERVER_BIND`. Before this module existed,
//! ANY argument that was not `admin …` or `peer …` fell through to that
//! boot — so `task-server --version` in the production pod logged "OTLP
//! export enabled" and started serving instead of printing a version.
//! A question about the binary must never open its data or take a port,
//! so these are answered here, before telemetry, before the runtime does
//! anything, and the process exits.
//!
//! An argument this binary does not know is now an error (exit 2) rather
//! than a silent boot: a typo'd flag starting a second server on a host
//! that already runs one is the same failure as `--version` was.

use std::io::Write as _;

/// The version every artifact of a release reports: the release's, which
/// `main` recorded (crates/release-version), else the committed workspace
/// version.
#[must_use]
pub fn version() -> &'static str {
    task_release_version::get()
}

/// What `--version` prints, exactly (one line). The release check
/// (`scripts/check-versions.sh --bin`) and `self-update` both compare
/// against this shape, so keep it `<name> <version>`.
#[must_use]
pub fn version_line() -> String {
    format!("task-server {}", version())
}

/// `--help`.
pub const USAGE: &str = "\
task-server — a headless Task node.

Task has no central server: every install is a node, and this binary is
one with no window, for a desktop, a home server or a cluster that should
stay on and hold data.

usage:
  task-server                       host the orgs under $TASK_DATA_ROOT (the cluster's mode)
  task-server peer sign-in          sign this node in as your account (approve the code on your phone)
  task-server peer                  run as one more of your devices: serve, and keep in step with them
  task-server peer status           who it is signed in as, and the orgs it holds
  task-server admin …               operator commands on this node's data root (`task-server admin` lists them)
  task-server service write <dir>   write the systemd unit / launchd plist that keeps it running
  task-server service show <file>   print one of those files
  task-server self-update           replace this binary with the newest release (sha256-checked)
  task-server --version | -V        print the version and exit
  task-server --help | -h           print this and exit

environment:
  TASK_DATA_ROOT     the node's data (default ./data)
  TASK_SERVER_BIND   the address it serves on (default 0.0.0.0:9090)

docs: https://github.com/FastTrackStudios/task/blob/main/docs/headless-node.md";

/// What the first argument asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invocation {
    /// No arguments: boot the hosted server.
    Serve,
    /// `--version` / `-V` / `version`.
    Version,
    /// `--help` / `-h` / `help`.
    Help,
    /// `service …`.
    Service,
    /// `self-update …`.
    SelfUpdate,
    /// `admin …` / `peer …` — the other dispatchers own these.
    Delegated,
    /// Anything else: refuse rather than boot.
    Unknown,
}

/// Classify argv (without the program name).
#[must_use]
pub fn classify(args: &[String]) -> Invocation {
    match args.first().map(String::as_str) {
        None => Invocation::Serve,
        Some("--version" | "-V" | "version") => Invocation::Version,
        Some("--help" | "-h" | "help") => Invocation::Help,
        Some("service") => Invocation::Service,
        Some("self-update") => Invocation::SelfUpdate,
        Some("admin" | "peer") => Invocation::Delegated,
        Some(_) => Invocation::Unknown,
    }
}

/// Handle everything this module owns. `Ok(true)` = handled, exit now;
/// `Ok(false)` = boot the server or hand argv to `admin`/`peer`.
pub async fn dispatch() -> eyre::Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match classify(&args) {
        Invocation::Serve | Invocation::Delegated => Ok(false),
        Invocation::Version => {
            // Ignore a closed stdout (`task-server -V | head -0`).
            let _ = writeln!(std::io::stdout(), "{}", version_line());
            Ok(true)
        }
        Invocation::Help => {
            let _ = writeln!(std::io::stdout(), "{USAGE}");
            Ok(true)
        }
        Invocation::Service => crate::service_units::cli(&args[1..]).map(|()| true),
        Invocation::SelfUpdate => crate::self_update::cli(&args[1..]).await.map(|()| true),
        Invocation::Unknown => {
            eprintln!(
                "task-server: unknown argument `{}`\n\n{USAGE}",
                args.first().map(String::as_str).unwrap_or_default()
            );
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn version_and_help_never_reach_the_server() {
        for a in ["--version", "-V", "version"] {
            assert_eq!(classify(&argv(&[a])), Invocation::Version, "{a}");
        }
        for a in ["--help", "-h", "help"] {
            assert_eq!(classify(&argv(&[a])), Invocation::Help, "{a}");
        }
    }

    #[test]
    fn only_a_bare_invocation_boots() {
        assert_eq!(classify(&[]), Invocation::Serve);
        assert_eq!(classify(&argv(&["peer"])), Invocation::Delegated);
        assert_eq!(
            classify(&argv(&["admin", "list-users"])),
            Invocation::Delegated
        );
        // A typo must not start a second server.
        assert_eq!(classify(&argv(&["--verison"])), Invocation::Unknown);
        assert_eq!(classify(&argv(&["serve"])), Invocation::Unknown);
    }

    #[test]
    fn the_version_line_is_name_space_version() {
        assert_eq!(
            version_line(),
            format!("task-server {}", task_release_version::get())
        );
        assert_eq!(version_line().split_whitespace().count(), 2);
    }
}
