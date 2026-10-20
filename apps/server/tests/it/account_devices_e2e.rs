#![allow(clippy::large_futures)]
//! `offline.account` end to end: two devices signed in with one account
//! find each other through the issuer's device address book and sync —
//! no host, no hex string copied between them.
//!
//! The issuer is architect's auth engine in-process (the same
//! `DeviceService` `fts-auth` mounts); the devices are two nodes, dialling
//! each other over iroh (addresses through `TASK_IROH_PEER_DIR`, the
//! offline stand-in for discovery). The phone signs in first, so its
//! personal org is the account's; the laptop joins it under a slug of
//! its own (it holds a `personal` of its own) and keeps its own data.

use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use architect_auth::proto::AuthServiceClient;
use architect_auth::transport::vox::AuthVoxService;
use architect_auth::{DeviceServiceClient, SignInEmailPassword, SignUpEmailPassword};
use task_server::account_devices::{ThisDevice, sync_account};
use task_server::local_node::{LocalNode, NodeOptions, PERSONAL_SLUG};
use vault_proto::{IfMatch, VaultSyncClient};
use vox_core::FromVoxLane;

const NOTE: &str = "Practice log.md";

fn opts(root: &std::path::Path, name: &str) -> NodeOptions {
    NodeOptions {
        owner_name: name.into(),
        peers: true,
        ..NodeOptions::at(root)
    }
}

fn client<C: FromVoxLane>(caller: &vox_core::Caller) -> C {
    C::from_vox_lane(caller.clone(), None)
}

async fn sign_in(node: &LocalNode) -> String {
    let owner = node.owner().clone();
    let anon = node.caller(&owner.org, None).await.expect("anonymous lane");
    let auth: AuthServiceClient = client(&anon);
    auth.sign_in_email_password(SignInEmailPassword {
        email: owner.email.clone(),
        password: owner.password.clone(),
        ip_address: None,
        user_agent: Some("account-devices-e2e".into()),
    })
    .await
    .expect("sign in on the node")
    .token
}

async fn sweep(node: &LocalNode, slug: &str) {
    let endpoint = node.endpoint(slug).expect("the shared org is bound here");
    tokio::time::timeout(
        Duration::from_secs(90),
        task_server::device_sync::sweep(node.state(), slug, &endpoint),
    )
    .await
    .expect("the sweep did not finish");
}

async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..300 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

fn device(name: &str, platform: &str, account: uuid::Uuid) -> ThisDevice {
    ThisDevice {
        device_id: uuid::Uuid::new_v4(),
        name: name.into(),
        platform: platform.into(),
        account,
    }
}

/// The role `account` holds in `slug` on `node` (its home org's
/// memberships store — the fence a FastTrackStudio account passes).
async fn role_on(node: &LocalNode, account: uuid::Uuid, slug: &str) -> Option<String> {
    let home = node.state().data_root.org(PERSONAL_SLUG);
    let store = task_server::memberships::Memberships::open(&home.memberships_db())
        .await
        .expect("memberships");
    store
        .role_for(account, slug)
        .await
        .expect("role")
        .and_then(|m| m.role)
}

