//! The C ABI a macOS File Provider extension is built on.
//!
//! # Why there is a Rust half at all
//!
//! macOS reaches the cloud-folder behaviour a different way than Linux
//! does. There is no FUSE: the system loads a **File Provider
//! extension** from the app bundle and asks *it* for material, where on
//! Linux the kernel asks us. The callbacks are Objective-C/Swift and
//! must be answered in an appex, so the extension itself cannot be a
//! crate.
//!
//! What it needs, though, is exactly two things Swift has no business
//! knowing:
//!
//! - **what a [pointer stub](files::stub) is** — a dehydrated file must
//!   be reported at the size of the content it stands for, and the
//!   authority on that size is the stub file, not a sidecar index;
//! - **how to reach the agent** — hydration is the agent's job, because
//!   the agent owns the store and hydration writes into it. Two writers
//!   to one jj repo is the bug this avoids by construction.
//!
//! Everything else the extension does — enumerating a directory,
//! handing back a file URL, writing an edit through — is `FileManager`
//! against the live tree, which is already on disk. That is what keeps
//! this surface small: five functions, no model, no state machine.
//!
//! # The shape of the ABI
//!
//! Strings out are heap-allocated C strings the caller frees with
//! [`fts_fp_free`]; strings in are borrowed for the length of the call.
//! Fallible calls return `0` on success and `-1` on failure, and the
//! reason is available from [`fts_fp_last_error`] until the next call on
//! that thread — the errno convention, because the caller is C and this
//! is the one error protocol every C caller already has.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int};
use std::path::Path;

use files_daemon_proto::service::DaemonControlServiceClient;

thread_local! {
    /// The reason the last call on this thread failed. Per-thread
    /// because the File Provider host calls the extension from several,
    /// and a shared slot would hand one caller another's failure.
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn fail(msg: impl Into<String>) -> c_int {
    let msg = msg.into();
    tracing::warn!(error = %msg, "file provider bridge call failed");
    let owned = CString::new(msg).unwrap_or_else(|_| c"error".into());
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(owned));
    -1
}

fn succeed() -> c_int {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
    0
}

/// Borrow a C string argument, or fail the call.
///
/// # Safety
/// `ptr` must be null or a NUL-terminated string valid for this call.
unsafe fn borrow<'a>(ptr: *const c_char, what: &str) -> Result<&'a str, String> {
    if ptr.is_null() {
        return Err(format!("{what} was null"));
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| format!("{what} was not utf-8"))
}

/// Hand a string to the caller, who frees it with [`fts_fp_free`].
fn hand_over(s: String) -> *mut c_char {
    CString::new(s).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Why the last call on this thread failed, or null if it did not.
///
/// The returned string belongs to the caller; free it with
/// [`fts_fp_free`].
#[unsafe(no_mangle)]
pub extern "C" fn fts_fp_last_error() -> *mut c_char {
    LAST_ERROR.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or(std::ptr::null_mut(), |e| hand_over(e.to_string_lossy().into_owned()))
    })
}

/// Free a string this library returned.
///
/// # Safety
/// `s` must be null, or a pointer this library returned and that has
/// not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fts_fp_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

/// What a path in a live tree really is.
///
/// The one answer the extension cannot work out for itself: `stat` on a
/// dehydrated file reports the placeholder's couple of hundred bytes,
/// and reporting that to the system would tell every app in the world
/// that a two-gigabyte take is empty.
#[derive(serde::Serialize)]
struct Facts {
    /// Bytes the content really has — the stub's recorded size when the
    /// file is dehydrated, the file's own size when it is not.
    size: u64,
    /// Is this a placeholder rather than the content?
    dehydrated: bool,
    /// The executable bit the content carries, which for a stub is
    /// recorded rather than present.
    executable: bool,
}

