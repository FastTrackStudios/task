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
    /// The key behind `endpoint`, kept so a restart comes back as the
    /// same machine — what a real device does by persisting it
    /// (`files-daemon`'s `DeviceIdentity::endpoint_key`).
    key: iroh::SecretKey,
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
        // The project's page, as both companies declared it: the album is
        // music production *and* video production, which is what makes
        // REAPER's layout and Resolve's both apply here. A root's
        // conventions are what its project declares — a page saying only
        // music would leave `Proxies` unmapped (`session_media.rs`).
        std::fs::write(
            tree.join("project.md"),
            b"---\ntype: project\ntitle: Album\ncapabilities: [music-production, video-production]\n---\n\n# Album\n",
        )
        .unwrap();
        // Deliberately the largest thing here: it is what the laptop is
        // trying not to carry.
        std::fs::write(
            tree.join("Footage").join("Proxies").join("reel.mov"),
            vec![0u8; Self::REEL_BYTES as usize],
        )
        .unwrap();

        Self::holding(dir, tree, "Album").await
    }

    /// A laptop whose `tree` — already written, under `dir` — becomes
    /// its one root, adopted in place, checkpointed and served.
    ///
    /// What [`Self::open`] does with the album, for a chapter that needs
    /// a different tree on the device (a vault, say).
    pub async fn holding(dir: tempfile::TempDir, tree: std::path::PathBuf, name: &str) -> Self {
        let backend =
            files::FilesBackend::new(dir.path(), dir.path().join("vault")).expect("laptop backend");

        let album = files::service::RootsService::adopt(
            &backend,
            files::service::roots::AdoptRequest {
                path: tree.to_string_lossy().into_owned(),
                name: name.into(),
                flavor: RootFlavor::Media,
                hash_content: true,
            },
        )
        .await
        .expect("adopt the project on the laptop");
        let album = RootId::new(album.id);
        backend.settled(album).await;

        // Content into the store first, so dehydrating is dropping a
        // local copy rather than losing the file.
        files::service::VersionService::checkpoint(&backend, album, None)
            .await
            .expect("checkpoint");

        // The machine's identity, and its address. A real device persists
        // this key — `files.device.identity` requires the identity to
        // survive restart and the expiry of any login — and the test
        // keeps it in memory for the same reason: [`Self::restart`] comes
        // back on it.
        let key = iroh::SecretKey::generate();
        let endpoint = serve(&backend, key.clone()).await;

        Self {
            _dir: dir,
            backend,
            endpoint,
            key,
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
        let backend =
            files::FilesBackend::new(dir.path(), dir.path().join("vault")).expect("laptop backend");
        backend
            .adopt_replica(
                album.get(),
                "Album",
                tree.to_str().expect("utf-8 path"),
                RootFlavor::Media,
            )
            .expect("adopt the replica");

        let key = iroh::SecretKey::generate();
        let endpoint = serve(&backend, key.clone()).await;

        Self {
            _dir: dir,
            backend,
            endpoint,
            key,
            album,
            tree,
        }
    }

    /// Stop this laptop's endpoint and bring it back on the same key.
    ///
    /// What a restart is to every *other* machine: the same endpoint id
    /// at fresh addresses. The laptop's disk and backend are untouched —
    /// this is the device's identity surviving, which is the half of a
    /// restart the sync lane's ownership rests on.
    pub async fn restart(&mut self) {
        self.endpoint.close().await;
        self.endpoint = serve(&self.backend, self.key.clone()).await;
    }

    /// Stop this laptop altogether and start it again on the same disk
    /// and the same key.
    ///
    /// Unlike [`Self::restart`], the backend goes too: a fresh one is
    /// opened over the same data directory, so what comes back is what
    /// the laptop wrote down — the lane's state file, the stubs and the
    /// resident bytes on disk — and nothing its last process remembered.
    pub async fn reboot(&mut self) {
        self.endpoint.close().await;
        let dir = self._dir.path().to_path_buf();
        self.backend = files::FilesBackend::new(&dir, dir.join("vault"))
            .expect("the laptop's backend, reopened over its own disk");
        self.endpoint = serve(&self.backend, self.key.clone()).await;
    }

    /// This laptop's data directory — where its lane state and stores
    /// live, beside the tree.
    #[must_use]
    pub fn data_dir(&self) -> &std::path::Path {
        self._dir.path()
    }

    /// Open the files sync lane on `origin` **as this device**.
    ///
    /// Signs nothing: the endpoint iroh proved is the identity the far
    /// gate sees, and the far lane looks it up among its enrolled devices.
    pub async fn files_sync(&self, origin: iroh::EndpointId) -> files::SyncServiceClient {
        dial_files_sync(&self.endpoint, origin).await
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
    pub async fn dial_replica(&self, origin: &iroh::Endpoint) -> files_sync::SyncServiceClient {
        let link = architect::iroh_link::connect(&self.endpoint, origin.id())
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

/// Bind `key` and serve `backend` on it the way a device does — the
/// replica lane and a device's own corner of the sync lane, to endpoints
/// it admits. See `files_sync::serve_peer`.
async fn serve(backend: &files::FilesBackend, key: iroh::SecretKey) -> iroh::Endpoint {
    let endpoint = crate::net::bind(key).await;
    let serving = endpoint.clone();
    let served = backend.clone();
    tokio::spawn(async move {
        files_sync::serve_peer(served, "laptop".into(), &serving).await;
    });
    endpoint
}

/// Open the files sync lane on `origin`, dialling from `from`.
async fn dial_files_sync(
    from: &iroh::Endpoint,
    origin: iroh::EndpointId,
) -> files::SyncServiceClient {
    let link = architect::iroh_link::connect(from, origin)
        .await
        .expect("dial the origin");
    vox_core::initiator_on(link)
        .establish()
        .await
        .expect("establish the files sync lane")
}

/// A device that talks to an org's host and holds nothing the test
/// looks at — only an identity.
///
/// What the host-side chapter needs from a machine is exactly what
/// `files.device.identity` says a device has: a key it holds and
/// persists, so that it is the same device after it restarts. Its disk
/// is the daemon's business and not what is being asserted.
pub struct PeerDevice {
    key: iroh::SecretKey,
    pub endpoint: iroh::Endpoint,
}

impl PeerDevice {
    /// A machine with a fresh key, bound and published.
    pub async fn new() -> Self {
        let key = iroh::SecretKey::generate();
        let endpoint = crate::net::bind(key.clone()).await;
        Self { key, endpoint }
    }

    /// The endpoint id this machine presents — what an org enrols.
    #[must_use]
    pub fn id(&self) -> String {
        self.endpoint.id().to_string()
    }

    /// Stop and come back on the same key — a restart, as every other
    /// machine sees one.
    pub async fn restart(&mut self) {
        self.endpoint.close().await;
        self.endpoint = crate::net::bind(self.key.clone()).await;
    }

    /// Open the files sync lane on `origin` as this device.
    pub async fn files_sync(&self, origin: iroh::EndpointId) -> files::SyncServiceClient {
        dial_files_sync(&self.endpoint, origin).await
    }
}
