//! Per-device key/value storage: what `localStorage` is in the browser,
//! on every target.
//!
//! The shell remembers small per-device things — the Tasks view mode,
//! open tabs, the explorer's state, which wiki pages were read, the
//! presence name. Each of those used to read `localStorage` through
//! `web_sys` and do nothing on native, so the desktop and the phone
//! forgot all of it at every launch. This is the one place that knows
//! where such a value lives:
//!
//! - **wasm** — `window.localStorage`, as before (same keys, so nothing
//!   a browser already stored is lost).
//! - **native** — one JSON object at `<data dir>/local-store.json`, the
//!   same directory the server registry and cached sessions use, so on
//!   iOS it sits in `Application Support` and survives relaunches.
//!
//! Reads and writes are synchronous and small; the native file is read
//! once and kept in memory, and rewritten whole on each change.

/// The stored value for `key`, if any.
#[must_use]
pub fn get(key: &str) -> Option<String> {
    imp::get(key)
}

/// Store `value` under `key`, replacing what was there.
pub fn set(key: &str, value: &str) {
    imp::set(key, value);
}

/// Forget `key`.
pub fn remove(key: &str) {
    imp::remove(key);
}

#[cfg(target_arch = "wasm32")]
mod imp {
    fn storage() -> Option<web_sys::Storage> {
        web_sys::window().and_then(|w| w.local_storage().ok().flatten())
    }

    pub(super) fn get(key: &str) -> Option<String> {
        storage()?.get_item(key).ok().flatten()
    }

    pub(super) fn set(key: &str, value: &str) {
        if let Some(s) = storage() {
            let _ = s.set_item(key, value);
        }
    }

    pub(super) fn remove(key: &str) {
        if let Some(s) = storage() {
            let _ = s.remove_item(key);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{LazyLock, Mutex};

    static STORE: LazyLock<Mutex<BTreeMap<String, String>>> = LazyLock::new(|| Mutex::new(load()));

    fn path() -> Option<PathBuf> {
        Some(crate::server_registry::data_dir()?.join("local-store.json"))
    }

    fn load() -> BTreeMap<String, String> {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    fn persist(map: &BTreeMap<String, String>) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string(map) {
            // Write-then-rename, so a crash mid-write leaves the old
            // file rather than half of a new one.
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    pub(super) fn get(key: &str) -> Option<String> {
        STORE.lock().ok()?.get(key).cloned()
    }

    pub(super) fn set(key: &str, value: &str) {
        let Ok(mut map) = STORE.lock() else { return };
        if map.get(key).map(String::as_str) == Some(value) {
            return;
        }
        map.insert(key.to_owned(), value.to_owned());
        persist(&map);
    }

    pub(super) fn remove(key: &str) {
        let Ok(mut map) = STORE.lock() else { return };
        if map.remove(key).is_some() {
            persist(&map);
        }
    }
}
