//! End-to-end cover for the ACCOUNT-scoped MCP lane (`POST /mcp`).
//!
//! Its own binary, deliberately: the harness sets `TASK_DATA_ROOT` as
//! a process env var, so two tests in one binary race each other's
//! data root. Same reason `mcp_e2e.rs` holds exactly one test.

use serde_json::{Value, json};
use task_server::AppState;

const TOKEN: &str = "mcp-account-secret";

/// POST one JSON-RPC request, with an optional bearer.
async fn rpc(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    method: &str,
    params: Value,
) -> Value {
    let mut req = client.post(url).json(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    }));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    req.send()
        .await
        .expect("POST /mcp")
        .json()
        .await
        .expect("json-rpc body")
}

/// A tool call's decoded payload: the content text parsed as JSON,
/// plus the isError flag.
async fn call_tool(client: &reqwest::Client, url: &str, name: &str, args: Value) -> (bool, Value) {
    let res = rpc(
        client,
        url,
        Some(TOKEN),
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
    let payload = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
    (is_err, payload)
}

/// The ACCOUNT-scoped lane (`POST /mcp`): one endpoint, every org the
/// caller can reach, with `org` selecting per call.
///
/// Uses the static `TASK_MCP_TOKEN`, which reaches every hosted org —
/// the session-token path additionally consults the identity locker,
/// which needs a home org and a linked credential to be meaningful.
#[tokio::test(flavor = "multi_thread")]
async fn mcp_account_lane_spans_orgs() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: one test per binary, so nothing races this env setup.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::set_var("TASK_MCP_TOKEN", TOKEN);
        for var in ["TASK_SERVER_ORG", "TASK_SERVER_VAULT_ROOT"] {
            std::env::remove_var(var);
        }
    }
    let data_root = org_proto::DataRoot::from_env().unwrap();
    data_root.ensure().unwrap();
    for (slug, name, home) in [("home", "Home", true), ("second", "Second", false)] {
        let root = data_root.init_org(slug, name, home).unwrap();
        std::fs::create_dir_all(root.vault_dir()).unwrap();
    }

    let state = AppState::new(None).await.expect("boot AppState");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, task_server::router(state)).await;
    });
    let url = format!("http://127.0.0.1:{port}/mcp");
    let client = reqwest::Client::new();

    // ── initialize names the account, not one org ────────────────
    let init = rpc(&client, &url, None, "initialize", json!({})).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "task");

    // ── list_orgs reports the whole reachable set ────────────────
    let (err, orgs) = call_tool(&client, &url, "list_orgs", json!({})).await;
    assert!(!err, "list_orgs failed: {orgs}");
    let slugs: Vec<&str> = orgs["orgs"]
        .as_array()
        .expect("orgs")
        .iter()
        .filter_map(|o| o["slug"].as_str())
        .collect();
    assert!(
        slugs.contains(&"home") && slugs.contains(&"second"),
        "{orgs}"
    );
    assert_eq!(orgs["default"], "home", "home org is the default: {orgs}");

    // ── every tool carries the `org` argument ────────────────────
    let listed = rpc(&client, &url, Some(TOKEN), "tools/list", json!({})).await;
    let tools = listed["result"]["tools"].as_array().expect("tools");
    let create = tools
        .iter()
        .find(|t| t["name"] == "create_task")
        .expect("create_task listed");
    assert!(
        create["inputSchema"]["properties"]["org"].is_object(),
        "create_task should take `org`: {create}"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "list_orgs"),
        "list_orgs is advertised"
    );

    // ── a call routes to the org it names ────────────────────────
    let (err, made) = call_tool(
        &client,
        &url,
        "create_task",
        json!({ "text": "second-org work", "org": "second" }),
    )
    .await;
    assert!(!err, "create_task in `second` failed: {made}");

    // …and lands ONLY there: separate vaults, not one pooled list.
    let (err, second) = call_tool(&client, &url, "list_tasks", json!({ "org": "second" })).await;
    assert!(!err, "{second}");
    assert!(
        second["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .any(|t| t["title"] == "second-org work"),
        "task missing from its own org: {second}"
    );
    let (err, home) = call_tool(&client, &url, "list_tasks", json!({})).await;
    assert!(!err, "{home}");
    assert!(
        !home["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .any(|t| t["title"] == "second-org work"),
        "a task leaked across orgs into the default: {home}"
    );

    // ── an unhosted org is refused, not silently defaulted ───────
    // Protocol-level (-32600), not a tool error: naming an org you
    // cannot reach is a bad request, and falling back to the default
    // would silently write to the wrong vault.
    let refused = rpc(
        &client,
        &url,
        Some(TOKEN),
        "tools/call",
        json!({ "name": "list_tasks", "arguments": { "org": "nope" } }),
    )
    .await;
    assert_eq!(refused["error"]["code"], -32600, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("nope")),
        "the error should name the org: {refused}"
    );
}
