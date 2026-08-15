#![allow(clippy::large_futures)]
//! `/org/{slug}/media/{path}` authorization.
//!
//! This route served the org's whole `resources/` tree — files AND
//! directory listings — to any anonymous caller, and it is NOT a vox
//! path, so `TASK_ENFORCE_PERMISSIONS` never reached it. Verified open on
//! production 2026-08-07 (a clean `curl` returned a JSON listing of the
//! song library). Locking down vox alone would have left this wide open
//! while looking sealed.
//!
//! The fix mirrors `/blobs/*`, which had it right all along: a short-lived
//! Ed25519-signed token in the query string, minted over vox by a caller
//! the permission gate already allowed. `<audio>` and `<img>` can't set an
//! `Authorization` header, so the URL is the only channel.
//!
//! Rollout matches the vox work: OFF unless `TASK_ENFORCE_MEDIA_TOKEN=1`,
//! so the check can be observed in production before it refuses anything.

use task_server::{AppState, AuthState, capability::ServerKeypair, router};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Boot a server with one org and a media file on disk.
/// `enforce` drives `TASK_ENFORCE_MEDIA_TOKEN`.
async fn boot(enforce: bool) -> eyre::Result<(String, AppState, tempfile::TempDir)> {
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        if enforce {
            std::env::set_var("TASK_ENFORCE_MEDIA_TOKEN", "1");
        } else {
            std::env::remove_var("TASK_ENFORCE_MEDIA_TOKEN");
        }
    }
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .init_org("media-test", "Media Test", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;

    // Drop a song file where the colocated-media route serves from.
    let songs = data_root
        .org("media-test")
        .path()
        .join("resources")
        .join("songs")
        .join("hymn");
    std::fs::create_dir_all(&songs)?;
    std::fs::write(songs.join("manifest.json"), br#"{"stems":[]}"#)?;
    // A sibling directory, to prove a grant doesn't leak sideways.
    let other = data_root
        .org("media-test")
        .path()
        .join("resources")
        .join("songs")
        .join("hymn-outtakes");
    std::fs::create_dir_all(&other)?;
    std::fs::write(other.join("secret.txt"), b"not yours")?;

    let state = AppState::new_with_auth(auth, ServerKeypair::generate_ephemeral()).await?;
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}"), state, tmp))
}

/// Mint a grant the way a client does — through the service, so the test
/// exercises the real signing path rather than reconstructing a token.
async fn grant(state: &AppState, prefix: &str) -> String {
    use media_proto::MediaService as _;
    let org = state.org("media-test").expect("org hosted");
    let keypair = org.attachments.keypair.clone();
    let svc = task_server::media::MediaServiceImpl::new(
        org.attachments.clone(),
        org.slug.clone(),
        keypair,
    );
    svc.media_grant(prefix.to_string())
        .await
        .expect("mint grant")
        .token
}

async fn get(url: &str) -> (u16, String) {
    let r = reqwest::get(url).await.expect("request");
    let status = r.status().as_u16();
    (status, r.text().await.unwrap_or_default())
}

/// One test, not two: `TASK_ENFORCE_MEDIA_TOKEN` is process-wide and read
/// per request, so two `#[tokio::test]`s in this binary would race each
/// other's flag setting. Sequencing them here also lets the flip be
/// exercised against ONE running server, which is closer to what the
/// operator actually does.
#[tokio::test(flavor = "multi_thread")]
async fn media_route_observes_then_enforces() -> eyre::Result<()> {
    // ── Phase 1: flag off. Behaviour must be byte-for-byte what it was
    //    before this change — shipping the check hot would black out
    //    every `<audio>`/`<img>` in the deployed bundle as it rolled.
    let (base, state, _tmp) = boot(false).await?;
    let file = format!("{base}/org/media-test/media/songs/hymn/manifest.json");

    let (status, body) = get(&file).await;
    assert_eq!(status, 200, "observe-only must not refuse");
    assert!(body.contains("stems"), "unexpected body: {body}");
    let (status, _) = get(&format!("{base}/org/media-test/media/songs")).await;
    assert_eq!(status, 200, "observe-only must not refuse a listing");

    // ── Phase 2: the operator flips it. Same server, no restart — the
    //    flag is read per request.
    // SAFETY: single-threaded point in this test; no other test in this
    // binary (there is only one) reads the env concurrently.
    unsafe {
        std::env::set_var("TASK_ENFORCE_MEDIA_TOKEN", "1");
    }

    // ── No token: refused. This is the production hole, closed.
    let (status, _) = get(&file).await;
    assert_eq!(status, 401, "an unauthenticated media read must be refused");

    // ── Directory listing is refused too — it enumerated the library.
    let (status, body) = get(&format!("{base}/org/media-test/media/songs")).await;
    assert_eq!(
        status, 401,
        "an unauthenticated directory listing must be refused; got: {body}"
    );

    // ── A valid grant for this song: served.
    let token = grant(&state, "songs/hymn").await;
    let (status, body) = get(&format!("{file}?token={token}")).await;
    assert_eq!(status, 200, "a validly granted read must be served");
    assert!(body.contains("stems"), "unexpected body: {body}");

    // ── The SAME grant must not reach a sibling whose name merely shares
    //    the prefix. This is the classic string-prefix bug in a path
    //    grant, and it is the reason the match is segment-aware.
    let (status, _) = get(&format!(
        "{base}/org/media-test/media/songs/hymn-outtakes/secret.txt?token={token}"
    ))
    .await;
    assert_eq!(
        status, 401,
        "a grant for `songs/hymn` must not unlock `songs/hymn-outtakes`"
    );

    // ── Garbage token: refused, not treated as absent-but-fine.
    let (status, _) = get(&format!("{file}?token=not-a-real-token")).await;
    assert_eq!(status, 401, "an invalid token must be refused");

    Ok(())
}
