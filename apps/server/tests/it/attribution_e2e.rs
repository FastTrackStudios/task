#![allow(clippy::large_futures)]
//! Who made it (`attribution.*`): every inbox item and task records the
//! authenticated principal behind its create — the person signed in, or
//! the bot whose key made the call — and nothing a caller sends changes
//! that.
//!
//! A node with its person signed in makes **Atlas**, a bot with every
//! tool. Then, through the real transports:
//!
//! - Atlas captures and makes a task over MCP with its key, each call
//!   claiming in its arguments to be the person; both records say Atlas;
//! - the person does the same over MCP with their session, and over the
//!   RPC lane with a payload naming Atlas; every record says the person;
//! - an edit naming someone else leaves the creator as it was;
//! - the `created_by` filters (MCP `list_inbox` / `list_tasks`, RPC
//!   `list_inbox_by` / `query`) keep exactly one creator's records.

use architect_auth::SignInEmailPassword;
use architect_auth::proto::AuthServiceClient;
use serde_json::{Value, json};
use task_server::bots::{self, OrgChoice, Tools};
use task_server::local_node::{LocalNode, NodeOptions, PERSONAL_SLUG};
use vox_core::FromVoxLane;

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

/// The payload of one tool call that must succeed.
async fn call(http: &reqwest::Client, url: &str, key: &str, name: &str, args: Value) -> Value {
    let res = rpc(
        http,
        url,
        key,
        "tools/call",
        json!({ "name": name, "arguments": args }),
    )
    .await;
    let result = &res["result"];
    assert!(!result.is_null(), "{name} answered {res}");
    assert!(
        !result["isError"].as_bool().unwrap_or(false),
        "{name} failed: {res}"
    );
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    serde_json::from_str(text).unwrap_or_else(|_| json!(text))
}

fn rows(v: &Value, key: &str) -> Vec<Value> {
    v.get(key)
        .and_then(Value::as_array)
        .or_else(|| v.as_array())
        .cloned()
        .unwrap_or_else(|| panic!("no `{key}` rows in {v}"))
}

fn creators(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r["created_by"].as_str().unwrap_or("none").to_owned())
        .collect()
}

