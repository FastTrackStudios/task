//! Task as a menu bar app: it starts at login, lives in the menu bar,
//! and its window comes and goes without the app quitting.
//!
//! - **At login** — a LaunchAgent (`~/Library/LaunchAgents/
//!   app.fasttrackstudio.task.plist`) starts the installed bundle with
//!   `--background`: no window, no Dock icon, just the menu bar item.
//!   Written only from a bundle in `/Applications`, rewritten when the
//!   path in it is not this bundle's, and `TASK_LOGIN_ITEM=0` removes it.
//! - **Closing the window hides it** (`WindowCloseBehaviour::WindowHides`)
//!   and the Dock icon goes with it; "Open Task" brings both back.
//!
//! A LaunchAgent rather than `SMAppService`: it can pass `--background`,
//! which is how a launch at login knows to stay out of the way.

/// Was this launch asked to stay in the menu bar (a login launch)?
#[must_use]
pub fn background_launch() -> bool {
    std::env::args().any(|a| a == "--background")
}

/// Show or hide the Dock icon (and the app's place in Cmd-Tab).
#[cfg(target_os = "macos")]
pub fn dock_icon(visible: bool) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let policy = if visible {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    };
    app.setActivationPolicy(policy);
    if visible {
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn dock_icon(_visible: bool) {}

#[cfg(target_os = "macos")]
const LABEL: &str = "app.fasttrackstudio.task";

/// Keep the login LaunchAgent pointing at this bundle.
#[cfg(target_os = "macos")]
pub fn ensure_login_item() {
    use std::path::PathBuf;

    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let plist = home.join(format!("Library/LaunchAgents/{LABEL}.plist"));
    if std::env::var("TASK_LOGIN_ITEM").is_ok_and(|v| v == "0") {
        if plist.exists() {
            let _ = std::fs::remove_file(&plist);
            tracing::info!("login item: removed (TASK_LOGIN_ITEM=0)");
        }
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // Only the installed app: a build run from `target/` or a mounted
    // disk image must not become what starts at login.
    if !exe.starts_with("/Applications") || !exe.to_string_lossy().contains(".app/Contents/MacOS/")
    {
        return;
    }
    let body = login_plist(&exe.to_string_lossy());
    if std::fs::read_to_string(&plist).is_ok_and(|have| have == body) {
        return;
    }
    let written = plist
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&plist, &body));
    match written {
        // Not loaded now: this launch is already running. launchd reads
        // the directory at the next login.
        Ok(()) => {
            tracing::info!(path = %plist.display(), "login item: Task starts at login, in the menu bar")
        }
        Err(e) => tracing::warn!(%e, "login item: could not write the LaunchAgent"),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_login_item() {}

#[cfg(target_os = "macos")]
fn login_plist(exe: &str) -> String {
    let exe = exe
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>--background</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
    )
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn the_login_agent_starts_the_bundle_in_the_background() {
        let body = super::login_plist("/Applications/Task.app/Contents/MacOS/TaskAppDesktop");
        assert!(
            body.contains("<string>/Applications/Task.app/Contents/MacOS/TaskAppDesktop</string>")
        );
        assert!(body.contains("<string>--background</string>"));
        assert!(body.contains("<key>RunAtLoad</key>\n    <true/>"));
    }
}
