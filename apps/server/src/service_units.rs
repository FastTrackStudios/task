//! The service files that keep a headless node running: a systemd unit
//! (Linux) and a launchd agent (macOS), each with a companion that runs
//! `task-server self-update` once a day.
//!
//! Rendered by the binary itself (`task-server service write <dir>`) so
//! the paths in them are the ones on the machine that runs it — launchd
//! has no `~`, and a unit pointing at the wrong binary fails quietly at
//! the next boot. The defaults are committed under `deploy/node/`, and a
//! test keeps those copies byte-identical to what this module renders,
//! so the documented files and the rendered ones cannot drift.
//!
//! A node is a peer, not a hub (docs/spec/offline.md, `offline.peers`):
//! the default mode is `task-server peer`, one more of the person's
//! devices that happens to stay on. `--host` renders the hosted mode
//! instead (a bare `task-server`, the cluster pod's shape).

use std::path::{Path, PathBuf};

use eyre::{WrapErr as _, bail, eyre};

/// Which init system the files are for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Init {
    Systemd,
    Launchd,
}

impl Init {
    /// The init system of the platform this binary was built for.
    #[must_use]
    pub fn native() -> Self {
        if cfg!(target_os = "macos") {
            Self::Launchd
        } else {
            Self::Systemd
        }
    }
}

/// How the node runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `task-server peer`: one of the account's devices, always on.
    Peer,
    /// `task-server`: host the orgs under the data root.
    Host,
}

/// The systemd unit's name, and the launchd agent's label.
pub const SYSTEMD_UNIT: &str = "task-server.service";
pub const SYSTEMD_UPDATE_UNIT: &str = "task-server-update.service";
pub const SYSTEMD_UPDATE_TIMER: &str = "task-server-update.timer";
pub const LAUNCHD_LABEL: &str = "app.fasttrackstudio.task-server";
pub const LAUNCHD_UPDATE_LABEL: &str = "app.fasttrackstudio.task-server.update";

const DOCS: &str = "https://github.com/FastTrackStudios/task/blob/main/docs/headless-node.md";

/// Everything the files mention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeService {
    pub init: Init,
    pub mode: Mode,
    /// The `task-server` binary the service runs (and `self-update`
    /// replaces).
    pub binary: PathBuf,
    /// `TASK_DATA_ROOT`.
    pub data_root: PathBuf,
    /// `TASK_SERVER_BIND`.
    pub bind: String,
    /// systemd: the account the node runs as.
    pub user: String,
    /// launchd: where stdout/stderr go.
    pub log_file: PathBuf,
}

impl NodeService {
    /// A system service on Linux: binary in /usr/local/bin, data in
    /// /var/lib/task-server (a systemd `StateDirectory`), user `task`.
    #[must_use]
    pub fn systemd_defaults() -> Self {
        Self {
            init: Init::Systemd,
            mode: Mode::Peer,
            binary: PathBuf::from("/usr/local/bin/task-server"),
            data_root: PathBuf::from("/var/lib/task-server"),
            bind: "0.0.0.0:9090".to_owned(),
            user: "task".to_owned(),
            log_file: PathBuf::new(),
        }
    }

    /// A per-user agent on macOS: everything under `home`, so the node
    /// and its updates need no admin rights.
    #[must_use]
    pub fn launchd_defaults(home: &Path) -> Self {
        Self {
            init: Init::Launchd,
            mode: Mode::Peer,
            binary: home.join(".local/bin/task-server"),
            data_root: home.join("Library/Application Support/task-server"),
            bind: "0.0.0.0:9090".to_owned(),
            user: String::new(),
            log_file: home.join("Library/Logs/task-server.log"),
        }
    }

    /// The defaults for `init` on this machine.
    pub fn defaults_for(init: Init) -> eyre::Result<Self> {
        Ok(match init {
            Init::Systemd => Self::systemd_defaults(),
            Init::Launchd => Self::launchd_defaults(&home()?),
        })
    }

