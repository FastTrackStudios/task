#![allow(clippy::large_futures)]
//! `query_base` over MCP: an agent runs the seeded gigs base
//! (`examples/studio/acme-audio/Vault/Gigs/Gigs.base`) and gets rows with
//! formula values, link-follow (`note.opener.key` reads the song note) and
//! `file.hasLink()` filters, all scoped to the org it was admitted to.
//!
//! Driven as a real agent would be: a Library bot (notes and wikis only)
//! of a person who is a member of `acme-audio` alone, through the account
//! lane (`POST /mcp`) with nothing but its key.
//!
//! - the seeded `.base` runs by path, every view, with column + formula
//!   values;
//! - one view runs by name, and an inline YAML base runs without a file;
//! - a bad request (no base, an unknown view) is a tool error the model
//!   can read;
//! - the same call against `vnt-video`, an org the bot was never given,
//!   is refused before anything is read.

use serde_json::{Value, json};
use task_server::bots::{self, OrgChoice, Tools};

async fn rpc(http: &reqwest::Client, url: &str, key: &str, method: &str, params: Value) -> Value {
    http.post(url)
        .bearer_auth(key)
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .expect("POST /mcp")
        .json()
        .await
        .expect("a JSON-RPC body")
}

/// `(is_error, payload)` of one tool call that the protocol accepted.
async fn call(http: &reqwest::Client, url: &str, key: &str, args: Value) -> (bool, Value) {
    let res = rpc(
        http,
        url,
        key,
        "tools/call",
        json!({ "name": "query_base", "arguments": args }),
    )
    .await;
    let result = &res["result"];
    assert!(!result.is_null(), "query_base was refused: {res}");
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    (
        result["isError"].as_bool().unwrap_or(false),
        serde_json::from_str(text).unwrap_or_else(|_| json!(text)),
    )
}

/// The rows of one view in a `query_base` payload, by view name.
fn rows<'a>(payload: &'a Value, view: &str) -> Vec<&'a Value> {
    let v = payload["views"]
        .as_array()
        .expect("views")
        .iter()
        .find(|v| v["name"] == view)
        .unwrap_or_else(|| panic!("view `{view}` missing: {payload}"));
    v["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .flat_map(|g| g["rows"].as_array().expect("rows").iter())
        .collect()
}

