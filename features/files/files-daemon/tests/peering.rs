//! Two machines, each serving and each pulling — sync that is actually
//! bidirectional in the shipped code path.
//!
//! The engine (`files-sync`) has always been able to do this: its own
//! suite proves edits flow both ways, because there both sides are
//! backends in one process and the test wires each to serve the other.
//! What a *daemon* could do was strictly less — it dialled a
//! coordinator and pulled, and mounted nothing anyone could dial back.
//! On a real desk that is a laptop whose work never leaves it, with no
//! error to show for it, because the missing half is not a failure: it
//! is a call nobody can make.
//!
//! So these tests never touch `reconcile` directly. They drive
//! [`SyncDaemon`] the way the binary and the desktop app do — bind,
//! admit, dial by endpoint id, tick — and assert on bytes on the other
//! machine's disk.
//!
//! # Why an address book
//!
//! A deployed endpoint publishes to n0's DNS and is dialled by bare id
//! from anywhere. A test process has no internet and would not wait for
//! one, so every endpoint here carries a [`files::AddressBook`] and
//! publishes into it as it binds — the same substitution the
//! integration suite makes, and beneath `connect`, so nothing above it
//! knows there is a book at all.

use std::sync::OnceLock;
use std::time::Duration;

use architect::iroh_link::iroh;
use files::cadence::{CadenceConfig, TestClock};
use files::{AddressBook, FilesBackend, FilesService as _, RootFlavor};
use files_daemon::SyncDaemon;

/// The addresses every endpoint in this process can resolve.
fn book() -> AddressBook {
    static BOOK: OnceLock<AddressBook> = OnceLock::new();
    BOOK.get_or_init(AddressBook::new).clone()
}

/// Bind an endpoint and publish it, standing in for the pkarr record a
/// deployed endpoint writes at the same moment.
async fn bind(key: iroh::SecretKey) -> iroh::Endpoint {
    let endpoint = files::bind_endpoint(key, Some(book()))
        .await
        .expect("bind an endpoint");
    book().add_endpoint_info(endpoint.addr());
    endpoint
}

/// One machine: a store, a daemon over it, and an endpoint it serves
/// its replica lane on.
struct Machine {
    _dir: tempfile::TempDir,
    dir: std::path::PathBuf,
    backend: FilesBackend,
    daemon: SyncDaemon,
    endpoint_id: String,
    /// This machine's key, kept so a "restart" can come back at the same
    /// address — which is what a real one does, reading it off disk.
    key: iroh::SecretKey,
    clock: std::sync::Arc<TestClock>,
}

impl Machine {
    async fn open() -> Self {
        let dir = tempfile::tempdir().expect("machine dir");
        // A test clock, so the cadence assertions below simulate
        // quiescence rather than sleeping through it.
        let clock = std::sync::Arc::new(TestClock::default());
        let backend = FilesBackend::with_cadence(
            dir.path(),
            dir.path().join("vault"),
            CadenceConfig::default(),
            clock.clone(),
        )
        .expect("backend");
        let daemon = SyncDaemon::open(backend.clone(), dir.path().join("daemon")).expect("daemon");
        let key = iroh::SecretKey::generate();
        let endpoint = bind(key.clone()).await;
        let endpoint_id = daemon.attach_endpoint(endpoint);
        Self {
            dir: dir.path().to_path_buf(),
            _dir: dir,
            backend,
            daemon,
            endpoint_id,
            key,
            clock,
        }
    }

    /// A root with one file in it, checkpointed — content in the store,
    /// which is what a peer can pull.
    async fn with_album(&self, contents: &[u8]) -> uuid::Uuid {
        let tree = self.dir.join("Album");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("mix.wav"), contents).unwrap();
        let root = self
            .backend
            .create_root(
                tree.to_string_lossy().into_owned(),
                "Album".into(),
                RootFlavor::Media,
            )
            .await
            .expect("create the root");
        self.backend
            .checkpoint_now(root.id, None)
            .await
            .expect("checkpoint");
        root.id
    }

    fn album_tree(&self) -> std::path::PathBuf {
        self.dir.join("Album")
    }

    fn read(&self, rel: &str) -> Vec<u8> {
        std::fs::read(self.album_tree().join(rel)).expect("read on this machine")
    }
}