    /// The node's own command line after the binary.
    fn node_args(&self) -> &'static [&'static str] {
        match self.mode {
            Mode::Peer => &["peer"],
            Mode::Host => &[],
        }
    }

    /// Every file for this init system, as `(file name, contents)`.
    #[must_use]
    pub fn files(&self) -> Vec<(&'static str, String)> {
        match self.init {
            Init::Systemd => vec![
                (SYSTEMD_UNIT, self.systemd_unit()),
                (SYSTEMD_UPDATE_UNIT, self.systemd_update_unit()),
                (SYSTEMD_UPDATE_TIMER, systemd_update_timer()),
            ],
            Init::Launchd => vec![
                (
                    "app.fasttrackstudio.task-server.plist",
                    self.launchd_plist(),
                ),
                (
                    "app.fasttrackstudio.task-server.update.plist",
                    self.launchd_update_plist(),
                ),
            ],
        }
    }

    /// The node itself, as a systemd system service.
    #[must_use]
    pub fn systemd_unit(&self) -> String {
        let bin = systemd_quote(&self.binary.to_string_lossy());
        let exec = std::iter::once(bin)
            .chain(self.node_args().iter().map(|a| (*a).to_owned()))
            .collect::<Vec<_>>()
            .join(" ");
        let root = self.data_root.to_string_lossy();
        // /var/lib/<name> is what StateDirectory= manages: systemd creates
        // it owned by the service user. Anywhere else the operator makes
        // the directory (the docs say how).
        let state_dir = root
            .strip_prefix("/var/lib/")
            .filter(|rest| !rest.is_empty())
            .map(|rest| format!("StateDirectory={rest}\nStateDirectoryMode=0700\n"))
            .unwrap_or_default();
        let what = match self.mode {
            Mode::Peer => "one more of your devices, always on",
            Mode::Host => "hosting the orgs under its data root",
        };
        // Say how to prepare it where the operator will read it: the user
        // and data root first, and — for a peer — the account it belongs
        // to, without which it can do nothing.
        let first = format!(
            "#\n\
             # Before the first start (as root): create its user and data root{and_sign_in}:\n\
             #   useradd --system --home-dir {root_q} --shell /usr/sbin/nologin {user}\n\
             #   install -d -o {user} -g {user} -m 0700 {root_q}\n\
             {sign_in}",
            and_sign_in = match self.mode {
                Mode::Peer =>
                    ", then sign it in\n# as your account (a code to approve on your phone)",
                Mode::Host => "",
            },
            sign_in = match self.mode {
                Mode::Peer => format!(
                    "#   sudo -u {user} env TASK_DATA_ROOT={root_q} {bin_q} peer sign-in\n",
                    user = self.user,
                    root_q = shell_quote(&root),
                    bin_q = shell_quote(&self.binary.to_string_lossy()),
                ),
                Mode::Host => String::new(),
            },
            user = self.user,
            root_q = shell_quote(&root),
        );
        format!(
            "# A headless Task node: {what}.\n\
             # Rendered by `task-server service write`; see {DOCS}\n\
             {first}\
             \n\
             [Unit]\n\
             Description=Task node (headless)\n\
             Documentation={DOCS}\n\
             After=network-online.target\n\
             Wants=network-online.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             User={user}\n\
             Group={user}\n\
             Environment={env_root}\n\
             Environment={env_bind}\n\
             Environment=RUST_LOG=task_server=info\n\
             {state_dir}\
             ExecStart={exec}\n\
             Restart=on-failure\n\
             RestartSec=5s\n\
             NoNewPrivileges=true\n\
             PrivateTmp=true\n\
             ProtectSystem=strict\n\
             ProtectHome=read-only\n\
             ReadWritePaths={root_unit}\n\
             \n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            user = self.user,
            env_root = systemd_quote(&format!("TASK_DATA_ROOT={root}")),
            env_bind = systemd_quote(&format!("TASK_SERVER_BIND={}", self.bind)),
            root_unit = systemd_quote(&root),
        )
    }

    /// The daily update: `self-update`, then a restart of the node only
    /// if the binary changed (`--then` runs only after a replacement).
    ///
    /// It runs as root, because it replaces a root-owned binary and asks
    /// systemd to restart the node, so it is walled in to what that takes:
    /// the only writable path is the binary's directory, no privilege can
    /// be gained, and a run that hangs is stopped.
    #[must_use]
    pub fn systemd_update_unit(&self) -> String {
        let bin_dir = self
            .binary
            .parent()
            .map_or_else(|| "/".into(), Path::to_string_lossy);
        format!(
            "# Keeps the Task node on the newest release: once a day (task-server-update.timer)\n\
             # `self-update` checks the GitHub releases, verifies the release's signature, the\n\
             # binary's sha256 and that it reports the release's version, swaps it in, and only\n\
             # then restarts the node. Root, sandboxed: it can write only the binary's directory.\n\
             # Rendered by `task-server service write`; see {DOCS}\n\
             \n\
             [Unit]\n\
             Description=Update the Task node from GitHub Releases\n\
             Documentation={DOCS}\n\
             After=network-online.target\n\
             Wants=network-online.target\n\
             \n\
             [Service]\n\
             Type=oneshot\n\
             ExecStart={bin} self-update --then systemctl try-restart {SYSTEMD_UNIT}\n\
             TimeoutStartSec=45min\n\
             NoNewPrivileges=true\n\
             PrivateTmp=true\n\
             PrivateDevices=true\n\
             ProtectSystem=strict\n\
             ProtectHome=read-only\n\
             ReadWritePaths={bin_dir}\n\
             ProtectKernelTunables=true\n\
             ProtectKernelModules=true\n\
             ProtectKernelLogs=true\n\
             ProtectControlGroups=true\n\
             RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6\n\
             RestrictNamespaces=true\n\
             RestrictSUIDSGID=true\n\
             LockPersonality=true\n",
            bin = systemd_quote(&self.binary.to_string_lossy()),
            bin_dir = systemd_quote(&bin_dir),
        )
    }

    /// The node itself, as a launchd agent of the logged-in user.
    #[must_use]
    pub fn launchd_plist(&self) -> String {
        let mut args = vec![self.binary.to_string_lossy().into_owned()];
        args.extend(self.node_args().iter().map(|a| (*a).to_owned()));
        let log = self.log_file.to_string_lossy();
        plist(&[
            ("Label", Value::Str(LAUNCHD_LABEL.to_owned())),
            ("ProgramArguments", Value::Array(args)),
            (
                "EnvironmentVariables",
                Value::Dict(vec![
                    (
                        "TASK_DATA_ROOT".to_owned(),
                        self.data_root.to_string_lossy().into_owned(),
                    ),
                    ("TASK_SERVER_BIND".to_owned(), self.bind.clone()),
                    ("RUST_LOG".to_owned(), "task_server=info".to_owned()),
                ]),
            ),
            ("RunAtLoad", Value::Bool(true)),
            ("KeepAlive", Value::Bool(true)),
            ("ThrottleInterval", Value::Int(5)),
            ("ProcessType", Value::Str("Background".to_owned())),
            ("StandardOutPath", Value::Str(log.clone().into_owned())),
            ("StandardErrorPath", Value::Str(log.into_owned())),
        ])
    }

    /// The daily update for the launchd agent. `launchctl kickstart -k`
    /// restarts the node in place; `--then` runs it only after a swap.
    #[must_use]
    pub fn launchd_update_plist(&self) -> String {
        let log = self.log_file.to_string_lossy();
        plist(&[
            ("Label", Value::Str(LAUNCHD_UPDATE_LABEL.to_owned())),
            (
                "ProgramArguments",
                Value::Array(vec![
                    self.binary.to_string_lossy().into_owned(),
                    "self-update".to_owned(),
                    "--then".to_owned(),
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    format!("/bin/launchctl kickstart -k gui/$(/usr/bin/id -u)/{LAUNCHD_LABEL}"),
                ]),
            ),
            // Once a day, and once at login, so a laptop that was asleep
            // when the day's check fell due still catches up.
            ("StartInterval", Value::Int(86_400)),
            ("RunAtLoad", Value::Bool(true)),
            ("ProcessType", Value::Str("Background".to_owned())),
            ("StandardOutPath", Value::Str(log.clone().into_owned())),
            ("StandardErrorPath", Value::Str(log.into_owned())),
        ])
    }
}