/// Facts about one path, as JSON — see [`Facts`].
///
/// Returns null on failure, with the reason in [`fts_fp_last_error`].
///
/// # Safety
/// `path` must be a NUL-terminated string valid for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fts_fp_facts(path: *const c_char) -> *mut c_char {
    let path = match unsafe { borrow(path, "path") } {
        Ok(p) => p,
        Err(e) => {
            fail(e);
            return std::ptr::null_mut();
        }
    };
    let path = Path::new(path);
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            fail(format!("{}: {e}", path.display()));
            return std::ptr::null_mut();
        }
    };

    // `probe`, not `read`: this is an enumeration path, and one odd
    // small file must not take down a whole listing. A file whose magic
    // is present but whose body is garbage is handled as the ordinary
    // content its bytes are, loudly.
    let facts = match files::stub::probe(path) {
        Some(stub) => Facts {
            size: stub.size,
            dehydrated: true,
            executable: stub.executable,
        },
        None => Facts {
            size: meta.len(),
            dehydrated: false,
            executable: {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    meta.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    false
                }
            },
        },
    };

    succeed();
    match serde_json::to_string(&facts) {
        Ok(json) => hand_over(json),
        Err(e) => {
            fail(format!("serializing the facts: {e}"));
            std::ptr::null_mut()
        }
    }
}

/// The roots the running agent holds, as JSON: `[{"id","name","path"}]`.
///
/// This is how the extension learns which directory a File Provider
/// domain is a window onto — the domain is created per root, and the
/// tree it mirrors is whatever the agent says.
///
/// Returns null on failure, with the reason in [`fts_fp_last_error`].
#[unsafe(no_mangle)]
pub extern "C" fn fts_fp_roots() -> *mut c_char {
    #[derive(serde::Serialize)]
    struct Root {
        id: String,
        name: String,
        path: String,
    }

    let roots = match on_the_agent(async |client| client.shares().await.map_err(|e| e.to_string())) {
        Ok(r) => r,
        Err(e) => {
            fail(e);
            return std::ptr::null_mut();
        }
    };
    let roots: Vec<Root> = roots
        .into_iter()
        .map(|(id, name, path)| Root {
            id: id.to_string(),
            name,
            path,
        })
        .collect();

    succeed();
    match serde_json::to_string(&roots) {
        Ok(json) => hand_over(json),
        Err(e) => {
            fail(format!("serializing the roots: {e}"));
            std::ptr::null_mut()
        }
    }
}

/// Bring one path's content resident, and block until it is.
///
/// The extension calls this from `fetchContents`, which is allowed to
/// take as long as it takes — the system shows progress and the app
/// that opened the file waits, which is the honest behaviour and the
/// same one a slow disk produces.
///
/// # Safety
/// `root_id` and `rel_path` must be NUL-terminated strings valid for
/// this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fts_fp_hydrate(root_id: *const c_char, rel_path: *const c_char) -> c_int {
    let (root_id, rel_path) = match (
        unsafe { borrow(root_id, "root_id") },
        unsafe { borrow(rel_path, "rel_path") },
    ) {
        (Ok(a), Ok(b)) => (a, b.to_string()),
        (Err(e), _) | (_, Err(e)) => return fail(e),
    };
    let Ok(id) = root_id.parse::<uuid::Uuid>() else {
        return fail(format!("{root_id} is not a root id"));
    };

    match on_the_agent(async move |client| {
        client
            .hydrate(id, rel_path)
            .await
            .map_err(|e| e.to_string())
    }) {
        Ok(()) => succeed(),
        Err(e) => fail(e),
    }
}

/// Release one path's bytes, leaving the file listed at its real size.
///
/// The system asks for this when a person picks "Remove Download", and
/// having it is what makes a Mac with a small disk able to hold a large
/// project at all.
///
/// # Safety
/// `root_id` and `rel_path` must be NUL-terminated strings valid for
/// this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fts_fp_evict(root_id: *const c_char, rel_path: *const c_char) -> c_int {
    let (root_id, rel_path) = match (
        unsafe { borrow(root_id, "root_id") },
        unsafe { borrow(rel_path, "rel_path") },
    ) {
        (Ok(a), Ok(b)) => (a, b.to_string()),
        (Err(e), _) | (_, Err(e)) => return fail(e),
    };
    let Ok(id) = root_id.parse::<uuid::Uuid>() else {
        return fail(format!("{root_id} is not a root id"));
    };

    match on_the_agent(async move |client| {
        client
            .dehydrate(id, rel_path)
            .await
            .map_err(|e| e.to_string())
    }) {
        Ok(()) => succeed(),
        Err(e) => fail(e),
    }
}

