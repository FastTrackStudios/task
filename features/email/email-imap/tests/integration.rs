//! End-to-end integration tests for `email-imap::Backend`
//! against the scripted mock server in `mock_server.rs`.
//!
//! Requires the `test-plaintext` feature (set by `dev-dependencies`
//! in Cargo.toml — `cargo test -p email-imap` picks it up
//! automatically).
//!
//! Each test asserts both the **observable result** (folders
//! listed, envelopes returned, etc) AND the **wire transcript**
//! the mock saw — so a regression in `Backend` that produces
//! the wrong IMAP command shape fails the transcript assertion
//! before the result one.

#![cfg(feature = "test-plaintext")]

mod mock_server;

use email_config::{AccountConfig, BackendKind, FolderAliases, TlsMode};
use email_imap::Backend;
use email_proto::{AccountId, EmailSync, FlagDelta, SeqRange};
use email_secret::Secret;
use mock_server::MockServer;

fn config(addr: std::net::SocketAddr, aliases: FolderAliases) -> AccountConfig {
    AccountConfig {
        id: AccountId("test".into()),
        name: "test".into(),
        address: "you@example.com".into(),
        display_name: None,
        backend: BackendKind::Imap {
            host: addr.ip().to_string(),
            port: addr.port(),
            tls: TlsMode::None,
            username: "you".into(),
            password: Secret::raw("hunter2"),
            submit: None,
        },
        signature: None,
        folder_aliases: aliases,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn list_folders_round_trips_through_mock() {
    let server = MockServer::spawn(1).await;
    let backend = Backend::from_configs([config(server.addr, FolderAliases::new())]).unwrap();
    // The trait method `block_on`s internally; call it from a
    // blocking task so we don't try to nest runtimes.
    let b = backend.clone();
    let folders = tokio::task::spawn_blocking(move || b.list_folders("test"))
        .await
        .unwrap()
        .unwrap();

    let names: Vec<&str> = folders.iter().map(|f| f.id.as_str()).collect();
    assert!(names.contains(&"INBOX"), "got: {names:?}");
    assert!(names.contains(&"Sent"), "got: {names:?}");
    assert!(names.contains(&"Drafts"), "got: {names:?}");

    let t = server.transcript.lock().await;
    assert!(t.contains("LOGIN"), "expected LOGIN; got: {:?}", t.lines);
    assert!(t.contains("LIST"), "expected LIST; got: {:?}", t.lines);
    assert!(t.contains("LOGOUT"), "expected LOGOUT; got: {:?}", t.lines);
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_envelopes_parses_mock_headers() {
    let server = MockServer::spawn(1).await;
    let backend = Backend::from_configs([config(server.addr, FolderAliases::new())]).unwrap();
    let b = backend.clone();
    let envs =
        tokio::task::spawn_blocking(move || b.fetch_envelopes("test", "INBOX", SeqRange::All))
            .await
            .unwrap()
            .unwrap();

    // The mock serves two envelopes.
    assert_eq!(envs.len(), 2);
    let subjects: Vec<&str> = envs.iter().map(|e| e.subject.as_str()).collect();
    assert!(subjects.contains(&"First"));
    assert!(subjects.contains(&"Second"));

    let t = server.transcript.lock().await;
    assert!(t.contains("SELECT"));
    assert!(t.contains("UID FETCH"));
    assert!(
        t.contains("BODY.PEEK[HEADER]"),
        "expected the backend to use BODY.PEEK[HEADER] for envelopes; got: {:?}",
        t.lines
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn folder_alias_translates_on_the_wire() {
    let server = MockServer::spawn(1).await;
    let mut aliases = FolderAliases::new();
    // UI sees "Sent"; backend (mock) actually serves it under
    // the same name in this fixture — but the alias still
    // routes through the resolver, so the wire SELECT is the
    // backend name, never the alias.
    aliases.insert("MyArchive", "Sent");
    let backend = Backend::from_configs([config(server.addr, aliases)]).unwrap();
    let b = backend.clone();
    let _ =
        tokio::task::spawn_blocking(move || b.fetch_envelopes("test", "MyArchive", SeqRange::All))
            .await
            .unwrap()
            .unwrap();

    let t = server.transcript.lock().await;
    assert!(
        t.contains("SELECT \"Sent\"") || t.contains("SELECT Sent"),
        "expected SELECT Sent (alias-resolved); got: {:?}",
        t.lines
    );
    assert!(
        !t.contains("MyArchive"),
        "alias name should never reach the wire; got: {:?}",
        t.lines
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_flags_issues_uid_store() {
    // locate_uid scans every folder via uid_search; the mock
    // server's SEARCH returns a hit on every call, so the first
    // folder (INBOX) wins. That means one full LIST + one
    // SELECT + one SEARCH connection, then a fresh connection
    // for the actual STORE. Mock spawns enough cycles.
    let server = MockServer::spawn(4).await;
    let backend = Backend::from_configs([config(server.addr, FolderAliases::new())]).unwrap();
    let b = backend.clone();
    tokio::task::spawn_blocking(move || {
        b.set_flags(
            "test",
            "<m1@mock>",
            FlagDelta {
                add: vec!["\\Answered".into()],
                remove: vec![],
            },
        )
    })
    .await
    .unwrap()
    .unwrap();

    let t = server.transcript.lock().await;
    assert!(
        t.contains("UID SEARCH"),
        "expected UID SEARCH from locate_uid; got: {:?}",
        t.lines
    );
    assert!(
        t.contains("UID STORE"),
        "expected UID STORE from set_flags; got: {:?}",
        t.lines
    );
    assert!(
        t.contains("+FLAGS"),
        "expected +FLAGS direction; got: {:?}",
        t.lines
    );
    assert!(
        t.contains("\\Answered"),
        "expected \\Answered in the store args; got: {:?}",
        t.lines
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_message_does_store_and_expunge() {
    let server = MockServer::spawn(4).await;
    let backend = Backend::from_configs([config(server.addr, FolderAliases::new())]).unwrap();
    let b = backend.clone();
    tokio::task::spawn_blocking(move || b.delete_message("test", "<m1@mock>"))
        .await
        .unwrap()
        .unwrap();

    let t = server.transcript.lock().await;
    assert!(
        t.contains("\\Deleted"),
        "expected \\Deleted in STORE args; got: {:?}",
        t.lines
    );
    assert!(
        t.contains("UID EXPUNGE"),
        "expected UID EXPUNGE; got: {:?}",
        t.lines
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn append_draft_writes_to_drafts_folder() {
    let server = MockServer::spawn(1).await;
    let backend = Backend::from_configs([config(server.addr, FolderAliases::new())]).unwrap();

    let draft = email_proto::Draft {
        from: email_proto::Addr {
            name: Some("Me".into()),
            email: "you@example.com".into(),
        },
        to: vec![email_proto::Addr {
            name: None,
            email: "bob@example.com".into(),
        }],
        cc: vec![],
        bcc: vec![],
        subject: "Test draft".into(),
        body_text: "body".into(),
        body_html: None,
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    };
    let b = backend.clone();
    let message_id = tokio::task::spawn_blocking(move || b.append_draft("test", draft))
        .await
        .unwrap()
        .unwrap();
    assert!(!message_id.is_empty(), "expected a fresh Message-ID");

    let t = server.transcript.lock().await;
    assert!(t.contains("APPEND"), "expected APPEND; got: {:?}", t.lines);
    assert!(
        t.contains("Drafts"),
        "expected APPEND target Drafts; got: {:?}",
        t.lines
    );
    assert!(
        t.contains("\\Draft"),
        "expected the \\Draft flag in APPEND args; got: {:?}",
        t.lines
    );
}
