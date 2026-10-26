//! Keeping the installed app current from GitHub Releases.
//!
//! Every product release `vX.Y.Z` of FastTrackStudios/task carries the
//! desktop builds beside the node binaries, one manifest per platform:
//!
//! | platform | the update | its manifest | built by |
//! |---|---|---|---|
//! | macOS | `Task-macos.zip` (the notarized, stapled app; `Task-macos.dmg` is the first install) | `latest-macos.json` | `macos/build-dmg.sh`, uploaded by `macos/release.sh` |
//! | Linux | `Task-linux-x86_64.AppImage` | `latest-linux.json` | `.github/workflows/desktop-linux.yml` |
//!
//! A manifest says the build's version, where the download is, and its
//! sha256. Releases before 26.10 published the macOS app on its own
//! `desktop-vX.Y.Z` tag; those still parse, so discovery reads both.
//!
//! In the background — at launch, then every few hours — the app asks
//! the releases API for the newest release carrying ITS platform's
//! manifest, and if it is newer than what runs, downloads the update and
//! checks its sha256. On macOS it then unpacks the zip and checks the app
//! inside is signed by our team and accepted by Gatekeeper. Only then is
//! it staged. A staged update is installed the next time the app starts
//! (the old copy is swapped out before the window opens), or at once from
//! the tray's "Check for updates". A check that fails for any reason
//! leaves the installed app exactly as it was.
//!
//! Only an app running as the thing it would replace updates itself: a
//! `.app` bundle on macOS, an AppImage (`$APPIMAGE`) on Linux. A `dx
//! serve` or `cargo run` build has nothing to replace, and the nix
//! package (NixOS hosts, from cachix) updates through the fleet.
//!
//! Release discovery and version order are `architect::release`'s (the
//! same as `task-server self-update`). Its signed install does not fit a
//! desktop app: it swaps one bare binary checked by a minisign-signed
//! `SHA256SUMS`, while an update here is a whole notarized `.app` (or a
//! self-contained AppImage), staged and swapped after the process exits.
//! That half stays in this file.

use architect::release::{self, GithubRelease};

/// The tag prefixes desktop builds are published under: the product
/// release `vX.Y.Z`, and `desktop-vX.Y.Z`, which carried the macOS app
/// until 26.10.
const TAG_PREFIXES: [&str; 2] = ["v", "desktop-v"];

/// The asset that describes this platform's build in a release.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const LINUX_MANIFEST: &str = "latest-linux.json";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MACOS_MANIFEST: &str = "latest-macos.json";
#[cfg(target_os = "linux")]
const MANIFEST_ASSET: &str = LINUX_MANIFEST;
#[cfg(target_os = "macos")]
const MANIFEST_ASSET: &str = MACOS_MANIFEST;

/// A published desktop release and where its manifest is.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
struct DesktopRelease {
    version: String,
    manifest_url: String,
}

/// What a release says about one platform's build (`latest-*.json`).
/// macOS names the download `zip`, Linux `appimage`.
#[derive(Debug, serde::Deserialize)]
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
struct Manifest {
    version: String,
    #[serde(alias = "zip", alias = "appimage")]
    url: String,
    sha256: String,
}

/// The release version a tag names, if it is a desktop-carrying tag
/// (`vX.Y.Z` or `desktop-vX.Y.Z`).
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
fn tag_version(tag: &str) -> Option<&str> {
    TAG_PREFIXES
        .iter()
        .find_map(|prefix| tag.strip_prefix(prefix))
        .filter(|v| release::is_release_version(v))
}

/// The newest release that is neither a draft nor a prerelease and
/// carries `manifest_asset` — so a release still waiting for this
/// platform's build is passed over until it lands.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
fn newest_release(releases: &[GithubRelease], manifest_asset: &str) -> Option<DesktopRelease> {
    releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            let version = tag_version(&r.tag_name)?;
            let manifest = r.assets.iter().find(|a| a.name == manifest_asset)?;
            Some(DesktopRelease {
                version: version.to_owned(),
                manifest_url: manifest.browser_download_url.clone(),
            })
        })
        .reduce(|best, r| {
            if release::newer(&r.version, &best.version) {
                r
            } else {
                best
            }
        })
}

