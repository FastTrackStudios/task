//! Chapter — a CardDAV password goes nowhere but where it is used.
//!
//! The contacts feature pulls people from Nextcloud or iCloud with an
//! app-specific password. Before this chapter the account file kept that
//! password in plaintext inside the vault — and the vault is a File Root,
//! so its history kept it, its manifest listed it and every replica
//! pulled it. Three rules replace that, each with a test here, all over
//! the wire:
//!
//! - `contacts.secret.out-of-vault` — the file holds a reference; the
//!   password is in a credential store (here the server's envelope,
//!   keyed by its auth secret), and a restart still syncs.
//! - `vault.private.node-only` — the account directory is node-private:
//!   not in the vault manifest, not readable by name, not in any peer's
//!   copy.
//! - `contacts.transport.https-only` and
//!   `contacts.transport.same-origin-credentials` — an http origin is
//!   refused at save, and a server that redirects elsewhere never
//!   receives a request carrying the password there.
//!
//! The fake CardDAV server is real TLS on loopback
//! (`contacts::testing::FakeCardDav`), trusted through the test CA the
//! harness hands every server (`TASK_CARDDAV_EXTRA_CA`).
//!
//! The last test is the ADR 0006 host-absent stage: two device nodes, no
//! server booted at all. The node holding the account syncs CardDAV on
//! its own, and the peer that replicates its vault gets the contacts and
//! nothing about the account.

use std::path::Path;

use contacts::testing::{Behaviour, FakeCardDav, Host, vcard};
use contacts_proto::{CardDavAccount, CardDavProvider, Contacts as _};
use files::model::RootFlavor;
use files_domain::Hosting;

use integration::client::Session;
use integration::device::Laptop;
use integration::scenario::Scenario;

/// Distinctive enough that finding it anywhere is finding the password.
const PASSWORD: &str = "it-carddav-app-password-7f3a9c41";

fn addressbook() -> Behaviour {
    Behaviour {
        credentials: Some(("studio@acme-audio.example".into(), PASSWORD.into())),
        cards: vec![
            (
                "ada.vcf".into(),
                "etag-ada".into(),
                vcard("uid-ada-it", "Ada Lovelace"),
            ),
            (
                "grace.vcf".into(),
                "etag-grace".into(),
                vcard("uid-grace-it", "Grace Hopper"),
            ),
        ],
        ..Behaviour::default()
    }
}

fn account(id: &str, server_url: &str, password: &str) -> CardDavAccount {
    let mut a = CardDavAccount::create(
        id,
        "Studio address book",
        CardDavProvider::NEXTCLOUD,
        "2026-10-20T09:00:00Z",
    );
    a.server_url = server_url.into();
    a.username = "studio@acme-audio.example".into();
    a.password = password.into();
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

/// Every relative file path under `dir`.
fn paths_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(dir) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out
}

