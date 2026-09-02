//! End-to-end cover for the `telemetry_*` tools on the ACCOUNT-scoped
//! MCP lane (`POST /mcp`): Tempo and Loki read through the server.
//!
//! Its own binary, deliberately: the harness sets `TASK_DATA_ROOT` and
//! the telemetry URLs as process env vars, so two tests in one binary
//! would race each other. Same reason `mcp_account_e2e.rs` holds
//! exactly one test.
//!
//! Tempo and Loki are axum fakes on ephemeral ports serving canned JSON
//! and recording every query string they receive, so the test asserts
//! both the shaping the agent sees and the pass-through the backend
//! sees.

// Every e2e binary compiles the whole of `support`; each uses a subset.
#[allow(dead_code)]
mod support;

use std::sync::{Arc, Mutex};

use axum::{Router, extract::State, routing::get};
use serde_json::{Value, json};

const TOKEN: &str = "mcp-telemetry-operator";

/// What the fakes were asked, in order (raw query strings).
type Seen = Arc<Mutex<Vec<String>>>;

async fn fake_search(
    State(seen): State<Seen>,
    req: axum::http::Request<axum::body::Body>,
) -> axum::Json<Value> {
    seen.lock()
        .unwrap()
        .push(req.uri().query().unwrap_or_default().to_owned());
    axum::Json(json!({
        "traces": [{
            "traceID": "0af7651916cd43dd8448eb211c80319c",
            "rootServiceName": "task-server",
            "rootTraceName": "http.request",
            "startTimeUnixNano": "1756720800000000000",
            "durationMs": 42,
            "spanSet": { "matched": 2, "spans": [{}, {}] }
        }]
    }))
}

