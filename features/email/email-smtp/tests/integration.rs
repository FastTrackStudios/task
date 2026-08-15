//! End-to-end tests for `email-smtp::SmtpSender` against the
//! scripted SMTP mock in `mock_server.rs`. Requires the
//! `test-plaintext` feature (set by dev-dependencies).

#![cfg(feature = "test-plaintext")]

mod mock_server;

use email_config::{SmtpConfig, TlsMode};
use email_proto::{Addr, Draft};
use email_secret::Secret;
use email_smtp::SmtpSender;
use mock_server::MockServer;

fn smtp_config(addr: std::net::SocketAddr) -> SmtpConfig {
    SmtpConfig {
        host: addr.ip().to_string(),
        port: addr.port(),
        tls: TlsMode::None,
        username: "you".into(),
        password: Secret::raw("hunter2"),
    }
}

fn base_draft() -> Draft {
    Draft {
        from: Addr {
            name: Some("Alice".into()),
            email: "alice@example.com".into(),
        },
        to: vec![Addr {
            name: None,
            email: "bob@example.com".into(),
        }],
        cc: vec![Addr {
            name: None,
            email: "cc@example.com".into(),
        }],
        bcc: vec![Addr {
            name: None,
            email: "bcc@example.com".into(),
        }],
        subject: "Hello from the test".into(),
        body_text: "first line\nsecond line".into(),
        body_html: None,
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn send_completes_full_smtp_dance() {
    let server = MockServer::spawn().await;
    let sender = SmtpSender::new(smtp_config(server.addr));

    let message_id = sender.send(&base_draft()).await.unwrap();
    assert!(message_id.contains('@'), "expected an RFC2822-style id");

    let t = server.transcript.lock().await;
    assert!(t.contains("EHLO"), "expected EHLO; got: {:?}", t.lines);
    assert!(t.contains("AUTH PLAIN") || t.contains("AUTH LOGIN"));
    assert!(t.contains("MAIL FROM"));
    assert!(t.contains("RCPT TO:<bob@example.com>"));
    assert!(t.contains("RCPT TO:<cc@example.com>"));
    assert!(t.contains("RCPT TO:<bcc@example.com>"));
    assert!(t.contains("DATA"));
    // mail-send doesn't always issue QUIT before drop — connection
    // close is the spec'd alternative, so we don't assert here.
}

#[tokio::test(flavor = "multi_thread")]
async fn data_payload_contains_headers_and_body() {
    let server = MockServer::spawn().await;
    let sender = SmtpSender::new(smtp_config(server.addr));
    let _ = sender.send(&base_draft()).await.unwrap();

    let t = server.transcript.lock().await;
    let payload = t.payload_str();
    assert!(payload.contains("Subject: Hello from the test"));
    assert!(payload.contains("alice@example.com"));
    assert!(payload.contains("bob@example.com"));
    assert!(payload.contains("first line"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_draft_without_recipient() {
    // No mock needed — `build_message` fails before any wire I/O.
    let server = MockServer::spawn().await;
    let sender = SmtpSender::new(smtp_config(server.addr));
    let mut d = base_draft();
    d.to.clear();
    d.cc.clear();
    d.bcc.clear();
    let err = sender.send(&d).await.unwrap_err();
    assert!(
        matches!(err, email_smtp::SendError::Build(_)),
        "expected BuildError; got: {err}"
    );
}
