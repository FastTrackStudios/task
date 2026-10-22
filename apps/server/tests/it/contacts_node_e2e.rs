#![allow(clippy::large_futures)]
//! Contacts on every node (`contacts.node.every-node`, ADR 0006's
//! host-absent stage; contacts spec card C6).
//!
//! The contacts backend — the directory (`VaultContacts`) and the
//! CardDAV pull (`sync_account`) — is built per org by the one boot
//! every node runs (`build_org_state`): `task-server`, a desktop or
//! phone's own node (`LocalNode`), a node joining an org. Every lane
//! reaches it through the org router behind the permission gate, so
//! the UI's call site (`crates/ui/src/feeds.rs`) is the same against a
//! host and against the device.
//!
//! Two stages:
//!
//! 1. **The seed, server and device.** The example studio's contacts,
//!    served by a server and by a desktop node over the planted example,
//!    are the same people the example commits; with the server stopped
//!    the device still lists them and edits one.
//! 2. **Two device nodes, no host.** One node holds a CardDAV account
//!    and pulls the address book itself (the fake TLS server
//!    `contacts::testing` provides), behind the enforced gate. The other
//!    node gets the people by the ordinary vault sync, edits one, and the
//!    first node pulls the edit back. The account and its password never
//!    leave the node that has them.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use architect_auth::SignInEmailPassword;
use architect_auth::proto::AuthServiceClient;
use contacts::testing::{Behaviour, FakeCardDav, Host, vcard};
use contacts_proto::{CardDavAccount, CardDavProvider, Contact, ContactsClient};
use task_server::local_node::{JoinSpec, LocalNode, NodeOptions};
use vox_core::FromVoxLane;

use crate::support;

const BAND: &str = "the-band";

/// Distinctive enough that finding it anywhere is finding the password.
const PASSWORD: &str = "node-carddav-app-password-5be1d7a2";

fn client<C: FromVoxLane>(caller: &vox_core::Caller) -> C {
    C::from_vox_lane(caller.clone(), None)
}

/// The node's own person, signed in through `slug`'s auth lane.
async fn sign_in(node: &LocalNode, slug: &str) -> String {
    let owner = node.owner().clone();
    let anon = node.caller(slug, None).await.expect("anonymous lane");
    let auth: AuthServiceClient = client(&anon);
    auth.sign_in_email_password(SignInEmailPassword {
        email: owner.email.clone(),
        password: owner.password.clone(),
        ip_address: None,
        user_agent: Some("contacts-node-e2e".into()),
    })
    .await
    .expect("sign in on the node")
    .token
}

/// `(id, full_name)` of every contact the example commits for `org`.
fn committed_contacts(org: &str) -> BTreeSet<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/studio")
        .join(org)
        .join("Vault/Records/contacts");
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(&dir).expect("the example's contacts dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a committed contact");
        let field = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key))
                .map(|v| v.trim().to_owned())
                .unwrap_or_else(|| panic!("{} has no `{key}`", path.display()))
        };
        out.insert((field("id:"), field("full_name:")));
    }
    out
}

fn people(contacts: &[Contact]) -> BTreeSet<(String, String)> {
    contacts
        .iter()
        .map(|c| (c.id.clone(), c.full_name.clone()))
        .collect()
}

// ── Stage 1: the seed, on the server and on a device ────────────────

