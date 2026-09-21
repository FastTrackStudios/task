//! End-to-end cover for the wiki tools on the MCP lane
//! (`POST /org/{slug}/mcp`): the loop an agent runs to scaffold and
//! edit a wiki through Claude — the same backends the vox `wiki/*`
//! services serve, reached as MCP tools.
//!
//! - `create_wiki` → `list_wikis` shows it, private by default;
//! - `write_wiki_page` → `read_wiki_page` round-trips markdown + sha;
//!   a write with the fresh sha lands, one with a stale sha is refused
//!   and says so;
//! - `write_wiki_purpose` → `read_wiki_purpose`, and the schema pair;
//! - `search_wiki` finds the page by a word in its body;
//! - `list_wiki_subscriptions` answers for the vault and a wiki, and
//!   `subscribe_wiki` refuses a malformed id before any backend work.
//!
//! The wiki plugin is core (never deny-listed), so unlike `mcp_e2e.rs`
//! there is no disabled-plugin leg here.
//!
//! Self-sandboxed the same way `mcp_e2e.rs` is: tempdir data root via
//! `TASK_DATA_ROOT`, one test per binary so the env setup races
//! nothing.

use serde_json::{Value, json};
use task_server::AppState;

const TOKEN: &str = "mcp-wiki-e2e-secret";

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

async fn tool_names(client: &reqwest::Client, url: &str) -> Vec<String> {
    let listed = rpc(client, url, Some(TOKEN), "tools/list", json!({})).await;
    listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect()
}

const WIKI_TOOLS: &[&str] = &[
    "list_wikis",
    "describe_wiki",
    "create_wiki",
    "wiki_local_changes",
    "wiki_push_changes",
    "list_wiki_pages",
    "read_wiki_page",
    "write_wiki_page",
    "read_wiki_schema",
    "write_wiki_schema",
    "read_wiki_purpose",
    "write_wiki_purpose",
    "search_wiki",
    "list_wiki_subscriptions",
    "subscribe_wiki",
];