/// Run one call against the running agent, synchronously.
///
/// A fresh runtime and a fresh connection per call, deliberately. The
/// extension is loaded and unloaded by the system at times it does not
/// announce, and a long-lived connection across that is a socket the
/// host process keeps open through a suspension — a stall to debug
/// later, for a saving that does not matter: these calls happen when a
/// person opens a file, not in a loop.
fn on_the_agent<F, T>(call: F) -> Result<T, String>
where
    F: AsyncFnOnce(DaemonControlServiceClient) -> Result<T, String>,
{
    let bind = std::env::var("FTS_FILES_DAEMON_BIND").unwrap_or_else(|_| "127.0.0.1:4055".into());
    let url = format!("ws://{bind}/vox");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("starting a runtime: {e}"))?;

    runtime.block_on(async move {
        // Connecting gets a deadline of its own. The agent is a local
        // socket, so it either answers immediately or is not there —
        // and the caller is a filesystem extension the system kills for
        // being slow, having first made every app looking at the folder
        // wait on it. A hang here is a hung Finder, so there must not
        // be a path through this function that does not return.
        let client: DaemonControlServiceClient =
            match tokio::time::timeout(CONNECT_DEADLINE, vox::connect_lane(&url).establish()).await
            {
                Ok(Ok(client)) => client,
                Ok(Err(e)) => {
                    return Err(format!(
                        "no sync agent answering on {url} ({e}) — is Task installed?"
                    ));
                }
                Err(_) => {
                    return Err(format!(
                        "the sync agent on {url} did not answer within {}s",
                        CONNECT_DEADLINE.as_secs()
                    ));
                }
            };

        // The call itself gets a longer one: hydrating a take is meant
        // to take a while, and the system shows progress for it. Long
        // is not the same as unbounded.
        match tokio::time::timeout(CALL_DEADLINE, call(client)).await {
            Ok(outcome) => outcome,
            Err(_) => Err(format!(
                "the sync agent did not finish within {}s",
                CALL_DEADLINE.as_secs()
            )),
        }
    })
}

/// How long to wait for a socket on this machine. Generous for a local
/// connect, short enough that a missing agent is reported rather than
/// waited on.
const CONNECT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// How long to wait for the work. Fetching a multi-gigabyte take from a
/// peer over a slow link is the case this must not cut short.
const CALL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

#[cfg(test)]
mod tests {
    use super::*;

    /// The one claim the extension exists to make: a dehydrated file
    /// reports the size of what it stands for, not the size of the note
    /// saying it is gone.
    #[test]
    fn a_stub_reports_the_content_size_not_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        let stub = files::stub::Stub {
            file_id: "a".repeat(64),
            size: 2_000_000_000,
            executable: false,
        };
        std::fs::write(&path, stub.to_bytes()).unwrap();

        let c = CString::new(path.to_str().unwrap()).unwrap();
        let json = unsafe { fts_fp_facts(c.as_ptr()) };
        assert!(!json.is_null());
        let json = unsafe { CStr::from_ptr(json) }.to_str().unwrap().to_string();
        let facts: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(facts["size"], 2_000_000_000u64);
        assert_eq!(facts["dehydrated"], true);
        // Sanity: the file on disk is nothing like that.
        assert!(std::fs::metadata(&path).unwrap().len() < 4096);
    }

    /// Ordinary content is ordinary — no stub, its own size.
    #[test]
    fn resident_content_reports_itself() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"session notes").unwrap();

        let c = CString::new(path.to_str().unwrap()).unwrap();
        let json = unsafe { fts_fp_facts(c.as_ptr()) };
        let json = unsafe { CStr::from_ptr(json) }.to_str().unwrap().to_string();
        let facts: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(facts["size"], 13);
        assert_eq!(facts["dehydrated"], false);
    }

    /// A failure the caller can act on, in the one protocol a C caller
    /// already has: null out, reason in `last_error`.
    #[test]
    fn a_missing_path_leaves_a_reason_behind() {
        let c = CString::new("/nowhere/at/all/take.wav").unwrap();
        assert!(unsafe { fts_fp_facts(c.as_ptr()) }.is_null());
        let err = fts_fp_last_error();
        assert!(!err.is_null());
        let err = unsafe { CStr::from_ptr(err) }.to_str().unwrap();
        assert!(err.contains("take.wav"), "unhelpful error: {err}");
    }
}