/// The headline: a laptop's offline work reaches the server, because
/// the server can pull it.
///
/// The pull direction was never in doubt. What this asserts is the one
/// the daemon could not do at all — the laptop *answering* a dial, so
/// its own history has a way off the machine. Both halves run through
/// `SyncDaemon`, over iroh, with admission on both sides.
// t[verify files.topology.multi-server]
#[tokio::test(flavor = "multi_thread")]
async fn edits_flow_both_ways_between_two_daemons() {
    let server = Machine::open().await;
    let laptop = Machine::open().await;
    let album = server.with_album(b"the rough mix").await;

    // Admission is symmetric because pulling is: each side is about to
    // dial the other, and each gate knows nothing but endpoint ids.
    server.daemon.admit_peer(&laptop.endpoint_id);
    laptop.daemon.admit_peer(&server.endpoint_id);

    // Down: the laptop takes the album, adopting it sight unseen.
    laptop
        .daemon
        .sync_from_peer(&server.endpoint_id, album, vec![], &laptop.dir)
        .await
        .expect("choose the album from the server");
    laptop.daemon.tick().await;
    assert_eq!(
        laptop.read("mix.wav"),
        b"the rough mix",
        "the laptop never received the album"
    );

    // Work on the plane, and a save point before the lid closes.
    std::fs::write(laptop.album_tree().join("mix.wav"), b"the final mix").unwrap();
    laptop
        .daemon
        .checkpoint_now(album)
        .await
        .expect("checkpoint the laptop's work");

    // Up: the server pulls the laptop. This is the call that had no
    // answer before — the daemon served nothing.
    server
        .daemon
        .sync_from_peer(&laptop.endpoint_id, album, vec![], &server.dir)
        .await
        .expect("the server chooses to pull the laptop");
    server.daemon.tick().await;
    assert_eq!(
        server.read("mix.wav"),
        b"the final mix",
        "the laptop's work never left the laptop"
    );
}

/// A machine that holds nothing starts from "what have you got".
///
/// Without this the daemon had to be *told* a root id, which is fine
/// for an app that already browsed the org and impossible for a fresh
/// install, which is exactly the case an installer creates.
#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_machine_discovers_what_the_peer_holds() {
    let server = Machine::open().await;
    let fresh = Machine::open().await;
    let album = server.with_album(b"stems").await;
    server.daemon.admit_peer(&fresh.endpoint_id);

    let found = fresh
        .daemon
        .peer_roots(&server.endpoint_id)
        .await
        .expect("ask the server what it holds");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].id, album);
    assert_eq!(found[0].name, "Album");

    // And adopting it needs nothing the discovery did not carry: no
    // name, no flavor, no path on the far side.
    fresh
        .daemon
        .sync_from_peer(&server.endpoint_id, album, vec![], &fresh.dir)
        .await
        .expect("adopt and choose");
    fresh.daemon.tick().await;
    assert_eq!(fresh.read("mix.wav"), b"stems");
}

/// A verified stranger is still a stranger.
///
/// The handshake proves an endpoint id; it says nothing about whether
/// this store serves that id. Admission is the whole model on a device,
/// so this is the test that it is actually applied and not merely
/// available.
// t[verify files.peering.scope] — a peer sees nothing about an org it
// does not host, and an endpoint the store never admitted hosts nothing
#[tokio::test(flavor = "multi_thread")]
async fn an_unadmitted_peer_is_refused() {
    let server = Machine::open().await;
    let stranger = Machine::open().await;
    server.with_album(b"private").await;
    // Deliberately no `admit_peer`.

    let refused = stranger.daemon.peer_roots(&server.endpoint_id).await;
    assert!(
        refused.is_err(),
        "an unadmitted machine listed the org's roots: {refused:?}"
    );
}

/// Local edits become history on their own, or there is nothing for the
/// other side to pull.
///
/// The daemon used to reconcile against a local store that no
/// background work ever wrote to: unless a person pressed
/// checkpoint-now, a laptop's edits stayed uncaptured and the "pull me"
/// half would have found nothing even once it existed. `tick` runs the
/// cadence pass first, so a session that has gone quiet is captured
/// before the pull that carries it.
#[tokio::test(flavor = "multi_thread")]
async fn a_tick_captures_the_local_session_before_it_pulls() {
    let laptop = Machine::open().await;
    let album = laptop.with_album(b"take one").await;
    let before = laptop
        .backend
        .sync_heads(album)
        .expect("heads")
        .first()
        .cloned();

    // Work, then quiet — the shape of a session ending, simulated
    // rather than slept through.
    std::fs::write(laptop.album_tree().join("mix.wav"), b"take two").unwrap();
    laptop
        .backend
        .hint_activity(album, vec!["mix.wav".into()])
        .await
        .expect("hint");
    laptop.clock.advance_minutes(45);

    laptop.daemon.tick().await;

    let after = laptop
        .backend
        .sync_heads(album)
        .expect("heads")
        .first()
        .cloned();
    assert_ne!(
        before, after,
        "a quiet session was never captured, so a peer pulling this machine would get its old work"
    );
}

