#![allow(clippy::large_futures)]
//! The share guest lane + file-request inbox (issue #272), end to end:
//! an anonymous vox connection to `/org/{slug}/share/{token}/vox`
//! comments and draws with no account (attribution records the link),
//! reaches exactly the review's file and nothing else, and file-request
//! uploads land in the per-link incoming area for owner promotion.

use std::sync::Arc;

use files::FilesService as _;
use files_proto::{AnnotationPoint, AnnotationStroke, FilesServiceClient, NewReviewComment};
use files_transcode::transcoder::FakeTranscoder;
use share_proto::{NewShareLink, ShareCapabilities, ShareService as _, ShareTarget};
use task_server::{AppState, AuthState, capability::ServerKeypair, router};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Boot a server with a Media root holding `cut.mov` (reviewed) and
/// `secret.mov` (not shared).
async fn boot() -> eyre::Result<(String, AppState, uuid::Uuid, tempfile::TempDir)> {
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
    }
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .init_org("guest-test", "Guest Test", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let state = AppState::new_with_auth(auth, ServerKeypair::generate_ephemeral()).await?;
    drop(guard);

    let org = state.org("guest-test").expect("org hosted");
    org.files.set_transcoder(Arc::new(FakeTranscoder));
    let root_dir = tmp
        .path()
        .join("orgs")
        .join("guest-test")
        .join("files")
        .join("session");
    std::fs::create_dir_all(&root_dir)?;
    std::fs::write(root_dir.join("cut.mov"), b"VIDEO the reviewed cut")?;
    std::fs::write(root_dir.join("secret.mov"), b"VIDEO not for guests")?;
    let root = org
        .files
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "session".into(),
            files_proto::RootFlavor::Media,
        )
        .await
        .map_err(|e| eyre::eyre!("create root: {e}"))?;
    org.files
        .checkpoint_now(root.id, None)
        .await
        .map_err(|e| eyre::eyre!("checkpoint: {e}"))?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}"), state, root.id, tmp))
}

fn svc(state: &AppState) -> task_server::share::ShareServiceImpl {
    let org = state.org("guest-test").expect("org hosted");
    task_server::share::ShareServiceImpl::new(
        org.shares.clone(),
        "guest-test".into(),
        "http://test".into(),
        Some(org.files.clone()),
    )
}