/// Once a day, at a randomised time, and catch up after downtime.
#[must_use]
pub fn systemd_update_timer() -> String {
    format!(
        "# Runs task-server-update.service once a day.\n\
         # Rendered by `task-server service write`; see {DOCS}\n\
         \n\
         [Unit]\n\
         Description=Check daily for a new Task node release\n\
         Documentation={DOCS}\n\
         \n\
         [Timer]\n\
         OnCalendar=daily\n\
         RandomizedDelaySec=1h\n\
         Persistent=true\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n"
    )
}

/// A value in a launchd property list.
enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    Array(Vec<String>),
    Dict(Vec<(String, String)>),
}

fn plist(entries: &[(&str, Value)]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
    );
    out.push_str(&format!(
        "<!-- A headless Task node. Rendered by `task-server service write`; see {DOCS} -->\n"
    ));
    out.push_str("<plist version=\"1.0\">\n<dict>\n");
    for (key, value) in entries {
        out.push_str(&format!("  <key>{}</key>\n", xml_escape(key)));
        match value {
            Value::Str(s) => out.push_str(&format!("  <string>{}</string>\n", xml_escape(s))),
            Value::Int(n) => out.push_str(&format!("  <integer>{n}</integer>\n")),
            Value::Bool(b) => out.push_str(if *b { "  <true/>\n" } else { "  <false/>\n" }),
            Value::Array(items) => {
                out.push_str("  <array>\n");
                for item in items {
                    out.push_str(&format!("    <string>{}</string>\n", xml_escape(item)));
                }
                out.push_str("  </array>\n");
            }
            Value::Dict(pairs) => {
                out.push_str("  <dict>\n");
                for (k, v) in pairs {
                    out.push_str(&format!(
                        "    <key>{}</key>\n    <string>{}</string>\n",
                        xml_escape(k),
                        xml_escape(v)
                    ));
                }
                out.push_str("  </dict>\n");
            }
        }
    }
    out.push_str("</dict>\n</plist>\n");
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// One word of a systemd `ExecStart=`/`Environment=`/path setting. Plain
/// words pass through; anything with a space, quote, backslash, `$` or
/// `%` is double-quoted with those escaped (`$` → `$$`, `%` → `%%`, the
/// two expansions systemd applies itself).
fn systemd_quote(s: &str) -> String {
    let escaped = s.replace('%', "%%").replace('$', "$$");
    if escaped
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\'))
    {
        format!("\"{}\"", escaped.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        escaped
    }
}

/// A POSIX-shell word (for the comment that tells you what to run).
fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | ':' | '='))
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn home() -> eyre::Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| eyre!("HOME is not set"))
}

