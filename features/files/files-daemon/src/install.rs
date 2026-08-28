//! Installing the daemon as a **background service** — the thing that
//! keeps syncing when the app window is closed.
//!
//! A sync engine that only runs while a person has an app open is not a
//! sync engine; it is a transfer button. What the product needs is the
//! shape every file-sync tool has: a small agent the operating system
//! starts at login and restarts when it dies, with the UI as an ordinary
//! client of its control socket.
//!
//! So this module writes that agent's registration — a launchd plist on
//! macOS, a systemd **user** unit on Linux — and hands it to the
//! platform's loader. Both point at the running binary's own path, so an
//! app bundle registers the copy inside itself and a `cargo install`
//! registers the one on `PATH`, with nothing to keep in step.
//!
//! # Why a user agent and not a system daemon
//!
//! Because it syncs a person's files, into a person's home directory,
//! under a device identity that lives in their data dir. A system-wide
//! service would run as root over a user's data, need a mechanism to
//! learn which user it was acting for, and hold an identity that outlives
//! the account it belongs to. The cost of a user agent is that it starts
//! at login rather than at boot; `linger` closes that on Linux, and on
//! macOS a LaunchAgent is what every comparable tool ships.
//!
//! # What is testable here, and what is not
//!
//! The rendering is pure and tested: given a config, exactly these bytes
//! at exactly this path. The loading is one process call per platform
//! and is not simulated — a test that asserted we shell out to
//! `launchctl` would assert our own source back to us. [`Plan`] is the
//! seam: it says what would be written and run, so an installer can
//! print it (`--dry-run`) and a test can read it.

use std::path::{Path, PathBuf};

use crate::error::{DaemonError, Result};

/// The service's reverse-DNS name — the same product identity the app
/// bundle carries (`apps/desktop/Dioxus.toml`), suffixed so a person
/// reading `launchctl list` can tell the agent from the app.
pub const SERVICE_LABEL: &str = "app.fasttrackstudio.task.sync";

/// The systemd unit's name on Linux.
pub const UNIT_NAME: &str = "task-sync.service";

/// What the installed service will run, and with what.
///
/// Every field ends up as an environment variable in the unit, because
/// that is what the daemon binary reads — one configuration surface for
/// the service, the shell, and a `docker run`, rather than three.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceConfig {
    /// The binary to run. Defaults to the running one.
    pub program: PathBuf,
    /// The peer to sync with: an org's iroh endpoint id.
    pub coordinator: Option<String>,
    /// Data dir — store, vault, device identity, endpoint key.
    pub data_dir: PathBuf,
    /// Where newly adopted roots land.
    pub roots_dir: PathBuf,
    /// The control socket the app connects to.
    pub bind: String,
    /// Seconds between reconcile ticks.
    pub interval_secs: u64,
}

impl ServiceConfig {
    /// The defaults an installer starts from: this binary, this user's
    /// home, the standard socket.
    pub fn for_this_binary(home: &Path) -> Result<Self> {
        let program = std::env::current_exe()
            .map_err(|e| DaemonError::Io(format!("locating this binary: {e}")))?;
        Ok(Self {
            program,
            coordinator: None,
            data_dir: home.join(".local/share/fts-files"),
            roots_dir: home.join("Task"),
            bind: "127.0.0.1:4055".into(),
            interval_secs: 30,
        })
    }

    fn env_pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = vec![
            (
                "FTS_FILES_DAEMON_DATA",
                self.data_dir.to_string_lossy().into_owned(),
            ),
            (
                "FTS_FILES_DAEMON_ROOTS",
                self.roots_dir.to_string_lossy().into_owned(),
            ),
            ("FTS_FILES_DAEMON_BIND", self.bind.clone()),
            (
                "FTS_FILES_DAEMON_INTERVAL_SECS",
                self.interval_secs.to_string(),
            ),
        ];
        if let Some(c) = &self.coordinator {
            pairs.push(("FTS_FILES_DAEMON_COORDINATOR", c.clone()));
        }
        pairs
    }
}

