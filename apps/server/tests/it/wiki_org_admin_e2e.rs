#![allow(clippy::large_futures)]
//! Who may grant Editor on a wiki (`wiki.edit.editor`), over the org
//! lane of a live node, from the org's real membership rows.
//!
//! A person creates a wiki and so is its only Editor. Beside them: an
//! org admin and the node's owner, neither an Editor, and a plain
//! member. The admin and the owner grant Editor; the member is refused;
//! an admin demoted to member stops granting at once. A bot is an org
//! admin only when its own membership row says `admin`.

use architect_auth::proto::AuthServiceClient;
use architect_auth::{CreateEmailPasswordUser, SignInEmailPassword};
use task_server::bots::{self, OrgChoice, Tools};
use task_server::local_node::{LocalNode, NodeOptions, PERSONAL_SLUG};
use uuid::Uuid;
use vox::VoxError;
use vox_core::FromVoxLane;
use wiki_live::backend::OrgRoles as _;
use wiki_proto::WikiError;
use wiki_proto::config::NewWiki;
use wiki_proto::service::edits::EditsClient;
use wiki_proto::service::registry::RegistryClient;

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
            user_agent: Some("wiki-org-admin-e2e".into()),
        })
        .await
        .expect("make a person");
    (s.user.id, s.token)
}

async fn edits(node: &LocalNode, token: &str) -> EditsClient {
    let lane = node
        .caller(PERSONAL_SLUG, Some(token.to_owned()))
        .await
        .expect("an org lane");
    EditsClient::from_vox_lane(lane, None)
}

async fn editors_of(client: &EditsClient, wiki: &str) -> Vec<String> {
    client
        .editors(wiki.to_owned())
        .await
        .expect("editors")
        .editors
}

fn refused<T: std::fmt::Debug>(r: Result<T, VoxError<WikiError>>) -> String {
    match r {
        Err(VoxError::User(e)) => match *e {
            WikiError::Refused(why) => why,
            other => panic!("expected a refusal, got {other:?}"),
        },
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// t[verify wiki.edit.editor]
#[tokio::test(flavor = "multi_thread")]
async fn an_org_admin_grants_editor_and_a_plain_member_cannot() {
    let _env = crate::support::env_lock().await;
    let root = tempfile::tempdir().unwrap();
    let node = LocalNode::open(NodeOptions {
        owner_name: "Cody Owner".into(),
        peers: false,
        ..NodeOptions::at(root.path())
    })
    .await
    .expect("the node opens");
    let personal = [PERSONAL_SLUG.to_owned()];

    // ── the cast ──
    let owner = node.owner().clone();
    let anon = node.caller(&owner.org, None).await.unwrap();
    let signed_in = AuthServiceClient::from_vox_lane(anon, None)
        .sign_in_email_password(SignInEmailPassword {
            email: owner.email.clone(),
            password: owner.password.clone(),
            ip_address: None,
            user_agent: Some("wiki-org-admin-e2e".into()),
        })
        .await
        .expect("the owner signs in");
    let (me, my_token) = (signed_in.user.id, signed_in.token);
    node.admit_account(me, &personal, "owner").await.unwrap();
    let (editor, editor_token) = person(&node, "editor@example.test").await;
    node.admit_account(editor, &personal, "member")
        .await
        .unwrap();
    let (admin, admin_token) = person(&node, "admin@example.test").await;
    node.admit_account(admin, &personal, "admin").await.unwrap();
    let (member, member_token) = person(&node, "member@example.test").await;
    node.admit_account(member, &personal, "member")
        .await
        .unwrap();

    // The editor makes the wiki, and is its one Editor.
    let lane = node
        .caller(PERSONAL_SLUG, Some(editor_token.clone()))
        .await
        .unwrap();
    let wiki = RegistryClient::from_vox_lane(lane, None)
        .create_wiki(NewWiki {
            title: "Bible Study".into(),
            ..Default::default()
        })
        .await
        .expect("the editor makes a wiki")
        .slug;
    let as_editor = edits(&node, &editor_token).await;
    assert_eq!(
        editors_of(&as_editor, &wiki).await,
        vec![editor.to_string()]
    );

    // A plain member is refused, for someone else and for themselves.
    let as_member = edits(&node, &member_token).await;
    for whom in [member, admin] {
        let why = refused(as_member.grant_editor(wiki.clone(), whom.to_string()).await);
        assert!(why.contains("org admin"), "{why}");
    }
    assert_eq!(
        editors_of(&as_editor, &wiki).await,
        vec![editor.to_string()]
    );

    // An org admin who is not an Editor grants Editor.
    let as_admin = edits(&node, &admin_token).await;
    as_admin
        .grant_editor(wiki.clone(), member.to_string())
        .await
        .expect("an org admin grants Editor");
    assert_eq!(
        editors_of(&as_editor, &wiki).await,
        vec![editor.to_string(), member.to_string()]
    );

    // So does the owner.
    edits(&node, &my_token)
        .await
        .grant_editor(wiki.clone(), me.to_string())
        .await
        .expect("the org's owner grants Editor");
    assert!(
        editors_of(&as_editor, &wiki)
            .await
            .contains(&me.to_string())
    );

    // Demoted to member, the admin stops granting on the next call.
    node.admit_account(admin, &personal, "member")
        .await
        .unwrap();
    let why = refused(as_admin.grant_editor(wiki.clone(), admin.to_string()).await);
    assert!(why.contains("org admin"), "{why}");
    assert!(
        !editors_of(&as_editor, &wiki)
            .await
            .contains(&admin.to_string())
    );
}

/// A bot holds `member` in the orgs it is given, and that is not admin;
/// only a row that says `admin` makes one an org admin.
// t[verify wiki.edit.editor]
#[tokio::test(flavor = "multi_thread")]
async fn a_bot_is_an_org_admin_only_when_its_row_says_admin() {
    let _env = crate::support::env_lock().await;
    let root = tempfile::tempdir().unwrap();
    let node = LocalNode::open(NodeOptions {
        owner_name: "Cody Owner".into(),
        peers: false,
        ..NodeOptions::at(root.path())
    })
    .await
    .expect("the node opens");
    let home = node.state().home_identity.clone().expect("a home org");
    let (me, _) = person(&node, "maker@example.test").await;
    node.admit_account(me, &[PERSONAL_SLUG.to_owned()], "owner")
        .await
        .unwrap();
    let atlas = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Atlas",
        &OrgChoice::Some(vec![PERSONAL_SLUG.to_owned()]),
        Tools::All,
        bots::Kind::Team,
    )
    .await
    .expect("Atlas is made");
    let roles = task_server::memberships::WikiRoles::new(home.memberships.clone(), PERSONAL_SLUG);
    let bot = atlas.bot.id.to_string();

    assert!(!roles.is_admin(&bot), "a bot's member row is not admin");
    assert!(!roles.is_admin("not-a-uuid"));
    assert!(!roles.is_admin(&Uuid::new_v4().to_string()), "no row");

    home.memberships
        .upsert(atlas.bot.id, PERSONAL_SLUG, Some("admin"))
        .await
        .unwrap();
    assert!(roles.is_admin(&bot), "an explicit admin row is admin");
}
