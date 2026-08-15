//! De-duplicate filesystem events that the writer caused. Without
//! this the loop is: writer writes → kernel fires inotify → watcher
//! reimports → block content marked dirty → writer fires again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Window during which a recent self-write event is suppressed.
/// Absorbs the kernel's atomic-save dance (rename + remove + create)
/// plus the debouncer's 500ms coalesce window.
const WINDOW: Duration = Duration::from_millis(1500);

#[derive(Debug, Default)]
pub struct SelfWriteGuard {
    recent: Mutex<HashMap<PathBuf, Instant>>,
}

impl SelfWriteGuard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Called immediately before / after the writer touches a file.
    pub fn mark(&self, path: &Path) {
        if let Ok(mut g) = self.recent.lock() {
            g.insert(path.to_path_buf(), Instant::now());
            prune(&mut g);
        }
    }

    /// True if `path` was marked within the suppression window.
    pub fn is_recent(&self, path: &Path) -> bool {
        if let Ok(mut g) = self.recent.lock() {
            prune(&mut g);
            return g.get(path).is_some_and(|t| t.elapsed() < WINDOW);
        }
        false
    }
}

fn prune(map: &mut HashMap<PathBuf, Instant>) {
    map.retain(|_, t| t.elapsed() < WINDOW);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn marks_then_recognizes() {
        let g = SelfWriteGuard::new();
        let p = Path::new("/tmp/foo.md");
        assert!(!g.is_recent(p));
        g.mark(p);
        assert!(g.is_recent(p));
    }

    #[test]
    fn expires_after_window() {
        let g = SelfWriteGuard::new();
        let p = Path::new("/tmp/bar.md");
        g.mark(p);
        sleep(WINDOW + Duration::from_millis(50));
        assert!(!g.is_recent(p));
    }
}
