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
