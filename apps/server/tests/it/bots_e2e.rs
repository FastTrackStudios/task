#![allow(clippy::large_futures)]
//! Bots over MCP (`mcp.bots`): an agent with an account of its own, in
//! the orgs its person gave it, holding a key.
//!
//! A node (what a personal server is) with its person signed in makes
//! two bots: **Atlas**, every tool in the personal org, and the
//! **Librarian**, notes and wikis only. Each is driven through the
//! account lane (`POST /mcp`) with nothing but its key, as a Hermes bot
//! would be:
//!
//! - Atlas lists and makes tasks, and the task is attributed to Atlas;
//! - the Librarian is offered no task tools, and is refused one by name;
//! - a bot cannot be given an org its person is not in;
//! - a revoked bot's key reaches nothing.

use architect_auth::SignInEmailPassword;
use architect_auth::proto::AuthServiceClient;
use serde_json::{Value, json};
use task_server::bots::{self, Tools};
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

async fn tool_names(http: &reqwest::Client, url: &str, key: &str) -> Vec<String> {
    let res = rpc(http, url, key, "tools/list", json!({})).await;
    res["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list answered {res}"))
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect()
}

/// `(is_error, payload)` of one tool call.
async fn call(
    http: &reqwest::Client,
    url: &str,
    key: &str,
    name: &str,
    args: Value,
) -> (bool, Value) {
    let res = rpc(
        http,
        url,
        key,
        "tools/call",
        json!({ "name": name, "arguments": args }),
    )
    .await;
    let result = &res["result"];
    if result.is_null() {
        return (true, res["error"].clone());
    }
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    (
        result["isError"].as_bool().unwrap_or(false),
        serde_json::from_str(text).unwrap_or_else(|_| json!(text)),
    )
}

// t[verify mcp.bots]
#[tokio::test(flavor = "multi_thread")]
async fn bots_reach_their_orgs_and_tools_with_their_key() {
    let _env = crate::support::env_lock().await;
    let root = tempfile::tempdir().unwrap();
    let node = LocalNode::open(NodeOptions {
        owner_name: "Sam Server".into(),
        peers: false,
        ..NodeOptions::at(root.path())
    })
    .await
    .expect("the node opens");
    let base = node.serve_loopback().await.unwrap();
    let url = format!("{}/mcp", base.replacen("ws://", "http://", 1));
    let http = reqwest::Client::new();

    // The person, signed in, and a member of their personal org.
    let owner = node.owner().clone();
    let anon = node.caller(&owner.org, None).await.unwrap();
    let me = AuthServiceClient::from_vox_lane(anon, None)
        .sign_in_email_password(SignInEmailPassword {
            email: owner.email.clone(),
            password: owner.password.clone(),
            ip_address: None,
            user_agent: Some("bots-e2e".into()),
        })
        .await
        .expect("the person signs in")
        .user
        .id;
    node.admit_account(me, &[PERSONAL_SLUG.to_owned()], "admin")
        .await
        .unwrap();

    let home = node
        .state()
        .home_identity
        .clone()
        .expect("a node has a home org");
    let personal = vec![PERSONAL_SLUG.to_owned()];
    let atlas = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Atlas",
        &personal,
        Tools::All,
    )
    .await
    .expect("Atlas is made");
    let librarian = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Librarian",
        &personal,
        Tools::Library,
    )
    .await
    .expect("the Librarian is made");
    assert!(atlas.key.starts_with("ak_"));

    // ── Atlas: every tool, and its writes are its own ──
    let tools = tool_names(&http, &url, &atlas.key).await;
    assert!(
        tools.iter().any(|t| t == "create_task"),
        "Atlas has the task tools: {tools:?}"
    );
    let (err, made) = call(
        &http,
        &url,
        &atlas.key,
        "create_task",
        json!({ "text": "Restring the Telecaster", "org": PERSONAL_SLUG }),
    )
    .await;
    assert!(!err, "Atlas makes a task: {made}");
    let (err, listed) = call(
        &http,
        &url,
        &atlas.key,
        "list_tasks",
        json!({ "org": PERSONAL_SLUG }),
    )
    .await;
    assert!(!err, "Atlas lists tasks: {listed}");
    assert!(listed.to_string().contains("Restring the Telecaster"));

    // ── the Librarian: notes and wikis, nothing else ──
    let tools = tool_names(&http, &url, &librarian.key).await;
    assert!(
        !tools.iter().any(|t| t == "create_task"),
        "no task tools: {tools:?}"
    );
    assert!(
        tools.iter().any(|t| t == "read_note"),
        "the notes: {tools:?}"
    );
    assert!(tools.iter().any(|t| t == "search_vault"));
    let (err, said) = call(
        &http,
        &url,
        &librarian.key,
        "create_task",
        json!({ "text": "x" }),
    )
    .await;
    assert!(err, "the Librarian is refused a task tool: {said}");
    let (err, wrote) = call(
        &http,
        &url,
        &librarian.key,
        "write_note",
        json!({ "path": "Library/Shelf.md", "content": "# Shelf\n", "org": PERSONAL_SLUG }),
    )
    .await;
    assert!(!err, "the Librarian writes a note: {wrote}");

    // ── a person can give only what they have ──
    let refused = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Snoop",
        &["someone-elses".to_owned()],
        Tools::All,
    )
    .await;
    assert!(refused.is_err(), "no bot gets an org its person is not in");

    // ── listing and revoking ──
    let mine = bots::list(&home.auth, &home.memberships, me).await.unwrap();
    assert_eq!(
        mine.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        ["Atlas", "Librarian"]
    );
    bots::revoke(&home.auth, &home.memberships, me, atlas.bot.id)
        .await
        .unwrap();
    let res = rpc(&http, &url, &atlas.key, "tools/list", json!({})).await;
    assert!(
        res["result"]["tools"].as_array().is_none_or(Vec::is_empty),
        "a revoked key reaches nothing: {res}"
    );
}

