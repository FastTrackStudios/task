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
//!
//! # A device is a peer
//!
//! It binds an iroh endpoint and serves the replica lane on it, so
//! another device can pull from it directly. That is what
//! `files.topology.multi-server` means by "where two peers can reach each
//! other, bytes move directly over iroh/QUIC" — two laptops in a studio
//! should move a session between themselves, not out to a server and
//! back.
//!
//! Its endpoint key **is** its device identity. `files.device.identity`
//! asks for "an identity it holds and persists itself — one per machine,
//! surviving restart and the expiry of any login session", which is
//! exactly what an endpoint secret key is. Two separate identities for
//! one machine would be two things to keep in step, and the transport's
//! is the one that cannot be faked.

use files::RootId;
use files::model::RootFlavor;

/// An editor's laptop with the composed album on it.
pub struct Laptop {
    /// Kept for its `Drop`: the whole device goes away with it.
    _dir: tempfile::TempDir,
    pub backend: files::FilesBackend,
    /// This machine's identity, and the address other peers dial. One
    /// thing, not two — see the module note.
    pub endpoint: iroh::Endpoint,
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

        // The machine's identity, and its address. A real device persists
        // this key — `files.device.identity` requires the identity to
        // survive restart and the expiry of any login — and a test lives
        // for one process, so a fresh key is the same thing here.
        let endpoint = architect::iroh_link::bind_endpoint(iroh::SecretKey::generate())
            .await
            .expect("bind the device's endpoint");

        // And it serves. Only the replica lane, only to endpoints it
        // admits — see `files_sync::serve_peer`.
        let serving = endpoint.clone();
        let served = backend.clone();
        tokio::spawn(async move {
            files_sync::serve_peer(served, "laptop".into(), &serving).await;
        });

        Self {
            _dir: dir,
            backend,
            endpoint,
            album,
            tree,
        }
    }

    /// A second machine, holding nothing, ready to pull `album`.
    ///
    /// Deliberately empty: it adopts the root's *id* with a local tree and
    /// no content, which is what a device looks like before a sync. Giving
    /// it the fixture would make a transfer test into a comparison of two
    /// prepared disks.
    pub async fn empty_peer(album: RootId) -> Self {
        let dir = tempfile::tempdir().expect("laptop data dir");
        let tree = dir.path().join("Album");
        let backend = files::FilesBackend::new(dir.path(), dir.path().join("vault"))
            .expect("laptop backend");
        backend
            .adopt_replica(
                album.get(),
                "Album",
                tree.to_str().expect("utf-8 path"),
                RootFlavor::Media,
            )
            .expect("adopt the replica");

        let endpoint = architect::iroh_link::bind_endpoint(iroh::SecretKey::generate())
            .await
            .expect("bind the device's endpoint");
        let serving = endpoint.clone();
        let served = backend.clone();
        tokio::spawn(async move {
            files_sync::serve_peer(served, "laptop".into(), &serving).await;
        });

        Self {
            _dir: dir,
            backend,
            endpoint,
            album,
            tree,
        }
    }

    /// This machine's identity, as an admitted-peer list records it.
    #[must_use]
    pub fn host_id(&self) -> files_domain::HostId {
        files_domain::HostId(self.endpoint.id().to_string())
    }

    /// Open the replica lane on `origin` as this device.
    ///
    /// Signs nothing: what reaches the far gate is the endpoint iroh
    /// proved during the handshake, so what authorises the pull is
    /// `origin` having admitted this machine.
    pub async fn dial_replica(
        &self,
        origin: &iroh::Endpoint,
    ) -> files_sync::SyncServiceClient {
        let link = architect::iroh_link::connect(&self.endpoint, origin.addr())
            .await
            .expect("dial the origin");
        vox_core::initiator_on(link)
            .establish()
            .await
            .expect("establish the replica lane")
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
