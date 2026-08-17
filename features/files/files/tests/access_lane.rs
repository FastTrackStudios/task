//! `AccessService` — the inward half of sharing, against a real
//! `FilesBackend`.
//!
//! In-process rather than over `LocalServer`, like `roots_lane.rs` and
//! for the same reason: the lane has no `permits.rs` row yet, so it is
//! not mounted on a router. These call the trait — and the
//! subject-carrying methods beneath it — exactly as a dispatcher would.
//!
//! The subject-carrying methods are the ones worth testing. No caller
//! identity reaches this trait, so `effective()` answers for the process
//! principal, which the lane treats as the root's owner; everything
//! interesting about granularity is about *somebody else*, and
//! `effective_for` / `authorise` are how that question is asked.

use files::FilesBackend;
use files::lane::access::this_principal;
use files_proto::id::{GrantId, PrincipalId, RootId, ShareId};
use files_proto::model::RootFlavor;
use files_proto::service::access::{AccessService, Capability, Subject};
use files_proto::service::roots::{AdoptRequest, RootsService};
use files_proto::{FilesFault, RootPath};

/// A backend with one adopted root holding `Sessions/Song` and
/// `Renders` — a granted subtree and a sibling that must stay invisible.
async fn adopted() -> (tempfile::TempDir, FilesBackend, RootId) {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let dir = data_dir.path().join("album");
    std::fs::create_dir_all(dir.join("Sessions").join("Song")).unwrap();
    std::fs::create_dir(dir.join("Renders")).unwrap();
    std::fs::write(dir.join("Sessions").join("Song").join("mix.wav"), b"take").unwrap();
    std::fs::write(dir.join("Renders").join("master.wav"), b"final").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let root = backend
        .adopt(AdoptRequest {
            path: dir.to_string_lossy().into_owned(),
            name: "Album".into(),
            flavor: RootFlavor::Media,
            hash_content: false,
        })
        .await
        .expect("adopt");
    (data_dir, backend, RootId::new(root.id))
}

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("path")
}

fn colleague() -> Subject {
    Subject::Person(PrincipalId::generate())
}

// t[verify files.access.internal-sharing]
#[tokio::test(flavor = "multi_thread")]
async fn a_folder_is_grantable_without_minting_a_link() {
    let (_tmp, backend, root) = adopted().await;
    let sam = colleague();

    let grant = backend
        .grant(
            sam.clone(),
            root,
            p("Sessions/Song"),
            vec![Capability::Read, Capability::Comment],
        )
        .await
        .expect("grant");
    assert_eq!(grant.subject, sam);
    assert_eq!(grant.granted_by, this_principal());

    let eff = backend
        .effective_for(&sam, root, &p("Sessions/Song/mix.wav"))
        .expect("resolved inside the granted subtree");
    assert!(eff.capabilities.contains(&Capability::Read));
    assert!(eff.capabilities.contains(&Capability::Comment));
    assert!(
        !eff.capabilities.contains(&Capability::Write),
        "a grant conveys the set it named and nothing adjacent"
    );

    assert!(
        backend.shares(root).await.expect("shares").is_empty(),
        "sharing inward must not mint a public link"
    );
}

// t[verify files.access.internal-sharing]
#[tokio::test(flavor = "multi_thread")]
async fn overlapping_grants_union_rather_than_replace() {
    let (_tmp, backend, root) = adopted().await;
    let sam = colleague();

    backend
        .grant(sam.clone(), root, p(""), vec![Capability::Read])
        .await
        .unwrap();
    backend
        .grant(
            sam.clone(),
            root,
            p("Sessions"),
            vec![Capability::Read, Capability::Write],
        )
        .await
        .unwrap();

    let eff = backend
        .effective_for(&sam, root, &p("Sessions/Song"))
        .expect("resolved");
    assert_eq!(
        eff.capabilities,
        vec![Capability::Read, Capability::Write],
        "one resolved answer, deduplicated, not a rules dump"
    );
}