async fn fake_trace(
    State(seen): State<Seen>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> axum::Json<Value> {
    seen.lock().unwrap().push(format!("trace={id}"));
    axum::Json(json!({
        "batches": [{
            "resource": { "attributes": [
                { "key": "service.name", "value": { "stringValue": "task-server" } }
            ]},
            "scopeSpans": [{ "spans": [
                {
                    "spanId": "aaaa", "parentSpanId": "", "name": "http.request",
                    "startTimeUnixNano": "1756720800000000000",
                    "endTimeUnixNano": "1756720800042000000",
                    "attributes": [
                        { "key": "auth.outcome", "value": { "stringValue": "rejected" } },
                        { "key": "auth.token_presented", "value": { "boolValue": true } }
                    ],
                    "status": { "code": 2, "message": "unauthorized" }
                }
            ]}]
        }]
    }))
}

async fn fake_logs(
    State(seen): State<Seen>,
    req: axum::http::Request<axum::body::Body>,
) -> axum::Json<Value> {
    let raw = req.uri().query().unwrap_or_default().to_owned();
    seen.lock().unwrap().push(raw.clone());
    // Echo the LogQL back as a log line so the shaped output proves
    // pass-through without the test reaching into `seen`.
    let echoed = raw
        .split('&')
        .find_map(|kv| kv.strip_prefix("query="))
        .map(|q| urlencoding_decode(q))
        .unwrap_or_default();
    axum::Json(json!({
        "status": "success",
        "data": { "resultType": "streams", "result": [{
            "stream": { "namespace": "task", "container": "task-server" },
            "values": [
                ["1756720800000000000", format!("\u{1b}[33mWARN\u{1b}[0m central auth: issuer_unreachable")],
                ["1756720801000000000", format!("echo: {echoed}")]
            ]
        }]}
    }))
}

/// Minimal percent-decoding for the echo above (reqwest encodes the
/// query; the fake only needs it readable again).
fn urlencoding_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Serve the fake Tempo + Loki on one ephemeral port; return its base URL.
async fn serve_fakes(seen: Seen) -> String {
    let app = Router::new()
        .route("/api/search", get(fake_search))
        .route("/api/traces/{id}", get(fake_trace))
        .route("/loki/api/v1/query_range", get(fake_logs))
        .with_state(seen);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{port}")
}

async fn rpc(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    method: &str,
    params: Value,
) -> Value {
    client
        .post(url)
        .bearer_auth(token)
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .expect("POST /mcp")
        .json()
        .await
        .expect("json-rpc body")
}

/// `(isError, payload)` of one tools/call; panics on a protocol error.
async fn call_tool(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    name: &str,
    args: Value,
) -> (bool, Value) {
    let res = rpc(
        client,
        url,
        token,
        "tools/call",
        json!({ "name": name, "arguments": args }),
    )
    .await;
    let result = &res["result"];
    assert!(
        !result.is_null(),
        "tools/call {name} returned a protocol error: {res}"
    );
    let is_err = result["isError"].as_bool().unwrap_or(false);
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    (
        is_err,
        serde_json::from_str(text).unwrap_or_else(|_| json!(text)),
    )
}

async fn tool_names(client: &reqwest::Client, url: &str, token: &str) -> Vec<String> {
    let listed = rpc(client, url, token, "tools/list", json!({})).await;
    listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect()
}

const TELEMETRY_TOOLS: [&str; 4] = [
    "telemetry_status",
    "telemetry_query_traces",
    "telemetry_get_trace",
    "telemetry_query_logs",
];

#[tokio::test(flavor = "multi_thread")]
async fn telemetry_tools_read_the_cluster_for_operators_only() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let fakes = serve_fakes(Arc::clone(&seen)).await;

    // SAFETY: one test per binary. The telemetry URLs and the static
    // token are read per request, not at boot, so setting them before
    // `boot_app_state` takes its env lock is race-free here.
    unsafe {
        std::env::set_var("TASK_TELEMETRY_TEMPO_URL", &fakes);
        std::env::set_var("TASK_TELEMETRY_LOKI_URL", format!("{fakes}/"));
        std::env::set_var("TASK_MCP_TOKEN", TOKEN);
    }
    let (state, _tmp) = support::boot_app_state().await.expect("boot");
    let home = state.org(support::ORG).expect("home org");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, task_server::router(state)).await;
    });
    let url = format!("http://127.0.0.1:{port}/mcp");
    let client = reqwest::Client::new();

    // ── listed when configured ───────────────────────────────────
    let names = tool_names(&client, &url, TOKEN).await;
    for t in TELEMETRY_TOOLS {
        assert!(
            names.contains(&t.to_owned()),
            "{t} missing from tools/list: {names:?}"
        );
    }
    let listed = rpc(&client, &url, TOKEN, "tools/list", json!({})).await;
    let traces_tool = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "telemetry_query_traces")
        .unwrap();
    assert!(
        traces_tool["inputSchema"]["properties"]
            .get("org")
            .is_none(),
        "telemetry tools span orgs and must not take `org`: {traces_tool}"
    );
    assert!(
        traces_tool["description"]
            .as_str()
            .unwrap()
            .contains("span.auth.outcome"),
        "description carries example queries: {traces_tool}"
    );

    // ── status: the static token is an operator ─────────────────
    let (err, status) = call_tool(&client, &url, TOKEN, "telemetry_status", json!({})).await;
    assert!(!err, "{status}");
    assert_eq!(status["allowed"], true, "{status}");
    assert_eq!(status["backends"]["tempo"], fakes, "{status}");
    assert_eq!(
        status["backends"]["loki"], fakes,
        "trailing slash dropped: {status}"
    );

    // ── traces: shaped rows, and the TraceQL reached Tempo ──────
    let traceql = r#"{ resource.service.name = "task-server" && span.auth.outcome = "rejected" }"#;
    let (err, traces) = call_tool(
        &client,
        &url,
        TOKEN,
        "telemetry_query_traces",
        json!({ "traceql": traceql, "since": "15m", "limit": 5 }),
    )
    .await;
    assert!(!err, "{traces}");
    assert_eq!(traces["count"], 1, "{traces}");
    assert_eq!(traces["truncated"], false);
    let row = &traces["traces"][0];
    assert_eq!(row["trace_id"], "0af7651916cd43dd8448eb211c80319c");
    assert_eq!(row["root_service"], "task-server");
    assert_eq!(row["duration_ms"], 42);
    assert_eq!(row["span_count"], 2);
    assert_eq!(row["start"], "2025-09-01T10:00:00.000Z");
    {
        let seen = seen.lock().unwrap();
        let search = seen
            .iter()
            .find(|q| q.contains("q="))
            .expect("Tempo saw a search");
        assert_eq!(
            urlencoding_decode(
                search
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("q="))
                    .unwrap()
            ),
            traceql
        );
        assert!(search.contains("limit=5"), "{search}");
        assert!(
            search.contains("start=") && search.contains("end="),
            "{search}"
        );
    }

    // ── one trace: spans flattened ──────────────────────────────
    let (err, trace) = call_tool(
        &client,
        &url,
        TOKEN,
        "telemetry_get_trace",
        json!({ "trace_id": "0af7651916cd43dd8448eb211c80319c" }),
    )
    .await;
    assert!(!err, "{trace}");
    let span = &trace["spans"][0];
    assert_eq!(span["service"], "task-server");
    assert_eq!(span["name"], "http.request");
    assert_eq!(span["status"], "error");
    assert_eq!(span["duration_ms"], 42.0);
    assert_eq!(span["attributes"]["auth.outcome"], "rejected");
    assert_eq!(span["attributes"]["auth.token_presented"], "true");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|q| q == "trace=0af7651916cd43dd8448eb211c80319c"),
        "Tempo saw the trace id"
    );
    let (err, bad) = call_tool(
        &client,
        &url,
        TOKEN,
        "telemetry_get_trace",
        json!({ "trace_id": "../etc" }),
    )
    .await;
    assert!(
        err,
        "non-hex id must be refused before reaching Tempo: {bad}"
    );

    // ── logs: ANSI stripped, newest first, LogQL passed through ─
    let logql = r#"{service_name="task-server"} |= "central auth""#;
    let (err, logs) = call_tool(
        &client,
        &url,
        TOKEN,
        "telemetry_query_logs",
        json!({ "logql": logql }),
    )
    .await;
    assert!(!err, "{logs}");
    assert_eq!(logs["count"], 2, "{logs}");
    assert_eq!(logs["logs"][0]["line"], format!("echo: {logql}"), "{logs}");
    assert_eq!(
        logs["logs"][1]["line"], "WARN central auth: issuer_unreachable",
        "{logs}"
    );
    assert_eq!(logs["logs"][1]["labels"]["container"], "task-server");
    {
        let seen = seen.lock().unwrap();
        let q = seen
            .iter()
            .find(|q| q.contains("direction=backward"))
            .expect("Loki saw a range query");
        assert!(q.contains("limit=20"), "default limit: {q}");
    }

    // ── bad `since` is a tool error naming the argument ─────────
    let (err, bad) = call_tool(
        &client,
        &url,
        TOKEN,
        "telemetry_query_traces",
        json!({ "traceql": "{}", "since": "soon" }),
    )
    .await;
    assert!(err && bad.as_str().unwrap().contains("since"), "{bad}");

    // ── a plain member's session is refused ─────────────────────
    let member = home
        .auth
        .auth
        .create_email_password_user(architect_auth::CreateEmailPasswordUser {
            email: "member@acme.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Member".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .expect("create member");
    let (err, refused) = call_tool(
        &client,
        &url,
        &member.token,
        "telemetry_query_traces",
        json!({ "traceql": traceql }),
    )
    .await;
    assert!(err, "a member must not read cluster telemetry: {refused}");
    assert!(refused.as_str().unwrap().contains("operator"), "{refused}");
    let (err, status) =
        call_tool(&client, &url, &member.token, "telemetry_status", json!({})).await;
    assert!(!err, "status itself answers: {status}");
    assert_eq!(status["allowed"], false, "{status}");
    // …but the same member still uses the org tools: the refusal is
    // scoped to telemetry, not the lane.
    let (err, tasks) = call_tool(&client, &url, &member.token, "list_tasks", json!({})).await;
    assert!(!err, "{tasks}");

    // ── an admin in the home org is an operator ─────────────────
    let admin = home
        .auth
        .auth
        .create_email_password_user(architect_auth::CreateEmailPasswordUser {
            email: "admin@acme.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Admin".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .expect("create admin");
    home.auth
        .auth
        .set_user_role_local_trusted(admin.user.id, Some("admin".into()))
        .await
        .expect("grant admin");
    let (err, status) = call_tool(&client, &url, &admin.token, "telemetry_status", json!({})).await;
    assert!(!err, "{status}");
    assert_eq!(
        status["allowed"], true,
        "home-org admin is an operator: {status}"
    );
    let (err, logs) = call_tool(
        &client,
        &url,
        &admin.token,
        "telemetry_query_logs",
        json!({ "logql": logql }),
    )
    .await;
    assert!(!err, "{logs}");
    assert_eq!(logs["count"], 2, "{logs}");

    // ── unconfigured: the tools vanish, and calling one is -32601 ─
    // SAFETY: same single-test binary; the config is read per request.
    unsafe {
        std::env::remove_var("TASK_TELEMETRY_TEMPO_URL");
        std::env::remove_var("TASK_TELEMETRY_LOKI_URL");
    }
    let names = tool_names(&client, &url, TOKEN).await;
    for t in TELEMETRY_TOOLS {
        assert!(
            !names.contains(&t.to_owned()),
            "{t} listed without a backend: {names:?}"
        );
    }
    assert!(
        names.contains(&"create_task".to_owned()),
        "org tools still listed"
    );
    let gone = rpc(
        &client,
        &url,
        TOKEN,
        "tools/call",
        json!({ "name": "telemetry_status", "arguments": {} }),
    )
    .await;
    assert_eq!(gone["error"]["code"], -32601, "{gone}");
}