const SERVICE_USAGE: &str = "\
usage:
  task-server service write <dir> [options]   write the files for this platform into <dir>
  task-server service show <file> [options]   print one file
  task-server service list [options]          the file names

options:
  --for systemd|launchd   which init system (default: this platform's)
  --binary <path>         the task-server the service runs     [systemd: /usr/local/bin/task-server,
                                                                launchd: ~/.local/bin/task-server]
  --data-root <dir>       TASK_DATA_ROOT                       [systemd: /var/lib/task-server,
                                                                launchd: ~/Library/Application Support/task-server]
  --bind <addr>           TASK_SERVER_BIND                     [0.0.0.0:9090]
  --user <name>           systemd: the account it runs as      [task]
  --log <file>            launchd: stdout/stderr               [~/Library/Logs/task-server.log]
  --host                  host orgs (`task-server`) instead of running as your device (`task-server peer`)
  --force                 write: replace files that already exist (default: refuse, change nothing)

systemd:  sudo task-server service write /etc/systemd/system && sudo systemctl daemon-reload
          sudo systemctl enable --now task-server.service task-server-update.timer
launchd:  task-server service write ~/Library/LaunchAgents
          launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/app.fasttrackstudio.task-server.plist
          launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/app.fasttrackstudio.task-server.update.plist";

/// Parse `service …` options into a [`NodeService`] (the defaults for the
/// chosen init system, then overrides). Returns the remaining positional
/// arguments.
pub fn parse_options(
    args: &[String],
    defaults: impl Fn(Init) -> eyre::Result<NodeService>,
) -> eyre::Result<(NodeService, Vec<String>)> {
    let mut init = Init::native();
    let mut positional = Vec::new();
    let mut overrides: Vec<(&str, String)> = Vec::new();
    let mut host = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| eyre!("{name} needs a value"))
        };
        match a.as_str() {
            "--for" => {
                init = match value("--for")?.as_str() {
                    "systemd" => Init::Systemd,
                    "launchd" => Init::Launchd,
                    other => bail!("--for takes systemd or launchd, not `{other}`"),
                }
            }
            "--binary" => overrides.push(("binary", value("--binary")?)),
            "--data-root" => overrides.push(("data-root", value("--data-root")?)),
            "--bind" => overrides.push(("bind", value("--bind")?)),
            "--user" => overrides.push(("user", value("--user")?)),
            "--log" => overrides.push(("log", value("--log")?)),
            "--host" => host = true,
            flag if flag.starts_with("--") => bail!("unknown option {flag}\n\n{SERVICE_USAGE}"),
            _ => positional.push(a.clone()),
        }
    }
    let mut svc = defaults(init)?;
    for (key, v) in overrides {
        match key {
            "binary" => svc.binary = absolute(&v)?,
            "data-root" => svc.data_root = absolute(&v)?,
            "bind" => {
                v.parse::<std::net::SocketAddr>().wrap_err_with(|| {
                    format!("--bind `{v}` is not an address like 0.0.0.0:9090")
                })?;
                svc.bind = v;
            }
            "user" => svc.user = v,
            "log" => svc.log_file = absolute(&v)?,
            _ => unreachable!(),
        }
    }
    if host {
        svc.mode = Mode::Host;
    }
    Ok((svc, positional))
}