// t[verify contacts.node.every-node] — the seeded people, served by the
// server and by a desktop node the same, and by the node alone once the
// server is stopped
#[tokio::test(flavor = "multi_thread")]
async fn a_device_node_serves_the_seeded_contacts_with_or_without_the_server() {
    let committed = committed_contacts(support::ORG);
    assert!(
        committed.len() >= 3,
        "fixture: the example seeds contacts: {committed:?}"
    );

    // ── The server over the example studio, served the way the binary
    //    serves it, with a handle so this test can stop it.
    let (state, _server_root) = support::boot_app_state()
        .await
        .expect("boot the server over the example studio");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the server");
    let url = format!(
        "ws://{}/vox",
        listener.local_addr().expect("server address")
    );
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, task_server::router(state)).await;
    });
    let on_server = {
        let contacts = vox::connect_lane::<ContactsClient>(&url)
            .establish()
            .await
            .expect("contacts on the server");
        contacts.list_contacts().await.expect("the server lists")
    };
    assert_eq!(
        people(&on_server),
        committed,
        "the server's contacts are the example's"
    );

    // ── A desktop node over the planted example: the same people.
    let root = tempfile::tempdir().unwrap();
    let node = {
        let _env = support::env_lock().await;
        // SAFETY: under the binary's env lock. No Bible download; the
        // gate enforced, so the device lane is a signed-in person's.
        unsafe {
            std::env::set_var("TASK_DEMO_NO_BIBLE", "1");
            std::env::set_var("TASK_ENFORCE_PERMISSIONS", "1");
        }
        LocalNode::open(NodeOptions {
            plant_example: true,
            peers: false,
            ..NodeOptions::at(root.path())
        })
        .await
        .expect("a desktop node over the planted example")
    };
    assert_eq!(node.owner().org, support::ORG, "fixture: Alice owns ACME");
    let token = sign_in(&node, support::ORG).await;
    let lane = node.caller(support::ORG, Some(token)).await.unwrap();
    let contacts: ContactsClient = client(&lane);
    let on_device = contacts.list_contacts().await.expect("the device lists");
    assert_eq!(
        people(&on_device),
        committed,
        "the device node's contacts are the server's"
    );

    // Behind the gate, not around it: the device's lane with no session
    // is refused, as a socket with no token would be.
    let anon: ContactsClient = client(&node.caller(support::ORG, None).await.unwrap());
    assert!(
        anon.list_contacts().await.is_err(),
        "an anonymous lane read the device's contacts"
    );

    // ── The server stops; the device alone lists and edits.
    server.abort();
    let _ = server.await;
    let gone = tokio::time::timeout(
        Duration::from_secs(5),
        vox::connect_lane::<ContactsClient>(&url).establish(),
    )
    .await;
    assert!(
        !matches!(gone, Ok(Ok(_))),
        "fixture: the server is still answering"
    );
    let (id, _) = committed.iter().next().cloned().expect("a seeded contact");
    let mut person = contacts
        .get_contact(id.clone())
        .await
        .expect("get with no server")
        .expect("the seeded contact is there");
    person.title = Some("Head engineer".into());
    person.updated = Some(chrono::Utc::now().to_rfc3339());
    contacts
        .upsert_contact(person.clone())
        .await
        .expect("edit with no server");
    let back = contacts
        .get_contact(id)
        .await
        .expect("read the edit back")
        .expect("still there");
    assert_eq!(back.title.as_deref(), Some("Head engineer"));
    assert_eq!(
        people(&contacts.list_contacts().await.expect("list after the edit")),
        committed,
        "an edit is not a new person"
    );
}

// ── Stage 2: two device nodes, no host ──────────────────────────────

fn addressbook() -> Behaviour {
    Behaviour {
        credentials: Some(("studio@the-band.example".into(), PASSWORD.into())),
        cards: vec![
            (
                "ada.vcf".into(),
                "etag-ada".into(),
                vcard("uid-ada-node", "Ada Lovelace"),
            ),
            (
                "grace.vcf".into(),
                "etag-grace".into(),
                vcard("uid-grace-node", "Grace Hopper"),
            ),
        ],
        ..Behaviour::default()
    }
}

fn account(server_url: &str) -> CardDavAccount {
    let mut a = CardDavAccount::create(
        "band-book",
        "The band's address book",
        CardDavProvider::NEXTCLOUD,
        "2026-10-22T09:00:00Z",
    );
    a.server_url = server_url.into();
    a.username = "studio@the-band.example".into();
    a.password = PASSWORD.into();
    a
}

