//! Keeping the installed app current from GitHub Releases.
//!
//! The direct-download build (`macos/build-dmg.sh`) is published as a
//! GitHub release of FastTrackStudios/task tagged `desktop-vX.Y.Z`
//! (`macos/release.sh`), carrying the DMG for a first install and, for
//! this module, `Task-macos.zip` (the notarized, stapled app) and
//! `latest-macos.json` (its version and sha256).
//!
//! In the background — at launch, then every few hours — the app asks
//! the releases API for the newest `desktop-v*` release, and if it is
//! newer than the running bundle, downloads the zip, checks its sha256,
//! unpacks it, and checks the app inside is signed by our team and
//! accepted by Gatekeeper. Only then is it staged. A staged update is
//! installed the next time the app starts (the old bundle is swapped
//! out before the window opens), or at once from the tray's "Restart
//! to update". A check that fails for any reason leaves the installed
//! app exactly as it was.
//!
//! macOS only, and only for an app running from a `.app` bundle: a `dx
//! serve` or `cargo run` build has nothing to replace.

#[cfg(target_os = "macos")]
mod imp {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Where releases live (a public repo: no token needed).
    const REPO: &str = "FastTrackStudios/task";
    /// The release tag prefix desktop builds use.
    const TAG_PREFIX: &str = "desktop-v";
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
        plist_value(&bundle()?.join("Contents/Info.plist"), "CFBundleShortVersionString")
    }

    /// `1.2.3` as numbers; anything unparsable is lowest.
    pub fn parse(v: &str) -> Vec<u64> {
        v.trim()
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .take(3)
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    }

    pub fn newer(candidate: &str, current: &str) -> bool {
        parse(candidate) > parse(current)
    }

    fn cache_dir() -> PathBuf {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        home.join("Library/Caches/app.fasttrackstudio.task/updates")
    }

    fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
        let out = Command::new("/usr/bin/curl")
            .args(["-fsSL", "--max-time", "600", "-H", "User-Agent: task-desktop-updater"])
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

    /// What a release says about itself (`latest-macos.json`).
    #[derive(serde::Deserialize)]
    struct Manifest {
        version: String,
        zip: String,
        sha256: String,
    }

    /// The newest published `desktop-v*` release's manifest URL.
    fn newest_manifest_url() -> Result<Option<(String, String)>, String> {
        let body = curl(&[&format!(
            "https://api.github.com/repos/{REPO}/releases?per_page=30"
        )])?;
        let releases: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| format!("releases: {e}"))?;
        let mut best: Option<(String, String)> = None;
        for r in releases.as_array().into_iter().flatten() {
            if r["draft"].as_bool() == Some(true) || r["prerelease"].as_bool() == Some(true) {
                continue;
            }
            let Some(tag) = r["tag_name"].as_str().filter(|t| t.starts_with(TAG_PREFIX)) else {
                continue;
            };
            let version = tag.trim_start_matches(TAG_PREFIX).to_owned();
            let Some(url) = r["assets"].as_array().into_iter().flatten().find_map(|a| {
                (a["name"].as_str() == Some("latest-macos.json"))
                    .then(|| a["browser_download_url"].as_str().map(str::to_owned))
                    .flatten()
            }) else {
                continue;
            };
            if best.as_ref().is_none_or(|(v, _)| newer(&version, v)) {
                best = Some((version, url));
            }
        }
        Ok(best)
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

    /// One check: stage the newest release if it is newer than what runs.
    /// `Ok(Some(v))` when `v` is staged (now or before).
    pub fn check() -> Result<Option<String>, String> {
        let Some(current) = current_version() else {
            return Ok(None);
        };
        let Some((version, manifest_url)) = newest_manifest_url()? else {
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
        curl(&["-o", &zip_s, &manifest.zip])?;
        let got = sha256(&zip)?;
        if !got.eq_ignore_ascii_case(manifest.sha256.trim()) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("the download's sha256 is {got}, not {}", manifest.sha256));
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
        tracing::info!(%version, "updater: update staged — installs on the next start");
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
            .max_by(|a, b| parse(a).cmp(&parse(b)));
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
        std::thread::spawn(|| loop {
            match check() {
                Ok(Some(v)) => tracing::info!(version = %v, "updater: update ready"),
                Ok(None) => tracing::debug!("updater: up to date"),
                Err(e) => tracing::warn!(%e, "updater: check failed"),
            }
            std::thread::sleep(EVERY);
        });
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn versions_compare_as_numbers() {
            assert!(newer("0.0.10", "0.0.9"));
            assert!(newer("0.1.0", "0.0.99"));
            assert!(!newer("0.0.3", "0.0.3"));
            assert!(!newer("0.0.2", "0.0.3"));
            assert!(newer("v1.0.0", "0.9.9"));
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::{check, install_and_relaunch, install_staged_at_launch, staged, start};

#[cfg(not(target_os = "macos"))]
pub fn install_staged_at_launch() -> bool {
    false
}
#[cfg(not(target_os = "macos"))]
pub fn start() {}
#[cfg(not(target_os = "macos"))]
pub fn staged() -> Option<String> {
    None
}
#[cfg(not(target_os = "macos"))]
pub fn check() -> Result<Option<String>, String> {
    Ok(None)
}
#[cfg(not(target_os = "macos"))]
pub fn install_and_relaunch(_version: &str) -> Result<(), String> {
    Err("updates install on macOS only".to_owned())
}
