//! Baseline + alert-once over a real maildir account: the first
//! background pass over pre-existing mail fires nothing; a
//! genuinely-new message gets exactly one notification mark,
//! drained via `unnotified` / `mark_notified` — the surface the
//! notifications system consumes.

use std::sync::Arc;

use email_product::{NoContacts, ProductAccount, ProductBackend};
use email_proto::{Account, AccountId, EmailProduct, EmailSyncStreamSource};

fn write_msg(dir: &std::path::Path, name: &str, msgid: &str, subject: &str) {
    std::fs::write(
        dir.join(name),
        format!(
            "Message-ID: {msgid}\r\n\
             From: alice@example.com\r\n\
             To: you@example.com\r\n\
             Subject: {subject}\r\n\
             Date: Mon, 14 Nov 2023 12:00:00 +0000\r\n\
             \r\n\
             body\r\n"
        ),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn baseline_silences_history_then_marks_new_mail_once() {
    let dir = tempfile::tempdir().unwrap();
    let account = Account {
        id: AccountId("acct".into()),
        name: "acct".into(),
        address: "you@example.com".into(),
        display_name: None,
    };
    let mailbox = email_maildir::Backend::single(account, dir.path().to_path_buf()).unwrap();

    // Two messages of pre-existing history.
    write_msg(
        &dir.path().join("new"),
        "1700000000.M1.h",
        "<old1@x>",
        "Old one",
    );
    write_msg(
        &dir.path().join("new"),
        "1700000001.M2.h",
        "<old2@x>",
        "Old two",
    );

    let product = ProductBackend::new(
        [ProductAccount {
            id: "acct".into(),
            root: dir.path().to_path_buf(),
            address: "you@example.com".into(),
        }],
        Arc::new(mailbox.clone()),
        mailbox.changes_hub().clone(),
        Arc::new(NoContacts),
    )
    .unwrap();

    // First pass: baseline — years of mail, zero notifications.
    product.background_pass_once().await;
    assert!(product.unnotified("acct", 10).unwrap().is_empty());

    // New mail arrives.
    write_msg(
        &dir.path().join("new"),
        "1700000002.M3.h",
        "<fresh@x>",
        "Fresh",
    );
    product.background_pass_once().await;
    let pending = product.unnotified("acct", 10).unwrap();
    assert_eq!(
        pending,
        vec!["fresh@x".to_string()],
        "parser strips message-id brackets"
    );

    // Repeated passes never re-mark it.
    product.background_pass_once().await;
    assert_eq!(product.unnotified("acct", 10).unwrap().len(), 1);

    // The notifier drains it; the mark never returns.
    let flipped = product
        .mark_notified("acct", vec!["fresh@x".to_string()])
        .unwrap();
    assert_eq!(flipped, 1);
    assert!(product.unnotified("acct", 10).unwrap().is_empty());
    product.background_pass_once().await;
    assert!(product.unnotified("acct", 10).unwrap().is_empty());

    // Draining twice reports zero flips (idempotent).
    assert_eq!(
        product
            .mark_notified("acct", vec!["fresh@x".to_string()])
            .unwrap(),
        0
    );

    // The triage pass ran too — the fresh message has derivations.
    let derivs = product
        .derivations("acct", vec!["fresh@x".to_string()])
        .unwrap();
    assert!(!derivs.is_empty(), "triage rows for the fresh message");
}
