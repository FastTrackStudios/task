//! End-to-end cover for the MCP agent surface (`POST /org/{slug}/mcp`).
//!
//! One booted `AppState` over an org whose `org.toml` deny-lists
//! `email`, exercised over real HTTP with JSON-RPC bodies:
//!
//! The denied plugin has to be a **non-core** one that owns tools.
//! `PluginSet::resolve` keeps core plugins whatever the deny-list says,
//! so denying a core plugin asserts that something which cannot be
//! turned off is off. This named `scheduling` until scheduling became
//! core, at which point it failed for a reason that read like a gate
//! bug and was not one.
//!
//! - **auth**: `initialize` answers without a token; `tools/list` and
//!   `tools/call` refuse a missing/wrong bearer and accept
//!   `TASK_MCP_TOKEN`;
//! - **round-trip**: `create_task` → `list_tasks` sees the task,
//!   `update_task` moves it to done;
//! - **plugin gate**: the email tools are absent from
//!   `tools/list` and `tools/call read_email` fails with a message
//!   naming the plugin, while unknown tools stay `-32601`;
//! - **discovery**: `api_reference` lists the vox surface with the
//!   email services flagged unmounted.
//!
//! Self-sandboxed: tempdir data root via `TASK_DATA_ROOT`, one test
//! per binary so the env setup races nothing.

use serde_json::{Value, json};
use task_server::AppState;

const TOKEN: &str = "mcp-e2e-secret";

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

