#![allow(clippy::large_futures)]
//! The gig recipe (`docs/recipes/gigs.md`) driven over MCP, against the
//! seeded release show in the example studio (`acme-audio`,
//! `Vault/Gigs/example-album-release*`). There is no gig code: what's
//! tested is that the generic tools carry the recipe.
//!
//! - `query_base` on `Setlist.base` returns the set in `position` order
//!   (not file-name order), with effective keys: `+4` on G is B, an
//!   absolute `E` is E, no offset keeps the song's A;
//! - `query_base` on `Readiness.base` flags the one use whose song's
//!   playback session is missing, and its `Not ready` view lists only
//!   that one;
//! - `query_base` on `Calendar.base` and `list_events { linked_to }`
//!   both find the rehearsal and the show linked to the gig note;
//! - `update_properties` moves one song's key and drops another's
//!   offset, and the next `query_base` reflects both;
//! - writing the missing session note makes the set ready.

use serde_json::{Value, json};

use crate::support;

const TOKEN: &str = "mcp-gig-recipe-e2e-secret";
const GIG: &str = "Gigs/example-album-release";

/// One MCP tool call; the payload parsed, plus the isError flag.
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
    assert!(!result.is_null(), "tools/call {name}: protocol error {res}");
    let is_err = result["isError"].as_bool().unwrap_or(false);
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    let payload = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
    (is_err, payload)
}

async fn ok(client: &reqwest::Client, url: &str, name: &str, args: Value) -> Value {
    let (is_err, payload) = call_tool(client, url, name, args).await;
    assert!(!is_err, "{name} failed: {payload}");
    payload
}

/// The rows of one view of `<GIG>/<base>.base`, via `query_base`.
async fn view_rows(client: &reqwest::Client, url: &str, base: &str, view: &str) -> Vec<Value> {
    let out = ok(
        client,
        url,
        "query_base",
        json!({ "path": format!("{GIG}/{base}.base"), "view": view }),
    )
    .await;
    let views = out["views"].as_array().expect("views");
    assert_eq!(views.len(), 1, "one view asked for: {out}");
    views[0]["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .flat_map(|g| g["rows"].as_array().expect("rows").iter().cloned())
        .collect()
}

/// `(row name, formula value)` per row.
fn formula<'a>(rows: &'a [Value], name: &str) -> Vec<(&'a str, &'a Value)> {
    rows.iter()
        .map(|r| (r["name"].as_str().unwrap_or_default(), &r["formulas"][name]))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn the_seeded_gig_runs_as_bases_and_follows_property_edits() {
    let (ws, _tmp) = support::boot_ws_env(&[("TASK_MCP_TOKEN", TOKEN)])
        .await
        .expect("boot");
    let url = format!(
        "{}/org/{}/mcp",
        ws.replace("ws://", "http://").trim_end_matches("/vox"),
        support::ORG
    );
    let client = reqwest::Client::new();
    let (b, e, a, d) = (json!("B"), json!("E"), json!("A"), json!("D"));
    let (t, f) = (json!(true), json!(false));

    // ── the setlist, in position order, with effective keys ──────
    let set = view_rows(&client, &url, "Setlist", "Setlist").await;
    assert_eq!(
        formula(&set, "effective_key"),
        [
            ("Night Drive (release show)", &b),
            ("Paper Moon (release show)", &e),
            ("Last Call (release show)", &a),
        ],
        "position order and effective keys: {set:?}"
    );
    let positions: Vec<&Value> = set.iter().map(|r| &r["values"]["position"]).collect();
    assert_eq!(positions, [&json!(1), &json!(2), &json!(3)], "{set:?}");
    assert_eq!(set[0]["formulas"]["song_key"], "G", "link-follow: {set:?}");
    assert_eq!(set[0]["values"]["lead"], "Maya", "{set:?}");

    // ── readiness ────────────────────────────────────────────────
    let ready = view_rows(&client, &url, "Readiness", "Readiness").await;
    assert_eq!(
        formula(&ready, "ready"),
        [
            ("Night Drive (release show)", &t),
            ("Paper Moon (release show)", &t),
            ("Last Call (release show)", &f),
        ],
        "{ready:?}"
    );
    // What's missing is the session: the song links a note nobody wrote.
    assert_eq!(ready[2]["formulas"]["session"], false, "{ready:?}");
    assert_eq!(ready[2]["formulas"]["chart"], true, "{ready:?}");
    let not_ready = view_rows(&client, &url, "Readiness", "Not ready").await;
    let names: Vec<&str> = not_ready
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert_eq!(names, ["Last Call (release show)"], "{not_ready:?}");

    // ── the calendar, from both sides ────────────────────────────
    let cal = view_rows(&client, &url, "Calendar", "Calendar").await;
    let (rehearsal, show) = (json!("rehearsal"), json!("show"));
    assert_eq!(
        formula(&cal, "role"),
        [
            ("example-album-release-rehearsal", &rehearsal),
            ("example-album-release-show", &show),
        ],
        "{cal:?}"
    );
    let events = ok(&client, &url, "list_events", json!({ "linked_to": GIG })).await;
    let ids: Vec<&str> = events["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "example-album-release-rehearsal",
            "example-album-release-show"
        ],
        "{events}"
    );

    // ── update_properties moves a key; the base follows ─────────
    let night_drive = format!("{GIG}/Night Drive (release show).md");
    let read = ok(&client, &url, "read_note", json!({ "path": night_drive })).await;
    let rev = read["revision"].as_str().expect("revision").to_owned();
    let out = ok(
        &client,
        &url,
        "update_properties",
        json!({ "path": night_drive, "set": { "key_offset": "+2" }, "expected_revision": rev }),
    )
    .await;
    assert_eq!(out["changed"], true, "{out}");
    let out = ok(
        &client,
        &url,
        "update_properties",
        json!({ "path": format!("{GIG}/Paper Moon (release show).md"), "remove": ["key_offset"] }),
    )
    .await;
    assert_eq!(out["changed"], true, "{out}");
    let set = view_rows(&client, &url, "Setlist", "Setlist").await;
    assert_eq!(
        formula(&set, "effective_key"),
        [
            ("Night Drive (release show)", &a),
            ("Paper Moon (release show)", &d),
            ("Last Call (release show)", &a),
        ],
        "G+2 is A; no offset is Paper Moon's own D: {set:?}"
    );

    // ── writing the missing session makes the set ready ─────────
    ok(
        &client,
        &url,
        "write_note",
        json!({
            "path": "Records/sessions/last-call-playback.md",
            "content": "---\nkind: session\ntitle: Last Call — playback session\nroom: a-room\nengineer: Sam Reeve\ndate: 2026-11-22\nbooked_hours: 2\n---\n\n# Last Call — playback session\n",
        }),
    )
    .await;
    let ready = view_rows(&client, &url, "Readiness", "Readiness").await;
    assert!(
        ready.iter().all(|r| r["formulas"]["ready"] == true),
        "{ready:?}"
    );
    let not_ready = view_rows(&client, &url, "Readiness", "Not ready").await;
    assert!(not_ready.is_empty(), "{not_ready:?}");
}
