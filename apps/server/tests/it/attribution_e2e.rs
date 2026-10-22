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
//!
//! On behalf of (`attribution.on-behalf-of`): Atlas is the person's
//! **personal** assistant, so its records also say whom it acted for —
//! the person, from its key's owner, whatever the arguments claim — and
//! a **team** bot's say nobody. The second test hands Atlas's work to an
//! employee: assignment stays separate from creation, the feed logs who
//! assigned it, and the employee finds it on their own list.

use architect_auth::SignInEmailPassword;
use architect_auth::proto::AuthServiceClient;
use serde_json::{Value, json};
use task_server::bots::{self, Kind, OrgChoice, Tools};
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

/// The error text of one tool call that must fail as a tool error.
async fn refused(http: &reqwest::Client, url: &str, key: &str, name: &str, args: Value) -> String {
    let res = rpc(
        http,
        url,
        key,
        "tools/call",
        json!({ "name": name, "arguments": args }),
    )
    .await;
    let result = &res["result"];
    assert!(
        result["isError"].as_bool().unwrap_or(false),
        "{name} should have been refused: {res}"
    );
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
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
        Kind::Personal,
    )
    .await
    .expect("Atlas is made");
    let atlas_ref = format!("bot:{}", atlas.bot.id);
    // A team bot: made by the same person, acting for nobody.
    let desk = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Studio Desk",
        &OrgChoice::Some(vec![PERSONAL_SLUG.to_owned()]),
        Tools::All,
        Kind::Team,
    )
    .await
    .expect("the Studio Desk is made");

    // ── Atlas, with its key, claiming to be the person ──
    // …and claiming to act for someone else entirely.
    let spoof = json!({
        "created_by": me_ref, "created_by_name": "Cody",
        "on_behalf_of": "user:00000000-0000-0000-0000-00000000beef",
        "on_behalf_of_name": "Mallory",
    });
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
    // t[verify attribution.on-behalf-of]
    // t[verify attribution.on-behalf-of.server-stamped]
    assert_eq!(
        made["on_behalf_of"],
        json!(me_ref),
        "Atlas acted for the person who owns its key, not the one it named: {made}"
    );
    assert_eq!(made["on_behalf_of_name"], json!("Cody"));

    // ── the team bot, with the same spoof: on behalf of nobody ──
    // t[verify attribution.bot-kind]
    let mut args = json!({ "text": "Order the studio's coffee", "org": PERSONAL_SLUG });
    args.as_object_mut()
        .unwrap()
        .extend(spoof.as_object().unwrap().clone());
    let desks = call(&http, &url, &desk.key, "create_task", args).await;
    assert_eq!(desks["created_by"], json!(format!("bot:{}", desk.bot.id)));
    assert!(
        desks["on_behalf_of"].is_null() && desks["on_behalf_of_name"].is_null(),
        "a team bot acts for nobody, whatever it claims: {desks}"
    );

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
    assert!(
        mine["on_behalf_of"].is_null(),
        "a person acts for themselves: {mine}"
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
    draft.on_behalf_of = Some(me_ref.clone());
    draft.on_behalf_of_name = Some("Cody".into());
    let rpc_task = tasks.create(draft).await.expect("create");
    assert_eq!(rpc_task.created_by.as_deref(), Some(me_ref.as_str()));
    assert_eq!(
        rpc_task.on_behalf_of, None,
        "a payload's on_behalf_of is never taken"
    );

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
    assert_eq!(strings.on_behalf_of.as_deref(), Some(me_ref.as_str()));
    assert_eq!(strings.on_behalf_of_name.as_deref(), Some("Cody"));
    assert_eq!(by_body("Book the piano tuner").on_behalf_of, None);
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

    // ── "on behalf of me" ──
    // t[verify attribution.on-behalf-of.filter]
    let listed = call(
        &http,
        &url,
        &token,
        "list_tasks",
        json!({ "on_behalf_of": "me", "org": PERSONAL_SLUG }),
    )
    .await;
    let listed = rows(&listed, "tasks");
    assert_eq!(
        listed
            .iter()
            .map(|t| t["title"].clone())
            .collect::<Vec<_>>(),
        vec![json!("Restring the Telecaster")],
        "what my assistant made for me, and nothing else: {listed:?}"
    );
    let listed = call(
        &http,
        &url,
        &token,
        "list_inbox",
        json!({ "on_behalf_of": "me", "org": PERSONAL_SLUG }),
    )
    .await;
    let listed = rows(&listed, "items");
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["created_by"], json!(atlas_ref));
    let by_rpc = tasks
        .query(task::TaskListFilter {
            on_behalf_of: Some(me.to_string()),
            ..Default::default()
        })
        .await
        .expect("query");
    assert_eq!(by_rpc.len(), 1);
}

