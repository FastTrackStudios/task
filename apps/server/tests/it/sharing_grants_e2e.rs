#![allow(clippy::large_futures)]
//! Sharing Grants, the owner's side (`docs/spec/sharing.md`), over the
//! org lane of a live node.
//!
//! A node's person owns the `personal` org and a project tree in it.
//! Beside them: a plain member, a signed-in person with no membership
//! row here (the guest — what a person of another org is to this one),
//! and a bot. Only the owner reaches the `sharing` service; everyone
//! else gets the same answer as for a grant that does not exist.

use architect_auth::proto::AuthServiceClient;
use architect_auth::{CreateEmailPasswordUser, SignInEmailPassword};
use project::ProjectService as _;
use share_proto::{
    GrantCapabilities, NewSharingGrant, SharingError, SharingServiceClient, not_shared_line,
};
use task_server::bots::{self, OrgChoice, Tools};
use task_server::local_node::{LocalNode, NodeOptions, PERSONAL_SLUG};
use task_server::sharing::{GrantEventKind, SharingGrant, is_live, now_unix};
use uuid::Uuid;
use vox::VoxError;
use vox_core::FromVoxLane;

async fn client(node: &LocalNode, bearer: Option<&str>) -> SharingServiceClient {
    let lane = node
        .caller(PERSONAL_SLUG, bearer.map(str::to_owned))
        .await
        .expect("an org lane");
    SharingServiceClient::from_vox_lane(lane, None)
}

/// The domain error behind a failed call.
fn refusal<T: std::fmt::Debug>(r: Result<T, VoxError<SharingError>>) -> SharingError {
    match r {
        Err(VoxError::User(e)) => *e,
        other => panic!("expected a SharingError, got {other:?}"),
    }
}

/// A person in the node's home account store, signed in.
async fn person(node: &LocalNode, email: &str) -> (Uuid, String) {
    let home = node.state().home_identity.clone().expect("a home org");
    let s = home
        .auth
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: email.into(),
            password: "correct-horse-battery-staple".into(),
            name: Some(email.into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: Some("sharing-e2e".into()),
        })
        .await
        .expect("make a person");
    (s.user.id, s.token)
}

fn project(node: &LocalNode, title: &str, parent: Option<Uuid>) -> Uuid {
    let org = node.state().org(PERSONAL_SLUG).expect("personal");
    org.projects
        .create(project::ProjectInfo {
            id: Uuid::new_v4(),
            title: title.into(),
            parent_id: parent,
            ..Default::default()
        })
        .expect("make a project")
        .id
}

fn ask(root: Uuid, subject: Uuid, digest: &str) -> NewSharingGrant {
    NewSharingGrant {
        root_id: root,
        subject,
        guest_org: "rockstars-of-tomorrow".into(),
        capabilities: GrantCapabilities::ReadComment,
        expires_at: None,
        preview_digest: digest.into(),
    }
}

