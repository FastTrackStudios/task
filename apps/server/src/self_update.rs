//! `task-server self-update`: keep a non-Nix node on the newest release.
//!
//! The same GitHub Releases logic the desktop's updater uses
//! (apps/desktop/src/updater.rs), for the headless binary: ask the
//! releases API for the newest `v<X.Y.Z>` release, and if it is newer than
//! this binary, check the release's `SHA256SUMS` is signed by the release
//! key pinned in this binary ([`RELEASE_SIGNING_KEY`], `SHA256SUMS.minisig`),
//! download this platform's `task-server-<os>-<arch>` asset, check it
//! against those checksums, check the downloaded binary reports the
//! release's version (`--version`), and only then rename it over this
//! one. A check that fails for any reason leaves the installed binary
//! exactly as it was.
//!
//! The signature is what makes the checksums mean anything: the binary
//! and `SHA256SUMS` come from the same release, so whoever can write a
//! release's assets can replace both, and a sha256 check alone is
//! integrity, not authenticity. The signing key is not in GitHub's
//! release storage; the public half is compiled in here.
//!
//! The running node keeps the old inode until it restarts, so replacing
//! the file under it is safe; `--then <command…>` runs a command (the
//! service files use it to restart the node) only when a new binary was
//! actually installed.
//!
//! A binary in the Nix store is never touched: a Nix host follows the
//! release by moving its flake pin (docs/headless-node.md).

use std::io::Write as _;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

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

/// The minisign signature of [`SUMS_ASSET`], made by the release
/// workflow (`.github/workflows/node-binaries.yml`) with the release key.
pub const SIG_ASSET: &str = "SHA256SUMS.minisig";

/// The release signing key: the minisign public key whose secret half
/// signs every release's `SHA256SUMS` (the `TASK_RELEASE_MINISIGN_KEY`
/// Actions secret). `self-update` installs nothing it does not verify
/// against this key, so a release whose assets were swapped by someone
/// who can write releases, but does not hold the key, is refused.
///
/// `None` until the key is provisioned: then nothing is installed, and
/// `self-update` says why. Rotating it is a code change, released like
/// any other; a node moves to the new key by installing a release signed
/// with the old one. The release workflow reads this const to check the
/// secret it signs with is this key's: keep it a `Some("<base64>")` literal.
pub const RELEASE_SIGNING_KEY: Option<&str> = None;

/// The trusted comment the release workflow signs into `SHA256SUMS.minisig`
/// (`minisign -t`). The signature covers it, so a valid signature lifted
/// from an older release onto a newer one names the wrong version and is
/// refused: a rollback to a release with a known hole is not an update.
#[must_use]
pub fn trusted_comment(version: &str) -> String {
    format!("task-server v{version} {SUMS_ASSET}")
}

