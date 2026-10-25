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
use architect_auth::{
    DeviceRegistration, DeviceServiceClient, SignInEmailPassword, SignUpEmailPassword,
};
use task_server::account_devices::{ThisDevice, device_id, sync_account};
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

/// Sweep `slug` on `node` until `cond` holds, the way the cadence would.
///
/// A sweep logs a peer it could not dial and moves on (a shut laptop is
/// ordinary), so one slow dial on a loaded runner makes that pass a no-op.
/// Sweeping once and then only waiting turned that into a 15 s timeout
/// (#257); repeating the sweep is what a deployed node does anyway.
async fn sweep_until(node: &LocalNode, slug: &str, what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..12 {
        sweep(node, slug).await;
        for _ in 0..100 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    panic!("timed out waiting for: {what}");
}

fn device(name: &str, platform: &str, account: uuid::Uuid) -> ThisDevice {
    ThisDevice {
        device_id: uuid::Uuid::new_v4(),
        name: name.into(),
        platform: platform.into(),
        account,
        retire: None,
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
    assert_eq!(
        role_on(&phone, account, PERSONAL_SLUG).await.as_deref(),
        Some("admin")
    );
    assert_eq!(
        role_on(&mac, account, &mac_slug).await.as_deref(),
        Some("admin")
    );
    assert_eq!(
        role_on(&mac, account, PERSONAL_SLUG).await.as_deref(),
        Some("admin")
    );
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
    let on_mac = mac
        .state()
        .data_root
        .org(mac_slug.as_str())
        .vault_dir()
        .join(NOTE);
    sweep_until(&mac, &mac_slug, "the note on the laptop", || {
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
    let on_phone = phone
        .state()
        .data_root
        .org(PERSONAL_SLUG)
        .vault_dir()
        .join(&made.path);
    sweep_until(
        &phone,
        PERSONAL_SLUG,
        "the laptop's task on the phone",
        || on_phone.is_file(),
    )
    .await;
}

// t[verify offline.account] — a device running Task and Studio is listed
// once: Task moves from its old random install id to the platform's (the
// one Studio derives), takes its old row off the account, and keeps its
// endpoint on the shared row Studio registered without one
#[tokio::test(flavor = "multi_thread")]
async fn a_device_running_task_and_studio_is_listed_once() {
    let env = crate::support::env_lock().await;
    let peer_dir = tempfile::tempdir().unwrap();
    // SAFETY: under the binary's env lock, held while the node binds.
    unsafe {
        std::env::set_var("TASK_IROH_PEER_DIR", peer_dir.path());
        std::env::set_var("TASK_DEVICE_SYNC_SECS", "0");
    }
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
            email: "once@example.com".into(),
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
    let root = tempfile::tempdir().unwrap();
    let mac = LocalNode::open(opts(root.path(), "Cody")).await.expect("mac");

    // An older Task registered this Mac under a random install id.
    let old_install = uuid::Uuid::new_v4();
    let old = ThisDevice {
        device_id: device_id(old_install, &account.to_string()),
        ..device("Cody's Mac", "macos", account)
    };
    sync_account(&mac, &devices, &token, &old)
        .await
        .expect("the old pass");

    // Studio, on the same Mac, registers under the platform's install id
    // (no node: no endpoint, no org to offer).
    let shared_install = uuid::Uuid::new_v4(); // stands for this_device::install_id()
    let shared = device_id(shared_install, &account.to_string());
    devices
        .register_device(
            token.clone(),
            DeviceRegistration {
                device_id: shared,
                name: "Cody's Mac".into(),
                platform: "macos".into(),
                endpoint_id: None,
                relay_url: None,
                offer_org: None,
            },
        )
        .await
        .expect("studio registers");
    // And a stray from a reinstall that lost its install-id file: same
    // name and platform, another random id.
    devices
        .register_device(
            token.clone(),
            DeviceRegistration {
                device_id: uuid::Uuid::new_v4(),
                name: "Cody's Mac".into(),
                platform: "macos".into(),
                endpoint_id: None,
                relay_url: None,
                offer_org: None,
            },
        )
        .await
        .expect("a stray row");
    assert_eq!(
        devices.list_devices(token.clone()).await.unwrap().devices.len(),
        3,
        "listed three times, which is the bug"
    );

    // Task moves to the shared id and retires its old row.
    let now = ThisDevice {
        device_id: shared,
        retire: Some(old.device_id),
        ..device("Cody's Mac", "macos", account)
    };
    let pass = sync_account(&mac, &devices, &token, &now)
        .await
        .expect("the moving pass");
    assert!(pass.retired, "the old row is off the account");
    assert!(
        pass.others.is_empty(),
        "its own old row is never another device"
    );
    let listed = devices.list_devices(token.clone()).await.unwrap().devices;
    assert_eq!(listed.len(), 1, "listed once: {listed:?}");
    assert_eq!(listed[0].device_id, shared);
    assert_eq!(
        listed[0].endpoint_id,
        mac.endpoint_id(PERSONAL_SLUG),
        "the shared row carries Task's node"
    );

    // Nothing left to retire: the next pass says so without asking.
    let again = sync_account(&mac, &devices, &token, &now)
        .await
        .expect("again");
    assert!(again.retired);
    assert_eq!(devices.list_devices(token).await.unwrap().devices.len(), 1);
    drop(env);
}