// t[verify attribution.created-by]
// t[verify attribution.server-stamped]
// t[verify attribution.filter]
#[tokio::test(flavor = "multi_thread")]
async fn records_say_who_made_them_and_nobody_can_claim_otherwise() {
    let _env = crate::support::env_lock().await;
    let root = tempfile::tempdir().unwrap();
    let node = LocalNode::open(NodeOptions {
        owner_name: "Cody".into(),
        peers: false,
        ..NodeOptions::at(root.path())
    })
    .await
    .expect("the node opens");
    let base = node.serve_loopback().await.unwrap();
    let url = format!("{}/mcp", base.replacen("ws://", "http://", 1));
    let http = reqwest::Client::new();

    // The person, signed in and admitted to their personal org.
    let owner = node.owner().clone();
    let anon = node.caller(&owner.org, None).await.unwrap();
    let session = AuthServiceClient::from_vox_lane(anon, None)
        .sign_in_email_password(SignInEmailPassword {
            email: owner.email.clone(),
            password: owner.password.clone(),
            ip_address: None,
            user_agent: Some("attribution-e2e".into()),
        })
        .await
        .expect("the person signs in");
    let me = session.user.id;
    let token = session.token.clone();
    node.admit_account(me, &[PERSONAL_SLUG.to_owned()], "admin")
        .await
        .unwrap();
    let me_ref = format!("user:{me}");

    let home = node
        .state()
        .home_identity
        .clone()
        .expect("a node has a home org");
    let atlas = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Atlas",
        &OrgChoice::Some(vec![PERSONAL_SLUG.to_owned()]),
        Tools::All,
    )
    .await
    .expect("Atlas is made");
    let atlas_ref = format!("bot:{}", atlas.bot.id);

    // ── Atlas, with its key, claiming to be the person ──
    let spoof = json!({ "created_by": me_ref, "created_by_name": "Cody" });
    let mut args = json!({ "body": "Order more 10-46 strings", "org": PERSONAL_SLUG });
    args.as_object_mut()
        .unwrap()
        .extend(spoof.as_object().unwrap().clone());
    call(&http, &url, &atlas.key, "capture_inbox", args).await;
    let mut args = json!({ "text": "Restring the Telecaster", "org": PERSONAL_SLUG });
    args.as_object_mut()
        .unwrap()
        .extend(spoof.as_object().unwrap().clone());
    let made = call(&http, &url, &atlas.key, "create_task", args).await;
    assert_eq!(
        made["created_by"],
        json!(atlas_ref),
        "the task is Atlas's: {made}"
    );
    assert_eq!(made["created_by_name"], json!("Atlas"));

    // ── the person, over MCP with their session, claiming to be Atlas ──
    let spoof = json!({ "created_by": atlas_ref });
    let mut args = json!({ "body": "Book the piano tuner", "org": PERSONAL_SLUG });
    args.as_object_mut()
        .unwrap()
        .extend(spoof.as_object().unwrap().clone());
    call(&http, &url, &token, "capture_inbox", args).await;
    let mut args = json!({ "text": "Call the venue about load-in", "org": PERSONAL_SLUG });
    args.as_object_mut()
        .unwrap()
        .extend(spoof.as_object().unwrap().clone());
    let mine = call(&http, &url, &token, "create_task", args).await;
    assert_eq!(
        mine["created_by"],
        json!(me_ref),
        "the task is the person's: {mine}"
    );

    // ── the person, over the RPC lane, with payloads naming Atlas ──
    let lane = node
        .caller(PERSONAL_SLUG, Some(token.clone()))
        .await
        .expect("signed-in lane");
    let inbox = inbox_proto::InboxClient::from_vox_lane(lane.clone(), None);
    let tasks = task::TaskServiceClient::from_vox_lane(lane, None);
    let mut item = inbox_proto::InboxItem::capture(
        uuid::Uuid::new_v4().to_string(),
        "Pick up the hired mic stands",
        "ui",
        chrono::Utc::now().to_rfc3339(),
    );
    item.created_by = Some(atlas_ref.clone());
    item.created_by_name = Some("Atlas".into());
    inbox
        .upsert_inbox_item(item.clone())
        .await
        .expect("capture");
    let mut draft = task::capture("Send the stems to mastering");
    draft.created_by = Some(atlas_ref.clone());
    let rpc_task = tasks.create(draft).await.expect("create");
    assert_eq!(rpc_task.created_by.as_deref(), Some(me_ref.as_str()));

    // An edit naming someone else keeps the creator.
    let mut edit = item.clone();
    edit.body = "Pick up the hired mic stands (Friday)".into();
    inbox.upsert_inbox_item(edit).await.expect("edit");
    let mut retitle = rpc_task.clone();
    retitle.title = "Send the final stems to mastering".into();
    retitle.created_by = Some(atlas_ref.clone());
    let after = tasks.update(retitle).await.expect("update");
    assert_eq!(after.created_by.as_deref(), Some(me_ref.as_str()));

    // ── every capture, and who made it ──
    let all = inbox.list_inbox().await.expect("list");
    let by_body = |b: &str| {
        all.iter()
            .find(|i| i.body.starts_with(b))
            .unwrap_or_else(|| panic!("no capture `{b}`"))
            .clone()
    };
    let strings = by_body("Order more 10-46 strings");
    assert_eq!(strings.created_by.as_deref(), Some(atlas_ref.as_str()));
    assert_eq!(strings.created_by_name.as_deref(), Some("Atlas"));
    assert_eq!(strings.source, "mcp", "the surface, not the author");
    assert_eq!(
        by_body("Book the piano tuner").created_by.as_deref(),
        Some(me_ref.as_str())
    );
    let stands = by_body("Pick up the hired mic stands");
    assert_eq!(stands.created_by.as_deref(), Some(me_ref.as_str()));
    assert_eq!(stands.body, "Pick up the hired mic stands (Friday)");

    // ── the filters ──
    let listed = call(
        &http,
        &url,
        &token,
        "list_inbox",
        json!({ "created_by": atlas_ref, "org": PERSONAL_SLUG }),
    )
    .await;
    let listed = rows(&listed, "items");
    assert_eq!(creators(&listed), vec![atlas_ref.clone()], "{listed:?}");
    let listed = call(
        &http,
        &url,
        &token,
        "list_tasks",
        json!({ "created_by": atlas_ref, "org": PERSONAL_SLUG }),
    )
    .await;
    let listed = rows(&listed, "tasks");
    assert_eq!(creators(&listed), vec![atlas_ref.clone()], "{listed:?}");
    let listed = call(
        &http,
        &url,
        &token,
        "list_tasks",
        json!({ "created_by": me.to_string(), "org": PERSONAL_SLUG }),
    )
    .await;
    let listed = rows(&listed, "tasks");
    assert_eq!(
        listed.len(),
        2,
        "a bare uuid finds the person's: {listed:?}"
    );

    let by_rpc = inbox
        .list_inbox_by(me_ref.clone())
        .await
        .expect("list_inbox_by");
    assert_eq!(by_rpc.len(), 2);
    assert!(
        by_rpc
            .iter()
            .all(|i| i.created_by.as_deref() == Some(me_ref.as_str()))
    );
    let by_rpc = tasks
        .query(task::TaskListFilter {
            created_by: Some(atlas_ref.clone()),
            ..Default::default()
        })
        .await
        .expect("query");
    assert_eq!(
        by_rpc.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
        vec!["Restring the Telecaster"]
    );
}
