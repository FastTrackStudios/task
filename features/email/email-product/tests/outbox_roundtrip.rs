//! Outbox end to end over the in-process transport: submit →
//! approve → the poller delivers through the real maildir
//! backend (mock SMTP transport) → Sent copy on disk + the full
//! event trail on the one `EmailChange` stream.
//!
//! Mirrors the `task` events_stream harness: `LayerRouter` +
//! `LocalServer`, the subscribe call held in flight, hub
//! `subscriber_count` polled before mutating so no event is
//! missed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use email_proto::{
    Account, AccountId, Addr, Draft, EmailChange, EmailEvent, EmailProductClient, EmailSync,
    EmailSyncStreamClient, EmailSyncStreamSource, OutboxStatus, SeqRange,
};
use email_product::{ProductAccount, ProductBackend};

/// Recording mock transport: delivery "succeeds" without a wire.
struct MockSubmit {
    sent: Mutex<Vec<(String, Vec<String>)>>,
}

impl email_maildir::Submit for MockSubmit {
    fn submit_raw<'a>(
        &'a self,
        from: &'a str,
        recipients: &'a [String],
        _raw: &'a [u8],
        message_id: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        self.sent
            .lock()
            .unwrap()
            .push((from.to_string(), recipients.to_vec()));
        Box::pin(async move { Ok(message_id) })
    }
}

async fn next_change(rx: &mut vox::Rx<EmailChange>) -> EmailChange {
    let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for an EmailChange")
        .expect("event channel errored")
        .expect("event stream closed early");
    let mut copied = None;
    let _ = frame.map(|ev| copied = Some(ev));
    copied.expect("SelfRef::map ran")
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_approve_deliver_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let account = Account {
        id: AccountId("acct".into()),
        name: "acct".into(),
        address: "you@example.com".into(),
        display_name: None,
    };
    let mock = Arc::new(MockSubmit {
        sent: Mutex::new(Vec::new()),
    });
    let mailbox = email_maildir::Backend::with_configured_accounts([email_maildir::AccountEntry {
        account,
        root: dir.path().to_path_buf(),
        aliases: email_config::FolderAliases::new(),
        submit: Some(mock.clone()),
    }]);

    // The product backend shares the mailbox backend's hub, so
    // outbox events and the Sent-copy NewMessage ride one stream.
    let hub = mailbox.changes_hub().clone();
    let product = ProductBackend::new(
        [ProductAccount {
            id: "acct".into(),
            root: dir.path().to_path_buf(),
            address: "you@example.com".into(),
        }],
        Arc::new(mailbox.clone()),
        hub.clone(),
        Arc::new(email_product::NoContacts),
    )
    .expect("open product stores");
    let poller = product.spawn_poller(Duration::from_millis(25));

    let router = LayerRouter::new()
        .merge(email_proto::product_layer(product.clone()))
        .merge(email_proto::stream_layer(mailbox.clone()));
    let scope = Scope::new();
    let local = LocalServer::serve(router, scope.clone());

    let client: EmailProductClient = local.establish().await.expect("product client");
    let stream: EmailSyncStreamClient = local.establish().await.expect("stream client");

    // Subscribe before mutating; wait for the sink to attach.
    let (tx, mut rx) = vox::channel::<EmailChange>();
    let subscription = tokio::spawn(async move {
        stream.changes(tx).await.expect("subscribe to changes");
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while hub.subscriber_count() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("subscriber sink never reached the hub");

    // ── submit: agent drafts, entry is PendingApproval ────────
    let draft = Draft {
        from: Addr {
            name: None,
            email: "you@example.com".into(),
        },
        to: vec![Addr {
            name: None,
            email: "bob@example.com".into(),
        }],
        cc: vec![],
        bcc: vec![],
        subject: "Round trip".into(),
        body_text: "outbox e2e".into(),
        body_html: None,
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    };
    let staged = client
        .submit_draft("acct".into(), draft, "agent:test".into())
        .await
        .expect("submit_draft");
    assert_eq!(staged.status, OutboxStatus::PendingApproval);
    assert_eq!(staged.origin, "agent:test");

    match next_change(&mut rx).await.event {
        EmailEvent::OutboxChanged { id, status } => {
            assert_eq!(id, staged.id);
            assert_eq!(status, OutboxStatus::PendingApproval);
        }
        other => panic!("expected OutboxChanged(PendingApproval), got {other:?}"),
    }

    // Nothing delivered while pending — give the poller a beat.
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(mock.sent.lock().unwrap().is_empty(), "sent while pending");

    // ── approve: the human gate opens; poller delivers ────────
    let approved = client
        .approve("acct".into(), staged.id)
        .await
        .expect("approve");
    assert_eq!(approved.status, OutboxStatus::Approved);

    // Event trail: Approved → Sending → NewMessage(Sent copy) →
    // Sent. Collect until the terminal Sent shows up.
    let mut statuses = Vec::new();
    let mut sent_copy_folder = None;
    loop {
        let change = next_change(&mut rx).await;
        assert_eq!(change.account, "acct");
        match change.event {
            EmailEvent::OutboxChanged { id, status } => {
                assert_eq!(id, staged.id);
                statuses.push(status);
                if status == OutboxStatus::Sent {
                    break;
                }
            }
            EmailEvent::NewMessage { folder, .. } => sent_copy_folder = Some(folder),
            other => panic!("unexpected event {other:?}"),
        }
    }
    assert_eq!(
        statuses,
        vec![
            OutboxStatus::Approved,
            OutboxStatus::Sending,
            OutboxStatus::Sent
        ]
    );
    assert_eq!(sent_copy_folder.as_deref(), Some("Sent"));

    // The mock transport saw exactly one delivery.
    {
        let sent = mock.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "you@example.com");
        assert_eq!(sent[0].1, vec!["bob@example.com".to_string()]);
    }

    // The outbox row is terminal + carries the sent Message-ID…
    let entries = client.list_outbox("acct".into()).await.expect("list_outbox");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, OutboxStatus::Sent);
    let mid = entries[0]
        .sent_message_id
        .clone()
        .expect("sent_message_id recorded");

    // …and the Sent copy is really in the maildir.
    let envs = mailbox
        .fetch_envelopes("acct", "Sent", SeqRange::All)
        .expect("fetch Sent");
    assert_eq!(envs.len(), 1);
    assert_eq!(envs[0].subject, "Round trip");
    // Message-IDs are stored verbatim — backends may include or
    // omit the angle brackets (see `email_proto::MessageId`).
    assert_eq!(
        envs[0].message_id.trim_matches(['<', '>']),
        mid.trim_matches(['<', '>'])
    );

    // ── cancel path: a second staged draft never delivers ─────
    let draft2 = Draft {
        from: Addr {
            name: None,
            email: "you@example.com".into(),
        },
        to: vec![Addr {
            name: None,
            email: "eve@example.com".into(),
        }],
        cc: vec![],
        bcc: vec![],
        subject: "Never sent".into(),
        body_text: String::new(),
        body_html: None,
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    };
    let staged2 = client
        .submit_draft("acct".into(), draft2, "user".into())
        .await
        .expect("submit second draft");
    let cancelled = client
        .cancel("acct".into(), staged2.id)
        .await
        .expect("cancel");
    assert_eq!(cancelled.status, OutboxStatus::Cancelled);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(mock.sent.lock().unwrap().len(), 1, "cancelled draft sent");

    poller.abort();
    subscription.abort();
    scope.close().await;
}
