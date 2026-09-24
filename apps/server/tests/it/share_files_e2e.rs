#![allow(clippy::large_futures)]
//! Files share links (issue #271), end to end over HTTP: a slice link
//! serves exactly its slice, a Named Version link serves the exact
//! change it names, view-only exposes renditions and never originals,
//! the access log receipts downloads, and every setting is retroactive.

use std::sync::Arc;

use files::service::{CurationService as _, VersionService as _};
use files_transcode::transcoder::FakeTranscoder;
use share_proto::{NewShareLink, ShareCapabilities, ShareService as _, ShareTarget};
use task_server::{AppState, AuthState, capability::ServerKeypair, router};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const V1_BYTES: &[u8] = b"VIDEOv1 the client cut ..............";

/// Boot a server whose org has a Media root:
/// `takes/cut.mov` + `takes/notes.txt` inside the shared slice,
/// `mix.wav` outside it.
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
        .init_org("share-test", "Share Test", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let state = AppState::new_with_auth(auth, ServerKeypair::generate_ephemeral()).await?;
    drop(guard);

    let org = state.org("share-test").expect("org hosted");
    org.files.set_transcoder(Arc::new(FakeTranscoder));
    let root_dir = tmp
        .path()
        .join("orgs")
        .join("share-test")
        .join("files")
        .join("session");
    std::fs::create_dir_all(root_dir.join("takes"))?;
    std::fs::write(root_dir.join("takes/cut.mov"), V1_BYTES)?;
    std::fs::write(root_dir.join("takes/notes.txt"), b"take notes")?;
    std::fs::write(root_dir.join("mix.wav"), b"AUDIO outside the slice")?;
    let root = crate::support::adopt_root(
        &org.files,
        &root_dir,
        "session",
        files_proto::RootFlavor::Media,
    )
    .await?;
    crate::support::checkpoint(&org.files, root.id).await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}"), state, root.id, tmp))
}

fn svc(state: &AppState) -> task_server::share::ShareServiceImpl {
    let org = state.org("share-test").expect("org hosted");
    task_server::share::ShareServiceImpl::new(
        org.shares.clone(),
        "share-test".into(),
        "http://test".into(),
        Some(org.files.clone()),
    )
}

fn options(caps: ShareCapabilities) -> NewShareLink {
    NewShareLink {
        label: "client link".into(),
        capabilities: Some(caps),
        password: None,
        expires_unix: None,
    }
}

async fn get(url: &str) -> (u16, String) {
    let r = reqwest::get(url).await.expect("request");
    (r.status().as_u16(), r.text().await.unwrap_or_default())
}

