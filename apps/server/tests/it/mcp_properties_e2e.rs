//! End-to-end cover for the property tools on the MCP surface
//! (`POST /org/{slug}/mcp`): `update_properties` and
//! `read_property_schema`, plus the schema check `write_note` gained.
//!
//! One booted `AppState` over a fresh org, driven over real HTTP:
//!
//! - **body untouched**: setting and removing properties leaves every
//!   byte after the frontmatter exactly as it was;
//! - **stale revision**: an `expected_revision` that isn't the note's
//!   current one is refused and nothing is written;
//! - **schema violation**: a value the note's kind doesn't accept is
//!   refused with a message naming the key and the allowed values;
//! - **visible to a base**: what `update_properties` writes is what a
//!   `.base` query over the same vault sees;
//! - **org schemas**: a `kind: schema` page under `Schemas/` declares a
//!   kind that `read_property_schema` lists and writes are held to.

use serde_json::{Value, json};
use task_server::AppState;
use vault::VaultSync as _;

const TOKEN: &str = "mcp-properties-secret";

/// The note's body: blank lines, a `---` rule of its own, a block id
/// and no trailing newline — everything a re-serializer could disturb.
const BODY: &str = "\n# Ship the release\n\nSome notes.  \n\n---\n\n- [ ] tag it ^tag\nlast line";

async fn call_tool(client: &reqwest::Client, url: &str, name: &str, args: Value) -> (bool, Value) {
    let res: Value = client
        .post(url)
        .bearer_auth(TOKEN)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        }))
        .send()
        .await
        .expect("POST /mcp")
        .json()
        .await
        .expect("json-rpc body");
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

