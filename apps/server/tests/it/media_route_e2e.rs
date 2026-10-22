#![allow(clippy::large_futures)]
//! Node parity for org-less song media (ADR 0006 audit C4): `GET
//! /media/songs/<slug>/manifest.json` is answered by **every** node, not
//! only by the `task-server` binary.
//!
//! The route used to be added in the binary's `main`, after
//! `task_server::router` had been built, so a desktop node serving the
//! same router on loopback (`LocalNode::serve_loopback`) answered the
//! UI's `/media/…` URLs with a 404 while the server it stands in for
//! served the file. It now lives in `task_server::media_router`, which
//! `router` merges.
//!
//! 1. [`a_desktop_node_serves_the_same_media_bytes_as_the_server`] — the
//!    server and a desktop node, each over the planted example studio,
//!    return the same bytes for a seeded song's manifest — the bytes
//!    committed under `examples/studio/acme-audio/Resources/songs/`.
//! 2. Host-absent stage, same test: the server is stopped, and two
//!    device nodes still serve that manifest to each other's clients,
//!    byte for byte.

use std::path::{Path, PathBuf};

use task_server::example_org::song_slug;
use task_server::local_node::{LocalNode, NodeOptions};

use crate::support;

/// A seeded audio deliverable of `acme-audio` whose song folder is
/// committed (`example_org::declared_tests` pins that every audio
/// deliverable has one).
const SONG: &str = "Single master";

/// The manifest as committed in the seeded example.
fn committed_manifest(slug: &str) -> Vec<u8> {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/studio")
        .join(support::ORG)
        .join("Resources/songs")
        .join(slug)
        .join("manifest.json");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `GET <base>/media/<rel>` on a fresh connection (no pooled socket can
/// outlive a stopped server): status, content type, body.
async fn get_media(base: &str, rel: &str) -> reqwest::Result<(u16, String, Vec<u8>)> {
    let resp = reqwest::Client::new()
        .get(format!("{base}/media/{rel}"))
        .send()
        .await?;
    let status = resp.status().as_u16();
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = resp.bytes().await?.to_vec();
    Ok((status, ct, body))
}

/// A desktop node over its own data root with the example studio
/// planted (debug builds plant it, as `just demo desktop` does), served
/// on loopback. Returns the node (kept alive by the caller) and its
/// `http://` base.
async fn device_node(root: &Path) -> (LocalNode, String) {
    let node = {
        let _env = support::env_lock().await;
        // SAFETY: under the binary's env lock. The plant would otherwise
        // download a Bible translation; this test reads song media only.
        unsafe { std::env::set_var("TASK_DEMO_NO_BIBLE", "1") };
        LocalNode::open(NodeOptions {
            plant_example: true,
            peers: false,
            ..NodeOptions::at(root)
        })
        .await
        .expect("a desktop node over the planted example")
    };
    let ws = node.serve_loopback().await.expect("served on loopback");
    let base = ws.replacen("ws://", "http://", 1);
    (node, base)
}

// t[verify offline.app-node] — node parity: what the server serves at
// `/media/…`, a desktop node's loopback serves too, with or without the
// server
#[tokio::test(flavor = "multi_thread")]
async fn a_desktop_node_serves_the_same_media_bytes_as_the_server() {
    let slug = song_slug(SONG);
    let rel = format!("songs/{slug}/manifest.json");
    let committed = committed_manifest(&slug);

    // ── The server: the example studio, served by `task_server::router`
    //    exactly as the binary serves it.
    let (state, _server_root) = support::boot_app_state()
        .await
        .expect("boot the server over the example studio");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the server");
    let server_base = format!("http://{}", listener.local_addr().expect("server addr"));
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, task_server::router(state)).await;
    });

    let (status, ct, from_server) = get_media(&server_base, &rel)
        .await
        .expect("the server answers");
    assert_eq!(status, 200, "the server serves {rel}");
    assert_eq!(ct, "application/json");
    assert_eq!(
        from_server, committed,
        "the server serves the committed manifest of `{slug}`"
    );

    // ── A desktop node: same request on its loopback, same bytes.
    let laptop_root = tempfile::tempdir().unwrap();
    let (_laptop, laptop) = device_node(laptop_root.path()).await;
    let (status, ct, from_laptop) = get_media(&laptop, &rel)
        .await
        .expect("the node's loopback answers");
    assert_eq!(
        status, 200,
        "a desktop node serves {rel} — the route is in `router`, not the binary"
    );
    assert_eq!(ct, "application/json");
    assert_eq!(
        from_laptop, from_server,
        "the node and the server serve the same bytes"
    );

    // A song no org has is a 404 on a node as on the server, and so is
    // a `..` segment: it never reaches past `resources/`.
    for missing in ["songs/no-such-song/manifest.json", "songs/%2E%2E/x"] {
        let (status, _, _) = get_media(&laptop, missing).await.expect("node answers");
        assert_eq!(status, 404, "node: /media/{missing}");
        let (status, _, _) = get_media(&server_base, missing)
            .await
            .expect("server answers");
        assert_eq!(status, 404, "server: /media/{missing}");
    }

    // ── Host absent: the server stops; two device nodes still serve the
    //    song, and to the same bytes the server served.
    server.abort();
    let _ = server.await;
    assert!(
        get_media(&server_base, &rel).await.is_err(),
        "the server is down — nothing answers at {server_base}"
    );
    let phone_root = tempfile::tempdir().unwrap();
    let (_phone, phone) = device_node(phone_root.path()).await;
    for (name, base) in [("laptop", &laptop), ("phone", &phone)] {
        let (status, _, body) = get_media(base, &rel)
            .await
            .unwrap_or_else(|e| panic!("{name}'s node answers with the server gone: {e}"));
        assert_eq!(status, 200, "{name}: {rel} with no server");
        assert_eq!(
            body, from_server,
            "{name}: the same bytes the server served"
        );
    }
}