/// One test, not several: the suite boots one real server and walks the
/// ACs in order (the env-var data root is process-wide).
#[tokio::test(flavor = "multi_thread")]
async fn share_links_scope_gate_and_receipt() -> eyre::Result<()> {
    let (base, state, root_id, _tmp) = boot().await?;
    let share = svc(&state);

    // ── AC 1: a slice link browses exactly its slice.
    let slice = share
        .create_link(
            ShareTarget::Slice {
                root_id,
                subpath: "takes".into(),
            },
            options(ShareCapabilities::default()),
        )
        .await
        .expect("mint slice link");
    let link_base = format!("{base}/org/share-test/share/{}", slice.token);

    let (status, body) = get(&link_base).await;
    assert_eq!(status, 200);
    assert!(
        body.contains("cut.mov") && body.contains("notes.txt"),
        "{body}"
    );
    assert!(
        !body.contains("mix.wav"),
        "the slice listing must not show the root's other files"
    );
    // Nothing outside the slice is addressable: paths are slice-relative
    // (mix.wav resolves to takes/mix.wav → nothing) and traversal 404s.
    let (status, _) = get(&format!("{link_base}/b/..%2Fmix.wav")).await;
    assert_ne!(status, 200, "traversal must not escape the slice");
    let (_, body) = get(&format!("{link_base}/b/mix.wav")).await;
    assert!(
        !body.contains("Download original"),
        "an out-of-slice name resolves to nothing servable"
    );

    // ── AC 3: view-only exposes renditions; downloads are refused.
    let (status, _) = get(&format!("{link_base}/rendition/proxy-720/cut.mov")).await;
    assert_eq!(status, 200, "view-only link streams the proxy rendition");
    let (status, _) = get(&format!("{link_base}/download/cut.mov")).await;
    assert_eq!(status, 403, "view-only link must not serve originals");

    // A download-capable link serves the original bytes.
    let dl = share
        .create_link(
            ShareTarget::Slice {
                root_id,
                subpath: "takes".into(),
            },
            options(ShareCapabilities {
                comment: false,
                download: true,
                file_request: false,
                documents: false,
            }),
        )
        .await
        .expect("mint download link");
    let dl_base = format!("{base}/org/share-test/share/{}", dl.token);
    let r = reqwest::get(format!("{dl_base}/download/cut.mov")).await?;
    assert_eq!(r.status().as_u16(), 200);
    assert_eq!(&r.bytes().await?[..], V1_BYTES, "originals byte-for-byte");

    // ── AC 4: the access log holds the views and the download receipt.
    let log = share.access_log(dl.token.clone()).await.expect("log");
    assert!(
        log.iter()
            .any(|e| e.kind == "download" && e.path == "cut.mov"),
        "download receipt recorded: {log:?}"
    );
    let log = share.access_log(slice.token.clone()).await.expect("log");
    assert!(log.iter().any(|e| e.kind == "view"), "landing view logged");
    assert!(
        log.iter().any(|e| e.kind == "rendition"),
        "rendition stream logged"
    );

    // A partial edit (capabilities: None) KEEPS the grant — "just
    // relabel" must not silently rewrite download.
    share
        .update_link(
            dl.token.clone(),
            NewShareLink {
                label: "relabeled".into(),
                capabilities: None,
                password: None,
                expires_unix: None,
            },
        )
        .await
        .expect("partial edit");
    let r = reqwest::get(format!("{dl_base}/download/cut.mov")).await?;
    assert_eq!(
        r.status().as_u16(),
        200,
        "None capabilities keeps the download grant"
    );

    // A negative expiry is a caller bug, refused — not "never expires".
    share
        .update_link(
            dl.token.clone(),
            NewShareLink {
                label: String::new(),
                capabilities: None,
                password: None,
                expires_unix: Some(-5),
            },
        )
        .await
        .expect_err("negative expiry refused");

    // ── AC 2: a Named Version link resolves the exact change — even
    //    after the live tree moves on.
    let org = state.org("share-test").expect("org");
    let v1_commit = org
        .files
        .chain(
            files::RootId::new(root_id),
            files::RootPath::parse("takes/cut.mov")?,
        )
        .await
        .expect("chain")[0]
        .commit_id
        .clone();
    let named = org
        .files
        .name_version(
            files::RootId::new(root_id),
            files::id::VersionId::from_commit_hex(&v1_commit),
            "client cut".into(),
        )
        .await
        .expect("name version");
    // v2 lands.
    std::fs::write(
        _tmp.path()
            .join("orgs/share-test/files/session/takes/cut.mov"),
        b"VIDEOv2 a totally different cut",
    )?;
    crate::support::checkpoint(&org.files, root_id)
        .await
        .expect("checkpoint v2");

    let nv = share
        .create_link(
            ShareTarget::NamedVersion { id: named.id },
            options(ShareCapabilities {
                comment: false,
                download: true,
                file_request: false,
                documents: false,
            }),
        )
        .await
        .expect("mint named version link");
    let nv_base = format!("{base}/org/share-test/share/{}", nv.token);
    let r = reqwest::get(format!("{nv_base}/download/takes/cut.mov")).await?;
    assert_eq!(r.status().as_u16(), 200);
    assert_eq!(
        &r.bytes().await?[..],
        V1_BYTES,
        "the Named Version link serves v1's exact content, not the live tree's v2"
    );

    // ── AC 5: retroactive controls.
    // Disable-not-delete: 410 immediately, listable still.
    share
        .set_link_disabled(slice.token.clone(), true)
        .await
        .expect("disable");
    let (status, _) = get(&link_base).await;
    assert_eq!(status, 410, "disabled link is gone, not deleted");
    share
        .set_link_disabled(slice.token.clone(), false)
        .await
        .expect("re-enable");
    let (status, _) = get(&link_base).await;
    assert_eq!(status, 200, "re-enabling brings it back");

    // Retroactive capability edit: the download link loses download.
    share
        .update_link(
            dl.token.clone(),
            options(ShareCapabilities {
                comment: false,
                download: false,
                file_request: false,
                documents: false,
            }),
        )
        .await
        .expect("retract download");
    let (status, _) = get(&format!("{dl_base}/download/cut.mov")).await;
    assert_eq!(status, 403, "capability edits are retroactive");

    // Password: set one, the open URL turns into a form, wrong pw 401,
    // right pw serves.
    share
        .update_link(
            slice.token.clone(),
            NewShareLink {
                label: String::new(),
                capabilities: None,
                password: Some("hunter2".into()),
                expires_unix: None,
            },
        )
        .await
        .expect("set password");
    let (status, body) = get(&link_base).await;
    assert_eq!(status, 200);
    assert!(body.contains("password"), "asks for the password: {body}");
    assert!(!body.contains("cut.mov"), "no listing before the password");
    let (status, _) = get(&format!("{link_base}?pw=wrong")).await;
    assert_eq!(status, 401);
    // Byte routes must never answer a missing password with a 200 HTML
    // form — a curl would save the form as the file and exit 0.
    let (status, _) = get(&format!("{link_base}/rendition/proxy-720/cut.mov")).await;
    assert_eq!(status, 401, "missing password on a byte route is a 401");
    let (status, body) = get(&format!("{link_base}?pw=hunter2")).await;
    assert_eq!(status, 200);
    assert!(body.contains("cut.mov"), "right password serves: {body}");

    // Expiry: a past expiry kills resolution.
    share
        .update_link(
            slice.token.clone(),
            NewShareLink {
                label: String::new(),
                capabilities: None,
                password: Some(String::new()),
                expires_unix: Some(chrono::Utc::now().timestamp() - 60),
            },
        )
        .await
        .expect("expire");
    let (status, _) = get(&link_base).await;
    assert_eq!(status, 410, "expired link is gone");

    // ── The org kill switch stops minting (existing links unaffected).
    share.set_sharing_disabled(true).await.expect("kill switch");
    let err = share
        .create_link(
            ShareTarget::Slice {
                root_id,
                subpath: String::new(),
            },
            options(ShareCapabilities::default()),
        )
        .await
        .expect_err("minting refused");
    assert!(err.to_string().contains("disabled"), "{err}");
    let (status, _) = get(&format!("{nv_base}/download/takes/cut.mov")).await;
    assert_eq!(status, 200, "existing links keep resolving");

    Ok(())
}