#[tokio::test(flavor = "multi_thread")]
async fn mcp_surface_end_to_end() {
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
    let org_root = data_root.init_org("alpha", "Alpha", true).unwrap();
    std::fs::create_dir_all(org_root.vault_dir()).unwrap();
    // Deny-list email — the plugin gate under test. Non-core, so it can
    // actually be denied; a core plugin survives any deny-list.
    let mut manifest = org_root.manifest().unwrap();
    manifest.disabled_plugins = vec!["email".to_owned()].into();
    manifest.write_to_dir(org_root.path()).unwrap();

    let state = AppState::new(None).await.expect("boot AppState");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = task_server::router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let url = format!("http://127.0.0.1:{port}/org/alpha/mcp");
    let client = reqwest::Client::new();

    // ── initialize: unauthenticated, carries orientation ─────────
    let init = rpc(&client, &url, None, "initialize", json!({})).await;
    let instructions = init["result"]["instructions"]
        .as_str()
        .expect("instructions");
    assert!(instructions.contains("org `alpha`"));

    // ── auth: refused without (or with a wrong) bearer ───────────
    let denied = rpc(&client, &url, None, "tools/list", json!({})).await;
    assert!(
        denied["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("token")),
        "unauthenticated tools/list must fail: {denied}"
    );
    let denied = rpc(&client, &url, Some("wrong"), "tools/list", json!({})).await;
    assert!(!denied["error"].is_null(), "bad bearer must fail: {denied}");

    // ── tools/list: filtered by the org's plugin set ─────────────
    let listed = rpc(&client, &url, Some(TOKEN), "tools/list", json!({})).await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for core in ["create_task", "claim_task", "api_reference", "write_note"] {
        assert!(
            names.contains(&core),
            "core tool `{core}` missing: {names:?}"
        );
    }
    for denied in ["read_email", "list_envelopes", "list_email_accounts"] {
        assert!(
            !names.contains(&denied),
            "email tool `{denied}` listed despite the deny-list"
        );
    }

    // ── round-trip: create → list → update ───────────────────────
    let (err, created) = call_tool(
        &client,
        &url,
        "create_task",
        json!({ "text": "Ship the MCP surface !high", "due": "2026-08-01" }),
    )
    .await;
    assert!(!err, "create_task failed: {created}");
    let id = created["id"].as_str().expect("created id").to_owned();
    assert_eq!(created["priority"], "high");

    let (err, listed) = call_tool(&client, &url, "list_tasks", json!({})).await;
    assert!(!err, "list_tasks failed: {listed}");
    assert!(
        listed["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .any(|t| t["id"] == id.as_str()),
        "created task not listed: {listed}"
    );

    let (err, updated) = call_tool(
        &client,
        &url,
        "update_task",
        json!({ "id": id, "status": "done" }),
    )
    .await;
    assert!(!err, "update_task failed: {updated}");
    assert_eq!(updated["status"], "done");

    // ── plugin gate on tools/call ────────────────────────────────
    let (err, refused) = call_tool(&client, &url, "read_email", json!({})).await;
    assert!(err, "disabled plugin's tool must be a tool-level error");
    let msg = refused.as_str().expect("error text");
    assert!(msg.contains("email"), "{msg}");
    assert!(msg.contains("disabled"), "{msg}");

    // Unknown tools stay protocol-level method-not-found.
    let unknown = rpc(
        &client,
        &url,
        Some(TOKEN),
        "tools/call",
        json!({ "name": "no_such_tool", "arguments": {} }),
    )
    .await;
    assert_eq!(unknown["error"]["code"], -32601, "{unknown}");

    // ── discovery: the full surface with mounted flags ───────────
    let (err, api) = call_tool(&client, &url, "api_reference", json!({})).await;
    assert!(!err, "api_reference failed: {api}");
    let services = api["services"].as_array().expect("services");
    assert!(services.len() > 50, "the surface is ~80 services");
    let calendar = services
        .iter()
        .find(|s| s["plugin"] == "email")
        .expect("email services are still listed");
    assert_eq!(calendar["mounted"], false, "…but flagged unmounted");
    assert!(
        services
            .iter()
            .filter(|s| s["plugin"] == "core")
            .all(|s| s["mounted"] == true),
        "core services stay mounted"
    );

    // Single-service expansion carries permits.
    let (err, task_api) = call_tool(
        &client,
        &url,
        "api_reference",
        json!({ "service": "TaskService" }),
    )
    .await;
    assert!(!err, "api_reference(service) failed: {task_api}");
    let methods = task_api["services"][0]["methods"]
        .as_array()
        .expect("methods");
    let create = methods
        .iter()
        .find(|m| m["name"] == "create")
        .expect("task create method");
    assert_eq!(create["permit"], "write tasks/**");

    // ── triage loop: see the unfiled, file it, watch it leave ────
    let (err, bare) = call_tool(
        &client,
        &url,
        "create_task",
        json!({ "text": "Telemetry + Observability: Sentry" }),
    )
    .await;
    assert!(!err, "create_task failed: {bare}");
    let bare_id = bare["id"].as_str().expect("id").to_owned();
    assert_eq!(bare["filed"], false, "a bare title is unfiled: {bare}");

    let (err, proj) = call_tool(
        &client,
        &url,
        "create_project",
        json!({ "title": "Task platform" }),
    )
    .await;
    assert!(!err, "create_project failed: {proj}");
    let project_id = proj["id"].as_str().expect("project id").to_owned();

    let (err, queue) = call_tool(&client, &url, "list_untriaged_tasks", json!({})).await;
    assert!(!err, "list_untriaged_tasks failed: {queue}");
    assert!(
        queue["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .any(|t| t["id"] == bare_id.as_str()),
        "the bare task must be in the triage queue: {queue}"
    );
    assert!(
        queue["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .any(|p| p["id"] == project_id.as_str()),
        "candidate homes ride along so triage is one call: {queue}"
    );

    // Filing needs an anchor — an empty call is a no-op, not a lie.
    let (err, _) = call_tool(&client, &url, "file_task", json!({ "id": bare_id })).await;
    assert!(err, "file_task with nothing to file by must fail");

    let (err, filed) = call_tool(
        &client,
        &url,
        "file_task",
        json!({
            "id": bare_id,
            "project": project_id,
            "reason": "Sentry wiring is platform observability work",
        }),
    )
    .await;
    assert!(!err, "file_task failed: {filed}");
    assert_eq!(filed["filed"], true);
    assert_eq!(filed["project_id"], project_id.as_str());
    assert_eq!(
        filed["projects"][0], "[[Task platform]]",
        "the markdown page keeps its human-readable wikilink: {filed}"
    );

    // And it's out of the queue — the loop terminates.
    let (err, queue) = call_tool(&client, &url, "list_untriaged_tasks", json!({})).await;
    assert!(!err, "list_untriaged_tasks failed: {queue}");
    assert!(
        !queue["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .any(|t| t["id"] == bare_id.as_str()),
        "a filed task must leave the triage queue: {queue}"
    );

    // A parent that doesn't exist would orphan the task invisibly.
    let (err, bad) = call_tool(
        &client,
        &url,
        "file_task",
        json!({ "id": bare_id, "parent": uuid::Uuid::new_v4().to_string() }),
    )
    .await;
    assert!(err, "filing under a nonexistent parent must fail: {bad}");
}
