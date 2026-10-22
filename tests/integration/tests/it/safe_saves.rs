#![allow(clippy::large_futures)]
//! Chapter — **two clients edit one thing, and the late save is told.**
//!
//! A person opens the studio's chart on a laptop and on a phone. They
//! fix a chord on the phone and save; then they finish a different edit
//! on the laptop and save that too. Before this chapter the laptop's
//! save simply replaced the phone's — a whole document written over
//! another, with nothing on either screen to say a fix had been lost.
//!
//! The Files lane never had that problem: an edited file is saved
//! `Save::replacing(etag)` and a file that moved on is `Stale`
//! (`docs/app-integration.md`, *Saving safely*). This chapter holds the
//! library lanes to the same rule, through the wire, on the seeded world:
//!
//! - a **chart** (`track-one`) and a **song** (`track-one`) carry a
//!   `revision` on every read; a save that sends it back lands only on
//!   that revision, and otherwise is `ResourcesError::Stale` carrying the
//!   revision the document is at now;
//! - a **setlist** (`Album Launch Set`) carries a `revision` too, and
//!   `CollectionService::save` — the whole-list save a set-list editor
//!   makes — is `CollectionError::Stale` when anything changed it since
//!   it was read.
//!
//! Each case is asserted three ways, because each is a separate claim:
//! the late save is refused; the early save is **still there** (refusal
//! that also lost the first edit would pass the first assertion); and the
//! late client recovers by re-reading and saving against the revision it
//! now holds, which is the whole of what "show both, let them choose"
//! asks of an app.
//!
//! The same three cases with no host at all — two device nodes, each
//! with its own client — are `apps/server/tests/it/safe_saves_e2e.rs`.

use collection_proto::{Collection, CollectionError, CollectionKind, CollectionSave};
use integration::client::Session;
use integration::scenario::Scenario;
use links_proto::NodeRef;
use resources_proto::{ChartDoc, ResourcesError, SongDoc};

/// The seeded chart and song both chapters edit: Track One, the
/// studio's first song, and its main chart.
const SEEDED: &str = "track-one";
/// The seeded setlist (`example_org::DECLARED_COLLECTIONS`).
const SEEDED_SET: &str = "Album Launch Set";

fn resources_err(e: architect::vox::VoxError<ResourcesError>) -> ResourcesError {
    match e {
        architect::vox::VoxError::User(e) => *e,
        other => ResourcesError::Io(other.to_string()),
    }
}

fn collection_err(e: architect::vox::VoxError<CollectionError>) -> CollectionError {
    match e {
        architect::vox::VoxError::User(e) => *e,
        other => CollectionError::Io(other.to_string()),
    }
}

async fn chart(who: &Session) -> ChartDoc {
    who.resources()
        .await
        .chart(SEEDED.to_owned())
        .await
        .expect("the seeded chart")
}

async fn song(who: &Session) -> SongDoc {
    who.resources()
        .await
        .song(SEEDED.to_owned())
        .await
        .expect("the seeded song")
}

async fn seeded_set(who: &Session) -> Collection {
    who.collections()
        .await
        .list(
            "acme-audio".to_owned(),
            Some(CollectionKind::new("setlist")),
        )
        .await
        .expect("list setlists")
        .into_iter()
        .find(|c| c.title == SEEDED_SET)
        .unwrap_or_else(|| panic!("the seed plants `{SEEDED_SET}`"))
}

fn save_of(c: &Collection, title: &str, items: Vec<NodeRef>) -> CollectionSave {
    CollectionSave {
        id: c.id.clone(),
        revision: c.revision,
        title: title.to_owned(),
        items,
    }
}

// t[verify storage.save.revision] — two clients, one seeded chart: the late
// save is Stale, the early one survives, and a re-read save lands
#[tokio::test(flavor = "multi_thread")]
async fn the_late_chart_save_is_stale_not_an_overwrite() {
    let world = Scenario::open().await;
    let (laptop, phone) = (world.as_alice().await, world.as_alice().await);

    let on_laptop = chart(&laptop).await;
    let on_phone = chart(&phone).await;
    assert!(
        !on_laptop.revision.is_empty(),
        "a read carries the revision"
    );
    assert_eq!(
        on_laptop.revision, on_phone.revision,
        "one document, one revision"
    );

    // The phone fixes a chord first.
    let phone_fix = format!("{}\n; phone: bridge is Bm, not D\n", on_phone.source);
    let saved = phone
        .resources()
        .await
        .upsert_chart(ChartDoc {
            source: phone_fix.clone(),
            ..on_phone.clone()
        })
        .await
        .expect("the phone's save lands");
    assert_ne!(saved.revision, on_phone.revision, "a save moves it on");

    // The laptop, still holding the old revision, saves its own edit.
    let laptop_edit = format!("{}\n; laptop: tag the outro\n", on_laptop.source);
    let late = laptop
        .resources()
        .await
        .upsert_chart(ChartDoc {
            source: laptop_edit.clone(),
            ..on_laptop.clone()
        })
        .await
        .map_err(resources_err)
        .expect_err("the laptop's save was made against an old revision");
    assert_eq!(
        late,
        ResourcesError::Stale(saved.revision.clone()),
        "Stale, carrying the revision the chart is at now"
    );

    // The phone's fix is what is there — the refusal wrote nothing.
    let now = chart(&laptop).await;
    assert_eq!(now.source, phone_fix, "the early save survived");
    assert_eq!(now.revision, saved.revision);

    // Show both, let them choose: the laptop takes the current chart,
    // keeps both edits, and saves against what it now holds.
    let merged = format!("{}; laptop: tag the outro\n", now.source);
    let landed = laptop
        .resources()
        .await
        .upsert_chart(ChartDoc {
            source: merged.clone(),
            ..now.clone()
        })
        .await
        .expect("a save against the current revision lands");
    let after = chart(&phone).await;
    assert_eq!(after.source, merged);
    assert_eq!(
        after.revision, landed.revision,
        "the revision handed back is the one on disk"
    );

    // An empty revision is the old, unconditional save — kept, because
    // every client written before revisions sends it.
    phone
        .resources()
        .await
        .upsert_chart(ChartDoc {
            revision: String::new(),
            ..on_phone
        })
        .await
        .expect("no revision, no condition");
}

