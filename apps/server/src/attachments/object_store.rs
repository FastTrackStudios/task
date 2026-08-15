//! Pluggable blob storage. Phase 7 ships [`LocalFsStore`]; future
//! Nextcloud / S3 backends implement the same trait.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use attachments_proto::AttachmentError;

/// Pluggable blob store. All backends are content-addressed: a
/// blob's identity IS its sha256 hash. Storing the same bytes
/// twice is a no-op.
#[async_trait]
pub trait ObjectStore: Send + Sync + 'static {
    /// Persist `bytes` under `content_hash`. Returns Ok even if
    /// the blob already exists (content-addressed → idempotent).
    async fn put_blob(&self, content_hash: &str, bytes: &[u8]) -> Result<(), AttachmentError>;

    /// Fetch every byte of the blob, in one shot. v0 — streaming
    /// follows when files get large.
    async fn get_blob(&self, content_hash: &str) -> Result<Vec<u8>, AttachmentError>;

    /// Size of the blob in bytes. Default falls back to reading the
    /// whole blob; backends should override with a metadata lookup.
    async fn blob_len(&self, content_hash: &str) -> Result<u64, AttachmentError> {
        Ok(self.get_blob(content_hash).await?.len() as u64)
    }

    /// Fetch `len` bytes starting at `start` (clamped to the blob's
    /// end). Serves HTTP Range requests so `<audio>` seek doesn't
    /// pull the whole file. Default reads everything and slices;
    /// backends should override with a seek + bounded read.
    async fn get_blob_range(
        &self,
        content_hash: &str,
        start: u64,
        len: u64,
    ) -> Result<Vec<u8>, AttachmentError> {
        let all = self.get_blob(content_hash).await?;
        let total = all.len() as u64;
        let s = start.min(total) as usize;
        let e = start.saturating_add(len).min(total) as usize;
        Ok(all[s..e].to_vec())
    }

    async fn blob_exists(&self, content_hash: &str) -> Result<bool, AttachmentError>;
}

/// Local filesystem backend. Stores blobs at
/// `<root>/<hash[0..2]>/<hash>` to keep individual directory
/// fan-outs manageable.
#[derive(Debug, Clone)]
pub struct LocalFsStore {
    root: PathBuf,
}

impl LocalFsStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Compute the on-disk path for a given content hash. Public
    /// so tests can poke at the layout without going through
    /// `get_blob`.
    #[must_use]
    pub fn path_for(&self, content_hash: &str) -> PathBuf {
        let prefix = &content_hash.get(..2).unwrap_or("");
        self.root.join(prefix).join(content_hash)
    }

    fn ensure_parent(path: &Path) -> Result<(), AttachmentError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AttachmentError::Internal(format!("mkdir {}: {e}", parent.display()))
            })?;
        }
        Ok(())
    }
}

#[async_trait]
impl ObjectStore for LocalFsStore {
    async fn put_blob(&self, content_hash: &str, bytes: &[u8]) -> Result<(), AttachmentError> {
        let path = self.path_for(content_hash);
        // Idempotent: if it's already there, leave it.
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(());
        }
        Self::ensure_parent(&path)?;
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| AttachmentError::Internal(format!("write {}: {e}", path.display())))?;
        Ok(())
    }

    async fn get_blob(&self, content_hash: &str) -> Result<Vec<u8>, AttachmentError> {
        let path = self.path_for(content_hash);
        match tokio::fs::read(&path).await {
            Ok(b) => Ok(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(AttachmentError::NotFound),
            Err(e) => Err(AttachmentError::Internal(format!(
                "read {}: {e}",
                path.display()
            ))),
        }
    }

    async fn blob_len(&self, content_hash: &str) -> Result<u64, AttachmentError> {
        let path = self.path_for(content_hash);
        match tokio::fs::metadata(&path).await {
            Ok(m) => Ok(m.len()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(AttachmentError::NotFound),
            Err(e) => Err(AttachmentError::Internal(format!(
                "stat {}: {e}",
                path.display()
            ))),
        }
    }

    async fn get_blob_range(
        &self,
        content_hash: &str,
        start: u64,
        len: u64,
    ) -> Result<Vec<u8>, AttachmentError> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let path = self.path_for(content_hash);
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AttachmentError::NotFound);
            }
            Err(e) => {
                return Err(AttachmentError::Internal(format!(
                    "open {}: {e}",
                    path.display()
                )));
            }
        };
        let io_err =
            |e: std::io::Error| AttachmentError::Internal(format!("read {}: {e}", path.display()));
        let total = file.metadata().await.map_err(io_err)?.len();
        let start = start.min(total);
        let len = len.min(total - start);
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(io_err)?;
        let mut buf = vec![
            0u8;
            usize::try_from(len).map_err(|e| {
                AttachmentError::Internal(format!("range too large for this platform: {e}"))
            })?
        ];
        file.read_exact(&mut buf).await.map_err(io_err)?;
        Ok(buf)
    }

    async fn blob_exists(&self, content_hash: &str) -> Result<bool, AttachmentError> {
        Ok(tokio::fs::try_exists(self.path_for(content_hash))
            .await
            .unwrap_or(false))
    }
}

/// Resolve `<data_root>/blobs` with the standard fallback
/// (data root resolves via [`org_proto::DataRoot::from_env`]
/// — typically `$HOME/.task/`). Created at first call.
pub fn default_blob_root() -> eyre::Result<PathBuf> {
    let root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    let dir = root.path().join("blobs");
    std::fs::create_dir_all(&dir).map_err(|e| eyre::eyre!("create {}: {e}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_then_get_round_trips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = LocalFsStore::new(tmp.path());
        let bytes = b"hello world".to_vec();
        let hash = "abcdef0123456789";
        store.put_blob(hash, &bytes).await.expect("put");
        assert!(store.blob_exists(hash).await.unwrap());
        let got = store.get_blob(hash).await.expect("get");
        assert_eq!(got, bytes);
    }

    #[tokio::test]
    async fn get_missing_returns_not_found() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = LocalFsStore::new(tmp.path());
        match store.get_blob("does-not-exist").await {
            Err(AttachmentError::NotFound) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn put_is_idempotent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = LocalFsStore::new(tmp.path());
        let hash = "deadbeef";
        store.put_blob(hash, b"v1").await.unwrap();
        // Same hash → no-op even when bytes don't match the first
        // write. Content-addressed stores rely on the caller to
        // pass a real sha256 of the bytes; we don't re-verify.
        store.put_blob(hash, b"v2").await.unwrap();
        assert_eq!(store.get_blob(hash).await.unwrap(), b"v1");
    }

    #[test]
    fn path_for_uses_2char_prefix() {
        let store = LocalFsStore::new("/tmp/x");
        let p = store.path_for("ab1234");
        assert!(p.to_string_lossy().ends_with("ab/ab1234"));
    }
}