/// One command an install runs.
///
/// `best_effort` is not laziness: unloading a service that is not loaded
/// and disabling a unit that was never enabled both exit non-zero, and
/// both are the *expected* outcome of a first install. Failing on them
/// would make a fresh machine the error case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub argv: Vec<String>,
    pub best_effort: bool,
}

impl Step {
    fn required<const N: usize>(argv: [&str; N]) -> Self {
        Self {
            argv: argv.iter().map(|s| (*s).to_string()).collect(),
            best_effort: false,
        }
    }

    fn best_effort<const N: usize>(argv: [&str; N]) -> Self {
        Self {
            argv: argv.iter().map(|s| (*s).to_string()).collect(),
            best_effort: true,
        }
    }
}

/// One thing an install does.
///
/// An ordered list rather than "files, then commands", because the order
/// is load-bearing in both directions: systemd cannot stop a unit whose
/// file it can no longer read, and will keep offering one it has not
/// re-read since the file went — so an uninstall is disable, *then*
/// remove, *then* reload, and no arrangement of phases expresses that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Write(PathBuf, String),
    Remove(PathBuf),
    Run(Step),
}

/// What an install would do, in order.
///
/// Separated from doing it so `--dry-run` shows a person exactly what is
/// about to touch their machine — the least an installer owes them —
/// and so the rendering can be asserted without a launchd on the test
/// machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub actions: Vec<Action>,
    /// What a person should know that the commands do not do — lingering
    /// on Linux, and anything else that needs their own authority.
    pub notes: Vec<String>,
}

impl Plan {
    /// The unit file this plan writes, if it writes one. For callers
    /// (and tests) that want to look at what will land.
    #[must_use]
    pub fn written(&self) -> Option<(&Path, &str)> {
        self.actions.iter().find_map(|a| match a {
            Action::Write(path, body) => Some((path.as_path(), body.as_str())),
            _ => None,
        })
    }

    /// Carry the plan out, in order.
    pub fn apply(&self) -> Result<()> {
        for action in &self.actions {
            match action {
                Action::Write(path, body) => {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            DaemonError::Io(format!("creating {}: {e}", parent.display()))
                        })?;
                    }
                    std::fs::write(path, body).map_err(|e| {
                        DaemonError::Io(format!("writing {}: {e}", path.display()))
                    })?;
                }
                Action::Remove(path) => match std::fs::remove_file(path) {
                    // Absent is the goal state, so a missing file is
                    // success rather than an error to explain.
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(DaemonError::Io(format!(
                            "removing {}: {e}",
                            path.display()
                        )));
                    }
                },
                Action::Run(step) => run(step)?,
            }
        }
        Ok(())
    }

    /// The plan as a person would read it, in the order it happens.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut out = String::new();
        for action in &self.actions {
            match action {
                Action::Write(path, _) => {
                    out.push_str(&format!("write   {}\n", path.display()));
                }
                Action::Remove(path) => {
                    out.push_str(&format!("remove  {}\n", path.display()));
                }
                Action::Run(step) => {
                    let tail = if step.best_effort { "   (ok to fail)" } else { "" };
                    out.push_str(&format!("run     {}{tail}\n", step.argv.join(" ")));
                }
            }
        }
        for note in &self.notes {
            out.push_str(&format!("note    {note}\n"));
        }
        out
    }
}

/// Run one step, tolerating a best-effort failure.
fn run(step: &Step) -> Result<()> {
    let Some((program, args)) = step.argv.split_first() else {
        return Ok(());
    };
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| DaemonError::Io(format!("running {program}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let complaint = format!(
        "{} {}: {}{}",
        program,
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim(),
        String::from_utf8_lossy(&output.stdout).trim(),
    );
    if step.best_effort {
        tracing::debug!("{complaint}");
        return Ok(());
    }
    Err(DaemonError::Io(complaint))
}

/// Where the service's registration lives for `home`.
#[must_use]
pub fn unit_path(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/LaunchAgents")
            .join(format!("{SERVICE_LABEL}.plist"))
    } else {
        home.join(".config/systemd/user").join(UNIT_NAME)
    }
}