/// The note's text after its frontmatter block.
fn body_of(text: &str) -> &str {
    let rest = text.strip_prefix("---\n").expect("has frontmatter");
    let end = rest.find("\n---\n").expect("closing fence") + "\n---\n".len();
    &rest[end..]
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn property_tools_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let env_guard = crate::support::env_lock().await;
    // SAFETY: one test per process (nextest), and the env lock is held.
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

    let state = AppState::new(None).await.expect("boot AppState");
    drop(env_guard);
    let org = state.org("alpha").expect("org alpha");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = task_server::router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let url = format!("http://127.0.0.1:{port}/org/alpha/mcp");
    let client = reqwest::Client::new();

    // ── both tools are listed ────────────────────────────────────
    let listed: Value = client
        .post(&url)
        .bearer_auth(TOKEN)
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for tool in ["update_properties", "read_property_schema"] {
        assert!(names.contains(&tool), "{tool} missing from {names:?}");
    }

    // ── a task note, and a base that filters on its status ───────
    let path = "Projects/release.md";
    let original = format!("---\nkind: task\ntitle: Ship the release\nstatus: todo\n---\n{BODY}");
    let (err, out) = call_tool(
        &client,
        &url,
        "write_note",
        json!({ "path": path, "content": original }),
    )
    .await;
    assert!(!err, "write_note: {out}");
    let (err, out) = call_tool(
        &client,
        &url,
        "write_note",
        json!({
            "path": "Done.base",
            "content": "filters:\n  and:\n    - 'status == \"done\"'\nviews:\n  - name: Done\n    type: table\n    order: [title, status, due]\n",
        }),
    )
    .await;
    assert!(!err, "write_note base: {out}");
    let done_rows = || -> Vec<(String, Vec<String>)> {
        let views = org.vault_sync.base_views("default", "Done.base").unwrap();
        views[0]
            .groups
            .iter()
            .flat_map(|g| &g.rows)
            .map(|r| (r.path.clone(), r.cells.clone()))
            .collect()
    };
    assert!(done_rows().is_empty(), "nothing is done yet");

    // ── read_note hands out a revision ───────────────────────────
    let (err, read) = call_tool(&client, &url, "read_note", json!({ "path": path })).await;
    assert!(!err, "read_note: {read}");
    let rev0 = read["revision"].as_str().expect("revision").to_string();
    assert_eq!(rev0.len(), 64, "a sha256: {rev0}");

    // ── set: body byte-identical, values normalized ──────────────
    let (err, out) = call_tool(
        &client,
        &url,
        "update_properties",
        json!({
            "path": path,
            "set": { "status": "done", "due": "2026-11-01T09:00:00Z", "owner": "ada" },
            "expected_revision": rev0,
        }),
    )
    .await;
    assert!(!err, "update_properties set: {out}");
    assert_eq!(out["changed"], true, "{out}");
    assert_eq!(
        out["set"]["due"], "2026-11-01",
        "a date is truncated: {out}"
    );
    let rev1 = out["revision"].as_str().expect("revision").to_string();
    assert_ne!(rev1, rev0);
    let (_, read) = call_tool(&client, &url, "read_note", json!({ "path": path })).await;
    let text = read["content"].as_str().unwrap();
    assert_eq!(body_of(text), BODY, "set must not touch the body:\n{text}");
    assert!(text.contains("status: done"), "{text}");
    assert!(text.contains("owner: ada"), "{text}");
    assert_eq!(read["revision"], rev1.as_str());

    // ── the base sees it ─────────────────────────────────────────
    let rows = done_rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, path);
    assert_eq!(
        rows[0].1,
        vec![
            "Ship the release".to_string(),
            "done".into(),
            "2026-11-01".into()
        ]
    );

    // ── a stale revision is refused, nothing written ─────────────
    let (err, out) = call_tool(
        &client,
        &url,
        "update_properties",
        json!({ "path": path, "set": { "status": "todo" }, "expected_revision": rev0 }),
    )
    .await;
    assert!(err, "a stale revision must be refused: {out}");
    let msg = out.as_str().unwrap_or_default();
    assert!(msg.starts_with("Stale"), "{msg}");
    assert!(msg.contains("read_note"), "says how to recover: {msg}");
    let (_, read) = call_tool(&client, &url, "read_note", json!({ "path": path })).await;
    assert_eq!(read["revision"], rev1.as_str(), "nothing was written");

    // ── a schema violation is refused, nothing written ───────────
    let (err, out) = call_tool(
        &client,
        &url,
        "update_properties",
        json!({ "path": path, "set": { "status": "finished", "due": "soon" } }),
    )
    .await;
    assert!(err, "an unknown status must be refused: {out}");
    let msg = out.as_str().unwrap_or_default();
    assert!(msg.contains("`status`"), "{msg}");
    assert!(
        msg.contains("in_progress"),
        "names the allowed values: {msg}"
    );
    assert!(msg.contains("`due`"), "every violation, in one go: {msg}");
    let (_, read) = call_tool(&client, &url, "read_note", json!({ "path": path })).await;
    assert_eq!(read["revision"], rev1.as_str(), "nothing was written");

    // ── remove: body byte-identical, base drops the row ──────────
    let (err, out) = call_tool(
        &client,
        &url,
        "update_properties",
        json!({ "path": path, "remove": ["status", "owner", "absent"], "expected_revision": rev1 }),
    )
    .await;
    assert!(!err, "update_properties remove: {out}");
    let (_, read) = call_tool(&client, &url, "read_note", json!({ "path": path })).await;
    let text = read["content"].as_str().unwrap();
    assert_eq!(
        body_of(text),
        BODY,
        "remove must not touch the body:\n{text}"
    );
    assert!(!text.contains("status:"), "{text}");
    assert!(!text.contains("owner:"), "{text}");
    assert!(text.contains("due: 2026-11-01"), "{text}");
    assert!(done_rows().is_empty(), "the base no longer matches");

    // ── a no-op reports itself and keeps the revision ────────────
    let rev2 = read["revision"].as_str().unwrap().to_string();
    let (err, out) = call_tool(
        &client,
        &url,
        "update_properties",
        json!({ "path": path, "remove": ["absent"] }),
    )
    .await;
    assert!(!err, "{out}");
    assert_eq!(out["changed"], false, "{out}");
    assert_eq!(out["revision"], rev2.as_str());

    // ── write_note can't introduce an off-schema value ───────────
    let (err, out) = call_tool(
        &client,
        &url,
        "write_note",
        json!({ "path": "Notes/n.md", "content": "---\nvisibility: everyone\n---\nhi\n" }),
    )
    .await;
    assert!(err, "node's visibility is an enum: {out}");
    assert!(
        out.as_str().unwrap_or_default().contains("visibility"),
        "{out}"
    );

    // ── read_property_schema: built-ins ──────────────────────────
    let (err, schema) = call_tool(&client, &url, "read_property_schema", json!({})).await;
    assert!(!err, "{schema}");
    let kinds: Vec<&str> = schema["kinds"]
        .as_array()
        .expect("kinds")
        .iter()
        .filter_map(|k| k["kind"].as_str())
        .collect();
    for kind in ["node", "task", "project", "area", "person", "daily"] {
        assert!(kinds.contains(&kind), "{kind} missing from {kinds:?}");
    }
    let (err, task) = call_tool(
        &client,
        &url,
        "read_property_schema",
        json!({ "kind": "task" }),
    )
    .await;
    assert!(!err, "{task}");
    let keys: Vec<&str> = task["properties"]
        .as_array()
        .expect("properties")
        .iter()
        .filter_map(|p| p["key"].as_str())
        .collect();
    assert!(keys.contains(&"status"), "{keys:?}");
    assert!(
        keys.contains(&"visibility"),
        "inherited from node: {keys:?}"
    );
    let (err, out) = call_tool(
        &client,
        &url,
        "read_property_schema",
        json!({ "kind": "nope" }),
    )
    .await;
    assert!(err, "{out}");
    assert!(
        out.as_str().unwrap_or_default().contains("Known kinds"),
        "{out}"
    );

    // ── org schemas: a schema page declares a kind ───────────────
    let (err, out) = call_tool(
        &client,
        &url,
        "write_note",
        json!({
            "path": "Schemas/recipe.md",
            "content": "---\nkind: schema\nfor: recipe\nextends: node\nproperties:\n  - key: servings\n    ty: { type: number }\n  - key: course\n    ty:\n      type: enum_with_metadata\n      options: [{ value: main }, { value: side }]\n---\nRecipes.\n",
        }),
    )
    .await;
    assert!(!err, "{out}");
    call_tool(
        &client,
        &url,
        "write_note",
        json!({ "path": "Schemas/broken.md", "content": "---\nkind: schema\n---\n" }),
    )
    .await;
    let (err, recipe) = call_tool(
        &client,
        &url,
        "read_property_schema",
        json!({ "kind": "recipe" }),
    )
    .await;
    assert!(!err, "{recipe}");
    assert_eq!(recipe["source"], "Schemas/recipe.md");
    let errors = recipe["errors"].as_array().expect("errors");
    assert!(
        errors.iter().any(|e| e["path"] == "Schemas/broken.md"),
        "a broken schema page is reported, not fatal: {recipe}"
    );

    call_tool(
        &client,
        &url,
        "write_note",
        json!({ "path": "Recipes/oats.md", "content": "---\nkind: recipe\n---\nStir.\n" }),
    )
    .await;
    let (err, out) = call_tool(
        &client,
        &url,
        "update_properties",
        json!({ "path": "Recipes/oats.md", "set": { "course": "dessert" } }),
    )
    .await;
    assert!(err, "dessert isn't a declared course: {out}");
    assert!(out.as_str().unwrap_or_default().contains("recipe"), "{out}");
    let (err, out) = call_tool(
        &client,
        &url,
        "update_properties",
        json!({ "path": "Recipes/oats.md", "set": { "course": "main", "servings": "2" } }),
    )
    .await;
    assert!(!err, "{out}");
    assert_eq!(out["set"]["servings"], 2.0, "coerced to a number: {out}");
}
