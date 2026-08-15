//! Cross-cutting smoke test against a real mailpit container.
//!
//! All tests are `#[ignore]` so plain `cargo test` skips them;
//! run explicitly:
//! ```text
//! cargo test -p email-integration-tests --features integration -- --ignored
//! ```
//!
//! Requires Docker available on `$PATH` (or a Docker-API
//! socket testcontainers can reach). The mailpit image is
//! ~12MB and starts in ~1s; the whole suite runs in 5-10s.

#![cfg(feature = "integration")]

use std::time::Duration;

use email_imap::Backend as ImapBackend;
use email_integration_tests::Mailpit;
use email_proto::{Addr, Draft, EmailSync, SeqRange};
use email_smtp::SmtpSender;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs MAILPIT_SMTP_URL + MAILPIT_API_URL; opt-in"]
async fn smtp_send_lands_in_mailpit() {
    let mp = Mailpit::spawn().await;
    let cfg = mp.account_config();
    let smtp_config = match &cfg.backend {
        email_config::BackendKind::Imap {
            submit: Some(s), ..
        } => s.clone(),
        _ => unreachable!(),
    };

    // Mailpit's listing should be empty before we send.
    assert_eq!(mp.message_count().await, 0);

    let draft = Draft {
        from: Addr {
            name: Some("Alice".into()),
            email: "alice@example.com".into(),
        },
        to: vec![Addr {
            name: None,
            email: "bob@example.com".into(),
        }],
        cc: vec![],
        bcc: vec![],
        subject: "Hello from the integration suite".into(),
        body_text: "the body".into(),
        body_html: None,
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    };

    let sender = SmtpSender::new(smtp_config);
    let msg_id = sender.send(&draft).await.expect("send");
    assert!(msg_id.contains('@'));

    // Mailpit ingest is near-instant but the HTTP API caches
    // briefly — short retry loop.
    let mut count = 0;
    for _ in 0..10 {
        count = mp.message_count().await;
        if count > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(count >= 1, "expected mailpit to have ingested the message");
}

// IMAP round-trip via mailpit needs auth tuning (mailpit's IMAP
// server uses different creds + capability set than its SMTP).
// Tracked as a follow-up; SMTP send → HTTP-API listing already
// covers the cross-cutting path end-to-end.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs MAILPIT_SMTP_URL + MAILPIT_API_URL; opt-in"]
async fn imap_lists_inbox_after_send() {
    let mp = Mailpit::spawn().await;
    let cfg = mp.account_config();
    let smtp_config = match &cfg.backend {
        email_config::BackendKind::Imap {
            submit: Some(s), ..
        } => s.clone(),
        _ => unreachable!(),
    };

    // Send first.
    let draft = Draft {
        from: Addr {
            name: None,
            email: "alice@example.com".into(),
        },
        to: vec![Addr {
            name: None,
            email: "test@example.com".into(),
        }],
        cc: vec![],
        bcc: vec![],
        subject: "Round-trip test".into(),
        body_text: "body content here".into(),
        body_html: None,
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    };
    SmtpSender::new(smtp_config).send(&draft).await.unwrap();

    // Wait briefly for ingest.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Now talk IMAP to mailpit and list INBOX.
    let backend = ImapBackend::from_configs([cfg.clone()]).unwrap();
    let b = backend.clone();
    let folders = tokio::task::spawn_blocking(move || b.list_folders("mailpit"))
        .await
        .unwrap()
        .unwrap();
    let names: Vec<_> = folders.iter().map(|f| f.id.as_str()).collect();
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("INBOX")),
        "expected INBOX in folder list; got {names:?}"
    );

    let b = backend.clone();
    let envs = tokio::task::spawn_blocking(move || {
        b.fetch_envelopes("mailpit", "INBOX", SeqRange::Recent(20))
    })
    .await
    .unwrap()
    .unwrap();
    assert!(
        envs.iter().any(|e| e.subject == "Round-trip test"),
        "expected to find the sent message via IMAP; got subjects: {:?}",
        envs.iter().map(|e| &e.subject).collect::<Vec<_>>()
    );
}