// t[verify offline.account] — two devices of one account, introduced by
// the issuer, share the first one's org and sync it
#[tokio::test(flavor = "multi_thread")]
async fn an_accounts_devices_find_each_other_through_the_issuer() {
    let env = crate::support::env_lock().await;
    let peer_dir = tempfile::tempdir().unwrap();
    // SAFETY: under the binary's env lock, held while both nodes bind.
    unsafe {
        std::env::set_var("TASK_IROH_PEER_DIR", peer_dir.path());
        std::env::set_var("TASK_DEVICE_SYNC_SECS", "0");
        std::env::set_var("TASK_ENFORCE_PERMISSIONS", "1");
    }

    // ── the issuer: fts-auth's device address book, in-process ──
    let issuer =
        task_server::AuthState::open("sqlite::memory:", "an-issuer-secret-at-least-32-bytes!")
            .await
            .expect("issuer");
    let router = LayerRouter::new()
        .merge(architect_auth::auth_service_layer(AuthVoxService::new(
            issuer.auth.clone(),
        )))
        .merge(architect_auth::device_service_layer(AuthVoxService::new(
            issuer.auth,
        )));
    let local = LocalServer::serve(router, Scope::new());
    let accounts: AuthServiceClient = local.establish().await.expect("issuer auth client");
    let devices: DeviceServiceClient = local.establish().await.expect("issuer device client");
    let signed_up = accounts
        .sign_up_email_password(SignUpEmailPassword {
            email: "cody@example.com".into(),
            password: "correct horse battery staple".into(),
            name: Some("Cody".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: Some("account-devices-e2e".into()),
        })
        .await
        .expect("an account");
    let (token, account) = (signed_up.token, signed_up.user.id);

    // ── two devices, each its own node with its own personal org ──
    let (phone_root, mac_root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let phone = LocalNode::open(opts(phone_root.path(), "Cody"))
        .await
        .expect("phone");
    let mac = LocalNode::open(opts(mac_root.path(), "Cody"))
        .await
        .expect("mac");
    let (phone_me, mac_me) = (
        device("Cody's iPhone", "ios", account),
        device("Studio Mac", "macos", account),
    );

    // The phone signs in first: its personal org becomes the account's.
    let p1 = sync_account(&phone, &devices, &token, &phone_me)
        .await
        .expect("phone pass");
    assert_eq!(p1.slug.as_deref(), Some(PERSONAL_SLUG));
    assert!(!p1.joined && p1.others.is_empty());
    let shared_id = phone.manifest(PERSONAL_SLUG).unwrap().id;
    let mac_own = mac.manifest(PERSONAL_SLUG).unwrap().id;
    assert_ne!(shared_id, mac_own, "two personal orgs to start with");

    // The laptop joins it — under its own slug, `personal` being its own.
    let m1 = sync_account(&mac, &devices, &token, &mac_me)
        .await
        .expect("mac pass");
    assert!(m1.joined);
    let mac_slug = m1.slug.clone().expect("the mac holds the shared org");
    assert_ne!(mac_slug, PERSONAL_SLUG);
    assert_eq!(mac.slug_of(shared_id).as_deref(), Some(mac_slug.as_str()));
    assert_eq!(
        mac.manifest(PERSONAL_SLUG).unwrap().id,
        mac_own,
        "its own org stays"
    );
    assert_eq!(m1.admitted, vec![phone.endpoint_id(PERSONAL_SLUG).unwrap()]);

    // The phone's next pass learns of the laptop and admits it.
    let p2 = sync_account(&phone, &devices, &token, &phone_me)
        .await
        .expect("phone again");
    assert!(!p2.joined);
    assert_eq!(p2.admitted, vec![mac.endpoint_id(&mac_slug).unwrap()]);
    // The signed-in account is let in where it should be: on the phone,
    // its org (the shared one); on the laptop, the shared org and the
    // laptop's own — so its FTS sign-in sees them, not an empty list.
    assert_eq!(role_on(&phone, account, PERSONAL_SLUG).await.as_deref(), Some("admin"));
    assert_eq!(role_on(&mac, account, &mac_slug).await.as_deref(), Some("admin"));
    assert_eq!(role_on(&mac, account, PERSONAL_SLUG).await.as_deref(), Some("admin"));
    // And a pass is idempotent.
    let m2 = sync_account(&mac, &devices, &token, &mac_me)
        .await
        .expect("mac again");
    assert!(!m2.joined && m2.slug.as_deref() == Some(mac_slug.as_str()));
    drop(env);

    // ── a note on the phone reaches the laptop's copy of the shared org ──
    let phone_token = sign_in(&phone).await;
    let phone_lane = phone
        .caller(PERSONAL_SLUG, Some(phone_token))
        .await
        .unwrap();
    let phone_vault: VaultSyncClient = client(&phone_lane);
    let log = "---\ntitle: Practice log\n---\nScales, 20 minutes\n";
    phone_vault
        .put_file(
            "default".into(),
            NOTE.into(),
            log.into(),
            IfMatch::CreateOnly,
        )
        .await
        .expect("the phone writes a note");
    sweep(&mac, &mac_slug).await;
    let on_mac = mac
        .state()
        .data_root
        .org(mac_slug.as_str())
        .vault_dir()
        .join(NOTE);
    eventually("the note on the laptop", || {
        std::fs::read_to_string(&on_mac).ok().as_deref() == Some(log)
    })
    .await;
    let mac_personal = mac
        .state()
        .data_root
        .org(PERSONAL_SLUG)
        .vault_dir()
        .join(NOTE);
    assert!(
        !mac_personal.exists(),
        "the laptop's own org is not the shared one"
    );

    // ── a task made on the laptop reaches the phone ──
    let mac_token = sign_in(&mac).await;
    let mac_lane = mac.caller(&mac_slug, Some(mac_token)).await.unwrap();
    let mac_tasks: task::TaskServiceClient = client(&mac_lane);
    let made = mac_tasks
        .create(task::capture("Restring the Telecaster"))
        .await
        .expect("the laptop makes a task");
    sweep(&phone, PERSONAL_SLUG).await;
    let on_phone = phone
        .state()
        .data_root
        .org(PERSONAL_SLUG)
        .vault_dir()
        .join(&made.path);
    eventually("the laptop's task on the phone", || on_phone.is_file()).await;
}