/// A `documents` link opens a session folder the way a public demo does:
/// the session and its chart whole, its takes as renditions only — and a
/// take renamed to look like a document is still a take.
// t[verify files.access.link-documents]
#[tokio::test(flavor = "multi_thread")]
async fn a_documents_link_opens_the_session_not_its_media() -> eyre::Result<()> {
    let (base, state, root_id, tmp) = boot().await?;
    let share = svc(&state);
    let org = state.org("share-test").expect("org hosted");
    let takes = tmp.path().join("orgs/share-test/files/session/takes");
    std::fs::write(takes.join("Song.RPP"), b"<REAPER_PROJECT 0.1 \"7.0\"\n>\n")?;
    std::fs::write(takes.join("Song.kf"), b"Title: Song\n")?;
    std::fs::write(takes.join("take.txt"), b"RIFF\x24\x00\x00\x00WAVEfmt ")?;
    crate::support::checkpoint(&org.files, root_id).await?;

    let plain = share
        .create_link(
            ShareTarget::Slice {
                root_id,
                subpath: "takes".into(),
            },
            options(ShareCapabilities::default()),
        )
        .await
        .expect("mint view-only link");
    let (status, _) = get(&format!(
        "{base}/org/share-test/share/{}/doc/Song.RPP",
        plain.token
    ))
    .await;
    assert_eq!(status, 403, "a view-only link opens no documents");

    let demo = share
        .create_link(
            ShareTarget::Slice {
                root_id,
                subpath: "takes".into(),
            },
            options(ShareCapabilities {
                documents: true,
                ..ShareCapabilities::default()
            }),
        )
        .await
        .expect("mint documents link");
    let link = format!("{base}/org/share-test/share/{}", demo.token);

    let r = reqwest::get(format!("{link}/doc/Song.RPP")).await?;
    assert_eq!(r.status().as_u16(), 200);
    assert!(
        r.headers()[reqwest::header::CONTENT_TYPE]
            .to_str()?
            .starts_with("text/plain"),
        "a session reads as text"
    );
    assert!(r.text().await?.starts_with("<REAPER_PROJECT"));
    let (status, body) = get(&format!("{link}/doc/Song.kf")).await;
    assert_eq!((status, body.as_str()), (200, "Title: Song\n"));

    // Media is refused whole, by name and by its bytes.
    let (status, _) = get(&format!("{link}/doc/cut.mov")).await;
    assert_eq!(status, 403, "a take is not a document");
    let (status, _) = get(&format!("{link}/doc/take.txt")).await;
    assert_eq!(status, 403, "a renamed take is still a take");
    let (status, _) = get(&format!("{link}/download/cut.mov")).await;
    assert_eq!(status, 403, "documents does not grant download");
    let (status, _) = get(&format!("{link}/rendition/proxy-720/cut.mov")).await;
    assert_eq!(status, 200, "the takes still stream as renditions");
    // Still only the slice.
    let (status, _) = get(&format!("{link}/doc/..%2Fmix.wav")).await;
    assert_ne!(status, 200, "traversal must not escape the slice");

    // The slice's files, listed as JSON — what a client streaming the
    // session reads first.
    let listed: serde_json::Value = reqwest::get(format!("{link}/list")).await?.json().await?;
    let paths: Vec<&str> = listed["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter_map(|e| e["path"].as_str())
        .collect();
    assert!(paths.contains(&"Song.RPP") && paths.contains(&"Song.kf"), "{paths:?}");

    let log = share.access_log(demo.token.clone()).await.expect("log");
    assert!(
        log.iter().any(|a| a.kind == "document" && a.path == "Song.RPP"),
        "document reads are receipted: {log:?}"
    );
    Ok(())
}

/// A take with its proxy committed beside it streams that proxy — whole
/// or by range — as its audio rendition; the original never serves, and
/// a take without one falls back to the derived rendition.
// t[verify files.access.link-proxies]
#[tokio::test(flavor = "multi_thread")]
async fn a_committed_proxy_is_the_audio_rendition() -> eyre::Result<()> {
    const PROXY: &[u8] = b"OggS\x00\x02 a vorbis proxy of the bass";
    let (base, state, root_id, tmp) = boot().await?;
    let share = svc(&state);
    let org = state.org("share-test").expect("org hosted");
    let takes = tmp.path().join("orgs/share-test/files/session/takes");
    std::fs::create_dir_all(takes.join("Proxies"))?;
    std::fs::write(takes.join("Bass.wav"), b"RIFF\x24\x00\x00\x00WAVE the original")?;
    std::fs::write(takes.join("Proxies/Bass.ogg"), PROXY)?;
    std::fs::write(takes.join("Keys.wav"), b"AUDIO keys, with no proxy")?;
    crate::support::checkpoint(&org.files, root_id).await?;

    let link = share
        .create_link(
            ShareTarget::Slice {
                root_id,
                subpath: "takes".into(),
            },
            options(ShareCapabilities::default()),
        )
        .await
        .expect("mint view-only link");
    let link = format!("{base}/org/share-test/share/{}", link.token);

    let r = reqwest::get(format!("{link}/rendition/audio/Bass.wav")).await?;
    assert_eq!(r.status().as_u16(), 200);
    assert_eq!(r.headers()[reqwest::header::CONTENT_TYPE], "audio/ogg");
    assert_eq!(&r.bytes().await?[..], PROXY, "the committed proxy, not a derived one");

    let r = reqwest::Client::new()
        .get(format!("{link}/rendition/audio/Bass.wav"))
        .header(reqwest::header::RANGE, "bytes=0-3")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 206, "a proxy streams by range");
    assert_eq!(
        r.headers()[reqwest::header::CONTENT_RANGE],
        format!("bytes 0-3/{}", PROXY.len()).as_str()
    );
    assert_eq!(&r.bytes().await?[..], b"OggS");

    let (status, _) = get(&format!("{link}/download/Bass.wav")).await;
    assert_eq!(status, 403, "the original still never serves");
    let (status, body) = get(&format!("{link}/rendition/audio/Keys.wav")).await;
    assert_eq!(status, 200);
    assert!(body.starts_with("audio-aac:"), "no proxy beside it: the derived rendition: {body}");

    assert_eq!(
        task_server::share::proxy_path("Media/Bass.wav").as_deref(),
        Some("Media/Proxies/Bass.ogg")
    );
    assert_eq!(task_server::share::proxy_path("Media/Proxies/Bass.ogg"), None);
    Ok(())
}
