#![allow(clippy::large_futures)]
//! Attachment blobs fail closed, stay inside their org, and stay with
//! their caller.
//!
//! Three holes this pins shut (cross-org sharing verification, Q1 and
//! required fixes 1, 2 and 8):
//!
//! 1. **Observe-only minted for anyone.** With `TASK_ENFORCE_PERMISSIONS`
//!    unset (dev and desktop nodes) the gate records a would-deny and
//!    lets the call through, so an anonymous caller could call
//!    `get_download_url(hash)` and then fetch the blob. The service now
//!    asks the org's permission engine itself and refuses, whatever the
//!    gate's mode.
//! 2. **The grant was a bearer bound only to the hash.** It is now bound
//!    to the minting org and the minting principal. Redeeming it for
//!    another org, or presenting it as someone else, is refused.
//! 3. **The blob routes served only the first hosted org.** Uploads and
//!    downloads for every other org were answered by the wrong org's
//!    service. The route now serves the org the token names.
//!
//! Everything goes through the real transport: vox over WebSocket for
//! the mint, HTTP for the PUT and the GET.

use architect_auth::CreateEmailPasswordUser;
use attachments_proto::{AttachmentServiceClient, CompleteUpload, ContentHashArg, InitiateUpload};
use task_server::{AppState, router};

const ORG_A: &str = "blob-a";
const ORG_B: &str = "blob-b";