// t[verify files.access.internal-sharing]
#[tokio::test(flavor = "multi_thread")]
async fn revocation_binds_on_the_next_request() {
    let (_tmp, backend, root) = adopted().await;
    let sam = colleague();

    let grant = backend
        .grant(sam.clone(), root, p("Sessions"), vec![Capability::Read])
        .await
        .unwrap();
    assert!(backend.effective_for(&sam, root, &p("Sessions")).is_ok());

    backend.revoke(grant.id).await.expect("revoke");

    // The very next call, with nothing flushed and no session to expire.
    match backend
        .effective_for(&sam, root, &p("Sessions"))
        .expect_err("revoked")
    {
        FilesFault::PathNotFound(path) => assert_eq!(path, p("Sessions")),
        other => panic!("expected absence after revocation, got {other:?}"),
    }
    assert!(
        backend.grants(root, None).await.unwrap().is_empty(),
        "and it is gone from the listing too"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_something_that_conveys_nothing_says_so() {
    let (_tmp, backend, _root) = adopted().await;
    let ghost = GrantId::generate();
    match backend.revoke(ghost).await.expect_err("nothing to revoke") {
        FilesFault::GrantRevoked(id) => assert_eq!(id, ghost),
        other => panic!("expected GrantRevoked, got {other:?}"),
    }
}

// t[verify files.access.granularity]
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_on_a_folder_says_nothing_about_its_parents() {
    let (_tmp, backend, root) = adopted().await;
    let sam = colleague();
    backend
        .grant(
            sam.clone(),
            root,
            p("Sessions/Song"),
            vec![Capability::Read, Capability::Write],
        )
        .await
        .unwrap();

    // The ancestor is nameable — Sam's own grant names it — and conveys
    // nothing at all.
    let parent = backend
        .effective_for(&sam, root, &p("Sessions"))
        .expect("an ancestor of a grant is nameable");
    assert!(
        parent.capabilities.is_empty(),
        "upward inheritance would hand out the whole tree: {:?}",
        parent.capabilities
    );
    assert!(
        backend
            .effective_for(&sam, root, &p(""))
            .expect("the root is an ancestor too")
            .capabilities
            .is_empty()
    );
}

// t[verify files.access.granularity]
#[tokio::test(flavor = "multi_thread")]
async fn outside_a_granted_subtree_a_path_is_absent_not_forbidden() {
    let (_tmp, backend, root) = adopted().await;
    let sam = colleague();
    backend
        .grant(
            sam.clone(),
            root,
            p("Sessions/Song"),
            vec![Capability::Read],
        )
        .await
        .unwrap();

    // `Renders/master.wav` exists. Sam must not be able to learn that.
    for path in ["Renders", "Renders/master.wav", "Sessions/Other"] {
        match backend
            .authorise(&sam, root, &p(path), Capability::Read)
            .expect_err("outside the subtree")
        {
            FilesFault::PathNotFound(got) => assert_eq!(got, p(path)),
            FilesFault::Denied { .. } => {
                panic!("{path}: `Denied` confirms the path exists — that is the leak")
            }
            other => panic!("{path}: expected PathNotFound, got {other:?}"),
        }
    }
}

// t[verify files.access.granularity]
#[tokio::test(flavor = "multi_thread")]
async fn inside_the_subtree_a_missing_capability_is_denied() {
    let (_tmp, backend, root) = adopted().await;
    let sam = colleague();
    backend
        .grant(
            sam.clone(),
            root,
            p("Sessions/Song"),
            vec![Capability::Read],
        )
        .await
        .unwrap();

    backend
        .authorise(&sam, root, &p("Sessions/Song/mix.wav"), Capability::Read)
        .expect("read is granted here");

    // Sam knows this file exists; refusing the write is not a leak, and
    // pretending it is absent would be a lie they can already disprove.
    match backend
        .authorise(&sam, root, &p("Sessions/Song/mix.wav"), Capability::Write)
        .expect_err("write was not granted")
    {
        FilesFault::Denied { action, path } => {
            assert_eq!(action, "Write");
            assert_eq!(path, p("Sessions/Song/mix.wav"));
        }
        other => panic!("expected Denied inside a granted subtree, got {other:?}"),
    }
}

// t[verify files.access.granularity]
#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_holding_nothing_sees_absence_everywhere() {
    let (_tmp, backend, root) = adopted().await;
    let stranger = colleague();
    assert!(matches!(
        backend
            .effective_for(&stranger, root, &p("Sessions/Song"))
            .expect_err("no grant, no answer"),
        FilesFault::PathNotFound(_)
    ));
}

// t[verify files.access.internal-sharing]
#[tokio::test(flavor = "multi_thread")]
async fn a_grantee_may_pass_on_only_what_they_hold() {
    let (_tmp, backend, root) = adopted().await;
    let sam = colleague();
    let dev = colleague();
    backend
        .grant(
            sam.clone(),
            root,
            p("Sessions"),
            vec![Capability::Read, Capability::Share],
        )
        .await
        .unwrap();

    backend
        .grant_as(
            &sam,
            dev.clone(),
            root,
            p("Sessions/Song"),
            vec![Capability::Read],
        )
        .expect("a subset of what Sam holds, inside Sam's subtree");

    match backend
        .grant_as(
            &sam,
            dev.clone(),
            root,
            p("Sessions"),
            vec![Capability::Write],
        )
        .expect_err("Sam cannot hand out a write they do not have")
    {
        FilesFault::Denied { action, .. } => assert_eq!(action, "Write"),
        other => panic!("expected Denied, got {other:?}"),
    }

    // And outside Sam's subtree the parent is not somewhere Sam can even
    // address, let alone grant over.
    assert!(matches!(
        backend
            .grant_as(&sam, dev, root, p("Renders"), vec![Capability::Read])
            .expect_err("outside Sam's grant"),
        FilesFault::PathNotFound(_)
    ));
}

// t[verify files.access.granularity]
#[tokio::test(flavor = "multi_thread")]
async fn grants_lists_what_governs_a_path() {
    let (_tmp, backend, root) = adopted().await;
    let sam = colleague();
    let dev = colleague();
    let over_sessions = backend
        .grant(sam, root, p("Sessions"), vec![Capability::Read])
        .await
        .unwrap();
    let over_renders = backend
        .grant(dev, root, p("Renders"), vec![Capability::Read])
        .await
        .unwrap();

    let here = backend
        .grants(root, Some(p("Sessions/Song")))
        .await
        .expect("grants");
    assert_eq!(here.len(), 1, "a sibling's grant does not govern this path");
    assert_eq!(here[0].id, over_sessions.id);

    let all = backend.grants(root, None).await.expect("grants");
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|g| g.id == over_renders.id));
}