/// The plan that installs the service for `home`.
///
/// # Errors
///
/// On a platform with neither launchd nor systemd — there is no honest
/// thing to write, and writing a unit nothing will read would report
/// success for a service that never starts.
pub fn install_plan(home: &Path, config: &ServiceConfig) -> Result<Plan> {
    if cfg!(target_os = "macos") {
        Ok(macos_plan(home, config))
    } else if cfg!(target_os = "linux") {
        Ok(linux_plan(home, config))
    } else {
        Err(DaemonError::BadRequest(format!(
            "no background-service integration for {} — run the daemon yourself, or keep the app open",
            std::env::consts::OS
        )))
    }
}

/// The plan that removes it.
pub fn uninstall_plan(home: &Path) -> Result<Plan> {
    let path = unit_path(home);
    if cfg!(target_os = "macos") {
        Ok(Plan {
            actions: vec![
                Action::Run(Step::best_effort([
                    "launchctl",
                    "bootout",
                    &format!("gui/{}/{SERVICE_LABEL}", uid()),
                ])),
                Action::Remove(path),
            ],
            notes: vec!["synced content is left exactly where it is".into()],
        })
    } else if cfg!(target_os = "linux") {
        Ok(Plan {
            // Disable while systemd can still read the unit, remove it,
            // then reload so it stops offering what is no longer there.
            actions: vec![
                Action::Run(Step::best_effort([
                    "systemctl",
                    "--user",
                    "disable",
                    "--now",
                    UNIT_NAME,
                ])),
                Action::Remove(path),
                Action::Run(Step::required(["systemctl", "--user", "daemon-reload"])),
            ],
            notes: vec!["synced content is left exactly where it is".into()],
        })
    } else {
        Err(DaemonError::BadRequest(
            "nothing to uninstall on this platform".into(),
        ))
    }
}

