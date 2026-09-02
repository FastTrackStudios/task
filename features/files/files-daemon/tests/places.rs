//! Places the org offers, and what the agent does with them.
//!
//! The mount is composed from places, and until now every place was
//! typed by hand — which is how a machine came to show `Wiki/Knowledge`
//! and nothing the org had grown since. The replica lane now carries
//! each root's place from the org, and these pin what the agent does
//! with the offer: take it when nobody here has decided, keep a
//! person's decision when they have, refuse a place another root
//! already holds, and remember read-only across a restart. No network:
//! an offer is a `WireRoot`, and the agent's answer is its own record.

use files::FilesBackend;
use files_daemon::SyncDaemon;
use files_sync::WireRoot;

fn offer(name: &str, place: Option<&str>, read_only: bool) -> WireRoot {
    WireRoot {
        id: uuid::Uuid::new_v4(),
        name: name.into(),
        flavor: files::RootFlavor::Media,
        place: place.map(str::to_string),
        read_only,
    }
}

fn daemon(dir: &std::path::Path) -> SyncDaemon {
    let backend = FilesBackend::new(dir, dir.join("vault")).unwrap();
    let daemon = SyncDaemon::open(backend, dir.join("daemon")).unwrap();
    daemon.restore_places();
    daemon
}

/// Two named wikis, a subscribed copy and a resource library offered by
/// an org land at the places the org said, with the read-only ones
/// marked — the top-level entries the mount then shows.
#[tokio::test(flavor = "multi_thread")]
async fn an_offered_place_is_taken_when_nobody_here_decided() {
    let dir = tempfile::tempdir().unwrap();
    let d = daemon(dir.path());

    let offered = [
        offer("Wiki", Some("acme/Wiki"), false),
        offer("Wiki — music-theory", Some("acme/Wiki/music-theory"), false),
        offer("Wiki — bible-study", Some("acme/Wiki/bible-study"), false),
        offer(
            "Subscribed — acme.test — music-theory",
            Some("acme/Subscribed/acme.test/music-theory"),
            true,
        ),
        offer("Resources", Some("acme/Resources"), true),
        // A device's root, or an old server: no place, placed by name.
        offer("Ghosts", None, false),
    ];
    let mut places: Vec<(String, bool)> = offered
        .iter()
        .map(|o| {
            (
                d.place_offered(o)
                    .unwrap_or_else(|| d.place_of(o.id, &o.name)),
                d.is_read_only(o.id),
            )
        })
        .collect();
    places.sort();
    assert_eq!(
        places,
        [
            ("Ghosts".to_string(), false),
            ("acme/Resources".to_string(), true),
            ("acme/Subscribed/acme.test/music-theory".to_string(), true),
            ("acme/Wiki".to_string(), false),
            ("acme/Wiki/bible-study".to_string(), false),
            ("acme/Wiki/music-theory".to_string(), false),
        ]
    );
}

/// `place` is a person's decision; an org's suggestion does not
/// overrule it.
#[tokio::test(flavor = "multi_thread")]
async fn a_place_somebody_set_here_wins_over_the_offer() {
    let dir = tempfile::tempdir().unwrap();
    let d = daemon(dir.path());
    let wiki = offer("Wiki — notes", Some("acme/Wiki/notes"), false);
    d.set_place(wiki.id, "acme/Wiki/My Notes").unwrap();
    assert_eq!(
        d.place_offered(&wiki).as_deref(),
        Some("acme/Wiki/My Notes")
    );
    assert_eq!(d.place_of(wiki.id, ""), "acme/Wiki/My Notes");
}

/// A place names one root. A machine that shared `~/.task/orgs/acme/wiki`
/// by hand and placed it at `acme/Wiki` keeps it there when the org
/// offers its own `Wiki` root at the same place; the offer is logged
/// and the root is placed by name until somebody sorts the two out.
#[tokio::test(flavor = "multi_thread")]
async fn a_place_another_root_holds_is_not_taken() {
    let dir = tempfile::tempdir().unwrap();
    let d = daemon(dir.path());
    let by_hand = uuid::Uuid::new_v4();
    d.set_place(by_hand, "acme/Wiki").unwrap();

    let offered = offer("Wiki", Some("acme/Wiki"), false);
    assert_eq!(d.place_offered(&offered), None);
    assert_eq!(d.place_of(by_hand, ""), "acme/Wiki");
    assert_eq!(d.place_of(offered.id, &offered.name), "Wiki");
}

/// An offer that climbs out of the tree is a string from another
/// machine and is treated as one.
#[tokio::test(flavor = "multi_thread")]
async fn an_offer_outside_the_tree_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let d = daemon(dir.path());
    for bad in ["../etc", "acme//Wiki", "/", ""] {
        let o = offer("x", Some(bad), false);
        assert_eq!(d.place_offered(&o), None, "{bad:?}");
    }
}

/// Read-only is a fact about how a root is shown, recorded beside the
/// places and restored with them — a reboot must not turn a subscribed
/// copy writable.
#[tokio::test(flavor = "multi_thread")]
async fn read_only_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let copy = offer(
        "Subscribed — acme.test — music-theory",
        Some("acme/Subscribed/acme.test/music-theory"),
        true,
    );
    {
        let d = daemon(dir.path());
        d.place_offered(&copy);
        assert!(d.is_read_only(copy.id));
    }
    let d = daemon(dir.path());
    assert!(d.is_read_only(copy.id));
    assert_eq!(
        d.place_of(copy.id, ""),
        "acme/Subscribed/acme.test/music-theory"
    );
}
