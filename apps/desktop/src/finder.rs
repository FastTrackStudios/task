//! The Finder extension, kept registered from inside the app.
//!
//! The File Provider extension ships in the bundle
//! (`Contents/PlugIns/TaskFileProvider.appex`), so an update replaces it
//! with the app. What the system also needs is to be told: the extension
//! enabled (`pluginkit`) and the one "Task" domain registered while the
//! agent holds anything to show. `Contents/MacOS/TaskFileProviderDomains
//! sync` does both and is idempotent, so the app runs it at every launch
//! — which after an update is the new bundle's extension — and again
//! whenever what the agent holds may have changed (a sign-in, a mount).

use std::process::Command;
use std::time::Duration;

fn tool() -> Option<std::path::PathBuf> {
    let here = std::env::current_exe().ok()?;
    let tool = here.parent()?.join("TaskFileProviderDomains");
    tool.is_file().then_some(tool)
}

/// Make Finder's "Task" match the agent. Blocking; call off the UI thread.
///
/// The agent may still be starting at launch, and the tool refuses
/// (rather than removing the domain) when it cannot reach it, so a few
/// tries are given before saying so.
pub fn sync_domain() -> String {
    let Some(tool) = tool() else {
        return "finder: no extension in this build".into();
    };
    let mut last = String::new();
    for attempt in 0..5 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(3));
        }
        match Command::new(&tool).arg("sync").output() {
            Ok(out) if out.status.success() => {
                let said = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                return if said.is_empty() {
                    "finder: Task is registered".into()
                } else {
                    format!("finder: {said}")
                };
            }
            Ok(out) => last = String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            Err(e) => return format!("finder: could not run {} ({e})", tool.display()),
        }
    }
    format!("finder: not registered — {last}")
}

/// [`sync_domain`] on a thread of its own, logged.
pub fn sync_domain_soon() {
    std::thread::spawn(|| tracing::info!("{}", sync_domain()));
}
