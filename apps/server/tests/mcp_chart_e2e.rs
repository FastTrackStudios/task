//! End-to-end cover for the chart tools on the MCP lane — the surface
//! a foreign browser app drives with nothing but `fetch` and a bearer
//! token.
//!
//! **Why this lane exists at all.** ADR 0003 makes the sibling apps
//! (Session, Signal, Ignition, Keyflow) ordinary clients of Task, and
//! the transport it names is vox over a WebSocket — the typed lane
//! Task's own web client rides, and the one `chart_library_e2e.rs`
//! exercises. Keyflow cannot link that client yet: it pins architect
//! v0.0.2 against Task's v0.7.1, and two architect majors in one wasm
//! binary means two reqwest majors, which fails at link time with
//! duplicate `intounderlyingsource_*` symbols out of rust-lld. Until
//! that pin moves it needs plain HTTP, and `POST /mcp` already is
//! plain HTTP with a bearer token and org resolution.
//!
//! So the thing under test is not a chart store — there is exactly one
//! of those, and `chart_library_e2e.rs` pins it through vox. It is the
//! claim that the HTTP lane reaches *that* store faithfully:
//!
//! - the four tools are listed, on both the org and the account lane;
//! - `write_chart` → `list_charts` → `read_chart` round-trips the
//!   source BYTE for byte, trailing spaces and all, because a chart is
//!   a document a musician typed and not a value to normalise;
//! - a listing carries no `source` — 200 charts must not land in one
//!   agent message, and Keyflow's index screen does not need the text;
//! - `delete_chart` is idempotent: `true` then `false`, never an error,
//!   because a second tab hitting delete is a race and not a fault;
//! - the account lane routes by `org` and orgs do not leak into each
//!   other — separate vaults, separate charts;
//! - a caller with no bearer, a wrong bearer, or an org it cannot
//!   reach gets nothing, and writes nothing.
//!
//! Self-sandboxed the way `mcp_e2e.rs` and `mcp_account_e2e.rs` are:
//! a tempdir data root via `TASK_DATA_ROOT`, one test per binary so
//! the env setup races nothing.

use serde_json::{Value, json};
use task_server::AppState;

const TOKEN: &str = "mcp-chart-e2e-secret";

/// The exact bytes the round-trip must preserve: a tab-indented line,
/// a line with trailing whitespace, a non-ASCII accidental, and no
/// final newline. Every one of these is something a well-meaning
/// "tidy the text up" step would eat.
const SOURCE: &str =
    "[Verse 1]\n| A | E | F#m | D |   \n\t| D | A |\n\n[Chorus]\n| B\u{266d} | E\u{266d} |";
const REVISED: &str = "[Verse 1]\n| G | D | Em | C |\n";

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

/// The tools this lane must advertise, in catalog order.
const CHART_TOOLS: &[&str] = &["list_charts", "read_chart", "write_chart", "delete_chart"];