#[tokio::test(flavor = "multi_thread")]
async fn mcp_wiki_tools_end_to_end() {
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
    let org_root = data_root.init_org("alpha", "Alpha", true).unwrap();
    std::fs::create_dir_all(org_root.vault_dir()).unwrap();

    let state = AppState::new(None).await.expect("boot AppState");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = task_server::router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let url = format!("http://127.0.0.1:{port}/org/alpha/mcp");
    let client = reqwest::Client::new();

    // ── orientation: the instructions teach the wiki loop ────────
    let init = rpc(&client, &url, None, "initialize", json!({})).await;
    let instructions = init["result"]["instructions"]
        .as_str()
        .expect("instructions");
    assert!(instructions.contains("list_wikis"), "{instructions}");
    assert!(instructions.contains("schema.md"), "{instructions}");

    // ── tools/list carries every wiki tool ───────────────────────
    let names = tool_names(&client, &url).await;
    for tool in WIKI_TOOLS {
        assert!(
            names.contains(&(*tool).to_owned()),
            "`{tool}` missing: {names:?}"
        );
    }

    // ── create → list ────────────────────────────────────────────
    let (err, created) = call_tool(
        &client,
        &url,
        "create_wiki",
        json!({
            "title": "Music Theory",
            "purpose": "Everything about harmony, modes and voice leading, for the band.",
        }),
    )
    .await;
    assert!(!err, "create_wiki failed: {created}");
    assert_eq!(created["slug"], "music-theory", "{created}");
    assert_eq!(
        created["visibility"], "private",
        "private by default: {created}"
    );
    assert_eq!(created["created"], true);

    let (err, listed) = call_tool(&client, &url, "list_wikis", json!({})).await;
    assert!(!err, "list_wikis failed: {listed}");
    let wikis = listed["wikis"].as_array().expect("wikis");
    let mine = wikis
        .iter()
        .find(|w| w["slug"] == "music-theory")
        .unwrap_or_else(|| panic!("created wiki not listed: {listed}"));
    assert_eq!(mine["title"], "Music Theory");
    assert!(
        mine["purpose"]
            .as_str()
            .is_some_and(|p| p.contains("harmony")),
        "purpose is summarised from purpose.md: {mine}"
    );

    // A second create with the same slug is refused, not duplicated.
    let (err, dup) = call_tool(
        &client,
        &url,
        "create_wiki",
        json!({ "title": "Music Theory", "purpose": "again" }),
    )
    .await;
    assert!(err, "duplicate slug must be refused: {dup}");

    let (err, described) = call_tool(
        &client,
        &url,
        "describe_wiki",
        json!({ "wiki": "music-theory" }),
    )
    .await;
    assert!(!err, "describe_wiki failed: {described}");
    assert_eq!(described["config"]["visibility"], "private");
    // The static token is the server acting as itself: no person to
    // name as Editor, so the Edit lane stays off and writes are direct.
    assert_eq!(described["config"]["has_edit_lane"], false, "{described}");
    assert_eq!(described["config"]["you_may_write_directly"], true);

    // ── schema and purpose round-trip ────────────────────────────
    let (err, schema) = call_tool(
        &client,
        &url,
        "read_wiki_schema",
        json!({ "wiki": "music-theory" }),
    )
    .await;
    assert!(!err, "read_wiki_schema failed: {schema}");
    assert!(
        schema["markdown"]
            .as_str()
            .is_some_and(|m| m.contains("frontmatter")),
        "a fresh wiki carries the default schema: {schema}"
    );
    let new_schema = "# Music Theory schema\n\nEvery page: `title:`, `type: concept|piece`.\n";
    let (err, wrote) = call_tool(
        &client,
        &url,
        "write_wiki_schema",
        json!({ "wiki": "music-theory", "markdown": new_schema }),
    )
    .await;
    assert!(!err, "write_wiki_schema failed: {wrote}");
    let (_, schema) = call_tool(
        &client,
        &url,
        "read_wiki_schema",
        json!({ "wiki": "music-theory" }),
    )
    .await;
    assert_eq!(schema["markdown"], new_schema);

    let new_purpose = "---\ntitle: \"Music Theory\"\n---\n\n# Music Theory\n\n\
                       Harmony and modes for the band. Out of scope: gear.\n";
    let (err, wrote) = call_tool(
        &client,
        &url,
        "write_wiki_purpose",
        json!({ "wiki": "music-theory", "markdown": new_purpose }),
    )
    .await;
    assert!(!err, "write_wiki_purpose failed: {wrote}");
    let (err, purpose) = call_tool(
        &client,
        &url,
        "read_wiki_purpose",
        json!({ "wiki": "music-theory" }),
    )
    .await;
    assert!(!err, "read_wiki_purpose failed: {purpose}");
    assert_eq!(purpose["markdown"], new_purpose);

    // ── pages: write → read → edit with sha → stale sha refused ──
    let page = "---\ntitle: Dorian mode\ntype: concept\n---\n\n\
                # Dorian mode\n\nThe second mode of the major scale; a minor sound \
                with a raised sixth. See [[Modes]].\n";
    let (err, wrote) = call_tool(
        &client,
        &url,
        "write_wiki_page",
        json!({ "wiki": "music-theory", "path": "concepts/dorian.md", "markdown": page }),
    )
    .await;
    assert!(!err, "write_wiki_page failed: {wrote}");
    assert_eq!(wrote["replaced_existing"], false);
    let sha1 = wrote["sha256"].as_str().expect("sha").to_owned();

    let (err, read) = call_tool(
        &client,
        &url,
        "read_wiki_page",
        json!({ "wiki": "music-theory", "path": "concepts/dorian.md" }),
    )
    .await;
    assert!(!err, "read_wiki_page failed: {read}");
    assert_eq!(read["markdown"], page);
    assert_eq!(
        read["sha256"],
        sha1.as_str(),
        "the write reports the sha a read sees"
    );

    let (err, pages) = call_tool(
        &client,
        &url,
        "list_wiki_pages",
        json!({ "wiki": "music-theory" }),
    )
    .await;
    assert!(!err, "list_wiki_pages failed: {pages}");
    let dorian = pages["pages"]
        .as_array()
        .expect("pages")
        .iter()
        .find(|p| p["path"] == "concepts/dorian.md")
        .unwrap_or_else(|| panic!("page not listed: {pages}"));
    assert_eq!(dorian["title"], "Dorian mode");
    assert_eq!(dorian["type"], "concept");

    let edited = format!("{page}\nUsed in \"So What\".\n");
    let (err, wrote) = call_tool(
        &client,
        &url,
        "write_wiki_page",
        json!({
            "wiki": "music-theory",
            "path": "concepts/dorian.md",
            "markdown": edited,
            "base_sha256": sha1,
        }),
    )
    .await;
    assert!(!err, "sha-guarded edit failed: {wrote}");
    assert_eq!(wrote["replaced_existing"], true);
    let sha2 = wrote["sha256"].as_str().expect("sha").to_owned();
    assert_ne!(sha1, sha2);

    // The first sha is stale now: the write is refused and the
    // message tells the model what to do.
    let (err, refused) = call_tool(
        &client,
        &url,
        "write_wiki_page",
        json!({
            "wiki": "music-theory",
            "path": "concepts/dorian.md",
            "markdown": "clobber",
            "base_sha256": sha1,
        }),
    )
    .await;
    assert!(err, "a stale sha must be refused: {refused}");
    let msg = refused.as_str().expect("error text");
    assert!(msg.contains("conflict"), "{msg}");
    assert!(msg.contains("read_wiki_page"), "{msg}");
    let (_, read) = call_tool(
        &client,
        &url,
        "read_wiki_page",
        json!({ "wiki": "music-theory", "path": "concepts/dorian.md" }),
    )
    .await;
    assert_eq!(
        read["markdown"], edited,
        "the refused write changed nothing"
    );

    // Unknown wiki / page: actionable, not a bare Debug dump.
    let (err, missing) = call_tool(
        &client,
        &url,
        "read_wiki_page",
        json!({ "wiki": "music-theory", "path": "concepts/nope.md" }),
    )
    .await;
    assert!(err, "{missing}");
    assert!(
        missing
            .as_str()
            .is_some_and(|m| m.contains("list_wiki_pages")),
        "{missing}"
    );
    let (err, missing) =
        call_tool(&client, &url, "list_wiki_pages", json!({ "wiki": "nope" })).await;
    assert!(err, "{missing}");

    // ── search finds the page ────────────────────────────────────
    let (err, hits) = call_tool(
        &client,
        &url,
        "search_wiki",
        json!({ "wiki": "music-theory", "query": "raised sixth" }),
    )
    .await;
    assert!(!err, "search_wiki failed: {hits}");
    assert!(
        hits["hits"]
            .as_array()
            .expect("hits")
            .iter()
            .any(|h| h["path"] == "concepts/dorian.md"),
        "search must find the page: {hits}"
    );

    // ── subscriptions answer for the vault and for a wiki ────────
    let (err, subs) = call_tool(&client, &url, "list_wiki_subscriptions", json!({})).await;
    assert!(!err, "list_wiki_subscriptions failed: {subs}");
    assert_eq!(subs["subscriber"], "vault");
    assert!(subs["subscriptions"].is_array());
    let (err, subs) = call_tool(
        &client,
        &url,
        "list_wiki_subscriptions",
        json!({ "wiki": "music-theory" }),
    )
    .await;
    assert!(!err, "list_wiki_subscriptions(wiki) failed: {subs}");
    assert_eq!(subs["subscriber"], "wiki:music-theory");
    // A malformed id is refused before any backend work.
    let (err, bad) = call_tool(
        &client,
        &url,
        "subscribe_wiki",
        json!({ "qualified_id": "no-domain" }),
    )
    .await;
    assert!(err, "{bad}");
    assert!(
        bad.as_str().is_some_and(|m| m.contains("domain/slug")),
        "{bad}"
    );
}
