pub use files_daemon_proto::service::DaemonError;

pub type Result<T> = std::result::Result<T, DaemonError>;

/// A sync-engine failure, as the control surface reports it.
///
/// A function rather than a `From` impl, and not by preference:
/// [`DaemonError`] lives in `files-daemon-proto` now — so a client can
/// speak this surface without depending on the agent — and the orphan
/// rule puts `From<files_sync::SyncError>` out of reach from here. It
/// could move to the proto crate, at the price of every client
/// depending on `files-sync`, which is the whole replica engine. One
/// helper at a handful of call sites is the cheaper side of that trade.
#[must_use]
pub fn from_sync(e: files_sync::SyncError) -> DaemonError {
    match e {
        files_sync::SyncError::NotFound(m) => DaemonError::NotFound(m),
        files_sync::SyncError::BadRequest(m) => DaemonError::BadRequest(m),
        files_sync::SyncError::Io(m) => DaemonError::Io(m),
    }
}

/// A backend store-seam failure, as the control surface reports it.
///
/// Same orphan-rule story as [`from_sync`]: the backend's in-process
/// [`files::FilesError`] and [`DaemonError`] live in two other crates,
/// and the proto crate depending on `files` would hand every client the
/// whole engine. (The lanes' [`files::FilesFault`] converts with `?` —
/// its `From` lives in the proto crate beside [`DaemonError`].)
#[must_use]
pub fn from_files(e: files::FilesError) -> DaemonError {
    match e {
        files::FilesError::NotFound(m) => DaemonError::NotFound(m),
        files::FilesError::AlreadyExists(m) | files::FilesError::BadRequest(m) => {
            DaemonError::BadRequest(m)
        }
        files::FilesError::Io(m) => DaemonError::Io(m),
    }
}

/// A root-relative path as the control surface was handed it, parsed
/// into the lanes' path type. A path the lanes would refuse is the
/// caller's mistake, so it reports as one.
pub fn root_path(raw: &str) -> Result<files::RootPath> {
    files::RootPath::parse(raw).map_err(|e| DaemonError::BadRequest(e.to_string()))
}