/// Service managers want absolute paths; resolve a relative one against
/// the current directory rather than writing something that only works
/// from here.
fn absolute(p: &str) -> eyre::Result<PathBuf> {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

/// `task-server service …`.
pub fn cli(args: &[String]) -> eyre::Result<()> {
    let Some(verb) = args.first() else {
        println!("{SERVICE_USAGE}");
        return Ok(());
    };
    // `--force` only means something to `write`; take it out before the
    // shared options are parsed.
    let force = args[1..].iter().any(|a| a == "--force");
    let rest: Vec<String> = args[1..]
        .iter()
        .filter(|a| *a != "--force")
        .cloned()
        .collect();
    let (svc, positional) = parse_options(&rest, NodeService::defaults_for)?;
    match verb.as_str() {
        "list" => {
            for (name, _) in svc.files() {
                println!("{name}");
            }
        }
        "show" => {
            let want = positional
                .first()
                .ok_or_else(|| eyre!("show needs a file name\n\n{SERVICE_USAGE}"))?;
            let (_, body) = svc
                .files()
                .into_iter()
                .find(|(name, _)| name == want)
                .ok_or_else(|| {
                    eyre!("no file `{want}` for this init system (`task-server service list`)")
                })?;
            print!("{body}");
        }
        "write" => {
            let dir = positional
                .first()
                .ok_or_else(|| eyre!("write needs a directory\n\n{SERVICE_USAGE}"))?;
            for path in write_files(Path::new(dir), &svc.files(), force)? {
                println!("wrote {}", path.display());
            }
        }
        "-h" | "--help" | "help" => println!("{SERVICE_USAGE}"),
        other => bail!("unknown service command `{other}`\n\n{SERVICE_USAGE}"),
    }
    Ok(())
}

/// Write `files` into `dir`. Without `force`, a file that already exists
/// stops the run before anything is written: a unit someone edited by
/// hand (a different user, a tightened sandbox) is not silently replaced.
/// New files are created exclusively, so a symlink at the name is not
/// followed.
fn write_files(dir: &Path, files: &[(&str, String)], force: bool) -> eyre::Result<Vec<PathBuf>> {
    use std::io::Write as _;
    std::fs::create_dir_all(dir).wrap_err_with(|| format!("create {}", dir.display()))?;
    let paths: Vec<PathBuf> = files.iter().map(|(name, _)| dir.join(name)).collect();
    if !force {
        let existing: Vec<String> = paths
            .iter()
            .filter(|p| p.symlink_metadata().is_ok())
            .map(|p| p.display().to_string())
            .collect();
        if !existing.is_empty() {
            bail!(
                "{} already exist{}; nothing written (compare with `task-server service show <file>`, \
                 or pass --force to replace)",
                existing.join(", "),
                if existing.len() == 1 { "s" } else { "" }
            );
        }
    }
    for (path, (_, body)) in paths.iter().zip(files) {
        let mut open = std::fs::OpenOptions::new();
        open.write(true);
        if force {
            open.create(true).truncate(true);
        } else {
            open.create_new(true);
        }
        open.open(path)
            .and_then(|mut f| f.write_all(body.as_bytes()))
            .wrap_err_with(|| format!("write {}", path.display()))?;
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `deploy/node/` holds the defaults as committed files; they must be
    /// exactly what the binary renders, or the docs describe a unit that
    /// `service write` would not produce.
    fn committed(rel: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/node")
            .join(rel);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    #[test]
    fn the_committed_systemd_files_are_what_the_binary_renders() {
        for (name, body) in NodeService::systemd_defaults().files() {
            assert_eq!(
                committed(&format!("systemd/{name}")),
                body,
                "deploy/node/systemd/{name} drifted from `task-server service show {name} --for systemd`; \
                 regenerate it with `task-server service write deploy/node/systemd --for systemd --force`"
            );
        }
    }

    #[test]
    fn the_committed_launchd_files_are_what_the_binary_renders() {
        // Committed with a placeholder home; `service write` fills the real one.
        for (name, body) in NodeService::launchd_defaults(Path::new("/Users/you")).files() {
            assert_eq!(
                committed(&format!("launchd/{name}")),
                body,
                "deploy/node/launchd/{name} drifted from the rendered agent; regenerate it with \
                 `HOME=/Users/you task-server service write deploy/node/launchd --for launchd --force`"
            );
        }
    }

    #[test]
    fn the_systemd_unit_runs_the_node_as_a_peer_in_its_data_root() {
        let unit = NodeService::systemd_defaults().systemd_unit();
        assert!(
            unit.contains("\nExecStart=/usr/local/bin/task-server peer\n"),
            "{unit}"
        );
        assert!(unit.contains("\nEnvironment=TASK_DATA_ROOT=/var/lib/task-server\n"));
        assert!(unit.contains("\nEnvironment=TASK_SERVER_BIND=0.0.0.0:9090\n"));
        assert!(unit.contains("\nStateDirectory=task-server\n"));
        assert!(unit.contains("\nReadWritePaths=/var/lib/task-server\n"));
        assert!(unit.contains("\nUser=task\n"));
        assert!(unit.contains("[Install]\nWantedBy=multi-user.target\n"));
    }

    #[test]
    fn host_mode_runs_the_bare_binary_and_a_custom_root_has_no_state_directory() {
        let args: Vec<String> = [
            "--host",
            "--data-root",
            "/srv/task node",
            "--bind",
            "127.0.0.1:8080",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let (svc, rest) = parse_options(&args, |_| Ok(NodeService::systemd_defaults())).unwrap();
        assert!(rest.is_empty());
        let unit = svc.systemd_unit();
        assert!(
            unit.contains("\nExecStart=/usr/local/bin/task-server\n"),
            "{unit}"
        );
        assert!(!unit.contains("StateDirectory="), "{unit}");
        assert!(
            unit.contains("\nEnvironment=\"TASK_DATA_ROOT=/srv/task node\"\n"),
            "{unit}"
        );
        assert!(
            unit.contains("\nReadWritePaths=\"/srv/task node\"\n"),
            "{unit}"
        );
        assert!(unit.contains("TASK_SERVER_BIND=127.0.0.1:8080"));
    }

    #[test]
    fn the_update_unit_restarts_the_node_only_through_self_update() {
        let unit = NodeService::systemd_defaults().systemd_update_unit();
        assert!(unit.contains(
            "\nExecStart=/usr/local/bin/task-server self-update --then systemctl try-restart task-server.service\n"
        ), "{unit}");
        assert!(unit.contains("Type=oneshot"));
        assert!(systemd_update_timer().contains("OnCalendar=daily"));
    }

    #[test]
    fn the_update_unit_is_sandboxed_to_the_binarys_directory() {
        let unit = NodeService::systemd_defaults().systemd_update_unit();
        for line in [
            "NoNewPrivileges=true",
            "PrivateTmp=true",
            "ProtectSystem=strict",
            "ReadWritePaths=/usr/local/bin",
            "TimeoutStartSec=45min",
        ] {
            assert!(unit.contains(&format!("\n{line}\n")), "{line}:\n{unit}");
        }
        let mut svc = NodeService::systemd_defaults();
        svc.binary = PathBuf::from("/opt/task node/bin/task-server");
        assert!(
            svc.systemd_update_unit()
                .contains("\nReadWritePaths=\"/opt/task node/bin\"\n")
        );
    }

    #[test]
    fn service_write_refuses_to_replace_files_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            ("a.service", "new a\n".to_owned()),
            ("b.timer", "new b\n".to_owned()),
        ];
        std::fs::write(dir.path().join("b.timer"), "edited by hand\n").unwrap();
        let err = write_files(dir.path(), &files, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("b.timer already exists"), "{err}");
        assert!(
            !dir.path().join("a.service").exists(),
            "nothing is written when one file is refused"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.timer")).unwrap(),
            "edited by hand\n"
        );

        write_files(dir.path(), &files, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.timer")).unwrap(),
            "new b\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.service")).unwrap(),
            "new a\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn service_write_does_not_follow_a_planted_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere");
        std::os::unix::fs::symlink(&target, dir.path().join("a.service")).unwrap();
        let files = vec![("a.service", "x\n".to_owned())];
        assert!(write_files(dir.path(), &files, false).is_err());
        assert!(!target.exists());
    }

    #[test]
    fn systemd_words_escape_what_systemd_would_expand() {
        assert_eq!(systemd_quote("/usr/bin/x"), "/usr/bin/x");
        assert_eq!(systemd_quote("/a b"), "\"/a b\"");
        assert_eq!(systemd_quote("100%"), "100%%");
        assert_eq!(systemd_quote("$HOME"), "$$HOME");
    }

    #[test]
    fn the_launchd_agent_is_valid_xml_with_absolute_paths() {
        let svc = NodeService::launchd_defaults(Path::new("/Users/sam & co"));
        let plist = svc.launchd_plist();
        assert!(plist.contains("<string>/Users/sam &amp; co/.local/bin/task-server</string>\n    <string>peer</string>"), "{plist}");
        assert!(plist.contains("<key>KeepAlive</key>\n  <true/>"));
        assert!(plist.contains(
            "<key>TASK_DATA_ROOT</key>\n    <string>/Users/sam &amp; co/Library/Application Support/task-server</string>"
        ));
        assert!(
            !plist.contains(" & "),
            "an unescaped ampersand breaks the plist"
        );
        let update = svc.launchd_update_plist();
        assert!(update.contains("<string>self-update</string>\n    <string>--then</string>"));
        assert!(
            update.contains("kickstart -k gui/$(/usr/bin/id -u)/app.fasttrackstudio.task-server<")
        );
        assert!(update.contains("<key>StartInterval</key>\n  <integer>86400</integer>"));
    }

    #[test]
    fn relative_paths_become_absolute_and_bad_options_are_refused() {
        let args: Vec<String> = ["--binary", "bin/task-server"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let (svc, _) = parse_options(&args, |_| Ok(NodeService::systemd_defaults())).unwrap();
        assert!(svc.binary.is_absolute());
        let bad: Vec<String> = ["--bind", "nine-oh-nine-oh"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert!(parse_options(&bad, |_| Ok(NodeService::systemd_defaults())).is_err());
        let unknown: Vec<String> = ["--frobnicate"].iter().map(|s| (*s).to_owned()).collect();
        assert!(parse_options(&unknown, |_| Ok(NodeService::systemd_defaults())).is_err());
    }
}