// t[verify sharing.grant.owner-only]
// t[verify sharing.grant.preview]
// t[verify sharing.grant.audit]
#[tokio::test(flavor = "multi_thread")]
async fn only_an_owner_grants_and_everyone_else_finds_nothing() {
    let _env = crate::support::env_lock().await;
    let root = tempfile::tempdir().unwrap();
    let node = LocalNode::open(NodeOptions {
        owner_name: "Cody Owner".into(),
        peers: false,
        ..NodeOptions::at(root.path())
    })
    .await
    .expect("the node opens");

    // ── the cast ──
    let owner = node.owner().clone();
    let anon = node.caller(&owner.org, None).await.unwrap();
    let signed_in = AuthServiceClient::from_vox_lane(anon, None)
        .sign_in_email_password(SignInEmailPassword {
            email: owner.email.clone(),
            password: owner.password.clone(),
            ip_address: None,
            user_agent: Some("sharing-e2e".into()),
        })
        .await
        .expect("the owner signs in");
    let me = signed_in.user.id;
    let personal = [PERSONAL_SLUG.to_owned()];
    node.admit_account(me, &personal, "owner").await.unwrap();

    let (member, member_token) = person(&node, "member@example.test").await;
    node.admit_account(member, &personal, "member")
        .await
        .unwrap();
    let (guest, guest_token) = person(&node, "guest@rockstars.test").await;

    let home = node.state().home_identity.clone().unwrap();
    let bot = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Atlas",
        &OrgChoice::Some(personal.to_vec()),
        Tools::All,
        bots::Kind::Team,
    )
    .await
    .expect("a bot");
    // A bot is a member; make this one an admin by hand, the way an
    // operator could, and it is still refused — a grant is a person's.
    node.admit_account(bot.bot.id, &personal, "admin")
        .await
        .unwrap();

    let crescendum = project(&node, "Crescendum", None);
    let song = project(&node, "Song One", Some(crescendum));
    let elsewhere = project(&node, "Somebody Else's Album", None);

    let owner_c = client(&node, Some(&signed_in.token)).await;

    // ── preview: the exact set, its digest, what stays home ──
    let preview = owner_c
        .preview_grant(crescendum, guest)
        .await
        .expect("preview");
    let ids: Vec<&str> = preview.visible.iter().map(|v| v.id.as_str()).collect();
    assert!(ids.contains(&crescendum.to_string().as_str()));
    assert!(
        ids.contains(&song.to_string().as_str()),
        "sub-projects ride along"
    );
    assert!(
        !ids.contains(&elsewhere.to_string().as_str()),
        "other projects stay out"
    );
    assert_eq!(preview.digest.len(), 64);
    assert_eq!(
        format!("not shared: {}", preview.not_shared.join(", ")),
        not_shared_line()
    );
    assert_eq!(
        not_shared_line(),
        "not shared: pricing, invoices, timer, contacts"
    );

    // ── a digest the owner did not see is refused ──
    let wrong = refusal(
        owner_c
            .create_grant(ask(crescendum, guest, &"0".repeat(64)))
            .await,
    );
    assert_eq!(
        wrong,
        SharingError::PreviewChanged {
            digest: preview.digest.clone()
        }
    );
    // …and so is one the subtree has moved past since the preview.
    let stale = preview.digest.clone();
    let _track = project(&node, "Song Two", Some(crescendum));
    let moved = refusal(owner_c.create_grant(ask(crescendum, guest, &stale)).await);
    let SharingError::PreviewChanged { digest: fresh } = moved else {
        panic!("expected PreviewChanged, got {moved:?}");
    };
    assert_ne!(fresh, stale);
    // A member of this org is not a grant subject.
    let to_member = refusal(owner_c.create_grant(ask(crescendum, member, &fresh)).await);
    assert!(
        matches!(to_member, SharingError::Invalid(_)),
        "{to_member:?}"
    );

    // ── the owner creates, lists, revokes ──
    let grant = owner_c
        .create_grant(ask(crescendum, guest, &fresh))
        .await
        .expect("the owner grants");
    assert!(grant.live);
    assert_eq!(grant.owner_org, PERSONAL_SLUG);
    assert_eq!(grant.subject, guest);
    assert_eq!(grant.granted_by, me);
    assert_eq!(grant.preview_digest, fresh);
    assert_eq!(grant.revoked_at, None);
    let listed = owner_c.list_grants().await.expect("list");
    assert_eq!(listed, vec![grant.clone()]);

    // ── everyone else: absent, on every method ──
    let unknown = refusal(owner_c.revoke_grant(Uuid::new_v4()).await);
    assert_eq!(
        unknown,
        SharingError::NotFound,
        "an unknown grant is absent"
    );
    for (who, token) in [
        ("member", Some(member_token.as_str())),
        ("guest holding the grant", Some(guest_token.as_str())),
        ("bot", Some(bot.key.as_str())),
        ("anonymous", None),
    ] {
        let c = client(&node, token).await;
        let answers = [
            format!("{:?}", c.list_grants().await),
            format!("{:?}", c.revoke_grant(grant.id).await),
            format!("{:?}", c.create_grant(ask(crescendum, guest, &fresh)).await),
            format!("{:?}", c.preview_grant(crescendum, guest).await),
        ];
        for a in &answers {
            // Either the gate refused outright, or the service answered
            // exactly what it answers for a grant that does not exist.
            assert!(
                a == &format!(
                    "{:?}",
                    Err::<(), _>(VoxError::User(Box::new(SharingError::NotFound)))
                ) || a.contains("InvalidPayload"),
                "{who} got {a}"
            );
            assert!(!a.contains("Ok("), "{who} was answered: {a}");
        }
    }
    // Nothing they did changed anything.
    assert_eq!(owner_c.list_grants().await.unwrap(), vec![grant.clone()]);

    let revoked = owner_c.revoke_grant(grant.id).await.expect("revoke");
    assert!(!revoked.live);
    assert_eq!(revoked.revoked_by, Some(me));
    let again = owner_c.revoke_grant(grant.id).await.expect("revoke again");
    assert_eq!(again, revoked, "revoke is idempotent");

    // ── the audit trail ──
    let org = node.state().org(PERSONAL_SLUG).unwrap();
    let events = org.sharing.store.events(grant.id).await.unwrap();
    let kinds: Vec<_> = events.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        vec![GrantEventKind::Created, GrantEventKind::Revoked]
    );
    for e in &events {
        assert_eq!(e.subject, guest);
        assert_eq!(e.root_id, crescendum);
        assert_eq!(e.capabilities, GrantCapabilities::ReadComment);
        assert_eq!(e.preview_digest, fresh);
        assert_eq!(e.actor, Some(me));
    }

    // ── an expiry in the past is not live, and is audited once ──
    let now = now_unix();
    let lapsed = SharingGrant {
        id: Uuid::new_v4(),
        owner_org: PERSONAL_SLUG.into(),
        root_id: crescendum,
        subject: guest,
        guest_org: "rockstars-of-tomorrow".into(),
        capabilities: GrantCapabilities::ReadComment,
        expires_at: Some(now - 60),
        granted_by: me,
        granted_at: now - 3_600,
        revoked_at: None,
        revoked_by: None,
        preview_digest: fresh.clone(),
    };
    assert!(!is_live(&lapsed, now));
    org.sharing.store.insert(&lapsed).await.unwrap();
    let past = refusal(
        owner_c
            .create_grant(NewSharingGrant {
                expires_at: Some(now - 1),
                ..ask(crescendum, guest, &fresh)
            })
            .await,
    );
    assert!(matches!(past, SharingError::Invalid(_)), "{past:?}");
    for _ in 0..2 {
        let all = owner_c.list_grants().await.unwrap();
        let row = all.iter().find(|g| g.id == lapsed.id).expect("listed");
        assert!(!row.live, "a lapsed grant is not live");
    }
    let kinds: Vec<_> = org
        .sharing
        .store
        .events(lapsed.id)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.kind, e.actor))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (GrantEventKind::Created, Some(me)),
            (GrantEventKind::Expired, None)
        ]
    );
}