// t[verify attribution.assignee.separate]
// t[verify attribution.assignee.logged]
// t[verify attribution.assignee.filter]
#[tokio::test(flavor = "multi_thread")]
async fn an_employee_gets_the_work_an_assistant_made_and_the_handoff_is_logged() {
    use notify_proto::{ActivityFeedClient, ActivityFilter, ActivityKind};

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
    let home = node
        .state()
        .home_identity
        .clone()
        .expect("a node has a home org");

    // An employee: a second person, a plain member.
    let priya = home
        .auth
        .auth
        .create_email_password_user(architect_auth::CreateEmailPasswordUser {
            email: "priya@acme.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Priya".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .expect("Priya signs up");
    let priya_id = priya.user.id;
    node.admit_account(priya_id, &[PERSONAL_SLUG.to_owned()], "member")
        .await
        .unwrap();
    // The feed, once both people exist, so its directory names them.
    task_server::notifier::spawn(node.state());

    let atlas = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Atlas",
        &OrgChoice::Some(vec![PERSONAL_SLUG.to_owned()]),
        Tools::All,
        Kind::Personal,
    )
    .await
    .expect("Atlas is made");

    // Atlas makes the task for Cody.
    let made = call(
        &http,
        &url,
        &atlas.key,
        "create_task",
        json!({ "text": "Collect streaming metadata", "org": PERSONAL_SLUG }),
    )
    .await;
    let id = made["id"].as_str().expect("an id").to_owned();
    assert_eq!(made["on_behalf_of"], json!(format!("user:{me}")));

    // Priya has nothing yet.
    let theirs = call(
        &http,
        &url,
        &priya.token,
        "list_tasks",
        json!({ "assignee": "me", "org": PERSONAL_SLUG }),
    )
    .await;
    assert!(rows(&theirs, "tasks").is_empty(), "{theirs}");

    // Cody hands it to Priya: an explicit assignment, over MCP.
    let assigned = call(
        &http,
        &url,
        &token,
        "update_task",
        json!({ "id": id, "assignees": [format!("human:{priya_id}")], "org": PERSONAL_SLUG }),
    )
    .await;
    assert_eq!(assigned["assignees"], json!([priya_id.to_string()]));
    assert_eq!(
        assigned["created_by"],
        json!(format!("bot:{}", atlas.bot.id)),
        "assigning is not creating: the creator stays Atlas: {assigned}"
    );
    assert_eq!(assigned["on_behalf_of"], json!(format!("user:{me}")));

    // The employee sees their assigned work — over MCP and over the
    // RPC lane alike; and "on behalf of me" is still Cody's view.
    let theirs = call(
        &http,
        &url,
        &priya.token,
        "list_tasks",
        json!({ "assignee": "me", "org": PERSONAL_SLUG }),
    )
    .await;
    let theirs = rows(&theirs, "tasks");
    assert_eq!(
        theirs.iter().map(|t| t["id"].clone()).collect::<Vec<_>>(),
        vec![json!(id)],
        "{theirs:?}"
    );
    let lane = node
        .caller(PERSONAL_SLUG, Some(priya.token.clone()))
        .await
        .expect("Priya's lane");
    let by_rpc = task::TaskServiceClient::from_vox_lane(lane, None)
        .query(task::TaskListFilter {
            assignee: Some(format!("user:{priya_id}")),
            ..Default::default()
        })
        .await
        .expect("query");
    assert_eq!(by_rpc.len(), 1);
    let not_for_priya = call(
        &http,
        &url,
        &priya.token,
        "list_tasks",
        json!({ "on_behalf_of": "me", "org": PERSONAL_SLUG }),
    )
    .await;
    assert!(rows(&not_for_priya, "tasks").is_empty());

    // The feed logs the assignment, by whoever made it — Cody, not the
    // assignee the feed would otherwise have to guess.
    let lane = node
        .caller(PERSONAL_SLUG, Some(token.clone()))
        .await
        .expect("Cody's lane");
    let feed = ActivityFeedClient::from_vox_lane(lane, None);
    let task_id: uuid::Uuid = id.parse().unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let entry = loop {
        let rows = feed
            .list(ActivityFilter::entity("task", task_id))
            .await
            .expect("activity");
        if let Some(a) = rows.into_iter().find(|a| a.kind == ActivityKind::Assigned) {
            break a;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the assignment was never logged"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert_eq!(entry.actor_id, me.to_string(), "{entry:?}");
    assert_eq!(entry.actor_name, "Cody", "{entry:?}");
}

// t[verify attribution.assignee.member]
#[tokio::test(flavor = "multi_thread")]
async fn work_is_only_assigned_to_accounts_in_the_org() {
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
    let home = node
        .state()
        .home_identity
        .clone()
        .expect("a node has a home org");

    // A member: Priya, made a moment ago, so the account directory
    // has never seen her.
    let priya = home
        .auth
        .auth
        .create_email_password_user(architect_auth::CreateEmailPasswordUser {
            email: "priya@acme.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Priya".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .expect("Priya signs up");
    let priya_id = priya.user.id;
    node.admit_account(priya_id, &[PERSONAL_SLUG.to_owned()], "member")
        .await
        .unwrap();
    // A bot in the org, and one that was granted no org at all.
    let atlas = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Atlas",
        &OrgChoice::Some(vec![PERSONAL_SLUG.to_owned()]),
        Tools::All,
        Kind::Personal,
    )
    .await
    .expect("Atlas is made");
    let stray = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Stray",
        &OrgChoice::None,
        Tools::All,
        Kind::Team,
    )
    .await
    .expect("the stray bot is made");

    let made = call(
        &http,
        &url,
        &token,
        "create_task",
        json!({ "text": "Print the setlists", "org": PERSONAL_SLUG }),
    )
    .await;
    let id = made["id"].as_str().expect("an id").to_owned();

    // Nobody: a well-formed id no store knows, a bot outside the org,
    // and something that isn't an account id at all.
    let ghost = "00000000-0000-0000-0000-00000000beef";
    for (who, says) in [
        (format!("human:{ghost}"), "isn't in this org"),
        (format!("human:{}", stray.bot.id), "isn't in this org"),
        ("human:priya".to_owned(), "not an account id"),
    ] {
        let err = refused(
            &http,
            &url,
            &token,
            "update_task",
            json!({ "id": id, "assignees": [who], "org": PERSONAL_SLUG }),
        )
        .await;
        assert!(err.contains(says), "update_task to {who}: {err}");
        let err = refused(
            &http,
            &url,
            &token,
            "claim_task",
            json!({ "id": id, "agent": who, "org": PERSONAL_SLUG }),
        )
        .await;
        assert!(err.contains(says), "claim_task as {who}: {err}");
    }
    // …and nothing was stored: the task's owners are as it was made.
    let listed = call(
        &http,
        &url,
        &token,
        "list_tasks",
        json!({ "org": PERSONAL_SLUG }),
    )
    .await;
    let now = rows(&listed, "tasks")
        .into_iter()
        .find(|t| t["id"] == json!(id))
        .expect("the task is listed");
    assert_eq!(
        now["assignees"], made["assignees"],
        "a refused assignment stores nothing: {now}"
    );

    // Members: the person themselves claiming it…
    let claimed = call(
        &http,
        &url,
        &token,
        "claim_task",
        json!({ "id": id, "agent": format!("human:{me}"), "force": true, "org": PERSONAL_SLUG }),
    )
    .await;
    assert_ne!(claimed["claim"], json!("lost"), "{claimed}");
    // …then handing it to a person made a moment ago, a bot in the
    // org, and a named agent (not an account, so not looked up).
    let assigned = call(
        &http,
        &url,
        &token,
        "update_task",
        json!({
            "id": id,
            "assignees": [format!("human:{priya_id}"), format!("human:{}", atlas.bot.id), "agent:scribe"],
            "org": PERSONAL_SLUG,
        }),
    )
    .await;
    assert_eq!(
        assigned["assignees"],
        json!([priya_id.to_string(), atlas.bot.id.to_string(), "scribe"]),
        "{assigned}"
    );
}
