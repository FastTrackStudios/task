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
    let personal = OrgChoice::Some(vec![PERSONAL_SLUG.to_owned()]);
    let atlas = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Atlas",
        &personal,
        Tools::All,
        Kind::Team,
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
        Kind::Team,
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
        &OrgChoice::Some(vec!["someone-elses".to_owned()]),
        Tools::All,
        Kind::Team,
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
            no_orgs: false,
            tools: BotTools::All,
            personal: false,
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
            no_orgs: false,
            tools: BotTools::Library,
            personal: false,
        })
        .await
        .is_err(),
        "an org the person is not in is refused"
    );
    // No orgs, said on purpose, is a bot the person still sees; an
    // empty list by accident, or no orgs with orgs, is refused.
    let quiet = bots
        .create_bot(CreateBotRequest {
            session_token: session.token.clone(),
            name: "Quiet".into(),
            orgs: Vec::new(),
            no_orgs: true,
            tools: BotTools::All,
            personal: false,
        })
        .await
        .expect("a bot with no orgs, on purpose");
    assert!(quiet.bot.orgs.is_empty());
    for (orgs, no_orgs) in [(Vec::new(), false), (vec![PERSONAL_SLUG.to_owned()], true)] {
        assert!(
            bots.create_bot(CreateBotRequest {
                session_token: session.token.clone(),
                name: "Muddled".into(),
                orgs,
                no_orgs,
                tools: BotTools::All,
                personal: false,
            })
            .await
            .is_err(),
            "no orgs is said once, and on its own"
        );
    }
    let listed = bots.list_bots(session.token.clone()).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|b| (b.name.as_str(), b.orgs.len()))
            .collect::<Vec<_>>(),
        [("Atlas", 1), ("Quiet", 0)],
        "the zero-org bot is listed, with no orgs"
    );
    for id in [made.bot.id.clone(), quiet.bot.id.clone()] {
        bots.revoke_bot(RevokeBotRequest {
            session_token: session.token.clone(),
            bot_id: id,
        })
        .await
        .unwrap();
    }
    assert!(bots.list_bots(session.token).await.unwrap().is_empty());
}

/// The secret both sides of [`a_bot_made_with_no_orgs_reaches_nothing_until_granted`]
/// sign with: the server it boots in this process and every
/// `task-server admin` it runs.
///
/// Named on purpose, because otherwise the two disagree whenever another
/// test ran first in the same process. A [`LocalNode`] installs its own
/// random secret process-wide (`task_server::install_auth_secret`, a
/// `OnceLock` nothing resets), so a later in-process boot without
/// `TASK_AUTH_SECRET` signs with that node's secret while the admin
/// child, a fresh process, falls back to the dev default. The key the
/// child mints then never verifies here, and `list_orgs` calls a live
/// bot key a dead one. `TASK_AUTH_SECRET` beats the installed secret,
/// so setting it on both sides makes them agree in any test order.
const ADMIN_AUTH_SECRET: &str = "bots-e2e-admin-test-auth-secret-32+!";

