//! Chapter — Task as the store an app keeps its work in.
//!
//! The nearest goal (`VISION.md`, "The rig loads from the cloud"): a
//! musician signs in on any machine and their work is there. For that,
//! an app reaching Task over the wire — through the real router, the
//! real gate, as the person — needs four things the Files lanes did not
//! give it until now:
//!
//! 1. **A place of its own**, without knowing a server path:
//!    `RootsService::create`.
//! 2. **A save that cannot silently lose another machine's**: an upload
//!    with an `Expect`, refused as `Stale` when the file moved on.
//! 3. **Access from who they are**, not only from grants: a member of the
//!    org reaches the org's roots by their role, while a client holding
//!    one granted folder reaches that folder and nothing else — not in a
//!    listing, not in a read, not in the live stream.
//! 4. **A manifest that can mean exact bytes**: a pinned `ContentRef`.
//!
//! Everything is asserted through `files_client::FilesClient`, the crate
//! an app would actually hold.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use files::id::PrincipalId;
use files::lane::caller::{Memberships, OrgRole, RoleFuture};
use files::service::access::Subject;
use files_client::{ClientError, Save, root_id};
use files_proto::model::RootFlavor;
use files_proto::service::FilesEvent;
use integration::client::Session;
use integration::scenario::Scenario;

/// The org's membership rows, as the server would inject them. The suite
/// signs people up on each org's own auth store and writes no rows, so
/// without this nobody has a role and only grants convey — which is the
/// half the rest of the suite already proves.
#[derive(Debug, Default)]
struct Rows(HashMap<PrincipalId, OrgRole>);

impl Memberships for Rows {
    fn role(&self, principal: PrincipalId) -> RoleFuture<'_> {
        let role = self.0.get(&principal).copied();
        Box::pin(async move { role })
    }
}

fn principal(subject: &Subject) -> PrincipalId {
    match subject {
        Subject::Person(p) => *p,
        other => panic!("the cast are people, not {other:?}"),
    }
}

// t[verify files.adopt.create]
// t[verify files.write.safe-save]
// t[verify files.access.granularity] — role baseline beside grants
#[tokio::test]
async fn a_member_keeps_their_work_in_task_and_a_client_sees_only_their_folder() {
    let s = Scenario::open().await;
    // Sam works at ACME: a member row. Casey is ACME's client: no row,
    // only the `Deliverables` grant the cast gave them.
    s.orgs
        .acme
        .backend
        .set_memberships(Arc::new(Rows(HashMap::from([(
            principal(&s.people.sam.subject),
            OrgRole::Member,
        )]))));

    let sam = Session::open(&s.orgs.acme, s.people.sam.token.clone()).await;
    let casey = Session::open(&s.orgs.acme, s.people.casey.token.clone()).await;
    let sam_files = sam.files_client().await;
    let casey_files = casey.files_client().await;

    // ── a client cannot mint a place in the org ──────────────────────
    let refused = casey_files
        .ensure_root("casey/stash", "Casey's stash", RootFlavor::Media)
        .await
        .expect_err("a grant on one folder is not a role");
    assert!(
        matches!(refused.fault(), Some(files::FilesFault::Denied { .. })),
        "refused as denied: {refused}"
    );

    // ── a member makes their store, and finds it again ───────────────
    let charts = sam_files
        .ensure_root("keyflow/charts", "Charts", RootFlavor::Media)
        .await
        .expect("a member creates a root");
    let again = sam_files
        .ensure_root("keyflow/charts", "Charts", RootFlavor::Media)
        .await
        .expect("and asking again is harmless");
    assert_eq!(charts.id, again.id);
    let root = root_id(&charts);

    // Casey subscribes to everything they can see before Sam writes.
    let casey_stream = casey.tree_stream().await;
    let heard_by_casey = Arc::new(std::sync::Mutex::new(Vec::<FilesEvent>::new()));
    let heard = Arc::clone(&heard_by_casey);
    let listening = tokio::spawn(async move {
        let _ = tokio::time::timeout(
            Duration::from_secs(4),
            files_client::FilesClient::events(&casey_stream, None, move |e| {
                heard.lock().unwrap().push(e);
                true
            }),
        )
        .await;
    });

    // ── a safe save, and a stale one refused ─────────────────────────
    let chart = br#"{"title":"Hosanna","key":"E","sections":["verse","chorus"]}"#;
    let first = sam_files
        .put(root, "hosanna.kf.json", chart, Save::create_only())
        .await
        .expect("the laptop saves the chart");
    let etag = first.content.clone().expect("an etag to save against");
    let edited = br#"{"title":"Hosanna","key":"F","sections":["verse","chorus"]}"#;
    sam_files
        .put(
            root,
            "hosanna.kf.json",
            edited,
            Save::replacing(etag.clone()),
        )
        .await
        .expect("the laptop saves over what it read");
    let late = sam_files
        .put(root, "hosanna.kf.json", b"{}", Save::replacing(etag))
        .await
        .expect_err("the phone saves over a copy that has moved on");
    assert!(late.is_stale(), "refused as stale, not applied: {late}");
    assert_eq!(
        sam_files
            .get(root, "hosanna.kf.json")
            .await
            .expect("read back"),
        edited.to_vec(),
        "the laptop's edit survived the phone's late save"
    );

    // ── the client sees none of it ───────────────────────────────────
    let listed = casey.roots().await.list().await.expect("Casey lists roots");
    assert!(
        listed.iter().all(|r| r.id != charts.id),
        "a root Casey holds nothing in is not even named to them"
    );
    match casey_files.entry(root, "hosanna.kf.json").await {
        Err(ClientError::Fault(_)) => {}
        other => panic!("Casey reads nothing in Sam's root: {other:?}"),
    }
    listening.await.expect("listener");
    {
        let heard = heard_by_casey.lock().unwrap();
        assert!(
            heard.iter().all(|e| !format!("{e:?}").contains("hosanna")),
            "the live stream told Casey nothing about Sam's root: {heard:?}"
        );
    }

    // …while the folder Casey *was* given is still theirs to read.
    let deliverables = casey
        .tree()
        .await
        .browse(s.acme_root, files::RootPath::parse("Deliverables").unwrap())
        .await;
    assert!(
        deliverables.is_ok(),
        "the grant still conveys: {deliverables:?}"
    );
}

// t[verify files.live.propagation] — the v2 stream, over the wire
#[tokio::test]
async fn an_app_hears_its_own_save_on_the_live_stream() {
    let s = Scenario::open().await;
    s.orgs
        .acme
        .backend
        .set_memberships(Arc::new(Rows(HashMap::from([(
            principal(&s.people.sam.subject),
            OrgRole::Member,
        )]))));
    let sam = Session::open(&s.orgs.acme, s.people.sam.token.clone()).await;
    let files = sam.files_client().await;
    let store = files
        .ensure_root("session/takes", "Takes", RootFlavor::Media)
        .await
        .expect("store");
    let root = root_id(&store);

    let stream = sam.tree_stream().await;
    let listening = tokio::spawn(async move {
        let mut landed = None;
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            files_client::FilesClient::events(&stream, Some(root), |e| {
                if let FilesEvent::Upload(files_proto::service::upload::UploadEvent::Completed(
                    entry,
                )) = e
                {
                    landed = Some(entry.path.to_string());
                    return false;
                }
                true
            }),
        )
        .await;
        landed
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    files
        .put(root, "vox take 3.wav", &[7u8; 50_000], Save::create_only())
        .await
        .expect("save");
    assert_eq!(
        listening.await.expect("listener").as_deref(),
        Some("vox take 3.wav"),
        "the subscriber heard the save land"
    );
}