async fn guest_client(base: &str, token: &str, pw: &str) -> eyre::Result<FilesServiceClient> {
    let ws = base.replace("http://", "ws://");
    let suffix = if pw.is_empty() {
        String::new()
    } else {
        format!("?pw={pw}")
    };
    vox::connect_lane(format!("{ws}/org/guest-test/share/{token}/vox{suffix}"))
        .establish()
        .await
        .map_err(|e| eyre::eyre!("guest connect: {e:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_lane_comments_scoped_and_attributed() -> eyre::Result<()> {
    let (base, state, root_id, _tmp) = boot().await?;
    let org = state.org("guest-test").expect("org");
    let share = svc(&state);

    // The owner starts the review; the guest link points at it.
    let review = org
        .files
        .review_for_file(root_id, "cut.mov".into())
        .await
        .expect("review");
    let link = share
        .create_link(
            ShareTarget::Review { id: review.id },
            NewShareLink {
                label: "client review".into(),
                capabilities: Some(ShareCapabilities {
                    comment: true,
                    download: false,
                    file_request: false,
                }),
                password: None,
                expires_unix: None,
            },
        )
        .await
        .expect("mint review link");

    // ── AC 1: an ANONYMOUS vox connection comments and draws; the
    //    comment records the link.
    let guest = guest_client(&base, &link.token, "").await?;
    let found = guest
        .find_review(root_id, "cut.mov".into())
        .await
        .expect("guest sees the review")
        .expect("scoped review");
    assert_eq!(found.id, review.id);
    let head = guest.chain(root_id, "cut.mov".into()).await.expect("chain")[0]
        .commit_id
        .clone();
    let posted = guest
        .add_review_comment(
            review.id,
            NewReviewComment {
                timecode_secs: 2.5,
                author: "External Client".into(),
                body: "love this cut".into(),
                commit_id: head.clone(),
                annotation: vec![AnnotationStroke {
                    points: vec![
                        AnnotationPoint { x: 0.2, y: 0.2 },
                        AnnotationPoint { x: 0.8, y: 0.6 },
                    ],
                    color: "#ff3355".into(),
                    width: 0.004,
                }],
            },
        )
        .await
        .expect("guest comment with drawing");
    assert!(
        posted.via_link.contains("client review"),
        "attribution records the link: {}",
        posted.via_link
    );
    assert_eq!(
        posted.author, "External Client (guest)",
        "identity is constrained at the boundary — a guest can't post as a member"
    );
    assert_eq!(posted.annotation.len(), 1, "the drawing came through");

    // The owner sees the same attributed comment on the org lane.
    let owner_view = org.files.review_comments(review.id).await.expect("owner");
    assert!(
        owner_view
            .iter()
            .any(|c| c.id == posted.id && !c.via_link.is_empty()),
        "the attributed comment is in the vault"
    );

    // ── AC 2: the lane reaches exactly the review's media.
    guest
        .rendition(
            root_id,
            "cut.mov".into(),
            files_proto::RenditionKind::Proxy720,
        )
        .await
        .expect("the review's proxy resolves");
    guest
        .rendition(
            root_id,
            "secret.mov".into(),
            files_proto::RenditionKind::Proxy720,
        )
        .await
        .expect_err("another file's rendition is refused");
    guest
        .browse(root_id, String::new())
        .await
        .expect_err("browsing is refused");
    // list_reviews is the guest's "what can I see": exactly its one
    // review, never the org's list — the entry page boots from this.
    let visible = guest.list_reviews(None).await.expect("scoped listing");
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, review.id);
    guest
        .delete_review_comment(posted.id)
        .await
        .expect_err("guests cannot delete");

    // A view-only review link cannot comment.
    let view_only = share
        .create_link(
            ShareTarget::Review { id: review.id },
            NewShareLink {
                label: "view only".into(),
                capabilities: Some(ShareCapabilities::default()),
                password: None,
                expires_unix: None,
            },
        )
        .await
        .expect("mint view-only link");
    let viewer = guest_client(&base, &view_only.token, "").await?;
    viewer
        .add_review_comment(
            review.id,
            NewReviewComment {
                timecode_secs: 1.0,
                author: "lurker".into(),
                body: "hi".into(),
                commit_id: head,
                annotation: Vec::new(),
            },
        )
        .await
        .expect_err("view-only guests cannot comment");

    // ── Revocation is retroactive MID-CONNECTION: the link is
    //    re-resolved on every call, not only at upgrade.
    share
        .set_link_disabled(link.token.clone(), true)
        .await
        .expect("disable");
    guest
        .review_comments(review.id)
        .await
        .expect_err("a disabled link cuts off connected guests");
    share
        .set_link_disabled(link.token.clone(), false)
        .await
        .expect("re-enable");
    guest
        .review_comments(review.id)
        .await
        .expect("re-enabling restores the connected guest");

    // ── The live-comment stream rides the guest lane too, filtered to
    //    this review.
    {
        let ws = base.replace("http://", "ws://");
        let stream: files_proto::FilesServiceStreamClient =
            vox::connect_lane(format!("{ws}/org/guest-test/share/{}/vox", link.token))
                .establish()
                .await
                .map_err(|e| eyre::eyre!("stream connect: {e:?}"))?;
        let (tx, mut rx) = vox::channel::<files_proto::FilesEvent>();
        let sub = tokio::spawn(async move { stream.events(tx).await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if sub.is_finished() {
            panic!("guest stream subscribe ended early: {:?}", sub.await);
        }
        // An org-lane comment lands as a guest-visible event…
        org.files
            .add_review_comment(
                review.id,
                NewReviewComment {
                    timecode_secs: 5.0,
                    author: "Owner".into(),
                    body: "replying".into(),
                    commit_id: posted.commit_id.clone(),
                    annotation: Vec::new(),
                },
            )
            .await
            .expect("owner comment");
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("event within 5s")
            .expect("stream open")
            .expect("one event");
        let mut owned: Option<files_proto::FilesEvent> = None;
        let _ = event.map(|ev| owned = Some(ev.clone()));
        assert!(
            matches!(owned, Some(files_proto::FilesEvent::ReviewCommentAdded(c)) if c.review_id == review.id),
            "the guest stream carries this review's comments"
        );
    }

    // ── AC 4: password gates the lane (the upgrade itself refuses).
    share
        .update_link(
            link.token.clone(),
            NewShareLink {
                label: String::new(),
                capabilities: None,
                password: Some("secret".into()),
                expires_unix: None,
            },
        )
        .await
        .expect("set password");
    assert!(
        guest_client(&base, &link.token, "").await.is_err(),
        "no password → no upgrade"
    );
    assert!(
        guest_client(&base, &link.token, "wrong").await.is_err(),
        "wrong password → no upgrade"
    );
    let _reconnected = guest_client(&base, &link.token, "secret")
        .await
        .expect("right password connects");

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn file_request_uploads_land_incoming_and_promote() -> eyre::Result<()> {
    let (base, state, root_id, tmp) = boot().await?;
    let share = svc(&state);

    let link = share
        .create_link(
            ShareTarget::Slice {
                root_id,
                subpath: String::new(),
            },
            NewShareLink {
                label: "drop zone".into(),
                capabilities: Some(ShareCapabilities {
                    comment: false,
                    download: false,
                    file_request: true,
                }),
                password: None,
                expires_unix: None,
            },
        )
        .await
        .expect("mint file-request link");
    let up_base = format!("{base}/org/guest-test/share/{}/upload", link.token);
    let client = reqwest::Client::new();

    // ── AC 3: uploads land in the incoming area — twice, never
    //    overwriting.
    let r = client
        .post(format!("{up_base}/mix-notes.pdf"))
        .body("version A")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 200);
    assert_eq!(r.text().await?, "mix-notes.pdf");
    let r = client
        .post(format!("{up_base}/mix-notes.pdf"))
        .body("version B")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 200);
    assert_eq!(
        r.text().await?,
        "mix-notes-2.pdf",
        "collisions suffix, never overwrite"
    );

    let incoming = share.list_incoming(link.token.clone()).await.expect("list");
    assert_eq!(incoming.len(), 2, "{incoming:?}");

    // A link WITHOUT the capability is refused.
    let plain = share
        .create_link(
            ShareTarget::Slice {
                root_id,
                subpath: String::new(),
            },
            NewShareLink {
                label: "plain".into(),
                capabilities: None,
                password: None,
                expires_unix: None,
            },
        )
        .await
        .expect("plain link");
    let r = client
        .post(format!(
            "{base}/org/guest-test/share/{}/upload/x.txt",
            plain.token
        ))
        .body("nope")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 403, "no file_request → no uploads");

    // ── Owner promotion moves the upload into the tree (no overwrite).
    share
        .promote_incoming(
            link.token.clone(),
            "mix-notes.pdf".into(),
            "docs/mix-notes.pdf".into(),
        )
        .await
        .expect("promote");
    let promoted = tmp
        .path()
        .join("orgs/guest-test/files/session/docs/mix-notes.pdf");
    assert_eq!(std::fs::read_to_string(&promoted)?, "version A");
    assert_eq!(
        share
            .list_incoming(link.token.clone())
            .await
            .expect("list")
            .len(),
        1,
        "the promoted upload left the queue"
    );
    // Promoting onto an existing tree file is refused.
    share
        .promote_incoming(
            link.token.clone(),
            "mix-notes-2.pdf".into(),
            "cut.mov".into(),
        )
        .await
        .expect_err("promotion never overwrites the tree");

    // A slice-scoped link promotes INTO its slice, not beside it.
    let scoped = share
        .create_link(
            ShareTarget::Slice {
                root_id,
                subpath: "inbox".into(),
            },
            NewShareLink {
                label: "scoped drop".into(),
                capabilities: Some(ShareCapabilities {
                    comment: false,
                    download: false,
                    file_request: true,
                }),
                password: None,
                expires_unix: None,
            },
        )
        .await
        .expect("scoped link");
    let r = client
        .post(format!(
            "{base}/org/guest-test/share/{}/upload/brief.txt",
            scoped.token
        ))
        .body("the brief")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 200);
    share
        .promote_incoming(scoped.token.clone(), "brief.txt".into(), "brief.txt".into())
        .await
        .expect("promote scoped");
    assert!(
        tmp.path()
            .join("orgs/guest-test/files/session/inbox/brief.txt")
            .is_file(),
        "the destination is slice-relative — it landed under inbox/"
    );

    // Upload receipts are in the access log.
    let log = share.access_log(link.token.clone()).await.expect("log");
    assert!(
        log.iter().filter(|e| e.kind == "upload").count() >= 2,
        "upload receipts: {log:?}"
    );

    Ok(())
}
