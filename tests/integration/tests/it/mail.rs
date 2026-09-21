//! The chapter where mail becomes work.
//!
//! A message arrives in somebody's mailbox and the thing it is *about*
//! lives somewhere else — a project at the company they work for, a
//! project at the company they work *with*. `EmailLinks` is what joins
//! the two, and the joining is the whole feature: without it a mailbox
//! is a pile of text nobody can ask a question of.
//!
//! # Why this needs no mail server
//!
//! An account here is a **maildir** — a directory of files
//! (`email_config::BackendKind::Maildir`). So the seeded world can
//! carry a mailbox the way it carries a project, and every assertion
//! below runs on a laptop with no network, no container and no
//! credentials. The `email-integration-tests` crate covers the other
//! half, IMAP and SMTP against a real mailpit, and it is Docker-gated
//! and `#[ignore]`d for exactly the reason this file is not.
//!
//! # Why the mailbox is in a third org
//!
//! Mail belongs to a person; projects belong to organisations. Alice's
//! mailbox is planted in `alice-personal` — her own org, the same one
//! that holds her Bible Study and Cooking wikis — and never in ACME's.
//! That is not tidiness, it is the access rule: a link is stored in the
//! org the *caller* owns, so "every email on this project" answers for
//! the person who filed it and is empty for everybody else, with no
//! per-person rules to enforce. The last test here is that claim, and
//! it is the one that would be expensive to get wrong.

use email_proto::{LinkTarget, SeqRange};

use integration::client::Session;
use integration::scenario::Scenario;

/// Alice's own organisation — where her mailbox lives, and with it the
/// links she makes from it. The same org `projects_tier.rs` and
/// `song_library.rs` boot as the guest, so the three chapters describe
/// one world rather than three.
const PERSONAL_ORG: &str = "alice-personal";

/// The seeded account id — the directory name under `<org>/mail/`.
const ACCOUNT: &str = "alice";

/// Two of the three seeded messages, by Message-ID. Bare, without the
/// angle brackets, which is how both `mail-parser` reports them and
/// how `MessageLink` stores them — if those two ever disagreed, every
/// link in the system would silently stop resolving, so the chapter
/// spells the ids out rather than reading one from the other.
const FROM_CLIENT: &str = "album-master-v3@example-client.test";
const FROM_VNT: &str = "shared-project-cut@vnt.video";

/// The generated vox clients take owned strings where the trait reads
/// `&str` — one `.to_owned()` per argument otherwise, and the
/// assertions are what this file is for.
fn owned(v: &str) -> String {
    v.to_owned()
}

/// Boot Alice's personal org beside ACME and sign her into it.
///
/// Beside rather than inside: her mailbox and ACME's projects are on
/// two different orgs on purpose, and a helper that quietly put them on
/// one would make every assertion below vacuous.
async fn personal(s: &Scenario) -> (integration::server::Server, Session) {
    let org = s
        .orgs
        .acme
        .start_beside("Alice Personal", PERSONAL_ORG, |_| {})
        .await;
    let owner = integration::people::account(&org, "alice@alice.test", "Alice").await;
    let session = Session::open(&org, owner.token.clone()).await;
    (org, session)
}

fn project_in(org: &str, id: &str) -> LinkTarget {
    LinkTarget {
        kind: "project".into(),
        id: id.into(),
        org: org.into(),
    }
}

