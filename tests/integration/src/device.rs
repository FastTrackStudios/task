//! One concept: a device that carries part of a project.
//!
//! A laptop is not a small server. It holds one composed project, under
//! the names the composition gave its halves, and it decides what to
//! keep resident by subscribing to *facets* — the kind of work it does —
//! rather than to path globs.
//!
//! The fixture below is deliberately lopsided: the sessions are a few
//! hundred bytes and the reel is 64 KB, because the whole question the
//! device chapter asks is which of them is on the disk.

use files::RootId;
use files::model::RootFlavor;

/// An editor's laptop with the composed album on it.
pub struct Laptop {
    /// Kept for its `Drop`: the whole device goes away with it.
    _dir: tempfile::TempDir,
    pub backend: files::FilesBackend,
    /// The composed project, as one root on this device.
    pub album: RootId,
    /// Where that project sits on this disk. Tests read it directly —
    /// the catalogue calling something a stub proves nothing if the
    /// bytes are still there.
    pub tree: std::path::PathBuf,
}

impl Laptop {
    /// Set the laptop up with the project already on it and pinned.
    pub async fn open() -> Self {
        let dir = tempfile::tempdir().expect("laptop data dir");
        let backend = files::FilesBackend::new(dir.path(), dir.path().join("vault"))
            .expect("laptop backend");

        // The project as it lands on a device: both companies' halves,
        // under the names the composition gave them.
        let tree = dir.path().join("Album");
        std::fs::create_dir_all(tree.join("Sessions").join("Audio Files")).unwrap();
        std::fs::create_dir_all(tree.join("Footage").join("Proxies")).unwrap();
        std::fs::write(
            tree.join("Sessions").join("Song.rpp"),
            b"REAPER project (fixture)",
        )
        .unwrap();
        std::fs::write(
            tree.join("Sessions").join("Audio Files").join("vox.wav"),
            b"vox take one",
        )
        .unwrap();
        // Deliberately the largest thing here: it is what the laptop is
        // trying not to carry.
        std::fs::write(
            tree.join("Footage").join("Proxies").join("reel.mov"),
            vec![0u8; Self::REEL_BYTES as usize],
        )
        .unwrap();

        let album = files::FilesService::create_root(
            &backend,
            tree.to_string_lossy().into_owned(),
            "Album".into(),
            RootFlavor::Media,
        )
        .await
        .expect("adopt the project on the laptop");
        let album = RootId::new(album.id);

        // Content into the store first, so dehydrating is dropping a
        // local copy rather than losing the file.
        files::FilesService::checkpoint_now(&backend, album.into(), None)
            .await
            .expect("checkpoint");

        Self {
            _dir: dir,
            backend,
            album,
            tree,
        }
    }

    /// The reel's real size, which a stub of it must keep reporting.
    pub const REEL_BYTES: u64 = 64 * 1024;

    /// The reel's path on this disk.
    #[must_use]
    pub fn reel(&self) -> std::path::PathBuf {
        self.tree.join("Footage").join("Proxies").join("reel.mov")
    }

    /// What the reel costs on this laptop right now, in bytes.
    #[must_use]
    pub fn reel_on_disk(&self) -> u64 {
        std::fs::metadata(self.reel()).map_or(u64::MAX, |m| m.len())
    }
}