/// Settings → Bots: the same lifecycle through `BotService` on the
/// server lane (`/server/vox`), as the person signed in — and refused
/// to anyone who is not.
// t[verify mcp.bots]
#[tokio::test(flavor = "multi_thread")]
async fn the_app_manages_bots_through_the_server_lane() {
    use org_proto::{BotServiceClient, BotTools, CreateBotRequest, RevokeBotRequest};

    let _env = crate::support::env_lock().await;
    let root = tempfile::tempdir().unwrap();
    let node = LocalNode::open(NodeOptions {
        owner_name: "Sam Server".into(),
        peers: false,
        ..NodeOptions::at(root.path())
    })
    .await
    .unwrap();
    let owner = node.owner().clone();
    let anon = node.caller(&owner.org, None).await.unwrap();
    let session = AuthServiceClient::from_vox_lane(anon, None)
        .sign_in_email_password(SignInEmailPassword {
            email: owner.email.clone(),
            password: owner.password.clone(),
            ip_address: None,
            user_agent: Some("bots-e2e".into()),
        })
        .await
        .unwrap();
    node.admit_account(session.user.id, &[PERSONAL_SLUG.to_owned()], "admin")
        .await
        .unwrap();
    let bots = BotServiceClient::from_vox_lane(node.server_caller().await.unwrap(), None);

    assert!(
        bots.list_bots("not-a-token".into()).await.is_err(),
        "nobody's token manages nobody's bots"
    );
    let made = bots
        .create_bot(CreateBotRequest {
            session_token: session.token.clone(),
            name: "Atlas".into(),
            orgs: vec![PERSONAL_SLUG.to_owned()],
            tools: BotTools::All,
        })
        .await
        .expect("the person makes Atlas from the app");
    assert!(made.key.starts_with("ak_"));
    let listed = bots.list_bots(session.token.clone()).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "Atlas");
    assert_eq!(listed[0].orgs, [PERSONAL_SLUG]);
    assert!(
        bots.create_bot(CreateBotRequest {
            session_token: session.token.clone(),
            name: "Snoop".into(),
            orgs: vec!["someone-elses".into()],
            tools: BotTools::Library,
        })
        .await
        .is_err(),
        "an org the person is not in is refused"
    );
    bots.revoke_bot(RevokeBotRequest {
        session_token: session.token.clone(),
        bot_id: made.bot.id.clone(),
    })
    .await
    .unwrap();
    assert!(bots.list_bots(session.token).await.unwrap().is_empty());
}
