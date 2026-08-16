//! Content-addressed storage for the small structural objects of the jj
//! object graph — trees, commits, copy histories — as distinct from file
//! *content*, which streams through [`task_files_chunk_store::ChunkStore`].
//! These objects are small (a directory's worth of entries, one commit's
//! metadata) so they are held whole in memory and hashed with blake3, the
//! same durable write-then-fsync-then-rename discipline the chunk store's
//! manifests use (see that crate's `write_manifest` doc).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::AsyncWriteExt as _;

use crate::error::{Error, Result};

/// A flat, content-addressed store of arbitrary object bytes, keyed by their
/// blake3 hash. Used for trees, commits, and copy-history records — never
/// for file content (that's the chunk store's job).
#[derive(Debug)]
pub struct ObjectStore {
    dir: PathBuf,
}

impl ObjectStore {
    pub async fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&dir).await?;
        Ok(Self { dir })
    }

    fn object_path(&self, hash: &blake3::Hash) -> PathBuf {
        self.dir.join(hash.to_hex())
    }

    /// Store `bytes`, returning their content address. Idempotent: writing
    /// the same bytes twice is a no-op the second time (beyond refreshing
    /// the object's mtime — see below), but a prior write that crashed
    /// before its data was durable (existing file present with the wrong
    /// length) is repaired rather than silently trusted.
    pub async fn write(&self, bytes: &[u8]) -> Result<blake3::Hash> {
        let hash = blake3::hash(bytes);
        let path = self.object_path(&hash);
        if let Ok(metadata) = tokio::fs::metadata(&path).await {
            if metadata.len() == bytes.len() as u64 {
                // `gc`'s `keep_newer` protection (and, for objects this
                // crate marks live independently of mtime, nothing) relies
                // on mtime reflecting the most recent write, not just the
                // first one — a caller re-`write`ing already-stored bytes
                // is exactly the "written concurrently with a gc pass"
                // case the `Backend::gc` contract asks backends to protect.
                // Skipping the rename is still correct (the bytes are
                // already durable), but the mtime must still be touched.
                self.touch(&path).await?;
                return Ok(hash);
            }
        }

        static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = self.dir.join(format!(
            "{}.tmp.{}.{unique}",
            hash.to_hex(),
            std::process::id()
        ));
        {
            let mut file = tokio::fs::File::create(&tmp_path).await?;
            file.write_all(bytes).await?;
            file.sync_all().await?;
        }
        tokio::fs::rename(&tmp_path, &path).await?;
        let dir = tokio::fs::File::open(&self.dir).await?;
        dir.sync_all().await?;
        Ok(hash)
    }

    /// Set `path`'s mtime to now. `std::fs::File::set_modified` has no
    /// tokio-native equivalent, so this hands the (already-open, already
    /// I/O-completed) file handle to a blocking thread rather than calling
    /// the blocking API directly on the async task.
    async fn touch(&self, path: &Path) -> Result<()> {
        let file = tokio::fs::OpenOptions::new().write(true).open(path).await?;
        let std_file = file.into_std().await;
        tokio::task::spawn_blocking(move || std_file.set_modified(std::time::SystemTime::now()))
            .await
            .map_err(|e| Error::Io(std::io::Error::other(e)))??;
        Ok(())
    }

    pub async fn read(&self, hash: &blake3::Hash) -> Result<Vec<u8>> {
        let path = self.object_path(hash);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::UnknownObject(hash.to_hex().to_string()))
            }
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Append `line` to a small per-key side index file (newline-separated),
    /// used to invert copy-history parent links into a children index (see
    /// `backend.rs`'s `get_related_copies`). This is the one piece of state
    /// here that isn't itself content-addressed — it's a derived index, safe
    /// to rebuild from the copy-history objects if ever lost.
    pub async fn append_index_line(
        &self,
        index_name: &str,
        key_hex: &str,
        line: &str,
    ) -> Result<()> {
        let index_dir = self.dir.join(index_name);
        tokio::fs::create_dir_all(&index_dir).await?;
        let path = index_dir.join(key_hex);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await?;
        Ok(())
    }

    pub async fn read_index_lines(&self, index_name: &str, key_hex: &str) -> Result<Vec<String>> {
        let path = self.dir.join(index_name).join(key_hex);
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => Ok(contents.lines().map(str::to_owned).collect()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Every object hash currently on disk, with its last-modified time
    /// (used by `gc.rs` as the `keep_newer` protection signal — this store
    /// has no separate creation-time metadata, so mtime is the proxy).
    pub async fn list_with_mtime(&self) -> Result<Vec<(blake3::Hash, std::time::SystemTime)>> {
        let mut out = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            // Skip temp files and index subdirectories (64 hex chars = blake3).
            if name.len() != 64 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            let Ok(hash) = blake3::Hash::from_hex(name) else {
                continue;
            };
            let metadata = entry.metadata().await?;
            out.push((hash, metadata.modified()?));
        }
        Ok(out)
    }

    pub async fn remove(&self, hash: &blake3::Hash) -> Result<()> {
        let path = self.object_path(hash);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test: `write`'s early-return for already-stored bytes
    /// used to skip touching the object's mtime, so a genuine re-write of
    /// live content (e.g. `VersionStoreBackend::open` re-deriving the
    /// empty tree on every startup) wouldn't refresh the "written
    /// concurrently with a gc pass" freshness signal `Backend::gc`'s
    /// `keep_newer` contract relies on.
    #[tokio::test]
    async fn write_refreshes_mtime_even_when_bytes_are_already_stored() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).await.unwrap();
        let bytes = b"same content, written twice";

        let hash = store.write(bytes).await.unwrap();
        let (_, first_mtime) = store
            .list_with_mtime()
            .await
            .unwrap()
            .into_iter()
            .find(|(h, _)| *h == hash)
            .unwrap();

        // Filesystem mtime resolution can be coarse (1s on some setups) —
        // sleep past it so a refreshed mtime is observably later, not just
        // not-earlier.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

        let hash_again = store.write(bytes).await.unwrap();
        assert_eq!(hash, hash_again);
        let (_, second_mtime) = store
            .list_with_mtime()
            .await
            .unwrap()
            .into_iter()
            .find(|(h, _)| *h == hash)
            .unwrap();

        assert!(
            second_mtime > first_mtime,
            "re-writing already-stored bytes must refresh mtime: {first_mtime:?} -> {second_mtime:?}"
        );
    }
}
