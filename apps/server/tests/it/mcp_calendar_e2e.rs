//! End-to-end cover for calendar events as part of the vault's graph,
//! over the MCP lane (`POST /org/{slug}/mcp`) and the vox lane, against
//! the example studio (`features/scheduling/spec/scheduling.md`):
//!
//! - `create_event` with `links` + `external_ref` stores both, and
//!   `list_events` returns them;
//! - creating again with the same `external_ref` returns the existing
//!   event instead of a second one, and `list_events { external_ref }`
//!   finds exactly it;
//! - `update_event` changes title / location / links and leaves the
//!   times alone;
//! - the note sees its events: `VaultGraph::backlinks` lists the event
//!   file, a `.base` over the vault filters on the link property, and
//!   `list_events { linked_to }` agrees;
//! - a tombstone hides the event from listings and keeps it on disk;
//! - rewriting a hand-made event file keeps its name, body and
//!   foreign properties.

use serde_json::{Value, json};
use vault_proto::VaultGraphClient;

use crate::support;

const TOKEN: &str = "mcp-calendar-e2e-secret";
const GIG: &str = "Gigs/caa-2026.md";

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

fn ids(listed: &Value) -> Vec<String> {
    listed["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|e| e["id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

// t[verify scheduling.event.links]
// t[verify scheduling.event.links.note-side]
// t[verify scheduling.event.external-ref]
// t[verify scheduling.event.external-ref.idempotent]
// t[verify scheduling.event.update]
// t[verify scheduling.event.tombstone]
// t[verify scheduling.event.rewrite-keeps]
#[tokio::test(flavor = "multi_thread")]
async fn events_link_notes_resync_by_ref_update_in_place_and_tombstone() {
    let (ws, tmp) = support::boot_ws_env(&[("TASK_MCP_TOKEN", TOKEN)])
        .await
        .expect("boot");
    let url = format!(
        "{}/org/{}/mcp",
        ws.replace("ws://", "http://").trim_end_matches("/vox"),
        support::ORG
    );
    let vault = support::org_root(&tmp).vault_dir();
    let client = reqwest::Client::new();

    ok(
        &client,
        &url,
        "write_note",
        json!({ "path": GIG, "content": "---\ntype: gig\ntitle: CAA 2026\n---\n\nThe gig.\n" }),
    )
    .await;

    // ── create with links + external_ref ──────────────────────────
    let made = ok(
        &client,
        &url,
        "create_event",
        json!({
            "title": "CAA 2026 — show",
            "start": "2026-11-06T19:00:00-06:00",
            "end": "2026-11-06T22:00:00-06:00",
            "location": "The Hall",
            "links": [{ "relation": "for", "target": "Gigs/caa-2026" }],
            "external_ref": "pco:plan_time:123",
        }),
    )
    .await;
    let id = made["id"].as_str().expect("id").to_owned();
    assert_eq!(
        made["links"],
        json!([{ "relation": "for", "target": "[[Gigs/caa-2026]]" }])
    );
    assert_eq!(
        made["external_ref"],
        json!({ "system": "pco", "id": "plan_time:123" })
    );
    let file = vault.join(format!("Records/events/{id}.md"));
    let raw = std::fs::read_to_string(&file).expect("event file");
    assert!(raw.contains("external_ref: pco:plan_time:123"), "{raw}");
    assert!(raw.contains("[[Gigs/caa-2026]]"), "{raw}");

    // A bad relation is refused, naming the rule.
    let (is_err, msg) = call_tool(
        &client,
        &url,
        "create_event",
        json!({
            "title": "x", "start": "2026-11-01T10:00:00Z", "end": "2026-11-01T11:00:00Z",
            "links": [{ "relation": "title", "target": "Gigs/caa-2026" }],
        }),
    )
    .await;
    assert!(is_err && msg.to_string().contains("lowercase slug"), "{msg}");

    // ── re-sync: same ref, no duplicate ───────────────────────────
    let again = ok(
        &client,
        &url,
        "create_event",
        json!({
            "title": "CAA 2026 — show (resent)",
            "start": "2026-11-06T19:00:00-06:00",
            "end": "2026-11-06T22:00:00-06:00",
            "external_ref": { "system": "pco", "id": "plan_time:123" },
        }),
    )
    .await;
    assert_eq!(again["id"], json!(id));
    assert_eq!(again["existing"], json!(true));
    assert_eq!(again["title"], json!("CAA 2026 — show"), "nothing rewritten");
    let by_ref = ok(
        &client,
        &url,
        "list_events",
        json!({ "external_ref": "pco:plan_time:123" }),
    )
    .await;
    assert_eq!(ids(&by_ref), vec![id.clone()]);
    let other_ref = ok(&client, &url, "list_events", json!({ "external_ref": "pco:plan_time:999" })).await;
    assert!(ids(&other_ref).is_empty());

    // ── update keeps the times ────────────────────────────────────
    let updated = ok(
        &client,
        &url,
        "update_event",
        json!({
            "id": id,
            "title": "CAA 2026 — the show",
            "location": "The Hall, main room",
            "links": [
                { "relation": "for", "target": "[[Gigs/caa-2026]]" },
                { "relation": "venue", "target": "Venues/The Hall" },
            ],
        }),
    )
    .await;
    assert_eq!(updated["title"], json!("CAA 2026 — the show"));
    assert_eq!(updated["location"], json!("The Hall, main room"));
    assert_eq!(updated["start"], json!("2026-11-06T19:00:00-06:00"));
    assert_eq!(updated["end"], json!("2026-11-06T22:00:00-06:00"));
    assert_eq!(updated["links"].as_array().map(Vec::len), Some(2));
    let listed = ok(&client, &url, "list_events", json!({ "from": "2026-11-06", "to": "2026-11-06" })).await;
    let row = listed["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == json!(id))
        .expect("listed after update")
        .clone();
    assert_eq!(row["start"], json!("2026-11-06T19:00:00-06:00"));
    assert_eq!(row["links"], updated["links"]);
    let (is_err, _) = call_tool(&client, &url, "update_event", json!({ "id": id })).await;
    assert!(is_err, "an update with nothing to change is refused");

    // ── the note's side ───────────────────────────────────────────
    let event_rel = format!("Records/events/{id}.md");
    let graph: VaultGraphClient = vox::connect_lane(&ws).establish().await.unwrap();
    let back = graph
        .backlinks("default".to_string(), GIG.to_string())
        .await
        .unwrap();
    assert!(back.contains(&event_rel), "the gig's backlinks: {back:?}");

    std::fs::write(
        vault.join("Gigs/caa-2026 events.base"),
        "filters:\n  and:\n    - 'type == \"scheduling-cal-event\"'\n    - 'note.for.contains(\"[[Gigs/caa-2026]]\")'\n\
         views:\n  - type: table\n    name: Events\n",
    )
    .unwrap();
    let snapshot = vault::obsidian::vault::Vault::open(&vault).expect("open vault");
    let views = vault::obsidian::base_query::query_all_views(&snapshot, "Gigs/caa-2026 events.base")
        .expect("query base");
    let names: Vec<&str> = views[0]
        .1
        .groups
        .iter()
        .flat_map(|(_, rows)| rows.iter().map(|r| r.basename.as_str()))
        .collect();
    assert_eq!(names, vec![id.as_str()], "the base finds the gig's events");

    let linked = ok(&client, &url, "list_events", json!({ "linked_to": "Gigs/caa-2026" })).await;
    assert_eq!(ids(&linked), vec![id.clone()]);

    // ── tombstone: hidden, kept, found by ref ─────────────────────
    let gone = ok(&client, &url, "update_event", json!({ "id": id, "tombstone": true })).await;
    assert!(gone["tombstone"].as_str().is_some(), "{gone}");
    let all = ok(&client, &url, "list_events", json!({})).await;
    assert!(!ids(&all).contains(&id), "a tombstoned event is not listed");
    let with = ok(&client, &url, "list_events", json!({ "include_tombstoned": true })).await;
    assert!(ids(&with).contains(&id));
    let by_ref = ok(&client, &url, "list_events", json!({ "external_ref": "pco:plan_time:123" })).await;
    assert_eq!(ids(&by_ref), vec![id.clone()], "a re-sync still finds it");
    assert!(file.is_file(), "a tombstone never deletes");
    let back = graph
        .backlinks("default".to_string(), GIG.to_string())
        .await
        .unwrap();
    assert!(back.contains(&event_rel), "links survive a tombstone");
    let revived = ok(&client, &url, "update_event", json!({ "id": id, "tombstone": false })).await;
    assert_eq!(revived["tombstone"], Value::Null);

    // ── a hand-made event file is rewritten in place ──────────────
    let hand = vault.join("Records/events/Sunday Rehearsal.md");
    std::fs::write(
        &hand,
        "---\ntitle: Sunday Rehearsal\ntype: event\nexperience: setlist\n\
         start: 2026-11-01T09:00:00Z\nend: 2026-11-01T10:00:00Z\n---\n\n# Set\n\n1. [[Rise and Shine]]\n",
    )
    .unwrap();
    ok(
        &client,
        &url,
        "update_event",
        json!({ "id": "Sunday Rehearsal", "links": [{ "relation": "for", "target": "Gigs/caa-2026" }] }),
    )
    .await;
    let raw = std::fs::read_to_string(&hand).expect("same file");
    assert!(raw.ends_with("\n# Set\n\n1. [[Rise and Shine]]\n"), "body kept: {raw}");
    assert!(raw.contains("experience: setlist"), "{raw}");
    assert!(raw.contains("type: event"), "{raw}");
    assert!(
        !vault.join("Records/events/Sunday-Rehearsal.md").exists(),
        "no twin file"
    );
    let linked = ok(&client, &url, "list_events", json!({ "linked_to": "caa-2026" })).await;
    assert_eq!(linked["count"], json!(2));
}