/// The seeded mailbox is a mailbox: an account, a folder, and messages
/// that parse.
///
/// The least interesting test here and the one worth running first. If
/// the maildir the seed commits is not readable as a maildir — a
/// missing `new/`, a filename git mangled, headers the parser refuses —
/// then everything below fails for a reason that has nothing to do with
/// linking, and this says so in one line.
#[tokio::test]
async fn the_seeded_mailbox_is_readable_over_the_wire() {
    let s = Scenario::open().await;
    let (_org, alice) = personal(&s).await;

    let accounts = alice.email().await.accounts().await.expect("accounts");
    let account = accounts
        .iter()
        .find(|a| a.id.0 == ACCOUNT)
        .expect("the seeded account is served");
    assert_eq!(
        account.address, "alice@alice.test",
        "the address comes from account.json, not from the directory name"
    );

    let envelopes = alice
        .email()
        .await
        .fetch_envelopes(owned(ACCOUNT), owned("INBOX"), SeqRange::All)
        .await
        .expect("read the inbox");
    assert_eq!(
        envelopes.len(),
        3,
        "three seeded messages; got {:?}",
        envelopes.iter().map(|e| &e.subject).collect::<Vec<_>>()
    );

    let from_client = envelopes
        .iter()
        .find(|e| e.message_id == FROM_CLIENT)
        .expect("the client's message, by its real Message-ID");
    assert_eq!(from_client.subject, "Album master — v3 notes");
}

/// A message is attached to a project, and the project can be asked
/// what mail is on it.
///
/// Both directions, because they are two lookups and only one of them
/// is the one a person does: `links_for_target` is "every email on this
/// project", `links_for_message` is "what is this email about". A store
/// that answered one and not the other would pass half a feature.
#[tokio::test]
async fn a_message_links_to_a_project_and_the_project_can_be_asked_for_its_mail() {
    let s = Scenario::open().await;
    let (_org, alice) = personal(&s).await;

    // The project this message is about, on ACME's disk — reached by
    // the id the projects lane gives it, not by a name written here.
    //
    // First Single and not Example Album, though the message is about
    // the album: the album's page is written by `admin demo`'s
    // reconciliation, and this suite boots from `example_org::install`
    // alone, so in *this* world the album is a directory of tracks with
    // no project page and `list()` is right not to return it. First
    // Single carries a committed `project.md`, so it exists wherever
    // the seed is planted.
    let project = s
        .as_alice()
        .await
        .projects()
        .await
        .list()
        .await
        .expect("ACME's projects")
        .into_iter()
        .find(|p| p.title == "First Single")
        .expect("the seeded single, whose page is committed");

    let links = alice.email_links().await;
    let target = project_in("acme-audio", &project.id.to_string());
    let made = links
        .link(owned(FROM_CLIENT), target.clone(), owned("alice"))
        .await
        .expect("link the message to the album");
    assert_eq!(made.message_id, FROM_CLIENT);
    assert_eq!(made.linked_by, "alice");

    let on_project = links
        .links_for_target(target.clone())
        .await
        .expect("every email on this project");
    assert_eq!(on_project.len(), 1, "one message filed against the album");
    assert_eq!(on_project[0].message_id, FROM_CLIENT);

    let about_message = links
        .links_for_message(owned(FROM_CLIENT))
        .await
        .expect("what this email is about");
    assert_eq!(about_message.len(), 1);
    assert_eq!(about_message[0].target, target);

    // Idempotent, because a rule that runs twice must be harmless.
    links
        .link(owned(FROM_CLIENT), target.clone(), owned("rule"))
        .await
        .expect("re-linking the same pair");
    assert_eq!(
        links
            .links_for_target(target)
            .await
            .expect("still one")
            .len(),
        1,
        "re-linking duplicated the row instead of updating it"
    );
}

/// The link points at the message, not at where the message sat.
///
/// This is the fix in #113 and the reason the key is a Message-ID
/// rather than a `(folder, uid)` pair. Filing a message is the single
/// most ordinary thing anyone does to their mail, and a link that broke
/// on it would break on the first day of use — quietly, because nothing
/// errors: the row is still there, it just stops matching anything.
///
/// So: link it in the INBOX, archive it, and ask again.
#[tokio::test]
async fn archiving_a_message_does_not_break_what_it_was_linked_to() {
    let s = Scenario::open().await;
    let (_org, alice) = personal(&s).await;

    let links = alice.email_links().await;
    let target = project_in("acme-audio", "album-under-test");
    links
        .link(owned(FROM_CLIENT), target.clone(), owned("alice"))
        .await
        .expect("link it where it sits, in the INBOX");

    // Archive does not exist in the seeded mailbox; the backend creates
    // it on demand, which is the ordinary first-Archive-click path.
    alice
        .email()
        .await
        .move_message(owned(ACCOUNT), owned(FROM_CLIENT), owned("Archive"))
        .await
        .expect("archive the message");

    let archived = alice
        .email()
        .await
        .fetch_envelopes(owned(ACCOUNT), owned("Archive"), SeqRange::All)
        .await
        .expect("read the archive");
    assert_eq!(
        archived.len(),
        1,
        "the move did not actually move anything, so the test below proves nothing"
    );
    assert_eq!(archived[0].message_id, FROM_CLIENT);

    let still = links
        .links_for_target(target)
        .await
        .expect("every email on this project, after the move");
    assert_eq!(
        still.len(),
        1,
        "the link was keyed on where the message sat, not on the message"
    );
    assert_eq!(still[0].message_id, FROM_CLIENT);
}

