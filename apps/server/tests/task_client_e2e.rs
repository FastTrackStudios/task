//! `task-client` end to end: the same code, both transports.
//!
//! ADR 0004 decision 4 says Task is embeddable as a library, and the
//! claim underneath it is that **an external app can do exactly what a
//! plugin does**. That claim is only worth anything if it is checked
//! against a real server and a real in-process backend, holding the
//! same client type and calling the same method — which is what this
//! binary does.
//!
//! Four things get pinned here, in one test because the embedded
//! backend is a process-wide singleton (see `task_client`'s `EMBEDDED`)
//! and a second test in the same binary would race it:
//!
//! 1. A `ProjectServiceClient` established over a WebSocket answers.
//! 2. The *same type*, established in-process with no socket, answers
//!    with the same rows. Only configuration differed.
//! 3. An explicitly-named remote that is down fails loud — it does
//!    **not** quietly fall back to a local copy of the world. This is
//!    the guard that makes the local-first fallback honest, and it is
//!    the one that would be easiest to lose in a refactor.
//! 4. A session file with the token written inline is rejected with an
//!    explanation, rather than silently producing anonymous requests.
//!
//! Runs against the example studio (`support::boot_app_state`), so the
//! world it asserts on is the seeded world every other e2e test uses.

use project::ProjectServiceClient;
use task_client::{Config, TaskClient};

#[allow(dead_code)]
mod support;

#[tokio::test(flavor = "multi_thread")]
async fn one_client_type_two_transports() {
    // Point the session at a path that does not exist, before anything
    // can read one. A test must never pick up the developer's real
    // credentials — nor fail on this machine because they happen to be
    // signed into something.
    let session_home = tempfile::tempdir().expect("tempdir");
    let session_file = session_home.path().join("session.json");
    // SAFETY: set once, before any client is built, and this binary
    // runs exactly one test.
    unsafe {
        std::env::set_var("TASK_SESSION_FILE", &session_file);
    }

    let (base_with_path, _tmp) = support::boot_ws().await.expect("boot ws server");
    // `boot_ws` hands back the per-org hint shape (`…/vox`); the client
    // normalizes it to a base, which is exactly the case
    // `normalize_server_base` exists for.
    let base = base_with_path.trim_end_matches("/vox").to_owned();

    // ── 1. over the wire ────────────────────────────────────────────
    let remote = TaskClient::remote(&base);
    assert!(!remote.is_embedded());
    let over_ws: ProjectServiceClient = remote
        .org(support::ORG)
        .await
        .expect("establish over the WebSocket transport");
    let ws_rows = over_ws.list().await.expect("list() over the wire");

    // ── 2. in-process ───────────────────────────────────────────────
    // The same generic call, the same client type. The ONLY difference
    // from the block above is the configuration on the line before it.
    let embedded = TaskClient::embedded();
    assert!(embedded.is_embedded());
    let in_process: ProjectServiceClient = embedded
        .org(support::ORG)
        .await
        .expect("establish over the in-process LocalServer");
    let local_rows = in_process.list().await.expect("list() in-process");

    let mut ws_titles: Vec<_> = ws_rows.iter().map(|p| p.title.clone()).collect();
    let mut local_titles: Vec<_> = local_rows.iter().map(|p| p.title.clone()).collect();
    ws_titles.sort();
    local_titles.sort();
    assert_eq!(
        ws_titles, local_titles,
        "the same org, reached two ways, is the same org"
    );

    // A second establish off the same client: `org()` hands out lanes,
    // it is not itself a connection.
    let _second: ProjectServiceClient = embedded
        .org(support::ORG)
        .await
        .expect("a second lane onto the same in-process org");

    // ── 3. an explicit remote fails loud ────────────────────────────
    // Port 1 is not a Task server on any machine. The org IS on disk
    // here, so if the fallback were unguarded this call would succeed —
    // and would answer from a different world than the one asked for.
    let dead = TaskClient::remote("ws://127.0.0.1:1");
    let Err(err) = dead.org::<ProjectServiceClient>(support::ORG).await else {
        panic!("a named remote that is down must fail, not fall back");
    };
    assert!(
        err.is_transport(),
        "the failure is a transport failure, not a not-found: {err}"
    );
    assert!(
        err.to_string().contains("127.0.0.1:1"),
        "the error names the endpoint it could not reach: {err}"
    );

    // The same target with the fallback armed (`embed: None`) still
    // fails, because the guard is "the localhost DEFAULT", not "any
    // loopback address" — a deliberately-named local port is as
    // explicit a target as a hostname.
    let armed = TaskClient::new(Config {
        server: Some("ws://127.0.0.1:1".into()),
        embed: None,
        use_session: false,
        ..Config::new()
    });
    assert!(
        armed
            .org::<ProjectServiceClient>(support::ORG)
            .await
            .is_err(),
        "a named local port is still a named target"
    );

    // ── 4. the session trap is loud ─────────────────────────────────
    // A session file with the token written inline and no `user_id`:
    // the shape a person reaches for, and the shape that used to parse
    // fine, print fine under `whoami`, and send every request with no
    // Authorization header at all.
    std::fs::write(
        &session_file,
        r#"{
          "home": "acme",
          "active": "acme",
          "servers": {
            "acme": { "url": "local", "slug": "acme-audio", "token": "pretend-token" }
          }
        }"#,
    )
    .expect("write a hand-made session file");
    let err = task_client::session::load().expect_err("an inline token must not be swallowed");
    let msg = err.to_string();
    assert!(
        msg.contains("inline `token`") && msg.contains("ROUTING"),
        "the error explains the routing-document rule: {msg}"
    );
    assert!(
        msg.contains("anonymous is not a member"),
        "…and names the symptom it would otherwise have produced: {msg}"
    );

    // With a `user_id` alongside it, the same file is repaired rather
    // than refused: the token is promoted into its own `0600` file and
    // the routing document is rewritten without it.
    std::fs::write(
        &session_file,
        r#"{
          "home": "acme",
          "active": "acme",
          "servers": {
            "acme": {
              "url": "local",
              "slug": "acme-audio",
              "token": "pretend-token",
              "user_id": "00000000-0000-0000-0000-000000000001",
              "email": "someone@example.com"
            }
          }
        }"#,
    )
    .expect("write a pre-split session file");
    let sess = task_client::session::load()
        .expect("a complete inline entry is adopted")
        .expect("…and yields a session");
    assert_eq!(
        sess.active_server().expect("active entry").token,
        "pretend-token"
    );
    let promoted = session_home.path().join("session-tokens").join("acme.json");
    assert!(
        promoted.is_file(),
        "the token was promoted to {}",
        promoted.display()
    );
    let rewritten = std::fs::read_to_string(&session_file).expect("re-read the routing document");
    assert!(
        !rewritten.contains("pretend-token"),
        "the routing document no longer holds the secret: {rewritten}"
    );

    // Nothing at all for a key is still a drop — but a reported one.
    std::fs::remove_file(&promoted).expect("remove the token file");
    let report = task_client::session::load_report().expect("load reports rather than fails");
    assert_eq!(report.dropped.len(), 1, "the empty key is reported");
    assert_eq!(report.dropped[0].key, "acme");
}
