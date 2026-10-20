#![allow(clippy::large_futures)]
//! The `task` CLI on the desktop app's node (`offline.cli`): one sign-in,
//! one copy of the data.
//!
//! The desktop boots a [`LocalNode`], serves it on loopback
//! ([`LocalNode::serve_loopback`], which writes `local-endpoint` into the
//! node root) and keeps its person's sign-in in its token store. A
//! [`TaskClient`] with nothing configured — no `--server`, no CLI session —
//! must find that node, present that sign-in, and read and write the
//! desktop's tasks: what `task list` / `task add` do on a Mac with the app
//! open.

use architect_auth::SignInEmailPassword;
use architect_auth::proto::AuthServiceClient;
use task_client::{Config, TaskClient};
use task_server::local_node::{LOCAL_ENDPOINT_FILE, LocalNode, NodeOptions, PERSONAL_SLUG};
use vox_core::FromVoxLane;

// t[verify offline.cli]
#[tokio::test(flavor = "multi_thread")]
async fn the_cli_works_on_the_desktop_node_as_its_person() {
    let _env = crate::support::env_lock().await;
    let app = tempfile::tempdir().unwrap();
    let root = app.path().join("task").join("node");
    // SAFETY: under the binary's env lock.
    unsafe {
        std::env::set_var("XDG_DATA_HOME", app.path());
        std::env::set_var("TASK_NODE_ROOT", &root);
        std::env::remove_var("TASK_SESSION_FILE");
        std::env::remove_var("TASK_VOX_URL");
    }

    // ── the desktop: a node, served on loopback, its person signed in ──
    let node = LocalNode::open(NodeOptions {
        owner_name: "Sam Desk".into(),
        peers: false,
        ..NodeOptions::at(&root)
    })
    .await
    .expect("the desktop's node opens");
    let base = node.serve_loopback().await.expect("served on loopback");
    assert!(root.join(LOCAL_ENDPOINT_FILE).is_file());
    let owner = node.owner().clone();
    let anon = node.caller(&owner.org, None).await.unwrap();
    let auth = AuthServiceClient::from_vox_lane(anon, None);
    let token = auth
        .sign_in_email_password(SignInEmailPassword {
            email: owner.email.clone(),
            password: owner.password.clone(),
            ip_address: None,
            user_agent: Some("desktop".into()),
        })
        .await
        .expect("the desktop signs its person in")
        .token;
    task_client::desktop::save_signin(&owner.email, &token).unwrap();

    // ── the CLI: nothing configured, so the desktop's node and sign-in ──
    let cli = TaskClient::new(Config {
        embed: Some(false),
        ..Config::new()
    });
    assert_eq!(cli.server_base(), base, "the CLI finds the running desktop");
    assert_eq!(
        task_client::desktop::active_signin().map(|(email, _)| email),
        Some(owner.email.clone())
    );
    let tasks: task::TaskServiceClient = cli.org(PERSONAL_SLUG).await.expect("dial the node");
    let made = tasks
        .create(task::capture("Restring the Telecaster"))
        .await
        .expect("the CLI writes as the desktop's person");

    // The desktop sees what the CLI wrote, through its own lane.
    let desk = node.caller(PERSONAL_SLUG, Some(token)).await.unwrap();
    let desk_tasks = task::TaskServiceClient::from_vox_lane(desk, None);
    let on_desk = desk_tasks.list().await.unwrap();
    assert!(
        on_desk.iter().any(|t| t.id == made.id),
        "one copy of the data"
    );

    // …and the CLI sees the desktop's.
    let listed = tasks.list().await.unwrap();
    assert!(listed.iter().any(|t| t.id == made.id));
}