/// Every byte of every file under `dir`, as lossy text.
fn every_byte_under(dir: &Path) -> String {
    let mut out = String::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(bytes) = std::fs::read(&p) {
                out.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
    }
    out
}

/// One pass of `node`'s sweep of the band's peers — what its timer runs.
async fn sweep(node: &LocalNode) {
    let endpoint = node.endpoint(BAND).expect("the band is bound here");
    tokio::time::timeout(
        Duration::from_secs(90),
        task_server::device_sync::sweep(node.state(), BAND, &endpoint),
    )
    .await
    .expect("the sweep did not finish");
}

/// Sweep both ways, a round at a time (what the nodes' timers do),
/// until `contacts` shows a contact satisfying `cond`.
async fn converge(
    what: &str,
    (a, b): (&LocalNode, &LocalNode),
    contacts: &ContactsClient,
    mut cond: impl FnMut(&[Contact]) -> bool,
) -> Vec<Contact> {
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        sweep(b).await;
        sweep(a).await;
        for _ in 0..20 {
            let listed = contacts.list_contacts().await.expect("list contacts");
            if cond(&listed) {
                return listed;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    panic!("timed out waiting for: {what}");
}

fn named<'a>(contacts: &'a [Contact], name: &str) -> Option<&'a Contact> {
    contacts.iter().find(|c| c.full_name == name)
}

