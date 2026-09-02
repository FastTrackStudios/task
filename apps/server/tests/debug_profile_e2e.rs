//! End-to-end cover for the operator profiling routes
//! (`GET /server/debug/profile`, `GET /server/debug/threads`): refused
//! without an operator, and a real one-second sample for one.
//!
//! Its own binary: the harness sets `TASK_MCP_TOKEN` as a process env
//! var, and pprof keeps one global profiler, so tests in one binary
//! would race each other on both. One test, like `mcp_telemetry_e2e`.

// Every e2e binary compiles the whole of `support`; each uses a subset.
#[allow(dead_code)]
mod support;

use serde_json::Value;

const TOKEN: &str = "debug-profile-operator";

#[tokio::test(flavor = "multi_thread")]
async fn debug_profile_routes_answer_operators_only() {
    // SAFETY: one test per binary; the static token is read per request,
    // not at boot, so setting it before `boot_app_state` takes its env
    // lock is race-free here.
    unsafe {
        std::env::set_var("TASK_MCP_TOKEN", TOKEN);
    }
    let (state, _tmp) = support::boot_app_state().await.expect("boot");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, task_server::router(state)).await;
    });
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // ── no token → 401, one-line body ────────────────────────────
    for path in ["/server/debug/profile?seconds=1", "/server/debug/threads"] {
        let res = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(res.status(), 401, "{path} without a bearer");
        let body = res.text().await.unwrap();
        assert_eq!(body.trim_end().lines().count(), 1, "one line: {body:?}");
        assert!(body.contains("operator"), "{body:?}");
    }
    // A bearer that is neither the static token nor a home-org admin.
    let res = client
        .get(format!("{base}/server/debug/threads"))
        .bearer_auth("not-the-operator")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401, "a stranger's bearer");

    // ── a bad format is a 400, not a sample ──────────────────────
    let res = client
        .get(format!("{base}/server/debug/profile?seconds=1&format=perf"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    // ── threads: a non-empty table with a tokio worker in it ─────
    let res = client
        .get(format!("{base}/server/debug/threads"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let rows: Vec<Value> = res.json().await.expect("json array");
    assert!(!rows.is_empty(), "no threads reported");
    for row in &rows {
        assert!(row["tid"].is_u64(), "{row}");
        assert!(row["name"].is_string(), "{row}");
        assert!(row["cpu_pct"].is_number(), "{row}");
    }
    assert!(
        rows.iter()
            .any(|r| r["name"].as_str().unwrap_or_default().starts_with("tokio")),
        "no tokio worker among {:?}",
        rows.iter().map(|r| r["name"].clone()).collect::<Vec<_>>()
    );
    let pcts: Vec<f64> = rows
        .iter()
        .map(|r| r["cpu_pct"].as_f64().unwrap())
        .collect();
    assert!(
        pcts.windows(2).all(|w| w[0] >= w[1]),
        "not sorted hottest-first: {pcts:?}"
    );

    // ── a one-second flamegraph for the operator ─────────────────
    // SIGPROF samples only threads on a CPU; an idle test process would
    // yield the "no samples" placeholder. Keep one thread busy through
    // both sampling windows so the graph carries real frames.
    let busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let burner = {
        let busy = std::sync::Arc::clone(&busy);
        std::thread::spawn(move || {
            let mut x: u64 = 1;
            while busy.load(std::sync::atomic::Ordering::Relaxed) {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                std::hint::black_box(x);
            }
        })
    };
    let res = client
        .get(format!("{base}/server/debug/profile?seconds=1"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "{}",
        res.text().await.unwrap_or_default()
    );
    let content_type = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        content_type.starts_with("image/svg+xml"),
        "content-type {content_type:?}"
    );
    let svg = res.text().await.unwrap();
    assert!(
        svg.contains("<svg"),
        "not an svg: {}",
        &svg[..svg.len().min(200)]
    );
    assert!(
        !svg.contains("no CPU samples"),
        "a spinning thread must be sampled, got the idle placeholder"
    );

    // ── and the gzipped pprof encoding ───────────────────────────
    let res = client
        .get(format!(
            "{base}/server/debug/profile?seconds=1&format=pprof"
        ))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers().get("content-type").unwrap(),
        "application/octet-stream"
    );
    let bytes = res.bytes().await.unwrap();
    assert_eq!(&bytes[..2], &[0x1f, 0x8b], "not gzip");

    busy.store(false, std::sync::atomic::Ordering::Relaxed);
    burner.join().unwrap();
}