/// Saving an account over the wire leaves no password in the vault, in
/// the vault's manifest, or in a peer's replica of the vault — and the
/// account directory is not even named there.
// t[verify contacts.secret.out-of-vault]
// t[verify vault.private.node-only]
#[tokio::test]
async fn a_saved_password_is_nowhere_the_vault_or_a_peer_can_see() {
    let s = Scenario::open().await;
    let fake = FakeCardDav::start(Host::Localhost, addressbook());
    let alice = s.as_alice().await;
    let contacts = alice.contacts().await;

    contacts
        .upsert_account(account(
            "it-studio",
            &format!("{}/dav/", fake.origin()),
            PASSWORD,
        ))
        .await
        .expect("save the account");
    let listed = contacts.list_accounts().await.expect("list accounts");
    let saved = listed.iter().find(|a| a.id == "it-studio").expect("listed");
    assert_eq!(saved.password, "", "the password came back over the wire");

    // On the host's disk: nowhere in the vault.
    let vault = s.orgs.acme.org_root().join("vault");
    assert!(
        vault.join("Records/contacts/.sync/it-studio.md").is_file(),
        "the account was not written where it lives"
    );
    assert!(
        !every_byte_under(&vault).contains(PASSWORD),
        "the password is in the vault"
    );

    // In the vault manifest: not the password, not even the directory.
    let lane = alice.vault().await;
    let manifest = lane
        .manifest("default".into())
        .await
        .expect("the vault manifest");
    let listed: Vec<&str> = manifest.files.iter().map(|e| e.path.as_str()).collect();
    assert!(
        listed.iter().any(|p| p.starts_with("Records/contacts/")),
        "fixture: the seeded contacts are in the manifest"
    );
    assert!(
        !listed.iter().any(|p| p.contains("contacts/.sync")),
        "the account directory is in the manifest: {listed:?}"
    );
    assert!(
        lane.get_file(
            "default".into(),
            "Records/contacts/.sync/it-studio.md".into()
        )
        .await
        .is_err(),
        "an account file was served by name"
    );

    // In a peer's copy of the vault root: the contacts, not the accounts.
    let vault_root = s
        .orgs
        .acme
        .backend
        .vault_root_id()
        .expect("the vault is a root");
    integration::scenario::pin(&s.orgs.acme, vault_root, "after saving a CardDAV account").await;
    s.orgs
        .acme
        .backend
        .admit_host(s.orgs.vnt.host_id(), Hosting::working());
    let peer_dir = tempfile::tempdir().expect("peer dir");
    let peer_tree = peer_dir.path().join("vault-copy");
    let peer_backend = files::FilesBackend::new(peer_dir.path(), peer_dir.path().join("vault"))
        .expect("peer backend");
    peer_backend
        .adopt_replica(
            vault_root.get(),
            "ACME Vault",
            peer_tree.to_str().expect("utf-8"),
            RootFlavor::Media,
        )
        .expect("adopt the replica");
    let replica = s.orgs.vnt.dial_replica(&s.orgs.acme).await;
    files_sync::reconcile(&peer_backend, &replica, vault_root.get())
        .await
        .expect("the peer pulls the vault");

    let copied = paths_under(&peer_tree);
    assert!(
        copied
            .iter()
            .any(|p| p.starts_with("Records/contacts/") && !p.contains(".sync")),
        "fixture: the peer got the contacts: {copied:?}"
    );
    assert!(
        !copied.iter().any(|p| p.contains("contacts/.sync")),
        "the account directory reached the peer: {copied:?}"
    );
    assert!(
        !every_byte_under(peer_dir.path()).contains(PASSWORD),
        "the password reached the peer"
    );
}

/// The password is in the credential store, so a sync works — and keeps
/// working after the server restarts with nothing in memory.
// t[verify contacts.secret.out-of-vault]
#[tokio::test]
async fn sync_resolves_the_reference_before_and_after_a_restart() {
    let s = Scenario::open().await;
    let fake = FakeCardDav::start(Host::Localhost, addressbook());
    let token = s.people.alice.token.clone();
    let contacts = s.as_alice().await.contacts().await;
    contacts
        .upsert_account(account(
            "it-restart",
            &format!("{}/dav/", fake.origin()),
            PASSWORD,
        ))
        .await
        .expect("save the account");

    let first = contacts
        .sync_account("it-restart".into())
        .await
        .expect("the first sync");
    assert_eq!(first.added, 2, "{first:?}");
    assert!(
        fake.requests_with_authorization() > 0,
        "fixture: the fake saw the sync"
    );

    let acme = s.orgs.acme.restart().await;
    let contacts = Session::open(&acme, token).await.contacts().await;
    let again = contacts
        .sync_account("it-restart".into())
        .await
        .expect("a sync after the restart");
    assert_eq!(again.skipped, 2, "{again:?}");
    let names: Vec<String> = contacts
        .list_contacts()
        .await
        .expect("list")
        .into_iter()
        .map(|c| c.full_name)
        .collect();
    assert!(names.iter().any(|n| n == "Ada Lovelace"), "{names:?}");
    assert!(!every_byte_under(&acme.org_root().join("vault")).contains(PASSWORD));
}