// t[verify contacts.node.every-node] — two device nodes, no host: each
// lists and edits through its own gated lane, and an edit on one is
// pulled by the other
// t[verify contacts.sync.host-absent] — the node holding the account
// pulls CardDAV itself with no host anywhere; the peer gets the people,
// never the account
#[tokio::test(flavor = "multi_thread")]
async fn two_device_nodes_share_contacts_and_the_account_stays_put() {
    let fake = FakeCardDav::start(Host::Localhost, addressbook());

    let env = support::env_lock().await;
    let peer_dir = tempfile::tempdir().unwrap();
    // SAFETY: under the binary's env lock, held while both nodes boot
    // the band's org (the contacts backend reads these at that boot).
    unsafe {
        std::env::set_var("TASK_IROH_PEER_DIR", peer_dir.path());
        // Sweeps run when this test says, not on a timer.
        std::env::set_var("TASK_DEVICE_SYNC_SECS", "0");
        // Enforced, not observed: every call below is a member's.
        std::env::set_var("TASK_ENFORCE_PERMISSIONS", "1");
        // The node's own credential store (the envelope keyed by its
        // auth secret) — no desktop keyring in CI.
        std::env::set_var("TASK_CONTACTS_SECRETS", "envelope");
        // The fake CardDAV server's test CA.
        std::env::set_var("TASK_CARDDAV_EXTRA_CA", contacts::testing::ca_pem_path());
    }
    let (a_root, b_root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let opts = |root: &Path| NodeOptions {
        owner_name: "Sam Device".into(),
        peers: true,
        ..NodeOptions::at(root)
    };
    let a = LocalNode::open(opts(a_root.path())).await.expect("phone");
    let b = LocalNode::open(opts(b_root.path())).await.expect("laptop");

    // The band's org, made on the phone; the laptop takes a node of it;
    // each admits the other (the join handshake `peer_docs_e2e` runs).
    let a_token = sign_in(&a, &a.owner().org.clone()).await;
    let mgmt: org_proto::OrgManagementServiceClient = client(&a.server_caller().await.unwrap());
    let band = mgmt
        .create_org(org_proto::CreateOrgRequest {
            session_token: a_token.clone(),
            slug: BAND.into(),
            display_name: "The Band".into(),
            is_home: false,
        })
        .await
        .expect("the phone creates the band's org");
    let a_peer = a.bind(BAND).await.expect("the phone binds the band");
    let joined = b
        .join(JoinSpec {
            slug: BAND.into(),
            id: band.id,
            display_name: band.display_name.clone(),
            peer: Some(a_peer),
            role: Some("member".into()),
        })
        .await
        .expect("the laptop joins");
    let b_peer = joined
        .endpoint_id
        .expect("the laptop is bound for the band");
    let a_lane = a.caller(BAND, Some(a_token)).await.unwrap();
    let a_sync: files_proto::service::sync::SyncServiceClient = client(&a_lane);
    a_sync
        .enroll_device(b_peer, "laptop".into())
        .await
        .expect("the phone admits the laptop");
    drop(env);

    let b_token = sign_in(&b, &b.owner().org.clone()).await;
    let b_lane = b.caller(BAND, Some(b_token)).await.unwrap();
    let (a_contacts, b_contacts): (ContactsClient, ContactsClient) =
        (client(&a_lane), client(&b_lane));

    // ── The phone pulls its CardDAV address book itself: no host.
    a_contacts
        .upsert_account(account(&format!("{}/dav/", fake.origin())))
        .await
        .expect("the phone saves the account");
    let report = a_contacts
        .sync_account("band-book".into())
        .await
        .expect("the phone syncs CardDAV on its own");
    assert_eq!(report.added, 2, "{report:?}");
    assert!(
        fake.requests_with_authorization() > 0,
        "fixture: the fake saw the phone's sync"
    );
    // And a person typed in on the phone.
    let typed = Contact::create(
        uuid::Uuid::new_v4().to_string(),
        "Linus the drummer",
        chrono::Utc::now().to_rfc3339(),
    );
    a_contacts
        .upsert_contact(typed.clone())
        .await
        .expect("the phone adds a person");

    let pair = (&a, &b);

    // ── The laptop gets all three by the vault sync.
    let on_b = converge("the phone's people on the laptop", pair, &b_contacts, |c| {
        ["Ada Lovelace", "Grace Hopper", "Linus the drummer"]
            .iter()
            .all(|n| named(c, n).is_some())
    })
    .await;
    let ada = named(&on_b, "Ada Lovelace").expect("Ada").clone();
    assert_eq!(
        ada.uid.as_deref(),
        Some("uid-ada-node"),
        "the synced card's identity crossed with it"
    );

    // ── The laptop edits a person; the phone pulls the edit.
    let mut edited = ada.clone();
    edited.phones = "+1 555 0100".into();
    edited.updated = Some(chrono::Utc::now().to_rfc3339());
    b_contacts
        .upsert_contact(edited)
        .await
        .expect("the laptop edits Ada");
    let on_a = converge("the laptop's edit on the phone", pair, &a_contacts, |c| {
        c.iter()
            .any(|p| p.id == ada.id && p.primary_phone() == Some("+1 555 0100"))
    })
    .await;
    assert_eq!(
        on_a.iter()
            .filter(|c| c.full_name == "Ada Lovelace")
            .count(),
        1,
        "the edit landed in place, not as a second Ada"
    );

    // ── The account stayed on the phone.
    let a_accounts = a_contacts.list_accounts().await.expect("phone accounts");
    assert!(
        a_accounts.iter().any(|x| x.id == "band-book"),
        "the phone still has its account"
    );
    let b_accounts = b_contacts.list_accounts().await.expect("laptop accounts");
    assert!(
        b_accounts.is_empty(),
        "an account reached the laptop: {b_accounts:?}"
    );
    assert!(
        !every_byte_under(b_root.path()).contains(PASSWORD),
        "the password reached the laptop"
    );
    // A second pull on the phone after the round trip: nothing new from
    // the server (ETags match), and the laptop's edit is not undone by it.
    let again = a_contacts
        .sync_account("band-book".into())
        .await
        .expect("a second pull");
    assert_eq!((again.added, again.skipped), (0, 2), "{again:?}");
    let ada_on_a = a_contacts
        .get_contact(ada.id.clone())
        .await
        .expect("the phone reads Ada")
        .expect("Ada is on the phone");
    assert_eq!(
        ada_on_a.primary_phone(),
        Some("+1 555 0100"),
        "the second pull undid the laptop's edit"
    );
}