/// One `task-server admin …` run against `data_root`, as an operator
/// would type it: `(succeeded, stdout + stderr)`.
fn admin(data_root: &std::path::Path, args: &[&str]) -> (bool, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_task-server"))
        .arg("admin")
        .args(args)
        .env("TASK_DATA_ROOT", data_root)
        .env("TASK_AUTH_SECRET", ADMIN_AUTH_SECRET)
        .env_remove("TASK_SERVER_ORG")
        .env_remove("TASK_SERVER_VAULT_ROOT")
        .env_remove("TASK_MCP_TOKEN")
        .env_remove("TASK_CENTRAL_AUTH_URL")
        .output()
        .expect("run task-server admin");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// The slugs `list_orgs` answers on the account lane, or the refusal.
async fn listed_orgs(http: &reqwest::Client, url: &str, key: &str) -> Result<Vec<String>, Value> {
    let (err, said) = call(http, url, key, "list_orgs", json!({})).await;
    if err {
        return Err(said);
    }
    Ok(said["orgs"]
        .as_array()
        .unwrap_or_else(|| panic!("list_orgs answered {said}"))
        .iter()
        .filter_map(|o| o["slug"].as_str().map(str::to_owned))
        .collect())
}

/// Whether `/org/{slug}/mcp` lets this key in: its `tools/list` and a
/// read in the org both answer.
async fn admitted(http: &reqwest::Client, base: &str, slug: &str, key: &str) -> bool {
    let url = format!("{base}/org/{slug}/mcp");
    let listed = rpc(http, &url, key, "tools/list", json!({})).await;
    let (err, _) = call(http, &url, key, "list_tasks", json!({})).await;
    let tools = listed["result"]["tools"]
        .as_array()
        .is_some_and(|t| !t.is_empty());
    assert_eq!(
        tools, !err,
        "`{slug}`: tools/list and a call disagree on admission: {listed}"
    );
    tools
}

/// The voyager shape: an operator mints a bot with
/// NO orgs, the person it belongs to grants it one later. Driven through
/// the real `task-server admin` verbs against a running server holding
/// both seeded studio orgs, and through both MCP lanes with only the key.
// t[verify mcp.bots]
#[tokio::test(flavor = "multi_thread")]
async fn a_bot_made_with_no_orgs_reaches_nothing_until_granted() {
    use task_server::example_org;

    const ORGS: [(&str, &str, bool); 2] = [
        ("acme-audio", "ACME Audio", true),
        ("vnt-video", "VNT Video", false),
    ];
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::support::env_lock().await;
    // SAFETY: under the binary's env lock, for the boot only. The secret
    // is the one `admin` signs with (see `ADMIN_AUTH_SECRET`).
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::set_var("TASK_AUTH_SECRET", ADMIN_AUTH_SECRET);
    }
    let data_root = org_proto::DataRoot::new(tmp.path().to_owned());
    data_root.ensure().unwrap();
    for (slug, name, home) in ORGS {
        let org = data_root.init_org(slug, name, home).unwrap();
        example_org::install(&org, slug).unwrap();
        if home {
            // A server that keeps bots has a memberships store; without
            // one cross-org identity (and so every bot) is off.
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
    let account = format!("{base}/mcp");
    let http = reqwest::Client::new();
    let root = tmp.path();

    // The person the bot is for — the Captain, say. Nobody signs in:
    // the operator names them with `--owner`.
    let owner = uuid::Uuid::new_v4().to_string();

    // ── no orgs is said on purpose, and on its own ──
    let (ok, said) = admin(
        root,
        &["bot", "create", "--name", "Voyager", "--owner", &owner],
    );
    assert!(
        !ok,
        "an empty org list without --no-orgs is refused: {said}"
    );
    let (ok, said) = admin(
        root,
        &[
            "bot",
            "create",
            "--name",
            "Voyager",
            "--owner",
            &owner,
            "--no-orgs",
            "--org",
            "acme-audio",
        ],
    );
    assert!(!ok, "--no-orgs with --org is refused: {said}");

    // ── made with none ──
    let (ok, said) = admin(
        root,
        &[
            "bot",
            "create",
            "--name",
            "Voyager",
            "--no-orgs",
            "--owner",
            &owner,
        ],
    );
    assert!(ok, "admin bot create --no-orgs: {said}");
    let key = said
        .split_whitespace()
        .find_map(|w| w.strip_prefix("TASK_TOKEN="))
        .unwrap_or_else(|| panic!("the key is printed once: {said}"))
        .to_owned();
    assert!(key.starts_with("ak_"));
    assert!(said.contains("(no orgs)"), "{said}");

    // ── it is in the list, by its principal, with no orgs ──
    let (ok, listed) = admin(root, &["bot", "list", "--owner", &owner]);
    assert!(ok, "{listed}");
    let line = listed
        .lines()
        .find(|l| l.contains("Voyager"))
        .unwrap_or_else(|| panic!("the zero-org bot is listed: {listed}"));
    let principal = line.split_whitespace().next().unwrap().to_owned();
    let bot_id: uuid::Uuid = principal
        .parse()
        .unwrap_or_else(|_| panic!("the first column is its principal uuid: {line}"));
    assert!(line.ends_with("(no orgs)"), "{line}");
    let mine = bots::list(&home.auth, &home.memberships, owner.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].id, bot_id);
    assert!(mine[0].orgs.is_empty());
    let (_, theirs) = admin(
        root,
        &["bot", "list", "--owner", &uuid::Uuid::new_v4().to_string()],
    );
    assert!(
        !theirs.contains("Voyager"),
        "a bot is listed only to its owner: {theirs}"
    );

    // ── and it reaches nothing ──
    for (slug, ..) in ORGS {
        assert!(
            !admitted(&http, &base, slug, &key).await,
            "a zero-org bot is refused on /org/{slug}/mcp"
        );
    }
    assert_eq!(
        listed_orgs(&http, &account, &key).await,
        Ok(Vec::new()),
        "list_orgs answers, and answers empty"
    );
    let (err, said) = call(&http, &account, &key, "list_tasks", json!({})).await;
    assert!(err, "nothing to act in: {said}");
    assert!(
        said.to_string()
            .contains("does not belong to any organization"),
        "told it is a real account with no orgs, not a dead key: {said}"
    );

    // ── the operator grants one org; it reaches exactly that ──
    let (ok, said) = admin(
        root,
        &["grant", "--org", "vnt-video", "--principal", &principal],
    );
    assert!(ok, "admin grant: {said}");
    assert!(admitted(&http, &base, "vnt-video", &key).await, "granted");
    assert!(
        !admitted(&http, &base, "acme-audio", &key).await,
        "and nothing else"
    );
    assert_eq!(
        listed_orgs(&http, &account, &key).await,
        Ok(vec!["vnt-video".to_owned()])
    );
    let (_, listed) = admin(root, &["bot", "list", "--owner", &owner]);
    assert!(
        listed
            .lines()
            .any(|l| l.starts_with(&principal) && l.ends_with("vnt-video")),
        "{listed}"
    );

    // ── an issuer sync that knows nothing of the grant leaves it ──
    // The grant is a `local` row; a sync replaces `issuer` rows only.
    home.memberships
        .sync_from_issuer(bot_id, &[])
        .await
        .unwrap();
    assert!(
        admitted(&http, &base, "vnt-video", &key).await,
        "an issuer sync does not take the operator's grant away"
    );
    assert_eq!(
        listed_orgs(&http, &account, &key).await,
        Ok(vec!["vnt-video".to_owned()])
    );

    // ── revoked: the key is dead and the bot is gone ──
    let (ok, said) = admin(
        root,
        &["bot", "revoke", "--bot", &principal, "--owner", &owner],
    );
    assert!(ok, "admin bot revoke: {said}");
    for (slug, ..) in ORGS {
        assert!(
            !admitted(&http, &base, slug, &key).await,
            "a revoked key is refused on /org/{slug}/mcp"
        );
    }
    let refused = listed_orgs(&http, &account, &key).await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.to_string().contains("not recognised")),
        "a revoked key is a dead key on the account lane: {refused:?}"
    );
    let (_, listed) = admin(root, &["bot", "list", "--owner", &owner]);
    assert!(
        !listed.contains(&principal),
        "a revoked bot drops out of the list: {listed}"
    );
    assert!(
        home.memberships.for_user(bot_id).await.unwrap().is_empty(),
        "and its rows with it"
    );
}