/// Check `sums` is the checksum file the release key signed for
/// `version`.
///
/// # Errors
///
/// A signature that does not decode, is not by `key`, or names another
/// release.
pub fn verify_sums(key: &str, sums: &[u8], sig: &str, version: &str) -> eyre::Result<()> {
    let key = minisign_verify::PublicKey::from_base64(key)
        .map_err(|e| eyre!("the pinned release key does not decode: {e}"))?;
    let signature = minisign_verify::Signature::decode(sig)
        .map_err(|e| eyre!("v{version}'s {SIG_ASSET} is not a minisign signature: {e}"))?;
    key.verify(sums, &signature, false).map_err(|e| {
        eyre!("v{version}'s {SUMS_ASSET} is not signed by the release key ({e}); nothing changed")
    })?;
    let want = trusted_comment(version);
    if signature.trusted_comment() != want {
        bail!(
            "v{version}'s {SIG_ASSET} was made for `{}`, not `{want}`; nothing changed",
            signature.trusted_comment()
        );
    }
    Ok(())
}

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
    /// `SHA256SUMS.minisig`, when the release carries one. Releases from
    /// before signing do not; they are still reported (`--check`), and
    /// never installed.
    pub sig_url: Option<String>,
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
                sig_url: url(SIG_ASSET),
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
after checking the release's SHA256SUMS is signed by the release key pinned in
this binary, the download against those checksums, and that it reports the
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
    // Bounded, so a stalled mirror ends the run instead of holding the
    // daily unit forever: a dial, a silence between reads, and the whole
    // exchange (a binary is tens of MiB; half an hour is a slow link).
    let http = reqwest::Client::builder()
        .user_agent(concat!(
            "task-server-self-update/",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .timeout(Duration::from_secs(30 * 60))
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

    let Some(key) = RELEASE_SIGNING_KEY else {
        bail!(
            "this build pins no release signing key, so it cannot tell a real release from \
             swapped assets and installs nothing; install a newer release by hand \
             (docs/headless-node.md)"
        );
    };
    let Some(sig_url) = found.sig_url.clone() else {
        bail!(
            "v{} carries no {SIG_ASSET}: only signed releases are installed; nothing changed",
            found.version
        );
    };
    let sums = get(found.sums_url.clone())
        .await?
        .error_for_status()
        .wrap_err("download SHA256SUMS")?
        .bytes()
        .await?;
    let sig = get(sig_url)
        .await?
        .error_for_status()
        .wrap_err_with(|| format!("download {SIG_ASSET}"))?
        .text()
        .await?;
    verify_sums(key, &sums, &sig, &found.version)?;
    let sums = std::str::from_utf8(&sums)
        .map_err(|_| eyre!("v{}'s {SUMS_ASSET} is not text", found.version))?;
    let want = expected_sha256(sums, asset).ok_or_else(|| {
        eyre!(
            "v{}'s {SUMS_ASSET} lists no sha256 for {asset}",
            found.version
        )
    })?;

    // Straight to the staged file, hashing as it lands: the binary is
    // never held in memory whole.
    let mut staged = staging_file(&exe)?;
    let mut response = get(found.binary_url.clone())
        .await?
        .error_for_status()
        .wrap_err_with(|| format!("download {asset}"))?;
    let mut hasher = sha2::Sha256::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .wrap_err_with(|| format!("download {asset}"))?
    {
        hasher.update(&chunk);
        staged
            .write_all(&chunk)
            .wrap_err_with(|| format!("write {}", staged.path().display()))?;
    }
    staged
        .as_file()
        .sync_all()
        .wrap_err_with(|| format!("write {}", staged.path().display()))?;
    let got = hex::encode(hasher.finalize());
    if got != want {
        bail!(
            "{asset} from v{} does not match its sha256 (got {got}, release says {want}); nothing changed",
            found.version
        );
    }

    install(staged, &exe, &found.version)?;
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

/// A new file beside `exe` to download into: in the same directory so the
/// final rename is one atomic step on one filesystem, under a random name
/// created exclusively (`O_EXCL`), so a file or symlink planted at a
/// guessable name fails the create instead of redirecting the write, and
/// owner-only until it has proved itself. Removed when dropped, so every
/// early return cleans up.
fn staging_file(exe: &Path) -> eyre::Result<tempfile::NamedTempFile> {
    let dir = exe
        .parent()
        .ok_or_else(|| eyre!("{} has no directory", exe.display()))?;
    let name = exe
        .file_name()
        .map_or_else(|| "task-server".into(), |n| n.to_string_lossy());
    let prefix = format!(".{name}.update-");
    let mut builder = tempfile::Builder::new();
    builder.prefix(&prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempfile_in(dir).wrap_err_with(|| {
        format!(
            "create a staging file in {} (is it writable by this user?)",
            dir.display()
        )
    })
}

/// Prove the staged binary is the release (`--version` answers
/// `task-server <version>`), then rename it over `exe`. The staged file
/// is removed on any failure.
fn install(staged: tempfile::NamedTempFile, exe: &Path, version: &str) -> eyre::Result<()> {
    // Close the write handle first: Linux refuses to exec a file that is
    // open for writing (ETXTBSY). The path still deletes itself on drop.
    let staged = staged.into_temp_path();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    let out = Command::new(&*staged)
        .arg("--version")
        .output()
        .wrap_err("run the downloaded binary")?;
    let said = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let expected = format!("task-server {version}");
    if !out.status.success() || said != expected {
        bail!("the downloaded binary says `{said}`, not `{expected}`; nothing changed");
    }
    staged
        .persist(exe)
        .map_err(|e| eyre!("replace {}: {}", exe.display(), e.error))?;
    Ok(())
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
        assert_eq!(c.sig_url, None, "an unsigned release is still found");
        let signed = json!([release("v0.2.11", &[A, SUMS_ASSET, SIG_ASSET])]);
        assert_eq!(
            pick(&signed, A, None).unwrap().sig_url.as_deref(),
            Some("https://example/v0.2.11/SHA256SUMS.minisig")
        );
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

    /// A throwaway minisign key (its secret half was discarded) and two
    /// signatures it made over [`SUMS`], one for v9.9.9 and one for v9.9.8.
    const TEST_KEY: &str = "RWRUN7wS6cvax7nVK8QgbSwxJb6P55M+ZNlJ0hTBX3rBaITOw8rqgj6T";
    const SUMS: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  task-server-linux-x86_64\n";
    const SIG_999: &str = "untrusted comment: signature from minisign secret key
RURUN7wS6cvax5cQlQV3dC+3yh2F6SdKL3DdhTxkIIfhuiNQQifK6SbByssBWrUjeIqVBdA4IscSsM3kSXdDUsLLqwmohqMPfAg=
trusted comment: task-server v9.9.9 SHA256SUMS
8JydgklHgR68fyD2Rf64Y1AU1AKfogNbIet0hmJVuJBF+qfBGUR4NhDew6geVZBOjY7WOXskZt2Cmh0FY9+OAw==
";
    const SIG_998: &str = "untrusted comment: signature from minisign secret key
RURUN7wS6cvax5cQlQV3dC+3yh2F6SdKL3DdhTxkIIfhuiNQQifK6SbByssBWrUjeIqVBdA4IscSsM3kSXdDUsLLqwmohqMPfAg=
trusted comment: task-server v9.9.8 SHA256SUMS
TIeWoirwEQq6d9jQVGiSIWZwl0LKU3S9HmhRvL1iyUcoHPuiJx9ShisEh1e0TOKyROLeEZP6rotFyl3XC+pKAQ==
";

    #[test]
    fn the_checksums_install_only_when_the_release_key_signed_them_for_this_version() {
        verify_sums(TEST_KEY, SUMS.as_bytes(), SIG_999, "9.9.9").unwrap();

        // Swapped checksums (someone who can write the release's assets).
        let swapped = SUMS.replace('a', "b");
        let err = verify_sums(TEST_KEY, swapped.as_bytes(), SIG_999, "9.9.9").unwrap_err();
        assert!(
            err.to_string().contains("not signed by the release key"),
            "{err}"
        );

        // A real signature, lifted from an older release.
        let err = verify_sums(TEST_KEY, SUMS.as_bytes(), SIG_998, "9.9.9").unwrap_err();
        assert!(
            err.to_string().contains("made for `task-server v9.9.8"),
            "{err}"
        );

        // The trusted comment edited to match: the global signature covers it.
        let edited = SIG_998.replace("v9.9.8", "v9.9.9");
        assert!(verify_sums(TEST_KEY, SUMS.as_bytes(), &edited, "9.9.9").is_err());

        // Some other key.
        let other = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        assert!(verify_sums(other, SUMS.as_bytes(), SIG_999, "9.9.9").is_err());
        assert!(verify_sums(TEST_KEY, SUMS.as_bytes(), "not a signature", "9.9.9").is_err());
    }

    #[test]
    fn the_pinned_release_key_decodes() {
        if let Some(key) = RELEASE_SIGNING_KEY {
            minisign_verify::PublicKey::from_base64(key).expect("RELEASE_SIGNING_KEY");
        }
        assert_eq!(trusted_comment("0.2.4"), "task-server v0.2.4 SHA256SUMS");
    }

    #[test]
    fn the_release_workflow_signs_what_self_update_verifies() {
        let wf = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/node-binaries.yml"),
        )
        .unwrap();
        assert!(
            wf.contains(SIG_ASSET),
            "node-binaries.yml does not attach {SIG_ASSET}"
        );
        assert!(
            wf.contains(r#"-t "task-server $TAG SHA256SUMS""#),
            "node-binaries.yml signs another trusted comment than `trusted_comment`"
        );
        assert!(
            wf.contains("RELEASE_SIGNING_KEY"),
            "the workflow checks its key against the pinned one"
        );
    }

    /// Stage `bytes` the way the download does.
    fn stage(exe: &Path, bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut staged = staging_file(exe).unwrap();
        staged.write_all(bytes).unwrap();
        staged
    }

    #[cfg(unix)]
    #[test]
    fn the_staging_file_is_new_unguessable_and_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("task-server");
        // What the old code would have written to: a planted symlink there
        // is simply not where the download goes.
        let guessable = dir
            .path()
            .join(format!(".task-server.update-{}", std::process::id()));
        std::os::unix::fs::symlink(dir.path().join("elsewhere"), &guessable).unwrap();
        let a = staging_file(&exe).unwrap();
        let b = staging_file(&exe).unwrap();
        assert_ne!(a.path(), b.path());
        for f in [&a, &b] {
            assert_eq!(f.path().parent(), Some(dir.path()));
            assert_ne!(f.path(), guessable);
            assert!(
                f.path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".task-server.update-")
            );
            let mode = f.as_file().metadata().unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        assert!(!dir.path().join("elsewhere").exists());
        let (pa, pb) = (a.path().to_owned(), b.path().to_owned());
        drop((a, b));
        assert!(
            !pa.exists() && !pb.exists(),
            "dropped staging files are removed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_staged_binary_that_lies_about_its_version_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("task-server");
        std::fs::write(&exe, b"old").unwrap();
        // A "binary" that answers the wrong version.
        let script = b"#!/bin/sh\necho 'task-server 0.0.1'\n";
        let err = install(stage(&exe, script), &exe, "9.9.9")
            .unwrap_err()
            .to_string();
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
        install(stage(&exe, honest), &exe, "9.9.9").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), honest);
    }
}
