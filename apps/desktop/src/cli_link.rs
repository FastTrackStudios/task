//! `task` on PATH, from the app.
//!
//! The direct-download app carries the CLI in its bundle
//! (`Contents/MacOS/task`, built by `macos/build-dmg.sh`), so it ships and
//! updates with the app. At launch the app links it to `~/.local/bin/task`
//! — on PATH in a default shell setup — unless something that is not our
//! link already sits there (a `cargo install`, say), which is left alone.
//! The CLI then talks to this app's node as the person signed in to it
//! (`task_client::desktop`).
//!
//! macOS only, and only from a `.app` bundle.

#[cfg(target_os = "macos")]
pub fn link() {
    use std::path::PathBuf;

    let Some(bundled) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("task")))
        .filter(|p| p.is_file())
    else {
        return;
    };
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let dir = home.join(".local/bin");
    let target = dir.join("task");
    match std::fs::symlink_metadata(&target) {
        // Ours already (pointing into a Task bundle) or nothing there.
        Ok(meta) if meta.file_type().is_symlink() => {
            let ours = std::fs::read_link(&target)
                .is_ok_and(|to| to.to_string_lossy().contains(".app/Contents/MacOS/task"));
            if !ours {
                tracing::info!(path = %target.display(), "cli: a `task` link of someone else's is on PATH; left alone");
                return;
            }
            if std::fs::read_link(&target).is_ok_and(|to| to == bundled) {
                return;
            }
            let _ = std::fs::remove_file(&target);
        }
        Ok(_) => {
            tracing::info!(path = %target.display(), "cli: a `task` binary is already on PATH; left alone");
            return;
        }
        Err(_) => {}
    }
    if let Err(e) =
        std::fs::create_dir_all(&dir).and_then(|()| std::os::unix::fs::symlink(&bundled, &target))
    {
        tracing::warn!(%e, path = %target.display(), "cli: could not link `task` onto PATH");
    } else {
        tracing::info!(path = %target.display(), "cli: `task` linked onto PATH");
    }
}

#[cfg(not(target_os = "macos"))]
pub fn link() {}