fn uid() -> u32 {
    // `id -u` without spawning anything: the daemon runs as the user
    // that owns the agent, so this is always our own.
    #[cfg(unix)]
    {
        // SAFETY: `getuid` is always safe — it takes no arguments,
        // cannot fail, and reads a value the kernel keeps for us.
        unsafe { libc_getuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn macos_plan(home: &Path, config: &ServiceConfig) -> Plan {
    let path = unit_path(home);
    let log = home.join("Library/Logs/task-sync.log");
    let mut env = String::new();
    for (key, value) in config.env_pairs() {
        env.push_str(&format!(
            "        <key>{key}</key>\n        <string>{}</string>\n",
            xml_escape(&value)
        ));
    }
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{SERVICE_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{program}</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
{env}    </dict>
    <!-- Start at login and stay started: a sync agent that stops when it
         crashes is a sync agent that silently stops syncing. -->
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        program = xml_escape(&config.program.to_string_lossy()),
        log = xml_escape(&log.to_string_lossy()),
    );

    let domain = format!("gui/{}", uid());
    Plan {
        actions: vec![
            Action::Write(path.clone(), plist),
            // Unload first, so re-installing replaces a running agent
            // instead of failing on it. On a fresh machine there is
            // nothing to unload and launchctl says so — expected, hence
            // best-effort.
            Action::Run(Step::best_effort([
                "launchctl",
                "bootout",
                &format!("{domain}/{SERVICE_LABEL}"),
            ])),
            Action::Run(Step::required([
                "launchctl",
                "bootstrap",
                &domain,
                &path.to_string_lossy(),
            ])),
        ],
        notes: vec![format!("logs: {}", log.display())],
    }
}

fn linux_plan(home: &Path, config: &ServiceConfig) -> Plan {
    let path = unit_path(home);
    let env: String = config
        .env_pairs()
        .into_iter()
        .map(|(k, v)| format!("Environment={k}={v}\n"))
        .collect();
    let unit = format!(
        "[Unit]\n\
         Description=Task file sync\n\
         Documentation=https://github.com/FastTrackStudios/task\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={program}\n\
         {env}\
         # A sync agent that stops when it crashes is a sync agent that\n\
         # silently stops syncing.\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        program = config.program.display(),
    );

    Plan {
        actions: vec![
            Action::Write(path, unit),
            // Reload before enabling: systemd enables what it has read,
            // so a freshly written unit is invisible until it re-reads.
            Action::Run(Step::required(["systemctl", "--user", "daemon-reload"])),
            Action::Run(Step::required([
                "systemctl",
                "--user",
                "enable",
                "--now",
                UNIT_NAME,
            ])),
        ],
        notes: vec![
            "a user service runs while you are logged in; `loginctl enable-linger $USER` keeps it running after you log out".into(),
            format!("logs: journalctl --user -u {UNIT_NAME} -f"),
        ],
    }
}

/// Whether the service is registered for `home`.
///
/// Deliberately a file check rather than a query to the loader: what is
/// being asked is "did we install this", and both loaders answer their
/// own question ("is it loaded right now") with a different one.
#[must_use]
pub fn is_installed(home: &Path) -> bool {
    unit_path(home).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ServiceConfig {
        ServiceConfig {
            program: PathBuf::from("/Applications/Task.app/Contents/MacOS/fts-files-daemon"),
            coordinator: Some("k5ncabc".into()),
            data_dir: PathBuf::from("/home/cody/.local/share/fts-files"),
            roots_dir: PathBuf::from("/home/cody/Task"),
            bind: "127.0.0.1:4055".into(),
            interval_secs: 30,
        }
    }

    #[test]
    fn the_unit_carries_every_setting_the_daemon_reads() {
        let home = PathBuf::from("/home/cody");
        let plan = install_plan(&home, &config()).expect("a plan for this platform");
        let (_, body) = plan.written().expect("a unit file");
        for expected in [
            "FTS_FILES_DAEMON_DATA",
            "FTS_FILES_DAEMON_ROOTS",
            "FTS_FILES_DAEMON_BIND",
            "FTS_FILES_DAEMON_INTERVAL_SECS",
            "FTS_FILES_DAEMON_COORDINATOR",
            "k5ncabc",
        ] {
            assert!(body.contains(expected), "{expected} missing from:\n{body}");
        }
    }

    /// A unit that does not come back is the failure mode a person
    /// cannot see: sync stops, nothing says so, and the next thing they
    /// learn is that a file is old.
    #[test]
    fn the_service_restarts_itself() {
        let home = PathBuf::from("/home/cody");
        let plan = install_plan(&home, &config()).unwrap();
        let (_, body) = plan.written().expect("a unit file");
        let restarts = body.contains("Restart=always") || body.contains("<key>KeepAlive</key>");
        assert!(restarts, "the unit does not restart the daemon:\n{body}");
    }

    /// No coordinator is a legitimate install — the machine serves its
    /// own content and is pulled by others — and must not write an
    /// empty variable the daemon would then try to dial.
    #[test]
    fn no_coordinator_writes_no_coordinator_variable() {
        let mut config = config();
        config.coordinator = None;
        let plan = install_plan(Path::new("/home/cody"), &config).unwrap();
        let (_, body) = plan.written().expect("a unit file");
        assert!(!body.contains("FTS_FILES_DAEMON_COORDINATOR"), "{body}");
    }

    #[test]
    fn installing_writes_into_the_given_home_and_nowhere_else() {
        let home = tempfile::tempdir().unwrap();
        let mut config = config();
        config.program = PathBuf::from("/usr/bin/true");
        let plan = install_plan(home.path(), &config).unwrap();
        // Only the write, not the loader commands — this asserts the
        // rendering lands where it should, not that launchd exists on a
        // CI box.
        let (path, body) = plan.written().expect("a unit file");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        assert!(is_installed(home.path()));
        assert!(
            unit_path(home.path()).starts_with(home.path()),
            "the unit was written outside the given home"
        );
    }

    #[test]
    fn a_plan_reads_as_what_it_will_do() {
        let plan = install_plan(Path::new("/home/cody"), &config()).unwrap();
        let described = plan.describe();
        assert!(described.contains("write"), "{described}");
        assert!(described.contains("run"), "{described}");
    }
}