// t[verify storage.save.revision] — two clients, one seeded song
#[tokio::test(flavor = "multi_thread")]
async fn the_late_song_save_is_stale_not_an_overwrite() {
    let world = Scenario::open().await;
    let (laptop, phone) = (world.as_alice().await, world.as_alice().await);

    let on_laptop = song(&laptop).await;
    let on_phone = song(&phone).await;
    assert!(!on_laptop.revision.is_empty());
    assert_eq!(on_laptop.revision, on_phone.revision);

    let saved = phone
        .resources()
        .await
        .upsert_song(SongDoc {
            key: "Bb".into(),
            ..on_phone.clone()
        })
        .await
        .expect("the phone's save lands");

    let late = laptop
        .resources()
        .await
        .upsert_song(SongDoc {
            tags: vec!["opener".into()],
            ..on_laptop.clone()
        })
        .await
        .map_err(resources_err)
        .expect_err("the laptop's save was made against an old revision");
    assert_eq!(late, ResourcesError::Stale(saved.revision.clone()));

    let now = song(&laptop).await;
    assert_eq!(now.key, "Bb", "the early save survived");
    assert!(
        !now.tags.contains(&"opener".to_owned()),
        "the refused save wrote nothing"
    );

    let mut tags = now.tags.clone();
    tags.push("opener".into());
    laptop
        .resources()
        .await
        .upsert_song(SongDoc { tags, ..now })
        .await
        .expect("a save against the current revision lands");
    let after = song(&phone).await;
    assert_eq!(after.key, "Bb");
    assert!(after.tags.contains(&"opener".to_owned()));
}

// t[verify storage.save.collection] — two clients, one seeded setlist: the
// late whole-list save is Stale, whatever changed it in between
#[tokio::test(flavor = "multi_thread")]
async fn the_late_setlist_save_is_stale_not_an_overwrite() {
    let world = Scenario::open().await;
    let (laptop, phone) = (world.as_alice().await, world.as_alice().await);

    let on_laptop = seeded_set(&laptop).await;
    let on_phone = seeded_set(&phone).await;
    assert_eq!(on_laptop.revision, on_phone.revision);
    let order: Vec<NodeRef> = on_phone.items.iter().map(|it| it.node.clone()).collect();
    assert!(order.len() >= 2, "the seeded set has songs to move");

    // The phone opens with the second song instead.
    let mut swapped = order.clone();
    swapped.swap(0, 1);
    let saved = phone
        .collections()
        .await
        .save(save_of(&on_phone, SEEDED_SET, swapped.clone()))
        .await
        .expect("the phone's save lands");
    assert_eq!(saved.revision, on_phone.revision + 1);

    // The laptop drops the last item from the list it read.
    let mut shorter = order.clone();
    shorter.pop();
    let late = laptop
        .collections()
        .await
        .save(save_of(&on_laptop, SEEDED_SET, shorter))
        .await
        .map_err(collection_err)
        .expect_err("the laptop's list was made against an old revision");
    assert_eq!(late, CollectionError::Stale(saved.revision));

    let now = seeded_set(&laptop).await;
    let held: Vec<NodeRef> = now.items.iter().map(|it| it.node.clone()).collect();
    assert_eq!(held, swapped, "the early save survived, whole");

    // A one-item change by anyone moves the revision on too: the
    // laptop's list was made without it, so it is stale against it.
    let renamed = phone
        .collections()
        .await
        .rename(now.id.clone(), "Album Launch Set (final)".into())
        .await
        .expect("rename");
    let stale_again = laptop
        .collections()
        .await
        .save(save_of(&now, SEEDED_SET, held.clone()))
        .await
        .map_err(collection_err)
        .expect_err("a rename since the read makes the list stale");
    assert_eq!(stale_again, CollectionError::Stale(renamed.revision));

    // Re-read, then save: lands.
    let fresh = laptop
        .collections()
        .await
        .get(now.id.clone())
        .await
        .expect("get")
        .expect("the set is still there");
    let mut kept = held;
    kept.pop();
    let landed = laptop
        .collections()
        .await
        .save(save_of(&fresh, &fresh.title, kept.clone()))
        .await
        .expect("a save against the current revision lands");
    assert_eq!(landed.revision, fresh.revision + 1);
    assert_eq!(
        landed
            .items
            .iter()
            .map(|it| it.node.clone())
            .collect::<Vec<_>>(),
        kept
    );
}
