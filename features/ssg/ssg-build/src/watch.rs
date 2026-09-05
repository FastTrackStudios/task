//! Watch a vault directory and re-render on save.
//!
//! A site bakes its vault in at compile time — that is what makes the
//! published pages fast and what `include_vault!` exists for. It also
//! means editing one word of prose costs a full rebuild of the site
//! crate, which for Keyflow is four or five minutes. Nobody edits a guide
//! that way.
//!
//! So a *dev server* renders the same vault at runtime instead, and this
//! tells it when to do so again. The render itself is
//! [`Vault::render`](crate::Vault::render) — the same method
//! [`Vault::emit`](crate::Vault::emit) calls, so the page a writer
//! previews is the page that ships.
//!
//! Behind the `watch` feature: `ssg-build` is consumed from build scripts
//! in four workspaces, and none of them should pay for a filesystem
//! watcher they never run.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long the directory must be quiet before a burst counts as done.
///
/// Editors write a file more than once when saving — a temp file, a
/// rename, sometimes a truncate and a write — and each is an event. Under
/// a re-render per event a single Cmd-S rebuilds the vault three times.
const QUIET: Duration = Duration::from_millis(100);

/// The longest a burst may defer the re-render.
///
/// Without it a directory being written continuously — a `git checkout`
/// across the whole vault — would never settle and the preview would
/// never update.
const MAX_WAIT: Duration = Duration::from_millis(500);

/// A live watch. Dropping it stops the thread.
///
/// The handle must be kept alive: `notify`'s watcher stops when its own
/// handle drops, and a watch nobody holds is a watch that silently does
/// nothing.
pub struct Watch {
    _watcher: notify_fs::RecommendedWatcher,
}

/// Call `on_change` whenever a markdown file under `dir` is written.
///
/// Debounced: one call per settled burst, not one per filesystem event.
/// The callback runs on the watcher's own thread, so it may block — a
/// vault re-render takes long enough to matter and should not be run on
/// an async executor's worker.
///
/// # Errors
/// If the directory cannot be watched.
pub fn on_change(
    dir: impl AsRef<Path>,
    on_change: impl Fn() + Send + 'static,
) -> notify_fs::Result<Watch> {
    use notify_fs::{RecursiveMode, Watcher};

    let (tx, rx) = mpsc::channel();
    let mut watcher = notify_fs::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })?;
    watcher.watch(dir.as_ref(), RecursiveMode::Recursive)?;

    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            if !is_interesting(&first) {
                continue;
            }
            // Drain the burst: keep extending the deadline while events
            // keep arriving, up to `MAX_WAIT` from the first one.
            let started = Instant::now();
            let mut deadline = started + QUIET;
            loop {
                let capped = deadline.min(started + MAX_WAIT);
                let Some(wait) = capped.checked_duration_since(Instant::now()) else {
                    break;
                };
                match rx.recv_timeout(wait) {
                    Ok(ev) => {
                        if is_interesting(&ev) {
                            deadline = Instant::now() + QUIET;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            on_change();
        }
    });

    Ok(Watch {
        _watcher: watcher,
    })
}

/// Is this event a markdown write worth re-rendering for?
///
/// Access events are excluded — `notify` reports reads on some platforms,
/// and re-rendering because something *looked* at the vault would put the
/// dev server in a loop with its own reads.
fn is_interesting(event: &notify_fs::Result<notify_fs::Event>) -> bool {
    let Ok(event) = event else {
        return false;
    };
    if matches!(event.kind, notify_fs::EventKind::Access(_)) {
        return false;
    }
    event
        .paths
        .iter()
        .any(|p| p.extension().is_some_and(|e| e == "md"))
}
