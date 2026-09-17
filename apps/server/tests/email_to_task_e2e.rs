//! Who owns the work a message creates.
//!
//! `email_to_task` turns a message into a task and assigns it to
//! whoever asked. That last clause is the whole reason the tool is
//! worth having in a shared org: an unassigned task shows up in
//! everyone's list, so mail-derived work would bury a team in items
//! only one person can act on.
//!
//! It is also the clause that quietly does not hold for an agent. The
//! MCP lane accepts two kinds of bearer, and only one of them is a
//! person:
//!
//! - a **session token** — resolves to `Principal::User { user_id }`,
//!   and the task is assigned;
//! - the **static `TASK_MCP_TOKEN`** — deliberately resolves to no
//!   principal at all (`mcp_principal` returns early on it), because a
//!   shared secret in a deployment's environment names a deployment
//!   rather than a human. The task is created and left unowned.
//!
//! Both are correct. Neither is discoverable from the tool's schema,
//! and the difference was found the hard way — an agent driving a
//! production org through the static token filed a run of mail-derived
//! tasks that all came back `assignees: null`, which reads like a bug
//! in assignment rather than the absence of anybody to assign to.
//!
//! So this pins both answers. The failure it exists to catch is a
//! change that makes the static token resolve to *some* principal:
//! every deployment-driven task would silently acquire an owner, and
//! whoever that turned out to be would start receiving other people's
//! work.
//!
//! Self-sandboxed: tempdir data root via `TASK_DATA_ROOT`, one test per
//! binary so the env setup races nothing.

use serde_json::{Value, json};
use task_server::AppState;

const STATIC_TOKEN: &str = "email-to-task-e2e-secret";
const ACCOUNT: &str = "alice";
const MESSAGE_ID: &str = "masters-due-friday@example-client.test";

/// One message, as a mail client would have written it to disk.
///
/// A maildir is a directory of files, which is what lets this test have
/// a mailbox at all — no IMAP, no container, no credentials.
const MESSAGE: &str = "\
Message-ID: <masters-due-friday@example-client.test>
From: Example Client <hello@example-client.test>
To: Alice Nguyen <alice@alice.test>
Subject: Masters due Friday for the label review
Date: Wed, 10 Sep 2026 16:02:00 +0000
MIME-Version: 1.0
Content-Type: text/plain; charset=utf-8

The label moved their review up. They need final masters by Friday.
";

async fn call_tool(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    name: &str,
    args: Value,
) -> (bool, Value) {
    let res: Value = client
        .post(url)
        .bearer_auth(token)
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
    (
        is_err,
        serde_json::from_str(text).unwrap_or_else(|_| json!(text)),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_becomes_a_task_owned_by_whoever_asked() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: one test per binary, so nothing races this env setup.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::set_var("TASK_MCP_TOKEN", STATIC_TOKEN);
        for var in ["TASK_SERVER_ORG", "TASK_SERVER_VAULT_ROOT"] {
            std::env::remove_var(var);
        }
    }
    let data_root = org_proto::DataRoot::from_env().unwrap();
    data_root.ensure().unwrap();
    let org_root = data_root.init_org("alpha", "Alpha", true).unwrap();
    std::fs::create_dir_all(org_root.vault_dir()).unwrap();

    // The mailbox, beside the vault rather than inside it — inside, the
    // next materialise of the vault root deletes it (see `AppState`'s
    // `mail_root`). Written before the server boots, because accounts
    // are discovered once, at construction.
    let account = org_root.path().join("mail").join(ACCOUNT);
    for sub in ["cur", "new", "tmp"] {
        std::fs::create_dir_all(account.join(sub)).unwrap();
    }
    std::fs::write(
        account.join("cur").join("1757500200.M1P1.host:2,S"),
        MESSAGE,
    )
    .unwrap();

    let state = AppState::new(None).await.expect("boot AppState");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = task_server::router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let url = format!("http://127.0.0.1:{port}/org/alpha/mcp");
    let client = reqwest::Client::new();

    // The mailbox is actually served — otherwise every assertion below
    // fails on `fetch_message` and says nothing about assignment.
    let (failed, accounts) = call_tool(
        &client,
        &url,
        STATIC_TOKEN,
        "list_email_accounts",
        json!({}),
    )
    .await;
    assert!(!failed, "list_email_accounts: {accounts}");
    assert_eq!(
        accounts["accounts"][0]["id"], ACCOUNT,
        "the seeded maildir is not being served: {accounts}"
    );

    // ── the deployment's own token: a task, and nobody to own it ─────
    let (failed, unowned) = call_tool(
        &client,
        &url,
        STATIC_TOKEN,
        "email_to_task",
        json!({ "account": ACCOUNT, "message_id": MESSAGE_ID }),
    )
    .await;
    assert!(!failed, "email_to_task over the static token: {unowned}");
    assert_eq!(
        unowned["title"], "Masters due Friday for the label review",
        "the subject is the sender's prose, not a line the user typed — quick-add \
         must not eat the words it recognises out of it, or the one thing a person \
         uses to recognise the message stops matching it"
    );
    assert!(
        unowned["due"].is_string(),
        "the parse still earns its keep: the sender wrote a day, so the task has \
         a due date: {unowned}"
    );
    assert!(
        unowned["assignees"].is_null(),
        "the static token names a deployment, not a person — it must not \
         resolve to a principal, or deployment-driven tasks acquire an \
         owner nobody chose: {unowned}"
    );

    // ── a person's session: the same call, and it lands on them ──────
    let org = state.org("alpha").expect("the org we just scaffolded");
    let bundle = org
        .auth
        .auth
        .create_email_password_user(architect_auth::CreateEmailPasswordUser {
            email: "alice@alice.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Alice".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .expect("sign Alice up");
    let user_id = bundle.user.id.to_string();

    let (failed, owned) = call_tool(
        &client,
        &url,
        &bundle.token,
        "email_to_task",
        json!({
            "account": ACCOUNT,
            "message_id": MESSAGE_ID,
            "title": "Deliver final masters",
        }),
    )
    .await;
    assert!(!failed, "email_to_task over a session token: {owned}");
    assert_eq!(
        owned["assignees"],
        json!([user_id]),
        "a task made from a person's mailbox is that person's task: {owned}"
    );

    // And the task carries the message it came from, both ways round —
    // the link is the whole point of routing mail through this tool
    // rather than through `create_task`.
    assert_eq!(owned["message_id"], MESSAGE_ID);
    let (failed, links) = call_tool(
        &client,
        &url,
        &bundle.token,
        "email_links",
        json!({ "message_id": MESSAGE_ID }),
    )
    .await;
    assert!(!failed, "email_links: {links}");
    let tasks: Vec<&Value> = links["links"]
        .as_array()
        .expect("links array")
        .iter()
        .filter(|l| l["kind"] == "task")
        .collect();
    assert_eq!(
        tasks.len(),
        2,
        "one link per task made from this message: {links}"
    );
}