/// One check: stage the newest release if it is newer than what runs.
/// `Ok(Some(v))` when `v` is staged (now or before). The check is one
/// `updater/check` span; its outcome and version are fields on it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn check() -> Result<Option<String>, String> {
    use architect_telemetry::wide;

    let span = tracing::info_span!("updater/check");
    let _entered = span.enter();
    wide::set("updater.platform", std::env::consts::OS);
    let checked = imp::check_once();
    wide::set(
        "updater.outcome",
        match &checked {
            Ok(Some(_)) => "staged",
            Ok(None) => "up_to_date",
            Err(_) => "failed",
        },
    );
    if let Ok(Some(version)) = &checked {
        wide::set("updater.version", version.clone());
    }
    checked
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::Duration;

    use architect::release::{self, newer};

    use super::{DesktopRelease, MANIFEST_ASSET, Manifest, newest_release};

    /// Where releases live (a public repo: no token needed).
    const REPO: &str = "FastTrackStudios/task";
    /// The team every update must be signed by.
    const TEAM_ID: &str = "28C2G63DA7";
    /// How often a running app looks again.
    const EVERY: Duration = Duration::from_secs(4 * 60 * 60);

    /// The version staged and ready to install, if any.
    static STAGED: Mutex<Option<String>> = Mutex::new(None);

    /// The running app's bundle (`…/Task.app`), if it runs from one.
    pub fn bundle() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let bundle = exe.ancestors().nth(3)?.to_path_buf();
        (bundle.extension().is_some_and(|e| e == "app")
            && bundle.join("Contents/Info.plist").is_file())
        .then_some(bundle)
    }

    fn plist_value(plist: &Path, key: &str) -> Option<String> {
        let out = Command::new("/usr/bin/plutil")
            .args(["-extract", key, "raw", "-o", "-"])
            .arg(plist)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    /// The running bundle's `CFBundleShortVersionString`.
    pub fn current_version() -> Option<String> {
        plist_value(
            &bundle()?.join("Contents/Info.plist"),
            "CFBundleShortVersionString",
        )
    }

    fn cache_dir() -> PathBuf {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        home.join("Library/Caches/app.fasttrackstudio.task/updates")
    }

    fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
        let out = Command::new("/usr/bin/curl")
            .args([
                "-fsSL",
                "--max-time",
                "600",
                "-H",
                "User-Agent: task-desktop-updater",
            ])
            .args(args)
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(format!(
                "curl {}: {}",
                args.last().unwrap_or(&""),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// The newest published release carrying the macOS build.
    fn newest_published() -> Result<Option<DesktopRelease>, String> {
        let body = curl(&[&format!(
            "{}/repos/{REPO}/releases?per_page=30",
            release::GITHUB_API
        )])?;
        let releases = release::parse_releases(&body).map_err(|e| format!("releases: {e}"))?;
        Ok(newest_release(&releases, MANIFEST_ASSET))
    }

    fn sha256(path: &Path) -> Result<String, String> {
        let out = Command::new("/usr/bin/shasum")
            .args(["-a", "256"])
            .arg(path)
            .output()
            .map_err(|e| format!("shasum: {e}"))?;
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .map(str::to_owned)
            .ok_or_else(|| "shasum gave nothing".to_owned())
    }

    /// The app at `app` is ours: a valid signature, our team, and
    /// Gatekeeper's acceptance (notarized).
    fn trusted(app: &Path) -> Result<(), String> {
        let verify = Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app)
            .output()
            .map_err(|e| format!("codesign: {e}"))?;
        if !verify.status.success() {
            return Err(format!(
                "the update's signature does not verify: {}",
                String::from_utf8_lossy(&verify.stderr).trim()
            ));
        }
        let info = Command::new("/usr/bin/codesign")
            .args(["-dv", "--verbose=2"])
            .arg(app)
            .output()
            .map_err(|e| format!("codesign: {e}"))?;
        let said = String::from_utf8_lossy(&info.stderr);
        if !said
            .lines()
            .any(|l| l.trim() == format!("TeamIdentifier={TEAM_ID}"))
        {
            return Err("the update is not signed by FastTrackStudio's team".to_owned());
        }
        let gate = Command::new("/usr/sbin/spctl")
            .args(["--assess", "--type", "execute"])
            .arg(app)
            .output()
            .map_err(|e| format!("spctl: {e}"))?;
        if !gate.status.success() {
            return Err(format!(
                "Gatekeeper refuses the update: {}",
                String::from_utf8_lossy(&gate.stderr).trim()
            ));
        }
        Ok(())
    }

    /// The app a staged version unpacked to (whatever the bundle is
    /// named); a path that does not exist when nothing is staged.
    fn staged_app(version: &str) -> PathBuf {
        let dir = cache_dir().join(version);
        std::fs::read_dir(&dir)
            .ok()
            .and_then(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| p.extension().is_some_and(|x| x == "app") && p.is_dir())
            })
            .unwrap_or_else(|| dir.join("Task.app"))
    }

    /// See [`super::check`], which wraps this in its span.
    pub fn check_once() -> Result<Option<String>, String> {
        let Some(current) = current_version() else {
            return Ok(None);
        };
        let Some(DesktopRelease {
            version,
            manifest_url,
        }) = newest_published()?
        else {
            return Ok(None);
        };
        if !newer(&version, &current) {
            return Ok(None);
        }
        if staged_app(&version).is_dir() {
            *STAGED.lock().map_err(|e| e.to_string())? = Some(version.clone());
            return Ok(Some(version));
        }
        let manifest: Manifest = serde_json::from_slice(&curl(&[&manifest_url])?)
            .map_err(|e| format!("manifest: {e}"))?;
        if manifest.version != version {
            return Err(format!(
                "release {version} describes itself as {}",
                manifest.version
            ));
        }
        let dir = cache_dir().join(&version);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cache: {e}"))?;
        let zip = dir.join("Task-macos.zip");
        let zip_s = zip.to_string_lossy().into_owned();
        curl(&["-o", &zip_s, &manifest.url])?;
        let got = sha256(&zip)?;
        if !got.eq_ignore_ascii_case(manifest.sha256.trim()) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!(
                "the download's sha256 is {got}, not {}",
                manifest.sha256
            ));
        }
        let unpack = Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(&zip)
            .arg(&dir)
            .status()
            .map_err(|e| format!("ditto: {e}"))?;
        let app = staged_app(&version);
        if !unpack.success() || !app.is_dir() {
            let _ = std::fs::remove_dir_all(&dir);
            return Err("the download did not unpack to an app".to_owned());
        }
        if let Err(e) = trusted(&app) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        let _ = std::fs::remove_file(&zip);
        *STAGED.lock().map_err(|e| e.to_string())? = Some(version.clone());
        Ok(Some(version))
    }

    /// The version staged for install, if any.
    pub fn staged() -> Option<String> {
        STAGED.lock().ok().and_then(|s| s.clone())
    }

    /// Replace the running bundle with the staged one once this process
    /// has exited, then open it. The swap is a detached shell: the app
    /// cannot replace its own bundle while it runs from it.
    pub fn install_and_relaunch(version: &str) -> Result<(), String> {
        let bundle = bundle().ok_or("not running from an app bundle")?;
        let staged = staged_app(version);
        if !staged.is_dir() {
            return Err(format!("no staged update {version}"));
        }
        let pid = std::process::id();
        let q = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"));
        let (b, s) = (q(&bundle), q(&staged));
        let old = q(&bundle.with_extension("app.old"));
        let script = format!(
            "while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; \
             rm -rf {old}; mv {b} {old} && mv {s} {b} && rm -rf {old}; \
             open {b}"
        );
        Command::new("/bin/sh")
            .args(["-c", &script])
            .spawn()
            .map_err(|e| format!("start the installer: {e}"))?;
        Ok(())
    }

    /// At launch, before the window: install an update staged by an
    /// earlier run, and say whether this process should exit for it.
    pub fn install_staged_at_launch() -> bool {
        let Some(current) = current_version() else {
            return false;
        };
        let Ok(entries) = std::fs::read_dir(cache_dir()) else {
            return false;
        };
        let best = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|v| newer(v, &current) && staged_app(v).is_dir())
            .reduce(|best, v| if newer(&v, &best) { v } else { best });
        let Some(version) = best else {
            // Nothing newer: clear what earlier installs left.
            let _ = std::fs::remove_dir_all(cache_dir());
            return false;
        };
        match trusted(&staged_app(&version)).and_then(|()| install_and_relaunch(&version)) {
            Ok(()) => {
                tracing::info!(%version, "updater: installing the staged update");
                true
            }
            Err(e) => {
                tracing::warn!(%version, %e, "updater: the staged update was not installed");
                let _ = std::fs::remove_dir_all(cache_dir().join(&version));
                false
            }
        }
    }

    /// Check now and then every few hours, off the main thread.
    pub fn start() {
        if bundle().is_none() {
            tracing::info!("updater: not running from an app bundle — off");
            return;
        }
        std::thread::spawn(|| {
            loop {
                if let Err(e) = super::check() {
                    tracing::warn!(%e, "updater: check failed");
                }
                std::thread::sleep(EVERY);
            }
        });
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::Duration;

    use architect::release::{self, newer};

    use super::{DesktopRelease, MANIFEST_ASSET, Manifest, newest_release};

    /// Where releases live (a public repo: no token needed).
    const REPO: &str = "FastTrackStudios/task";
    /// How often a running app looks again.
    const EVERY: Duration = Duration::from_secs(4 * 60 * 60);
    /// What a staged update is called inside its version's cache dir.
    const STAGED_NAME: &str = "Task.AppImage";

    /// The version staged and ready to install, if any.
    static STAGED: Mutex<Option<String>> = Mutex::new(None);

    /// The AppImage this process runs from: the AppImage runtime sets
    /// `$APPIMAGE` to its path. Absent for a nix or cargo build, which
    /// has nothing here to replace.
    pub fn appimage() -> Option<PathBuf> {
        std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .filter(|p| p.is_file())
    }

    /// The running build's version (the release's; crates/release-version).
    fn current_version() -> &'static str {
        task_release_version::get()
    }

    fn cache_dir() -> PathBuf {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        cache.join("app.fasttrackstudio.task/updates")
    }

    /// Where a staged version's AppImage is; a path that does not exist
    /// when nothing is staged.
    fn staged_file(version: &str) -> PathBuf {
        cache_dir().join(version).join(STAGED_NAME)
    }

    fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
        let out = Command::new("curl")
            .args([
                "-fsSL",
                "--max-time",
                "900",
                "-H",
                "User-Agent: task-desktop-updater",
            ])
            .args(args)
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(format!(
                "curl {}: {}",
                args.last().unwrap_or(&""),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// The newest published release carrying the Linux build.
    fn newest_published() -> Result<Option<DesktopRelease>, String> {
        let body = curl(&[&format!(
            "{}/repos/{REPO}/releases?per_page=30",
            release::GITHUB_API
        )])?;
        let releases = release::parse_releases(&body).map_err(|e| format!("releases: {e}"))?;
        Ok(newest_release(&releases, MANIFEST_ASSET))
    }

    fn sha256(path: &Path) -> Result<String, String> {
        let out = Command::new("sha256sum")
            .arg(path)
            .output()
            .map_err(|e| format!("sha256sum: {e}"))?;
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .map(str::to_owned)
            .ok_or_else(|| "sha256sum gave nothing".to_owned())
    }

    /// See [`super::check`], which wraps this in its span.
    pub fn check_once() -> Result<Option<String>, String> {
        if appimage().is_none() {
            return Ok(None);
        }
        let Some(DesktopRelease {
            version,
            manifest_url,
        }) = newest_published()?
        else {
            return Ok(None);
        };
        if !newer(&version, current_version()) {
            return Ok(None);
        }
        if staged_file(&version).is_file() {
            *STAGED.lock().map_err(|e| e.to_string())? = Some(version.clone());
            return Ok(Some(version));
        }
        let manifest: Manifest = serde_json::from_slice(&curl(&[&manifest_url])?)
            .map_err(|e| format!("manifest: {e}"))?;
        if manifest.version != version {
            return Err(format!(
                "release {version} describes itself as {}",
                manifest.version
            ));
        }
        let dir = cache_dir().join(&version);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cache: {e}"))?;
        // Downloaded under a temporary name, so an interrupted download is
        // never mistaken for a staged update.
        let part = dir.join(format!("{STAGED_NAME}.part"));
        let part_s = part.to_string_lossy().into_owned();
        curl(&["-o", &part_s, &manifest.url])?;
        let got = sha256(&part)?;
        if !got.eq_ignore_ascii_case(manifest.sha256.trim()) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!(
                "the download's sha256 is {got}, not {}",
                manifest.sha256
            ));
        }
        let staged = staged_file(&version);
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755))
            .and_then(|()| std::fs::rename(&part, &staged))
            .map_err(|e| {
                let _ = std::fs::remove_dir_all(&dir);
                format!("stage: {e}")
            })?;
        *STAGED.lock().map_err(|e| e.to_string())? = Some(version.clone());
        Ok(Some(version))
    }

    /// The version staged for install, if any.
    pub fn staged() -> Option<String> {
        STAGED.lock().ok().and_then(|s| s.clone())
    }

    /// Replace the running AppImage with the staged one once this process
    /// has exited, then start it. The copy lands beside the target first
    /// and is renamed over it, so the AppImage is never half-written; the
    /// cache may be on another filesystem.
    pub fn install_and_relaunch(version: &str) -> Result<(), String> {
        let target = appimage().ok_or("not running as an AppImage")?;
        let staged = staged_file(version);
        if !staged.is_file() {
            return Err(format!("no staged update {version}"));
        }
        let pid = std::process::id();
        let q = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"));
        let (t, s) = (q(&target), q(&staged));
        let new = q(&target.with_extension("AppImage.new"));
        let dir = q(&cache_dir().join(version));
        let script = format!(
            "while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; \
             cp {s} {new} && chmod 755 {new} && mv -f {new} {t} && rm -rf {dir}; \
             {t} </dev/null >/dev/null 2>&1 &"
        );
        Command::new("/bin/sh")
            .args(["-c", &script])
            .spawn()
            .map_err(|e| format!("start the installer: {e}"))?;
        Ok(())
    }

    /// At launch, before the window: install an update staged by an
    /// earlier run, and say whether this process should exit for it.
    pub fn install_staged_at_launch() -> bool {
        if appimage().is_none() {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(cache_dir()) else {
            return false;
        };
        let best = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|v| newer(v, current_version()) && staged_file(v).is_file())
            .reduce(|best, v| if newer(&v, &best) { v } else { best });
        let Some(version) = best else {
            // Nothing newer: clear what earlier installs left.
            let _ = std::fs::remove_dir_all(cache_dir());
            return false;
        };
        match install_and_relaunch(&version) {
            Ok(()) => {
                tracing::info!(%version, "updater: installing the staged update");
                true
            }
            Err(e) => {
                tracing::warn!(%version, %e, "updater: the staged update was not installed");
                let _ = std::fs::remove_dir_all(cache_dir().join(&version));
                false
            }
        }
    }

    /// Check now and then every few hours, off the main thread.
    pub fn start() {
        if appimage().is_none() {
            tracing::info!("updater: not running as an AppImage — off");
            return;
        }
        std::thread::spawn(|| {
            loop {
                if let Err(e) = super::check() {
                    tracing::warn!(%e, "updater: check failed");
                }
                std::thread::sleep(EVERY);
            }
        });
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use imp::{install_and_relaunch, install_staged_at_launch, staged, start};

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn install_staged_at_launch() -> bool {
    false
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn start() {}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn staged() -> Option<String> {
    None
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn check() -> Result<Option<String>, String> {
    Ok(None)
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn install_and_relaunch(_version: &str) -> Result<(), String> {
    Err("updates install on macOS and Linux (AppImage) only".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn published(tag: &str, assets: &[&str]) -> GithubRelease {
        GithubRelease {
            tag_name: tag.to_owned(),
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|a| release::GithubAsset {
                    name: (*a).to_owned(),
                    browser_download_url: format!("https://example/{tag}/{a}"),
                })
                .collect(),
        }
    }

    fn found(tag: &str, version: &str, manifest: &str) -> Option<DesktopRelease> {
        Some(DesktopRelease {
            version: version.into(),
            manifest_url: format!("https://example/{tag}/{manifest}"),
        })
    }

    #[test]
    fn the_product_release_carrying_the_manifest_wins() {
        let releases = [
            published("v26.10.1", &[MACOS_MANIFEST, LINUX_MANIFEST]),
            published("desktop-v0.2.4", &[MACOS_MANIFEST]),
            published("v26.10.0", &[MACOS_MANIFEST, LINUX_MANIFEST]),
        ];
        assert_eq!(
            newest_release(&releases, MACOS_MANIFEST),
            found("v26.10.1", "26.10.1", MACOS_MANIFEST)
        );
    }

    #[test]
    fn legacy_desktop_v_releases_still_parse() {
        let releases = [
            published("desktop-v0.0.10", &[MACOS_MANIFEST]),
            published("desktop-v0.0.9", &[MACOS_MANIFEST]),
            published("v0.0.1", &["task-node-linux-x86_64"]), // a server-only release
        ];
        assert_eq!(
            newest_release(&releases, MACOS_MANIFEST),
            found("desktop-v0.0.10", "0.0.10", MACOS_MANIFEST)
        );
    }

    #[test]
    fn each_platform_reads_only_its_own_manifest() {
        // The macOS build has landed on 26.10.1; the Linux one has not yet.
        let releases = [
            published("v26.10.1", &[MACOS_MANIFEST, "Task-macos.zip"]),
            published("v26.10.0", &[MACOS_MANIFEST, LINUX_MANIFEST]),
        ];
        assert_eq!(
            newest_release(&releases, MACOS_MANIFEST),
            found("v26.10.1", "26.10.1", MACOS_MANIFEST)
        );
        assert_eq!(
            newest_release(&releases, LINUX_MANIFEST),
            found("v26.10.0", "26.10.0", LINUX_MANIFEST)
        );
    }

    #[test]
    fn drafts_prereleases_and_odd_tags_are_skipped() {
        let mut draft = published("v27.1.0", &[LINUX_MANIFEST]);
        draft.draft = true;
        let mut pre = published("v27.0.0", &[LINUX_MANIFEST]);
        pre.prerelease = true;
        let releases = [
            draft,
            pre,
            published("vnext", &[LINUX_MANIFEST]),
            published("node-v30.0.0", &[LINUX_MANIFEST]),
            published("v26.10.0", &[LINUX_MANIFEST]),
        ];
        assert_eq!(
            newest_release(&releases, LINUX_MANIFEST),
            found("v26.10.0", "26.10.0", LINUX_MANIFEST)
        );
        assert_eq!(newest_release(&[], LINUX_MANIFEST), None);
    }

    #[test]
    fn both_manifest_shapes_read() {
        let mac: Manifest = serde_json::from_str(
            r#"{"version":"26.10.1","build":"7","zip":"https://x/Task-macos.zip","sha256":"ab"}"#,
        )
        .expect("macOS manifest");
        assert_eq!(mac.url, "https://x/Task-macos.zip");
        let linux: Manifest = serde_json::from_str(
            r#"{"version":"26.10.1","appimage":"https://x/Task-linux-x86_64.AppImage","sha256":"cd"}"#,
        )
        .expect("Linux manifest");
        assert_eq!(linux.url, "https://x/Task-linux-x86_64.AppImage");
        assert_eq!(linux.version, "26.10.1");
    }
}