#[tokio::test(flavor = "multi_thread")]
async fn mcp_chart_tools_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: one test per binary, so nothing races this env setup.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::set_var("TASK_MCP_TOKEN", TOKEN);
        for var in [
            "TASK_SERVER_ORG",
            "TASK_SERVER_VAULT_ROOT",
            "TASK_SERVER_WIKI_ROOT",
        ] {
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
    let org_url = format!("http://127.0.0.1:{port}/org/home/mcp");
    let account_url = format!("http://127.0.0.1:{port}/mcp");
    let client = reqwest::Client::new();

    // ── the four tools are on both lanes ─────────────────────────
    //
    // Keyflow will talk to the account lane (one endpoint, `org` per
    // call); an agent pinned to one org talks to the other. Both have
    // to carry the whole set or the client has to know which is which.
    for url in [&org_url, &account_url] {
        let listed = rpc(&client, url, Some(TOKEN), "tools/list", json!({})).await;
        let tools = listed["result"]["tools"].as_array().expect("tools");
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        for tool in CHART_TOOLS {
            assert!(
                names.contains(tool),
                "`{tool}` missing from {url}: {names:?}"
            );
        }
        // The account lane grows `org` on every tool; the org lane
        // must not, because the slug is already in the path.
        let write = tools
            .iter()
            .find(|t| t["name"] == "write_chart")
            .expect("write_chart listed");
        let has_org = write["inputSchema"]["properties"]["org"].is_object();
        assert_eq!(
            has_org,
            url == &account_url,
            "`org` argument on the wrong lane: {write}"
        );
        // `source` is required to write, `slug` is not: an omitted
        // slug is what makes this a create.
        let required: Vec<&str> = write["inputSchema"]["required"]
            .as_array()
            .expect("required")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            required.contains(&"title") && required.contains(&"source"),
            "{write}"
        );
        assert!(
            !required.contains(&"slug"),
            "an omitted slug creates: {write}"
        );
    }

    // ── unauthenticated and wrong-bearer callers get nothing ─────
    //
    // Checked BEFORE anything is written, so "the list is empty" below
    // cannot be mistaken for "the denied write silently worked".
    for (label, token) in [("no bearer", None), ("wrong bearer", Some("not-the-token"))] {
        let denied = rpc(
            &client,
            &org_url,
            token,
            "tools/call",
            json!({
                "name": "write_chart",
                "arguments": { "title": "Trespass", "source": "| A |" },
            }),
        )
        .await;
        assert!(
            !denied["error"].is_null() && denied["result"].is_null(),
            "{label} must be refused outright: {denied}"
        );
        let denied = rpc(&client, &org_url, token, "tools/list", json!({})).await;
        assert!(!denied["error"].is_null(), "{label} tools/list: {denied}");
    }

    // An org this caller cannot reach is a bad request, not a quiet
    // fallback to the default — writing a chart into the wrong vault
    // is exactly the failure the account lane must never have.
    let refused = rpc(
        &client,
        &account_url,
        Some(TOKEN),
        "tools/call",
        json!({ "name": "list_charts", "arguments": { "org": "nope" } }),
    )
    .await;
    assert_eq!(refused["error"]["code"], -32600, "{refused}");

    // Nothing above landed anywhere.
    let (err, empty) = call_tool(&client, &org_url, "list_charts", json!({})).await;
    assert!(!err, "list_charts on a fresh org failed: {empty}");
    assert_eq!(empty["count"], 0, "a refused write left a chart: {empty}");

    // ── write → list → read, byte for byte ───────────────────────
    let (err, saved) = call_tool(
        &client,
        &org_url,
        "write_chart",
        json!({
            "title": "Great Are You Lord",
            "source": SOURCE,
            "key": "A",
            "notation": "keyflow",
            "sections": ["verse-1", "chorus"],
        }),
    )
    .await;
    assert!(!err, "write_chart failed: {saved}");
    assert_eq!(
        saved["slug"], "great-are-you-lord",
        "slug from title: {saved}"
    );
    assert_eq!(
        saved["rel_path"], "great-are-you-lord.md",
        "ADR 0004: a chart is a shelf document, and the path an agent \
         gets back is the one `VaultSync` takes"
    );
    assert_eq!(saved["created"], true);
    assert_eq!(saved["node"], "chart:great-are-you-lord");

    let (err, listed) = call_tool(&client, &org_url, "list_charts", json!({})).await;
    assert!(!err, "list_charts failed: {listed}");
    assert_eq!(listed["count"], 1, "{listed}");
    let row = &listed["charts"][0];
    assert_eq!(row["slug"], "great-are-you-lord");
    assert_eq!(row["title"], "Great Are You Lord");
    assert_eq!(row["key"], "A");
    assert_eq!(row["notation"], "keyflow");
    assert!(
        row["updated_at"].as_str().is_some_and(|t| !t.is_empty()),
        "the listing stamps when it was written: {row}"
    );
    assert!(
        row.get("source").is_none(),
        "a listing must not carry chart text: {row}"
    );

    let (err, doc) = call_tool(
        &client,
        &org_url,
        "read_chart",
        json!({ "slug": "great-are-you-lord" }),
    )
    .await;
    assert!(!err, "read_chart failed: {doc}");
    assert_eq!(
        doc["source"].as_str().expect("source"),
        format!("{SOURCE}\n"),
        "the source must survive the round trip verbatim — but for the \
         one byte a fence cannot represent: ADR 0004 stores a chart in a \
         ```keyflow block in its own shelf document, and a block cannot \
         hold a source that does not end in a newline, so one is added. \
         Recorded in `resources_proto::assets`, and the only difference."
    );
    assert_eq!(doc["title"], "Great Are You Lord");
    assert_eq!(doc["key"], "A");
    assert_eq!(doc["sections"], json!(["verse-1", "chorus"]));

    // ── re-save by slug replaces the source, does not create ─────
    let (err, again) = call_tool(
        &client,
        &org_url,
        "write_chart",
        json!({
            "title": "Great Are You Lord",
            "slug": "great-are-you-lord",
            "source": REVISED,
            "key": "G",
            "sections": ["verse-1"],
        }),
    )
    .await;
    assert!(!err, "re-save failed: {again}");
    assert_eq!(again["created"], false, "the slug is the identity: {again}");
    let (_, doc) = call_tool(
        &client,
        &org_url,
        "read_chart",
        json!({ "slug": "great-are-you-lord" }),
    )
    .await;
    assert_eq!(doc["source"], REVISED, "the source is replaced, not merged");
    assert_eq!(doc["key"], "G");

    // A missing slug is a tool error the model can act on, not a
    // transport failure it can only report.
    let (err, missing) =
        call_tool(&client, &org_url, "read_chart", json!({ "slug": "nope" })).await;
    assert!(err, "reading a missing chart must be an error: {missing}");
    let text = missing.as_str().unwrap_or_default();
    assert!(text.contains("list_charts"), "say what to do next: {text}");

    // ── the account lane routes by `org`, and orgs do not mix ────
    let (err, elsewhere) = call_tool(
        &client,
        &account_url,
        "write_chart",
        json!({ "title": "Doxology", "source": "| G | C | D | G |", "org": "second" }),
    )
    .await;
    assert!(!err, "write_chart into `second` failed: {elsewhere}");
    assert_eq!(elsewhere["slug"], "doxology");

    let (_, in_second) = call_tool(
        &client,
        &account_url,
        "list_charts",
        json!({ "org": "second" }),
    )
    .await;
    let slugs: Vec<&str> = in_second["charts"]
        .as_array()
        .expect("charts")
        .iter()
        .filter_map(|c| c["slug"].as_str())
        .collect();
    assert_eq!(slugs, ["doxology"], "{in_second}");

    // Omitting `org` on the account lane is the caller's home org —
    // and the home org never saw the chart written to `second`.
    let (_, at_home) = call_tool(&client, &account_url, "list_charts", json!({})).await;
    let slugs: Vec<&str> = at_home["charts"]
        .as_array()
        .expect("charts")
        .iter()
        .filter_map(|c| c["slug"].as_str())
        .collect();
    assert_eq!(
        slugs,
        ["great-are-you-lord"],
        "a chart leaked across orgs: {at_home}"
    );

    // ── delete: true, then false, never an error ─────────────────
    let (err, gone) = call_tool(
        &client,
        &org_url,
        "delete_chart",
        json!({ "slug": "great-are-you-lord" }),
    )
    .await;
    assert!(!err, "delete_chart failed: {gone}");
    assert_eq!(gone["deleted"], true, "{gone}");

    let (err, again) = call_tool(
        &client,
        &org_url,
        "delete_chart",
        json!({ "slug": "great-are-you-lord" }),
    )
    .await;
    assert!(!err, "a second delete must not be an error: {again}");
    assert_eq!(again["deleted"], false, "{again}");

    let (_, after) = call_tool(&client, &org_url, "list_charts", json!({})).await;
    assert_eq!(after["count"], 0, "the chart is gone: {after}");
    let (err, _) = call_tool(
        &client,
        &org_url,
        "read_chart",
        json!({ "slug": "great-are-you-lord" }),
    )
    .await;
    assert!(err, "reading a deleted chart is an error");

    // Deleting in one org does not reach into another.
    let (_, in_second) = call_tool(
        &client,
        &account_url,
        "list_charts",
        json!({ "org": "second" }),
    )
    .await;
    assert_eq!(
        in_second["count"], 1,
        "`second` kept its chart: {in_second}"
    );
}
