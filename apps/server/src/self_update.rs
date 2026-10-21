//! `task-server self-update`: keep a non-Nix node on the newest release.
//!
//! The same GitHub Releases logic the desktop's updater uses
//! (apps/desktop/src/updater.rs), for the headless binary: ask the
//! releases API for the newest `v<X.Y.Z>` release, and if it is newer than
//! this binary, download this platform's `task-server-<os>-<arch>` asset,
//! check it against the release's `SHA256SUMS`, check the downloaded
//! binary reports the release's version (`--version`), and only then
//! rename it over this one. A check that fails for any reason leaves the
//! installed binary exactly as it was.
//!
//! The running node keeps the old inode until it restarts, so replacing
//! the file under it is safe; `--then <command…>` runs a command (the
//! service files use it to restart the node) only when a new binary was
//! actually installed.
//!
//! A binary in the Nix store is never touched: a Nix host follows the
//! release by moving its flake pin (docs/headless-node.md).

use std::path::{Path, PathBuf};
use std::process::Command;

use eyre::{WrapErr as _, bail, eyre};
use sha2::Digest as _;

/// Where releases live (a public repo: no token needed, but
/// `GITHUB_TOKEN` is sent when set, for the rate limit).
pub const REPO: &str = "FastTrackStudios/task";

/// The release asset holding this platform's binary, if releases carry
/// one. Keep in step with `.github/workflows/node-binaries.yml`.
#[must_use]
pub fn asset_name() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("task-server-linux-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("task-server-linux-aarch64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("task-server-macos-aarch64")
    } else {
        None
    }
}

/// The checksum file every release carries beside the binaries.
pub const SUMS_ASSET: &str = "SHA256SUMS";