/// A server that redirects to another host: that host receives no
/// request with the password — none at all, in fact.
// t[verify contacts.transport.same-origin-credentials]
#[tokio::test]
async fn a_redirect_to_another_host_never_carries_the_password() {
    let s = Scenario::open().await;
    let elsewhere = FakeCardDav::start(Host::Loopback, Behaviour::default());
    let fake = FakeCardDav::start(
        Host::Localhost,
        Behaviour {
            everything_to: Some(elsewhere.origin().to_string()),
            ..addressbook()
        },
    );
    let contacts = s.as_alice().await.contacts().await;
    contacts
        .upsert_account(account(
            "it-redirect",
            &format!("{}/dav/", fake.origin()),
            PASSWORD,
        ))
        .await
        .expect("save the account");

    let refused = contacts.sync_account("it-redirect".into()).await;
    assert!(
        refused.is_err(),
        "the sync followed the redirect: {refused:?}"
    );
    assert!(
        !fake.requests().is_empty(),
        "fixture: the sync never started"
    );
    assert_eq!(
        elsewhere.requests_with_authorization(),
        0,
        "the password crossed to another host"
    );
    assert!(
        elsewhere.requests().is_empty(),
        "a cross-origin redirect was followed: {:?}",
        elsewhere.requests()
    );
}

/// An http origin is refused when the account is saved.
// t[verify contacts.transport.https-only]
#[tokio::test]
async fn an_http_origin_is_refused() {
    let s = Scenario::open().await;
    let contacts = s.as_alice().await.contacts().await;
    let refused = contacts
        .upsert_account(account(
            "it-http",
            "http://cloud.acme-audio.example/dav/",
            PASSWORD,
        ))
        .await;
    assert!(refused.is_err(), "an http origin was saved");
    let listed = contacts.list_accounts().await.expect("list");
    assert!(!listed.iter().any(|a| a.id == "it-http"));
    assert!(!every_byte_under(&s.orgs.acme.org_root().join("vault")).contains(PASSWORD));
}

/// Host absent (ADR 0006): two device nodes and no server. The node
/// holding the account syncs CardDAV itself, restarts and syncs again;
/// the other replicates its vault and gets the contacts, never the
/// account.
// t[verify contacts.secret.out-of-vault]
// t[verify vault.private.node-only]
#[tokio::test]
async fn two_devices_with_no_host_keep_the_account_on_the_node_that_has_it() {
    let fake = FakeCardDav::start(Host::Localhost, addressbook());

    // The holder's vault, with an account and the contacts it pulled.
    let dir = tempfile::tempdir().expect("holder dir");
    let notes = dir.path().join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    let open = |root: &Path| {
        contacts::VaultContacts::with_secrets(
            root,
            contacts::TestSecrets::shared(),
            contacts::testing::trusting_transport(),
        )
        .expect("contacts on the device")
    };
    let local = open(&notes);
    local
        .upsert_account(&account(
            "dev-studio",
            &format!("{}/dav/", fake.origin()),
            PASSWORD,
        ))
        .expect("save on the device");
    let report = local
        .sync_account("dev-studio".into())
        .expect("the device syncs on its own");
    assert_eq!(report.added, 2, "{report:?}");

    // A restart of the device: a new backend over the same disk.
    drop(local);
    let again = open(&notes)
        .sync_account("dev-studio".into())
        .expect("a sync after the device restarted");
    assert_eq!(again.skipped, 2, "{again:?}");

    // The vault becomes the device's root; the other device pulls it.
    let holder = Laptop::holding(dir, notes, "Notes").await;
    let receiver = Laptop::empty_peer(holder.album).await;
    holder
        .backend
        .admit_host(receiver.host_id(), Hosting::working());
    let peer = receiver.dial_replica(&holder.endpoint).await;
    files_sync::reconcile(&receiver.backend, &peer, holder.album.get())
        .await
        .expect("device-to-device pull");

    let copied = paths_under(&receiver.tree);
    assert_eq!(
        copied
            .iter()
            .filter(|p| p.starts_with("Records/contacts/"))
            .count(),
        2,
        "the synced contacts should be on the other device: {copied:?}"
    );
    assert!(
        !copied.iter().any(|p| p.contains(".sync")),
        "the account directory reached the other device: {copied:?}"
    );
    assert!(
        !every_byte_under(&receiver.tree).contains(PASSWORD),
        "the password reached the other device"
    );
    assert!(
        holder
            .tree
            .join("Records/contacts/.sync/dev-studio.md")
            .is_file(),
        "the holder still has its account"
    );
}