/// Un-admitting a machine is possible, and it keeps the content.
///
/// Admission is the one list that decides who may read this machine's
/// whole history. A list you can add to and not remove from is not a
/// list anybody should trust — and the removal has to leave the files
/// alone, because "stop syncing with that laptop" is not "delete the
/// album".
#[tokio::test(flavor = "multi_thread")]
async fn forgetting_a_peer_stops_the_sync_and_keeps_the_files() {
    let server = Machine::open().await;
    let laptop = Machine::open().await;
    let album = server.with_album(b"the stems").await;
    server.daemon.admit_peer(&laptop.endpoint_id);
    laptop.daemon.admit_peer(&server.endpoint_id);
    laptop
        .daemon
        .sync_from_peer(&server.endpoint_id, album, vec![], &laptop.dir)
        .await
        .expect("choose the album");
    laptop.daemon.tick().await;
    assert_eq!(laptop.read("mix.wav"), b"the stems");

    // The laptop stops trusting the server, and stops pulling it.
    laptop.daemon.dismiss_peer(&server.endpoint_id);
    laptop.daemon.remove_sync_choice(album);

    assert!(
        laptop.daemon.peers().is_empty(),
        "the peer is still admitted: {:?}",
        laptop.daemon.peers()
    );
    assert!(
        laptop.daemon.status().roots.is_empty(),
        "it is still set to pull from a machine it no longer admits"
    );
    assert_eq!(
        laptop.read("mix.wav"),
        b"the stems",
        "forgetting a peer deleted the content it had brought"
    );

    // And it stays stopped: new work on the server does not arrive.
    std::fs::write(server.album_tree().join("mix.wav"), b"a newer mix").unwrap();
    server
        .backend
        .checkpoint_now(album, None)
        .await
        .expect("checkpoint");
    laptop.daemon.tick().await;
    assert_eq!(laptop.read("mix.wav"), b"the stems");
}

/// What a machine was syncing survives a restart.
///
/// A background service that forgets its choices when the machine
/// reboots is a background service that stops syncing — quietly, at the
/// least convenient moment, and looking exactly like one that is up to
/// date. The peer is dialled again on the way back, so the assertion is
/// on the *restored* daemon actually pulling.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_resumes_what_it_was_syncing() {
    let server = Machine::open().await;
    let laptop = Machine::open().await;
    let album = server.with_album(b"before the reboot").await;
    server.daemon.admit_peer(&laptop.endpoint_id);
    laptop.daemon.admit_peer(&server.endpoint_id);

    laptop
        .daemon
        .sync_from_peer(&server.endpoint_id, album, vec![], &laptop.dir)
        .await
        .expect("choose the album");
    laptop.daemon.tick().await;

    // The reboot: a second daemon over the same data dir and store,
    // told nothing about any peer.
    let restarted = SyncDaemon::open(laptop.backend.clone(), laptop.dir.join("daemon"))
        .expect("reopen the daemon");
    // On the same key: a real restart reads it off disk and comes back
    // at the same address, which is what the server admitted.
    restarted.attach_endpoint(bind(laptop.key.clone()).await);
    let resumed = restarted.restore_choices().await;
    assert_eq!(resumed, 1, "the laptop came back syncing nothing");

    // And it is a live choice, not a remembered name: new work on the
    // server reaches the restarted machine without anyone choosing
    // anything again.
    std::fs::write(server.album_tree().join("mix.wav"), b"after the reboot").unwrap();
    server
        .backend
        .checkpoint_now(album, None)
        .await
        .expect("checkpoint");
    restarted.tick().await;
    assert_eq!(
        laptop.read("mix.wav"),
        b"after the reboot",
        "the restarted machine is not pulling any more"
    );
}

/// Restarting keeps the address, or every admission it was given
/// becomes a lie.
// t[verify files.device.identity]
#[tokio::test(flavor = "multi_thread")]
async fn the_endpoint_id_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).unwrap();
    let data = dir.path().join("daemon");

    let first = SyncDaemon::open(backend.clone(), &data).unwrap();
    let id = first
        .bind_peering(Some(book()))
        .await
        .expect("bind the device endpoint");

    // A second daemon over the same data dir is the same machine: same
    // device id, same address, no re-registration anywhere.
    let second = SyncDaemon::open(backend, &data).unwrap();
    let again = second
        .bind_peering(Some(book()))
        .await
        .expect("rebind after restart");
    assert_eq!(id, again, "the device came back at a different address");
    assert_eq!(second.status().endpoint_id.as_deref(), Some(again.as_str()));
}

/// Dialling something that is not there fails as an error, not as a
/// hang: a tick that waits forever on an absent laptop never reaches
/// the peers that are present.
#[tokio::test(flavor = "multi_thread")]
async fn dialling_a_machine_that_is_not_there_gives_up() {
    let machine = Machine::open().await;
    let absent = iroh::SecretKey::generate().public().to_string();

    let started = std::time::Instant::now();
    let result = machine.daemon.peer_roots(&absent).await;
    assert!(result.is_err(), "dialled a machine that does not exist");
    assert!(
        started.elapsed() < files_daemon::peering::DIAL_TIMEOUT + Duration::from_secs(5),
        "the dial did not give up within its own timeout"
    );
}