// t[verify wiki.access.source-grant]
// t[verify wiki.access.shelf-visibility]
#[cfg(feature = "plugin-wiki")]
#[tokio::test(flavor = "multi_thread")]
async fn only_an_owner_opens_a_shelf_to_another_org_and_every_change_is_on_record() {
    use share_proto::{NewSourceGrant, SourceTier, SourceVisibility};
    use wiki_live::source_access::{self, AccessGrants};
    use wiki_proto::config::Visibility;
    use wiki_proto::subscription::SourceKind;

    let _env = crate::support::env_lock().await;
    let root = tempfile::tempdir().unwrap();
    let node = LocalNode::open(NodeOptions {
        owner_name: "Cody Owner".into(),
        peers: false,
        ..NodeOptions::at(root.path())
    })
    .await
    .expect("the node opens");

    let owner = node.owner().clone();
    let anon = node.caller(&owner.org, None).await.unwrap();
    let signed_in = AuthServiceClient::from_vox_lane(anon, None)
        .sign_in_email_password(SignInEmailPassword {
            email: owner.email.clone(),
            password: owner.password.clone(),
            ip_address: None,
            user_agent: Some("sharing-e2e".into()),
        })
        .await
        .expect("the owner signs in");
    let me = signed_in.user.id;
    let personal = [PERSONAL_SLUG.to_owned()];
    node.admit_account(me, &personal, "owner").await.unwrap();
    let (member, member_token) = person(&node, "member@example.test").await;
    node.admit_account(member, &personal, "member")
        .await
        .unwrap();

    let org_root = node
        .state()
        .org(PERSONAL_SLUG)
        .unwrap()
        .sharing
        .org_root
        .clone();
    let songs = org_root.join("assets").join("songs");
    std::fs::create_dir_all(&songs).unwrap();
    let ask = || NewSourceGrant {
        tier: SourceTier::Assets,
        slug: "songs".into(),
        subscriber: "rockstars-of-tomorrow".into(),
    };
    let grants = AccessGrants::open(&org_root);

    // ── a plain member, and nobody at all, can mint nothing ──
    for (who, token) in [("member", Some(member_token.as_str())), ("anonymous", None)] {
        let c = client(&node, token).await;
        let answers = [
            format!("{:?}", c.grant_source(ask()).await),
            format!(
                "{:?}",
                c.set_source_visibility(
                    SourceTier::Assets,
                    "songs".into(),
                    SourceVisibility::Public
                )
                .await
            ),
            format!("{:?}", c.list_source_grants().await),
            format!("{:?}", c.revoke_source_grant(Uuid::new_v4()).await),
        ];
        for a in &answers {
            assert!(
                a == &format!(
                    "{:?}",
                    Err::<(), _>(VoxError::User(Box::new(SharingError::NotFound)))
                ) || a.contains("InvalidPayload"),
                "{who} got {a}"
            );
        }
    }
    assert!(grants.list().unwrap().is_empty(), "nothing was written");
    assert_eq!(
        source_access::visibility(&org_root.join("assets"), &songs),
        Visibility::Private,
        "the shelf is still private"
    );

    // ── the owner grants, and the row names them ──
    let owner_c = client(&node, Some(&signed_in.token)).await;
    let minted = owner_c.grant_source(ask()).await.expect("the owner grants");
    assert!(minted.grant.live);
    assert_eq!(minted.grant.owner_org, PERSONAL_SLUG);
    assert_eq!(minted.grant.granted_by, me.to_string());
    assert!(!minted.secret.is_empty());
    assert!(grants.admits(SourceKind::Assets, "songs", "rockstars-of-tomorrow"));
    assert_eq!(
        owner_c.list_source_grants().await.unwrap(),
        vec![minted.grant.clone()]
    );
    // A second live grant for the same pair is refused, not merged.
    assert!(matches!(
        refusal(owner_c.grant_source(ask()).await),
        SharingError::Invalid(_)
    ));
    // Not to itself, and not of something it does not hold.
    assert!(matches!(
        refusal(
            owner_c
                .grant_source(NewSourceGrant {
                    subscriber: PERSONAL_SLUG.into(),
                    ..ask()
                })
                .await
        ),
        SharingError::Invalid(_)
    ));
    assert_eq!(
        refusal(
            owner_c
                .grant_source(NewSourceGrant {
                    slug: "nonesuch".into(),
                    ..ask()
                })
                .await
        ),
        SharingError::NotFound
    );

    // ── visibility, by the owner ──
    owner_c
        .set_source_visibility(
            SourceTier::Assets,
            "songs".into(),
            SourceVisibility::Unlisted,
        )
        .await
        .expect("the owner sets visibility");
    assert_eq!(
        source_access::visibility(&org_root.join("assets"), &songs),
        Visibility::Unlisted
    );

    // ── revoke: once, then idempotent ──
    let revoked = owner_c
        .revoke_source_grant(minted.grant.id)
        .await
        .expect("revoke");
    assert!(!revoked.live);
    assert_eq!(revoked.revoked_by, Some(me.to_string()));
    let again = owner_c
        .revoke_source_grant(minted.grant.id)
        .await
        .expect("revoke again");
    assert_eq!(again, revoked);
    assert!(!grants.admits(SourceKind::Assets, "songs", "rockstars-of-tomorrow"));

    // ── the record: append-only, and never the secret ──
    let log = std::fs::read_to_string(org_root.join("subscribed/.state/access-grants.jsonl"))
        .expect("the log");
    let events: Vec<&str> = log
        .lines()
        .map(|l| {
            l.split("\"event\":\"")
                .nth(1)
                .and_then(|r| r.split('"').next())
                .unwrap_or("?")
        })
        .collect();
    assert_eq!(events, vec!["granted", "visibility", "revoked"], "{log}");
    assert!(!log.contains(&minted.secret));
}