/// `1.2.3` as numbers; anything unparsable is lowest.
#[must_use]
pub fn parse(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

#[must_use]
pub fn newer(candidate: &str, current: &str) -> bool {
    parse(candidate) > parse(current)
}

fn is_release_tag(tag: &str) -> bool {
    tag.strip_prefix('v').is_some_and(|v| {
        let parts: Vec<&str> = v.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// A release that carries this platform's binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub version: String,
    pub binary_url: String,
    pub sums_url: String,
}

/// Pick from the releases API's answer: the newest `vX.Y.Z` release that
/// is neither a draft nor a prerelease and carries both `asset` and the
/// checksums — or exactly `want`, when given. (`desktop-v*` releases are
/// the desktop updater's and never match.)
#[must_use]
pub fn pick(releases: &serde_json::Value, asset: &str, want: Option<&str>) -> Option<Candidate> {
    let mut best: Option<Candidate> = None;
    for r in releases.as_array().into_iter().flatten() {
        if r["draft"].as_bool() == Some(true) || r["prerelease"].as_bool() == Some(true) {
            continue;
        }
        let Some(tag) = r["tag_name"].as_str().filter(|t| is_release_tag(t)) else {
            continue;
        };
        let version = tag.trim_start_matches('v').to_owned();
        if want.is_some_and(|w| w.trim_start_matches('v') != version) {
            continue;
        }
        let url = |name: &str| {
            r["assets"].as_array().into_iter().flatten().find_map(|a| {
                (a["name"].as_str() == Some(name))
                    .then(|| a["browser_download_url"].as_str().map(str::to_owned))
                    .flatten()
            })
        };
        let (Some(binary_url), Some(sums_url)) = (url(asset), url(SUMS_ASSET)) else {
            continue;
        };
        if best.as_ref().is_none_or(|b| newer(&version, &b.version)) {
            best = Some(Candidate {
                version,
                binary_url,
                sums_url,
            });
        }
    }
    best
}

/// The sha256 `SHA256SUMS` (the `sha256sum` format: `<hex>  <name>`, a
/// `*` before the name in binary mode) lists for `asset`.
#[must_use]
pub fn expected_sha256(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// Parsed `self-update` arguments.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Report only; change nothing.
    pub check: bool,
    /// Install exactly this version (also a downgrade), not the newest.
    pub to: Option<String>,
    /// Run after a new binary is installed (and only then).
    pub then: Vec<String>,
}

const USAGE: &str = "\
usage: task-server self-update [--check] [--to <X.Y.Z>] [--then <command> [args…]]

  --check            say whether a newer release exists; change nothing
  --to <X.Y.Z>       install that release instead of the newest (also downgrades)
  --then <cmd…>      after installing a new binary, run <cmd…> (everything after --then),
                     e.g. `--then systemctl try-restart task-server.service`

Replaces this binary with the newest v* release's task-server for this platform,
after checking it against the release's SHA256SUMS and that it reports the
release's version. Binaries in /nix/store are never replaced (move the flake pin).";

pub fn parse_options(args: &[String]) -> eyre::Result<Options> {
    let mut o = Options::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--check" | "--dry-run" => o.check = true,
            "--to" => {
                i += 1;
                let v = args.get(i).ok_or_else(|| eyre!("--to needs a version"))?;
                if !is_release_tag(&format!("v{}", v.trim_start_matches('v'))) {
                    bail!("--to takes a version like 0.2.3, not `{v}`");
                }
                o.to = Some(v.trim_start_matches('v').to_owned());
            }
            "--then" => {
                o.then = args[i + 1..].to_vec();
                if o.then.is_empty() {
                    bail!("--then needs a command");
                }
                break;
            }
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => bail!("unknown option `{other}`\n\n{USAGE}"),
        }
        i += 1;
    }
    Ok(o)
}

/// `task-server self-update …`.
pub async fn cli(args: &[String]) -> eyre::Result<()> {
    let opts = parse_options(args)?;
    let current = crate::node_cli::VERSION;
    let asset = asset_name().ok_or_else(|| {
        eyre!(
            "releases carry no task-server binary for {}-{}; build it from source",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .wrap_err("find this binary")?;
    if exe.starts_with("/nix/store") {
        bail!(
            "{} is in the Nix store: a Nix host updates by moving its `task` flake input to the new \
             release tag, not in place (docs/headless-node.md)",
            exe.display()
        );
    }

    // The GitHub API over rustls: the graph carries two backends, so one
    // has to be chosen before the first TLS dial.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::builder()
        .user_agent(concat!(
            "task-server-self-update/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()?;
    // The token only ever goes to the API host, never to the asset
    // download's redirect target.
    let token = std::env::var("GITHUB_TOKEN")
        .ok()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty());
    let get = |url: String| {
        let api = url.starts_with("https://api.github.com/");
        let mut req = http.get(url);
        if let (true, Some(t)) = (api, &token) {
            req = req.bearer_auth(t);
        }
        req.send()
    };

    let releases: serde_json::Value = get(format!(
        "https://api.github.com/repos/{REPO}/releases?per_page=30"
    ))
    .await
    .wrap_err("ask GitHub for the releases")?
    .error_for_status()
    .wrap_err("ask GitHub for the releases")?
    .json()
    .await
    .wrap_err("read the releases")?;

    let Some(found) = pick(&releases, asset, opts.to.as_deref()) else {
        match &opts.to {
            Some(v) => bail!("no release v{v} carries {asset} and {SUMS_ASSET}"),
            None => {
                println!("task-server {current}: no release carries {asset} yet");
                return Ok(());
            }
        }
    };
    if opts.to.is_none() && !newer(&found.version, current) {
        println!(
            "task-server {current} is current (newest release: v{})",
            found.version
        );
        return Ok(());
    }
    if opts.to.as_deref() == Some(current) {
        println!("task-server {current} is already installed");
        return Ok(());
    }
    if opts.check {
        println!(
            "update available: task-server {current} -> {}",
            found.version
        );
        return Ok(());
    }

    let sums = get(found.sums_url.clone())
        .await?
        .error_for_status()
        .wrap_err("download SHA256SUMS")?
        .text()
        .await?;
    let want = expected_sha256(&sums, asset).ok_or_else(|| {
        eyre!(
            "v{}'s {SUMS_ASSET} lists no sha256 for {asset}",
            found.version
        )
    })?;
    let bytes = get(found.binary_url.clone())
        .await?
        .error_for_status()
        .wrap_err_with(|| format!("download {asset}"))?
        .bytes()
        .await?;
    let got = hex::encode(sha2::Sha256::digest(&bytes));
    if got != want {
        bail!(
            "{asset} from v{} does not match its sha256 (got {got}, release says {want}); nothing changed",
            found.version
        );
    }

    install(&exe, &bytes, &found.version)?;
    println!("updated task-server {current} -> {}", found.version);

    if let Some((program, rest)) = opts.then.split_first() {
        let status = Command::new(program)
            .args(rest)
            .status()
            .wrap_err_with(|| format!("run `{}`", opts.then.join(" ")))?;
        if !status.success() {
            bail!(
                "`{}` failed after the update ({status})",
                opts.then.join(" ")
            );
        }
    }
    Ok(())
}

/// Stage `bytes` beside `exe`, prove the staged binary is the release
/// (`--version` answers `task-server <version>`), then rename it over
/// `exe` — one atomic step on the same filesystem.
fn install(exe: &Path, bytes: &[u8], version: &str) -> eyre::Result<()> {
    let dir = exe
        .parent()
        .ok_or_else(|| eyre!("{} has no directory", exe.display()))?;
    let staged: PathBuf = dir.join(format!(
        ".{}.update-{}",
        exe.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "task-server".to_owned()),
        std::process::id()
    ));
    let result = (|| -> eyre::Result<()> {
        std::fs::write(&staged, bytes).wrap_err_with(|| {
            format!(
                "write {} (is {} writable by this user?)",
                staged.display(),
                dir.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        }
        let out = Command::new(&staged)
            .arg("--version")
            .output()
            .wrap_err("run the downloaded binary")?;
        let said = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let expected = format!("task-server {version}");
        if !out.status.success() || said != expected {
            bail!("the downloaded binary says `{said}`, not `{expected}`; nothing changed");
        }
        std::fs::rename(&staged, exe).wrap_err_with(|| format!("replace {}", exe.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(tag: &str, assets: &[&str]) -> serde_json::Value {
        json!({
            "tag_name": tag,
            "draft": false,
            "prerelease": false,
            "assets": assets.iter().map(|a| json!({
                "name": a,
                "browser_download_url": format!("https://example/{tag}/{a}"),
            })).collect::<Vec<_>>(),
        })
    }

    const A: &str = "task-server-linux-x86_64";

    #[test]
    fn picks_the_newest_release_carrying_the_binary_and_its_sums() {
        let releases = json!([
            release("desktop-v9.9.9", &[A, SUMS_ASSET]),
            release("v0.2.10", &[A, SUMS_ASSET]),
            release("v0.2.9", &[A, SUMS_ASSET]),
            release("v0.3.0", &[A]), // no sums: skipped
            release("v0.4.0", &["task-server-macos-aarch64", SUMS_ASSET]), // other platform
            release("v1.0.0-rc.1", &[A, SUMS_ASSET]), // not X.Y.Z
        ]);
        let c = pick(&releases, A, None).unwrap();
        assert_eq!(c.version, "0.2.10");
        assert_eq!(
            c.binary_url,
            "https://example/v0.2.10/task-server-linux-x86_64"
        );
        assert_eq!(c.sums_url, "https://example/v0.2.10/SHA256SUMS");
        assert_eq!(pick(&releases, A, Some("0.2.9")).unwrap().version, "0.2.9");
        assert_eq!(pick(&releases, A, Some("v0.2.9")).unwrap().version, "0.2.9");
        assert!(pick(&releases, A, Some("0.3.0")).is_none());
    }

    #[test]
    fn drafts_and_prereleases_are_never_installed() {
        let mut draft = release("v5.0.0", &[A, SUMS_ASSET]);
        draft["draft"] = json!(true);
        let mut pre = release("v4.0.0", &[A, SUMS_ASSET]);
        pre["prerelease"] = json!(true);
        let releases = json!([draft, pre, release("v0.1.0", &[A, SUMS_ASSET])]);
        assert_eq!(pick(&releases, A, None).unwrap().version, "0.1.0");
    }

    #[test]
    fn reads_the_sha256sum_format() {
        let h = "a".repeat(64);
        let sums = format!(
            "{}  task-server-linux-aarch64\n{h}  {A}\n{} *task-server-macos-aarch64\n",
            "b".repeat(64),
            "C".repeat(64)
        );
        assert_eq!(expected_sha256(&sums, A), Some(h));
        assert_eq!(
            expected_sha256(&sums, "task-server-macos-aarch64"),
            Some("c".repeat(64))
        );
        assert_eq!(expected_sha256(&sums, "task-server-windows"), None);
        assert_eq!(expected_sha256("nothex  task-server-linux-x86_64", A), None);
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("0.2.10", "0.2.9"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(!newer("0.2.3", "0.2.3"));
        assert!(!newer("0.2.2", "0.2.3"));
    }

    #[test]
    fn options_parse_and_then_takes_the_rest() {
        let a = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            parse_options(&a(&[
                "--check",
                "--to",
                "v0.2.4",
                "--then",
                "systemctl",
                "try-restart",
                "--x"
            ]))
            .unwrap(),
            Options {
                check: true,
                to: Some("0.2.4".into()),
                then: a(&["systemctl", "try-restart", "--x"]),
            }
        );
        assert!(parse_options(&a(&["--to", "latest"])).is_err());
        assert!(parse_options(&a(&["--then"])).is_err());
        assert!(parse_options(&a(&["--force"])).is_err());
    }

    #[test]
    fn the_release_workflow_builds_this_platforms_asset() {
        // node-binaries.yml names the assets; self-update looks them up by
        // the same names. One list, checked from both ends.
        let wf = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/node-binaries.yml"),
        )
        .unwrap();
        for a in [
            "task-server-linux-x86_64",
            "task-server-linux-aarch64",
            "task-server-macos-aarch64",
        ] {
            assert!(
                wf.contains(&format!("asset: {a}")),
                "node-binaries.yml does not build {a}"
            );
        }
        if let Some(a) = asset_name() {
            assert!(wf.contains(&format!("asset: {a}")));
        }
        assert!(wf.contains(SUMS_ASSET));
    }

    #[cfg(unix)]
    #[test]
    fn a_staged_binary_that_lies_about_its_version_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("task-server");
        std::fs::write(&exe, b"old").unwrap();
        // A "binary" that answers the wrong version.
        let script = b"#!/bin/sh\necho 'task-server 0.0.1'\n";
        let err = install(&exe, script, "9.9.9").unwrap_err().to_string();
        assert!(err.contains("not `task-server 9.9.9`"), "{err}");
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            b"old",
            "the installed binary is untouched"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(leftovers.len(), 1, "the staged file is cleaned up");
        // And one that tells the truth is swapped in.
        let honest = b"#!/bin/sh\necho 'task-server 9.9.9'\n";
        install(&exe, honest, "9.9.9").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), honest);
    }
}