// t[verify vault.bases.query]
#[tokio::test(flavor = "multi_thread")]
async fn query_base_runs_the_seeded_gigs_base_in_the_callers_org_only() {
    use task_server::example_org;

    const ORGS: [(&str, &str, bool); 2] = [
        ("acme-audio", "ACME Audio", true),
        ("vnt-video", "VNT Video", false),
    ];
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::support::env_lock().await;
    // SAFETY: under the binary's env lock, for the boot only.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        for var in ["TASK_MCP_TOKEN", "TASK_SERVER_ORG", "TASK_SERVER_VAULT_ROOT"] {
            std::env::remove_var(var);
        }
    }
    let data_root = org_proto::DataRoot::new(tmp.path().to_owned());
    data_root.ensure().unwrap();
    for (slug, name, home) in ORGS {
        let org = data_root.init_org(slug, name, home).unwrap();
        example_org::install(&org, slug).unwrap();
        if home {
            // Bots live in the home org's memberships store.
            task_server::memberships::Memberships::open(&org.memberships_db())
                .await
                .unwrap();
        }
    }
    let state = task_server::AppState::new(None).await.expect("boot");
    drop(env);
    let home = state.home_identity.clone().expect("acme-audio is home");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        let _ = axum::serve(listener, task_server::router(state)).await;
    });
    let url = format!("{base}/mcp");
    let http = reqwest::Client::new();

    // A person in ACME only, and their notes-only bot.
    let person = uuid::Uuid::new_v4();
    home.memberships
        .upsert(person, "acme-audio", Some("member"))
        .await
        .unwrap();
    let bot = bots::create(
        &home.auth,
        &home.memberships,
        person,
        "Gig Runner",
        &OrgChoice::Some(vec!["acme-audio".to_owned()]),
        Tools::Library,
    )
    .await
    .expect("the bot is made");
    let key = bot.key;

    let listed = rpc(&http, &url, &key, "tools/list", json!({})).await;
    assert!(
        listed["result"]["tools"]
            .as_array()
            .is_some_and(|t| t.iter().any(|t| t["name"] == "query_base")),
        "a Library bot is offered query_base: {listed}"
    );

    // ── the seeded base, every view ──
    let (err, all) = call(
        &http,
        &url,
        &key,
        json!({ "org": "acme-audio", "path": "Gigs/Gigs.base" }),
    )
    .await;
    assert!(!err, "query_base by path: {all}");
    assert_eq!(all["base"], "Gigs/Gigs.base");
    let upcoming = rows(&all, "Upcoming gigs");
    let names: Vec<&str> = upcoming.iter().filter_map(|r| r["name"].as_str()).collect();
    assert_eq!(
        names,
        ["2026-11-06 The Hall", "2026-11-14 Wedding"],
        "gig notes only, sorted by date: {all}"
    );
    let hall = upcoming[0];
    assert_eq!(hall["path"], "Gigs/2026-11-06 The Hall.md");
    // Link-follow: the opener's key and the venue's city come from the
    // notes the gig links to.
    assert_eq!(hall["values"]["formula.opener_key"], "G", "{hall}");
    assert_eq!(hall["values"]["formula.venue_city"], "Austin", "{hall}");
    assert_eq!(hall["values"]["fee"], 400, "{hall}");
    assert_eq!(hall["values"]["formula.band_cut"], 320.0, "{hall}");
    assert_eq!(hall["formulas"]["plays_paper_moon"], false, "{hall}");
    let wedding = upcoming[1];
    assert_eq!(wedding["formulas"]["opener_key"], "D", "{wedding}");
    // The venue note doesn't exist: `exists()` sends the formula to TBD.
    assert_eq!(wedding["formulas"]["venue_city"], "TBD", "{wedding}");
    assert_eq!(wedding["formulas"]["plays_paper_moon"], true, "{wedding}");
    // `file.hasLink()` as a view filter.
    let paper_moon = rows(&all, "Gigs playing Paper Moon");
    assert_eq!(paper_moon.len(), 1, "{all}");
    assert_eq!(paper_moon[0]["name"], "2026-11-14 Wedding");

    // ── one view by name ──
    let (err, one) = call(
        &http,
        &url,
        &key,
        json!({ "org": "acme-audio", "path": "Gigs/Gigs.base", "view": "gigs playing paper moon" }),
    )
    .await;
    assert!(!err, "{one}");
    assert_eq!(one["views"].as_array().map(Vec::len), Some(1), "{one}");

    // ── an inline base over the same vault ──
    let (err, inline) = call(
        &http,
        &url,
        &key,
        json!({
            "org": "acme-audio",
            "yaml": "filters:\n  and:\n    - file.inFolder(\"Gigs/Songs\")\nformulas:\n  fast: \"bpm > 100\"\nviews:\n  - type: table\n    name: Songs\n    order: [file.name, key, formula.fast]\n",
        }),
    )
    .await;
    assert!(!err, "inline yaml: {inline}");
    assert_eq!(inline["base"], "(inline)");
    let songs = rows(&inline, "Songs");
    let fast: Vec<(&str, bool)> = songs
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap(),
                r["values"]["formula.fast"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(fast, [("Night Drive", true), ("Paper Moon", false)], "{inline}");
    // The note read is reported: the whole (small) seeded vault, none
    // unreadable, not capped.
    let notes = &inline["notes"];
    assert!(notes["read"].as_u64().unwrap_or(0) >= 5, "{notes}");
    assert_eq!(notes["read"], notes["total"], "{notes}");
    assert_eq!(notes["unreadable"], json!([]), "{notes}");
    assert_eq!(notes["capped"], false, "{notes}");

    // ── a caller-written base that fans out answers ──
    // f0 = f1 + f1, …, f15 = 1: 2^16 evaluations per row without the
    // per-row formula memo, 16 with it.
    let mut fan = String::from("filters:\n  and:\n    - file.inFolder(\"Gigs/Songs\")\nformulas:\n");
    for i in 0..15 {
        fan.push_str(&format!("  f{i}: \"formula.f{n} + formula.f{n}\"\n", n = i + 1));
    }
    fan.push_str("  f15: \"1\"\nviews:\n  - type: table\n    name: Fan\n    order: [file.name, formula.f0]\n");
    // (The evaluation count is asserted in vault_live's
    // `fanned_out_formulas_run_once_per_row`; a wall-clock bound here
    // would flake on a loaded host.)
    let (err, fanned) = call(&http, &url, &key, json!({ "org": "acme-audio", "yaml": fan })).await;
    assert!(!err, "fan-out base: {fanned}");
    let fan_rows = rows(&fanned, "Fan");
    assert_eq!(fan_rows.len(), 2, "{fanned}");
    for r in &fan_rows {
        assert_eq!(r["values"]["formula.f0"], 32768.0, "{r}");
    }
    // An oversized inline base is refused before it is parsed.
    let (err, said) = call(
        &http,
        &url,
        &key,
        json!({ "org": "acme-audio", "yaml": "#".repeat(64 * 1024 + 1) }),
    )
    .await;
    assert!(err, "an oversized inline base: {said}");
    assert!(said.to_string().contains("at most"), "{said}");

    // ── bad requests are tool errors the model can act on ──
    let (err, said) = call(&http, &url, &key, json!({ "org": "acme-audio" })).await;
    assert!(err, "neither path nor yaml: {said}");
    assert!(said.to_string().contains("exactly one of"), "{said}");
    let (err, said) = call(
        &http,
        &url,
        &key,
        json!({ "org": "acme-audio", "path": "Gigs/Gigs.base", "view": "Nope" }),
    )
    .await;
    assert!(err, "an unknown view: {said}");
    assert!(said.to_string().contains("Upcoming gigs"), "names the views: {said}");
    let (err, said) = call(
        &http,
        &url,
        &key,
        json!({ "org": "acme-audio", "path": "Gigs/Missing.base" }),
    )
    .await;
    assert!(err, "a missing base: {said}");

    // ── an org the bot was never given is refused outright ──
    let refused = rpc(
        &http,
        &url,
        &key,
        "tools/call",
        json!({
            "name": "query_base",
            "arguments": { "org": "vnt-video", "yaml": "views:\n  - type: table\n    name: All\n" },
        }),
    )
    .await;
    assert!(
        refused["result"].is_null(),
        "query_base in an org the caller isn't in must not run: {refused}"
    );
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("vnt-video")),
        "the refusal names the org: {refused}"
    );
    // …on the org lane too.
    let refused = rpc(
        &http,
        &format!("{base}/org/vnt-video/mcp"),
        &key,
        "tools/call",
        json!({
            "name": "query_base",
            "arguments": { "yaml": "views:\n  - type: table\n    name: All\n" },
        }),
    )
    .await;
    assert!(
        refused["result"].is_null(),
        "the org lane refuses it as well: {refused}"
    );
}