// t[verify files.access.internal-sharing]
#[tokio::test(flavor = "multi_thread")]
async fn a_link_is_paused_and_resumed_without_reissuing_it() {
    let (_tmp, backend, root) = adopted().await;
    let link = backend
        .create_share(
            root,
            p("Renders"),
            vec![Capability::Read, Capability::Download],
        )
        .await
        .expect("create_share");
    assert!(!link.disabled);
    assert!(!link.token.is_empty());

    let paused = backend
        .set_share_disabled(link.id, true)
        .await
        .expect("pause");
    assert!(paused.disabled);
    assert_eq!(paused.token, link.token, "pausing is not reissuing");

    assert!(
        backend
            .shares(root)
            .await
            .unwrap()
            .iter()
            .any(|l| l.id == link.id),
        "a paused link must still be findable, or pausing is deleting"
    );

    let resumed = backend
        .set_share_disabled(link.id, false)
        .await
        .expect("resume");
    assert!(!resumed.disabled);
    assert_eq!(
        resumed.token, link.token,
        "and everyone who already holds the link keeps it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_share_is_refused_rather_than_invented() {
    let (_tmp, backend, _root) = adopted().await;
    assert!(matches!(
        backend
            .set_share_disabled(ShareId::generate(), true)
            .await
            .expect_err("no such link"),
        FilesFault::Invalid(_)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_conveying_nothing_is_refused() {
    let (_tmp, backend, root) = adopted().await;
    assert!(matches!(
        backend
            .grant(colleague(), root, p("Sessions"), Vec::new())
            .await
            .expect_err("an empty capability set is not a grant"),
        FilesFault::Invalid(_)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn every_method_answers_a_typed_fault_for_an_unknown_root() {
    let (_tmp, backend, _root) = adopted().await;
    let ghost = RootId::generate();

    assert!(matches!(
        backend
            .grant(colleague(), ghost, p("x"), vec![Capability::Read])
            .await
            .expect_err("no such root"),
        FilesFault::RootNotFound(_)
    ));
    assert!(matches!(
        backend
            .effective(ghost, p("x"))
            .await
            .expect_err("no such root"),
        FilesFault::RootNotFound(_)
    ));
    assert!(matches!(
        backend.grants(ghost, None).await.expect_err("no such root"),
        FilesFault::RootNotFound(_)
    ));
    assert!(matches!(
        backend.shares(ghost).await.expect_err("no such root"),
        FilesFault::RootNotFound(_)
    ));
}

/// The identity gap, pinned rather than papered over: with no session on
/// this trait, `effective` answers for the process, which the lane
/// treats as the owner of the roots it adopted. When a dispatcher
/// supplies a caller, this test is the one that has to change.
#[tokio::test(flavor = "multi_thread")]
async fn the_trait_answers_for_the_process_principal() {
    let (_tmp, backend, root) = adopted().await;
    let mine = backend
        .effective(root, p("Renders"))
        .await
        .expect("effective");
    assert!(
        mine.capabilities.contains(&Capability::Share),
        "the server holds everything over a root it adopted itself"
    );
    assert_eq!(
        mine.capabilities,
        backend
            .effective_for(&Subject::Person(this_principal()), root, &p("Renders"))
            .unwrap()
            .capabilities,
        "`effective` is `effective_for(this_principal)` and nothing more"
    );
}

/// A grant is a decision a human made about who may see their work, and
/// `storage.tier.authored` says such a decision is durable. It used to
/// live in a process-lifetime `static`, so restarting the server silently
/// un-shared every folder on it — the failure nobody notices until a
/// collaborator is locked out.
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_survives_a_restart() {
    let (tmp, first, root) = adopted().await;
    let sam = colleague();
    let grant = first
        .grant(
            sam.clone(),
            root,
            p("Sessions/Song"),
            vec![Capability::Read],
        )
        .await
        .expect("grant");
    drop(first);

    // Asserted on disk as well as through the API, because a second
    // backend in *this* process shares the already-loaded cell — so the
    // trait calls below would pass against a process-wide static too. The
    // file is the part a real restart reads.
    let on_disk = std::fs::read_to_string(tmp.path().join("access.json")).expect("access.json");
    assert!(
        on_disk.contains(&grant.id.to_string()),
        "the grant was never written down: {on_disk}"
    );

    // A second backend over the same data directory is what a restart
    // looks like to this lane.
    let second = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("restart");
    assert!(
        second
            .grants(root, None)
            .await
            .expect("grants after restart")
            .iter()
            .any(|g| g.id == grant.id),
        "a grant a human made must outlive the process that recorded it"
    );
    assert!(
        second
            .effective_for(&sam, root, &p("Sessions/Song/mix.wav"))
            .expect("Sam still holds the subtree")
            .capabilities
            .contains(&Capability::Read),
        "and it must still resolve, not merely be listed"
    );
}

/// One org's grants and links are not another's.
///
/// `FilesBackend` is constructed once per org, so the lane's old
/// process-wide `static` was one table shared by every org on the server.
/// The lookups that gave it away are the ones keyed by id alone — `revoke`
/// and `set_share_disabled` scan without a root to scope them, so an org
/// could reach a row it had no way of knowing existed.
#[tokio::test(flavor = "multi_thread")]
async fn one_orgs_access_is_not_anothers() {
    async fn org(name: &str) -> (tempfile::TempDir, FilesBackend, RootId) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join(name);
        std::fs::create_dir_all(dir.join("Sessions").join("Song")).unwrap();
        let backend = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
        let root = backend
            .adopt(AdoptRequest {
                path: dir.to_string_lossy().into_owned(),
                name: name.into(),
                flavor: RootFlavor::Media,
                hash_content: false,
            })
            .await
            .expect("adopt");
        (tmp, backend, RootId::new(root.id))
    }

    let (_a_tmp, a, a_root) = org("alpha").await;
    let (_b_tmp, b, _b_root) = org("beta").await;

    let grant = a
        .grant(colleague(), a_root, p("Sessions"), vec![Capability::Read])
        .await
        .expect("alpha grants");
    let link = a
        .create_share(a_root, p("Sessions"), vec![Capability::Read])
        .await
        .expect("alpha shares");

    // Beta has no row with these ids, so both must read as absent —
    // anything else means beta reached into alpha's table.
    match b.revoke(grant.id).await.expect_err("not beta's grant") {
        FilesFault::GrantRevoked(id) => assert_eq!(id, grant.id),
        other => panic!("beta could see alpha's grant: {other:?}"),
    }
    assert!(
        matches!(
            b.set_share_disabled(link.id, true)
                .await
                .expect_err("not beta's link"),
            FilesFault::Invalid(_)
        ),
        "beta must not be able to pause a link alpha minted"
    );

    // And alpha's own rows are untouched by beta having asked.
    assert!(
        !a.shares(a_root)
            .await
            .expect("alpha shares")
            .iter()
            .any(|l| l.id == link.id && l.disabled),
        "beta's request reached alpha's link"
    );
    assert!(
        a.grants(a_root, None)
            .await
            .expect("alpha grants")
            .iter()
            .any(|g| g.id == grant.id)
    );
}