/// Boot ONE server hosting two orgs, observe-only (enforce unset).
async fn boot() -> eyre::Result<(String, AppState, tempfile::TempDir)> {
    let tmp = tempfile::tempdir()?;
    let guard = crate::support::env_lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState::new` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::remove_var("TASK_ENFORCE_PERMISSIONS");
        std::env::remove_var("TASK_SERVER_PUBLIC_URL");
    }
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .init_org(ORG_A, "Blob A", true)
        .map_err(|e| eyre::eyre!("scaffold {ORG_A}: {e}"))?;
    data_root
        .init_org(ORG_B, "Blob B", false)
        .map_err(|e| eyre::eyre!("scaffold {ORG_B}: {e}"))?;
    let state = AppState::new(None).await?;
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("127.0.0.1:{port}"), state, tmp))
}

/// A signed-in user that exists only in `slug`'s auth store. Returns the
/// session token.
pub(crate) async fn user(state: &AppState, slug: &str, email: &str) -> eyre::Result<String> {
    let bundle = state
        .org(slug)
        .ok_or_else(|| eyre::eyre!("{slug} not hosted"))?
        .auth
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: email.into(),
            password: "correct-horse-battery-staple".into(),
            name: None,
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed {email}: {e:?}"))?;
    Ok(bundle.token)
}

/// An `AttachmentService` client on `slug`'s lane, presenting `bearer`
/// at the upgrade (the native channel), or nothing at all.
pub(crate) async fn attachments(
    host: &str,
    slug: &str,
    bearer: Option<&str>,
) -> eyre::Result<AttachmentServiceClient> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let mut request = format!("ws://{host}/org/{slug}/vox").into_client_request()?;
    let headers = request.headers_mut();
    headers.insert("sec-websocket-protocol", "vox.v1".parse()?);
    if let Some(token) = bearer {
        headers.insert("authorization", format!("Bearer {token}").parse()?);
    }
    let (stream, _response) = tokio_tungstenite::connect_async(request).await?;
    vox_core::initiator_on(vox_websocket::WsLink::new(stream))
        .establish::<AttachmentServiceClient>()
        .await
        .map_err(|e| eyre::eyre!("establish: {e:?}"))
}

/// initiate → HTTP PUT → complete, the way the CLI does it. Returns the
/// content hash.
async fn upload(
    host: &str,
    client: &AttachmentServiceClient,
    bytes: &[u8],
    mime: &str,
) -> eyre::Result<String> {
    let ticket = client
        .initiate_upload(InitiateUpload {
            doc_id: "Songs/Blob.md".into(),
            filename: "blob.bin".into(),
            mime_type: mime.into(),
            size_bytes: bytes.len() as u64,
        })
        .await
        .map_err(|e| eyre::eyre!("initiate: {e:?}"))?;
    let resp = reqwest::Client::new()
        .put(format!("http://{host}{}", ticket.upload_url))
        .body(bytes.to_vec())
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    eyre::ensure!(
        status.is_success(),
        "PUT {}: {status} {body}",
        ticket.upload_url
    );
    let hash = body.trim().to_string();
    client
        .complete_upload(CompleteUpload {
            upload_id: ticket.upload_id,
            content_hash: hash.clone(),
        })
        .await
        .map_err(|e| eyre::eyre!("complete: {e:?}"))?;
    Ok(hash)
}

/// GET a (relative) blob URL, optionally with a bearer.
async fn fetch(host: &str, url: &str, bearer: Option<&str>) -> (u16, String, Vec<u8>) {
    let mut req = reqwest::Client::new().get(format!("http://{host}{url}"));
    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await.expect("request");
    let status = resp.status().as_u16();
    let mime = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    (
        status,
        mime,
        resp.bytes().await.unwrap_or_default().to_vec(),
    )
}

async fn mint(
    client: &AttachmentServiceClient,
    hash: &str,
) -> Result<attachments_proto::SignedDownloadUrl, String> {
    client
        .get_download_url(ContentHashArg {
            content_hash: hash.to_owned(),
        })
        .await
        .map_err(|e| format!("{e:?}"))
}

/// One test, one server: the enforce flag is process-wide, and the three
/// properties are cheapest to show against the same two orgs.
#[tokio::test(flavor = "multi_thread")]
async fn blobs_fail_closed_and_stay_in_their_org_and_with_their_caller() -> eyre::Result<()> {
    let (host, state, _tmp) = boot().await?;

    let alice = user(&state, ORG_A, "alice@a.test").await?; // member of A
    let mallory = user(&state, ORG_A, "mallory@a.test").await?; // another member of A
    let bob = user(&state, ORG_B, "bob@b.test").await?; // member of B only

    let alice_a = attachments(&host, ORG_A, Some(&alice)).await?;
    let bob_b = attachments(&host, ORG_B, Some(&bob)).await?;

    // ── Finding 3, uploads: each org's PUT lands in its own service.
    //    Before, the route held only the first hosted org's pending
    //    sessions, so one of these two returned 404.
    let alpha = b"alpha: org A's private stem".to_vec();
    let beta = b"beta: org B's private stem".to_vec();
    let hash_a = upload(&host, &alice_a, &alpha, "audio/x-org-a").await?;
    let hash_b = upload(&host, &bob_b, &beta, "audio/x-org-b").await?;

    // ── Finding 1: enforce OFF, and still no mint without membership.
    let anon_a = attachments(&host, ORG_A, None).await?;
    let err = mint(&anon_a, &hash_a)
        .await
        .expect_err("observe-only must not mint for an anonymous caller");
    assert!(err.contains("Forbidden"), "anonymous refusal: {err}");

    // A signed-in person with no membership in A (bob's session belongs
    // to B) resolves to anonymous on A's lane: refused too.
    let bob_on_a = attachments(&host, ORG_A, Some(&bob)).await?;
    let err = mint(&bob_on_a, &hash_a)
        .await
        .expect_err("a membership-free caller must not mint in org A");
    assert!(err.contains("Forbidden"), "membership-free refusal: {err}");

    // Refused before the store is consulted: no existence oracle.
    let err = mint(
        &anon_a,
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .await
    .expect_err("anonymous probe for a missing hash");
    assert!(
        err.contains("Forbidden"),
        "probe must not see NotFound: {err}"
    );

    // A member can.
    let grant_a = mint(&alice_a, &hash_a).await.expect("a member mints");
    assert!(
        grant_a
            .url
            .starts_with(&format!("/org/{ORG_A}/blobs/download/{hash_a}?token=")),
        "grant names its org: {}",
        grant_a.url
    );

    // ── Finding 2: the grant is bound to its caller and its org.
    let (status, mime, body) = fetch(&host, &grant_a.url, Some(&alice)).await;
    assert_eq!(status, 200, "the minting member redeems");
    assert_eq!(body, alpha);
    assert_eq!(mime, "audio/x-org-a", "served from org A's catalog");

    let (status, _, _) = fetch(&host, &grant_a.url, None).await;
    assert_eq!(status, 401, "a grant is not a bearer: no session, no blob");

    let (status, _, _) = fetch(&host, &grant_a.url, Some(&mallory)).await;
    assert_eq!(
        status, 403,
        "replayed by a different caller in the same org"
    );

    let (status, _, _) = fetch(&host, &grant_a.url, Some(&bob)).await;
    assert_eq!(status, 403, "replayed by a member of another org");

    let in_b = grant_a
        .url
        .replace(&format!("/org/{ORG_A}/"), &format!("/org/{ORG_B}/"));
    let (status, _, _) = fetch(&host, &in_b, Some(&alice)).await;
    assert_eq!(status, 403, "a grant minted in org A is refused for org B");

    // The unprefixed legacy path resolves the org from the token itself.
    let legacy = grant_a.url.replace(&format!("/org/{ORG_A}/"), "/");
    let (status, _, body) = fetch(&host, &legacy, Some(&alice)).await;
    assert_eq!(status, 200, "legacy path serves the token's org");
    assert_eq!(body, alpha);

    // ── Finding 3, downloads: org B's blob from org B's service.
    let grant_b = mint(&bob_b, &hash_b).await.expect("B's member mints in B");
    let (status, mime, body) = fetch(&host, &grant_b.url, Some(&bob)).await;
    assert_eq!(status, 200, "org B's grant redeems");
    assert_eq!(body, beta);
    assert_eq!(
        mime, "audio/x-org-b",
        "served from org B's catalog, not A's"
    );

    let in_a = grant_b
        .url
        .replace(&format!("/org/{ORG_B}/"), &format!("/org/{ORG_A}/"));
    let (status, _, _) = fetch(&host, &in_a, Some(&bob)).await;
    assert_eq!(status, 403, "a grant minted in org B is refused for org A");

    // A tampered token is not a grant at all.
    let (status, _, _) = fetch(&host, &format!("{}x", grant_a.url), Some(&alice)).await;
    assert_eq!(status, 401, "bad signature");

    Ok(())
}