/// A link may name a project in another org, and the link stays in the
/// linker's own org.
///
/// The second half is the access rule, and it is the half worth a test.
/// Alice files VNT's message against VNT's project; the row is written
/// in *her* org. Victor owns the project and is the owner of the org it
/// lives in — and gets nothing, because the store holding the link is
/// not his. That is what "private by construction" means here: not a
/// rule that is checked, a row that is somewhere else.
#[tokio::test]
async fn a_cross_org_link_is_stored_in_the_linkers_own_org_and_nowhere_else() {
    let s = Scenario::open().await;
    let (_org, alice) = personal(&s).await;

    let victor = s.as_victor().await;
    let shared = victor
        .projects()
        .await
        .list()
        .await
        .expect("VNT's projects")
        .into_iter()
        .find(|p| p.title == "Shared Project")
        .expect("the seeded shared project");
    let target = project_in("vnt-video", &shared.id.to_string());

    alice
        .email_links()
        .await
        .link(owned(FROM_VNT), target.clone(), owned("alice"))
        .await
        .expect("file VNT's message against VNT's project");

    let hers = alice
        .email_links()
        .await
        .links_for_target(target.clone())
        .await
        .expect("Alice asks");
    assert_eq!(hers.len(), 1, "she cannot see the link she just made");
    assert_eq!(
        hers[0].target.org, "vnt-video",
        "the link names the org its project belongs to"
    );

    // Victor, on VNT's own server, asking about VNT's own project.
    let his = victor
        .email_links()
        .await
        .links_for_target(target)
        .await
        .expect("Victor asks");
    assert!(
        his.is_empty(),
        "another org's member can read Alice's mail trail: {his:?}"
    );
}

/// Unlinking removes one link and leaves the others.
///
/// Worth its own test because the delete takes the same `(message,
/// target)` pair the insert does, and a store that keyed the delete on
/// the message alone would pass every test above while quietly
/// detaching a message from every project at once the first time
/// somebody corrected a mis-file.
#[tokio::test]
async fn unlinking_detaches_one_pair_and_leaves_the_rest() {
    let s = Scenario::open().await;
    let (_org, alice) = personal(&s).await;
    let links = alice.email_links().await;

    let album = project_in("acme-audio", "album");
    let single = project_in("acme-audio", "single");
    for target in [&album, &single] {
        links
            .link(owned(FROM_CLIENT), target.clone(), owned("alice"))
            .await
            .expect("one message, two projects");
    }

    links
        .unlink(owned(FROM_CLIENT), album.clone())
        .await
        .expect("correct the mis-file");

    assert!(
        links
            .links_for_target(album)
            .await
            .expect("the album")
            .is_empty(),
        "the link that was removed is still there"
    );
    assert_eq!(
        links
            .links_for_target(single)
            .await
            .expect("the single")
            .len(),
        1,
        "unlinking one pair detached the message from everything"
    );

    // And removing a link that is not there succeeds, so a rule that
    // cleans up twice does not fail the second time.
    links
        .unlink(owned(FROM_CLIENT), project_in("acme-audio", "never-linked"))
        .await
        .expect("unlinking a missing link is not an error");
}
