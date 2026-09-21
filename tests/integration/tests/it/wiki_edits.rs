//! Chapter — the Edit lane: how someone without Editor changes a wiki.
//!
//! Through the org router, as signed-in people. Sam works at ACME and
//! holds no Editor role on Music Theory; Alice owns ACME and is the
//! wiki's Editor. What is asserted is the lane's behaviour over the
//! wire, with the gate in front of it: who may open, who may land, what
//! a landing does to the page and to the board, and what a refusal
//! looks like to the person refused.
//!
//! # How Alice becomes Editor
//!
//! Editor is an account id in the wiki's own config, and the suite's
//! accounts exist only once `People::hire` has run — after the example
//! is planted. The demo (`admin demo`) grants the owner Editor at plant
//! time from the accounts it just created; this chapter does the same
//! thing on the planted wiki before the story starts, in
//! [`make_editor`]. Nobody in the suite holds an org admin role (the
//! org lane has no admin permits yet), so the RPC bootstrap path is
//! asserted where it can be — in `wiki-live`'s unit tests — and the
//! seed stands in for it here.

use std::path::Path;

use files::service::access::Subject;
use integration::client::Session;
use integration::people::Person;
use integration::scenario::Scenario;
use wiki_proto::config::{ProposerGate, WikiConfig};
use wiki_proto::service::edits::{EditStatus, NewEditRequest, PageChange};

const WIKI: &str = "music-theory";
const IONIAN: &str = "Concepts/Ionian.md";

/// The account id the gate resolves a person's token to.
fn id_of(person: &Person) -> String {
    match &person.subject {
        Subject::Person(id) => id.to_string(),
        other => panic!("a person's subject is a principal id, got {other:?}"),
    }
}

/// Grant `person` Editor on Music Theory the way the seed does: in the
/// wiki's own `_state/wiki.json`, beside the visibility the plant wrote.
fn make_editor(s: &Scenario, person: &Person) {
    let root = s.orgs.acme.org_root().join("wikis").join(WIKI);
    let path = root.join("_state").join("wiki.json");
    let mut config: WikiConfig = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| WikiConfig::implicit(WIKI));
    let id = id_of(person);
    if !config.is_editor(&id) {
        config.editors.push(id);
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
}

async fn as_sam(s: &Scenario) -> Session {
    Session::open(&s.orgs.acme, s.people.sam.token.clone()).await
}

/// A request from `who` changing Ionian with `edit`, against the page
/// as it is now.
async fn proposal(who: &Session, title: &str, edit: impl Fn(&str) -> String) -> NewEditRequest {
    let page = who
        .wiki_pages()
        .await
        .read_page(WIKI.to_string(), IONIAN.to_string())
        .await
        .expect("read Ionian");
    NewEditRequest {
        title: title.to_string(),
        summary: "From the chapter.".into(),
        changes: vec![PageChange {
            path: IONIAN.into(),
            base_sha256: page.sha256,
            base_markdown: page.markdown.clone(),
            markdown: edit(&page.markdown),
            delete: false,
        }],
        request_review: false,
    }
}

async fn page(who: &Session) -> String {
    who.wiki_pages()
        .await
        .read_page(WIKI.to_string(), IONIAN.to_string())
        .await
        .expect("read Ionian")
        .markdown
}

/// The whole wiki directory, minus the lane's own state, so a "byte
/// identical" claim is about the wiki and not about the request.
fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
            let p = entry.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if rel.starts_with("_state") {
                continue;
            }
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.push((rel, std::fs::read(&p).expect("read file")));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// The error's Debug rendering — the vox error wraps the typed
/// `WikiError`, and the variant name is what the assertion is about.
fn error_of<T: std::fmt::Debug, E: std::fmt::Debug>(r: Result<T, E>) -> String {
    match r {
        Ok(v) => panic!("expected an error, got {v:?}"),
        Err(e) => format!("{e:?}"),
    }
}

const SAMS_LINE: &str = "The first mode of the major scale, and the same set of intervals most";
const SAMS_EDIT: &str = "The first mode of the major scale — the one a chart means by \"major\" — and the same set of intervals most";

/// t[verify wiki.edit.request] — Sam, holding no Editor, opens a
/// request carrying the changed page against the version he read; the
/// page is unchanged and the request is listed.
///
/// t[verify wiki.edit.tracked] — the same id is an issue on ACME's
/// board, tagged `edit-request`.
#[tokio::test(flavor = "multi_thread")]
async fn sam_opens_a_request_without_editor() {
    let s = Scenario::open().await;
    make_editor(&s, &s.people.alice);
    let sam = as_sam(&s).await;
    let before = page(&sam).await;

    let editors = sam
        .wiki_edits()
        .await
        .editors(WIKI.to_string())
        .await
        .expect("editors");
    assert_eq!(
        editors.editors,
        vec![id_of(&s.people.alice)],
        "Sam can see who will review"
    );

    let req = sam
        .wiki_edits()
        .await
        .open_edit_request(
            WIKI.to_string(),
            proposal(&sam, "Say what major means", |p| {
                p.replace(SAMS_LINE, SAMS_EDIT)
            })
            .await,
        )
        .await
        .expect("Sam opens a request");
    assert_eq!(req.status, EditStatus::Open);
    assert_eq!(req.proposer, id_of(&s.people.sam));
    assert!(!req.auto_approved && !req.held);
    assert_eq!(
        page(&sam).await,
        before,
        "opening a request changed the page"
    );

    let listed = sam
        .wiki_edits()
        .await
        .list_edit_requests(WIKI.to_string(), false)
        .await
        .expect("list");
    assert!(
        listed.iter().any(|r| r.id == req.id),
        "the request is not listed"
    );

    let issue = sam
        .tasks()
        .await
        .get(req.id)
        .await
        .expect("the request is an issue");
    assert_eq!(issue.title, "Say what major means");
    assert!(
        issue.tags.0.iter().any(|t| t == "edit-request"),
        "issue tags: {:?}",
        issue.tags.0
    );
}

/// t[verify wiki.edit.reviewable] — Sam cannot accept his own; Alice
/// sees the diff, accepts, and the page is the proposal, logged with
/// Sam's name; the issue is `done`.
///
/// t[verify wiki.edit.claim] — Alice claims first, and the claim shows
/// on the request.
#[tokio::test(flavor = "multi_thread")]
async fn an_editor_claims_reviews_and_accepts() {
    let s = Scenario::open().await;
    make_editor(&s, &s.people.alice);
    let sam = as_sam(&s).await;
    let alice = s.as_alice().await;

    let req = sam
        .wiki_edits()
        .await
        .open_edit_request(
            WIKI.to_string(),
            proposal(&sam, "Say what major means", |p| {
                p.replace(SAMS_LINE, SAMS_EDIT)
            })
            .await,
        )
        .await
        .expect("open");
    let err = error_of(
        sam.wiki_edits()
            .await
            .accept_edit_request(WIKI.to_string(), req.id)
            .await,
    );
    assert!(
        err.contains("Refused"),
        "Sam accepted his own request: {err}"
    );

    let claimed = alice
        .wiki_edits()
        .await
        .claim_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("Alice claims");
    assert_eq!(claimed.claimed_by, id_of(&s.people.alice));
    assert!(!claimed.claimed_until.is_empty());

    let diff = alice
        .wiki_edits()
        .await
        .diff_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("diff");
    assert_eq!(diff.len(), 1);
    assert!(diff[0].current.contains(SAMS_LINE));
    assert!(diff[0].proposed.contains(SAMS_EDIT));
    assert!(!diff[0].stale && diff[0].applies);

    let accepted = alice
        .wiki_edits()
        .await
        .accept_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("Alice accepts");
    assert_eq!(accepted.status, EditStatus::Accepted);
    assert_eq!(accepted.resolved_by, id_of(&s.people.alice));
    assert!(
        page(&sam).await.contains(SAMS_EDIT),
        "the proposal did not land"
    );

    let log = std::fs::read_to_string(
        s.orgs
            .acme
            .org_root()
            .join("wikis")
            .join(WIKI)
            .join("log.md"),
    )
    .expect("log.md");
    assert!(
        log.contains(&id_of(&s.people.sam)),
        "the landing does not name Sam:\n{log}"
    );
    assert!(
        log.contains(&req.id.to_string()),
        "the landing does not name the request"
    );

    let issue = alice.tasks().await.get(req.id).await.expect("issue");
    assert_eq!(issue.status, "done");
}

/// t[verify wiki.edit.reviewable] — rejecting leaves the wiki
/// byte-identical and keeps the request's text.
#[tokio::test(flavor = "multi_thread")]
async fn rejecting_leaves_the_wiki_byte_identical() {
    let s = Scenario::open().await;
    make_editor(&s, &s.people.alice);
    let sam = as_sam(&s).await;
    let alice = s.as_alice().await;
    let root = s.orgs.acme.org_root().join("wikis").join(WIKI);
    let before = snapshot(&root);

    let req = sam
        .wiki_edits()
        .await
        .open_edit_request(
            WIKI.to_string(),
            proposal(&sam, "Say what major means", |p| {
                p.replace(SAMS_LINE, SAMS_EDIT)
            })
            .await,
        )
        .await
        .expect("open");
    let rejected = alice
        .wiki_edits()
        .await
        .reject_edit_request(WIKI.to_string(), req.id, "not the place".to_string())
        .await
        .expect("reject");
    assert_eq!(rejected.status, EditStatus::Rejected);
    assert_eq!(rejected.resolution, "not the place");
    assert!(
        rejected.changes[0].markdown.contains(SAMS_EDIT),
        "the text was lost"
    );
    assert_eq!(snapshot(&root), before, "rejecting changed the wiki");

    let kept = sam
        .wiki_edits()
        .await
        .get_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("still readable");
    assert_eq!(kept.status, EditStatus::Rejected);
    let issue = sam.tasks().await.get(req.id).await.expect("issue");
    assert_eq!(issue.status, "cancelled");
}

/// t[verify wiki.edit.rebase] — Alice changes a different region after
/// Sam opened: accepting merges both. Then the same line on both sides:
/// `Conflict`, nothing written, the request still open.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_request_merges_and_a_conflict_stays_open() {
    let s = Scenario::open().await;
    make_editor(&s, &s.people.alice);
    let sam = as_sam(&s).await;
    let alice = s.as_alice().await;

    let req = sam
        .wiki_edits()
        .await
        .open_edit_request(
            WIKI.to_string(),
            proposal(&sam, "Say what major means", |p| {
                p.replace(SAMS_LINE, SAMS_EDIT)
            })
            .await,
        )
        .await
        .expect("open");

    // Alice, an Editor, writes a different region directly.
    let current = alice
        .wiki_pages()
        .await
        .read_page(WIKI.to_string(), IONIAN.to_string())
        .await
        .expect("read");
    let alices = current
        .markdown
        .replace("- [[Modes]]\n", "- [[Modes]]\n- [[Dorian]]\n");
    assert_ne!(alices, current.markdown);
    alice
        .wiki_pages()
        .await
        .write_page(WIKI.to_string(), IONIAN.to_string(), alices, current.sha256)
        .await
        .expect("Alice writes directly");

    let diff = alice
        .wiki_edits()
        .await
        .diff_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("diff");
    assert!(diff[0].stale && diff[0].applies, "{:?}", diff[0]);
    alice
        .wiki_edits()
        .await
        .accept_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("a stale request that applies lands");
    let merged = page(&sam).await;
    assert!(
        merged.contains(SAMS_EDIT),
        "Sam's change was dropped:\n{merged}"
    );
    assert!(
        merged.contains("- [[Dorian]]"),
        "Alice's change was dropped:\n{merged}"
    );

    // The same line, both sides.
    let req2 = sam
        .wiki_edits()
        .await
        .open_edit_request(
            WIKI.to_string(),
            proposal(&sam, "Reword", |p| {
                p.replace("- [[Dorian]]", "- [[Dorian]] (the second)")
            })
            .await,
        )
        .await
        .expect("open");
    let current = alice
        .wiki_pages()
        .await
        .read_page(WIKI.to_string(), IONIAN.to_string())
        .await
        .expect("read");
    alice
        .wiki_pages()
        .await
        .write_page(
            WIKI.to_string(),
            IONIAN.to_string(),
            current
                .markdown
                .replace("- [[Dorian]]", "- [[Dorian]] (mode two)"),
            current.sha256,
        )
        .await
        .expect("Alice writes the same line");
    let root = s.orgs.acme.org_root().join("wikis").join(WIKI);
    let before = snapshot(&root);
    let err = error_of(
        alice
            .wiki_edits()
            .await
            .accept_edit_request(WIKI.to_string(), req2.id)
            .await,
    );
    assert!(err.contains("Conflict"), "expected Conflict: {err}");
    assert!(
        err.contains(IONIAN),
        "the conflict does not name the page: {err}"
    );
    assert_eq!(
        snapshot(&root),
        before,
        "a conflicting accept wrote something"
    );
    let still = sam
        .wiki_edits()
        .await
        .get_edit_request(WIKI.to_string(), req2.id)
        .await
        .expect("get");
    assert_eq!(
        still.status,
        EditStatus::Open,
        "a conflict resolved the request by itself"
    );
    // Both parties see the same conflict.
    let sams_view = sam
        .wiki_edits()
        .await
        .diff_edit_request(WIKI.to_string(), req2.id)
        .await
        .expect("Sam's diff");
    assert!(sams_view[0].stale && !sams_view[0].applies);
}

/// t[verify wiki.edit.tracked] — closing the issue from the board
/// closes the request, and nothing lands.
#[tokio::test(flavor = "multi_thread")]
async fn closing_the_issue_closes_the_request() {
    let s = Scenario::open().await;
    make_editor(&s, &s.people.alice);
    let sam = as_sam(&s).await;
    let alice = s.as_alice().await;
    let before = page(&sam).await;

    let req = sam
        .wiki_edits()
        .await
        .open_edit_request(
            WIKI.to_string(),
            proposal(&sam, "Say what major means", |p| {
                p.replace(SAMS_LINE, SAMS_EDIT)
            })
            .await,
        )
        .await
        .expect("open");

    let mut issue = alice.tasks().await.get(req.id).await.expect("issue");
    issue.status = "cancelled".into();
    alice
        .tasks()
        .await
        .update(issue)
        .await
        .expect("close the issue");

    let seen = sam
        .wiki_edits()
        .await
        .get_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("get");
    assert_eq!(seen.status, EditStatus::Closed);
    assert_eq!(page(&sam).await, before);
    let open = sam
        .wiki_edits()
        .await
        .list_edit_requests(WIKI.to_string(), false)
        .await
        .expect("list");
    assert!(
        !open.iter().any(|r| r.id == req.id),
        "a closed request is still open"
    );
    let err = error_of(
        alice
            .wiki_edits()
            .await
            .accept_edit_request(WIKI.to_string(), req.id)
            .await,
    );
    assert!(
        err.contains("IllegalState"),
        "a closed request was accepted: {err}"
    );
}

/// t[verify wiki.edit.auto-approve] — Alice's own change is `Accepted`
/// with `auto_approved` in one call and has a row; with review
/// requested it stays open.
#[tokio::test(flavor = "multi_thread")]
async fn an_editors_own_change_is_auto_approved() {
    let s = Scenario::open().await;
    make_editor(&s, &s.people.alice);
    let alice = s.as_alice().await;

    let req = alice
        .wiki_edits()
        .await
        .open_edit_request(
            WIKI.to_string(),
            proposal(&alice, "Say what major means", |p| {
                p.replace(SAMS_LINE, SAMS_EDIT)
            })
            .await,
        )
        .await
        .expect("open");
    assert_eq!(req.status, EditStatus::Accepted);
    assert!(req.auto_approved);
    assert!(page(&alice).await.contains(SAMS_EDIT));
    let issue = alice
        .tasks()
        .await
        .get(req.id)
        .await
        .expect("an auto-approved change has a row");
    assert_eq!(issue.status, "done");

    let mut reviewed = proposal(&alice, "Second thoughts", |p| {
        p.replace(SAMS_EDIT, SAMS_LINE)
    })
    .await;
    reviewed.request_review = true;
    let req2 = alice
        .wiki_edits()
        .await
        .open_edit_request(WIKI.to_string(), reviewed)
        .await
        .expect("open for review");
    assert_eq!(req2.status, EditStatus::Open);
    assert!(!req2.auto_approved);
    assert!(
        page(&alice).await.contains(SAMS_EDIT),
        "a reviewed change landed early"
    );

    let all = alice
        .wiki_edits()
        .await
        .list_edit_requests(WIKI.to_string(), true)
        .await
        .expect("every change and who made it");
    assert_eq!(all.len(), 2);
}

/// t[verify wiki.edit.gate] — with proposers closed, Sam's open is
/// refused and the refusal names the state; the gate is readable
/// before he tries.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_gate_refuses_with_the_state_named() {
    let s = Scenario::open().await;
    make_editor(&s, &s.people.alice);
    let sam = as_sam(&s).await;
    let alice = s.as_alice().await;

    let err = error_of(
        sam.wiki_edits()
            .await
            .set_proposer_gate(WIKI.to_string(), ProposerGate::Closed)
            .await,
    );
    assert!(err.contains("Refused"), "a non-Editor set the gate: {err}");
    alice
        .wiki_edits()
        .await
        .set_proposer_gate(WIKI.to_string(), ProposerGate::Closed)
        .await
        .expect("Alice closes the gate");
    let editors = sam
        .wiki_edits()
        .await
        .editors(WIKI.to_string())
        .await
        .expect("editors");
    assert_eq!(editors.gate, ProposerGate::Closed);

    let err = error_of(
        sam.wiki_edits()
            .await
            .open_edit_request(
                WIKI.to_string(),
                proposal(&sam, "Say what major means", |p| {
                    p.replace(SAMS_LINE, SAMS_EDIT)
                })
                .await,
            )
            .await,
    );
    assert!(err.contains("Refused"), "{err}");
    assert!(
        err.contains("closed"),
        "the refusal does not name the state: {err}"
    );
    // Sam still cannot write directly: the lane is on.
    let current = sam
        .wiki_pages()
        .await
        .read_page(WIKI.to_string(), IONIAN.to_string())
        .await
        .expect("read");
    let err = error_of(
        sam.wiki_pages()
            .await
            .write_page(
                WIKI.to_string(),
                IONIAN.to_string(),
                current.markdown.replace(SAMS_LINE, SAMS_EDIT),
                current.sha256,
            )
            .await,
    );
    assert!(
        err.contains("Refused"),
        "a non-Editor wrote directly: {err}"
    );
}

/// t[verify wiki.edit.claim] — two Editors: Alice claims, Sam's claim
/// is refused while hers stands, and once it has expired Sam may claim.
///
/// The TTL is shrunk through `TASK_WIKI_CLAIM_TTL_SECS`, which the
/// server reads when it builds the lane. The variable is set before the
/// world boots; `cargo nextest` runs each test in its own process, so
/// nothing else sees it.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_excludes_a_second_editor_until_it_expires() {
    // SAFETY: set before any server boots in this process, and this
    // binary's tests run one per process under nextest.
    unsafe { std::env::set_var("TASK_WIKI_CLAIM_TTL_SECS", "2") };
    let s = Scenario::open().await;
    make_editor(&s, &s.people.alice);
    let sam = as_sam(&s).await;
    let alice = s.as_alice().await;

    alice
        .wiki_edits()
        .await
        .grant_editor(WIKI.to_string(), id_of(&s.people.sam))
        .await
        .expect("Alice grants Sam Editor");
    let editors = sam
        .wiki_edits()
        .await
        .editors(WIKI.to_string())
        .await
        .expect("editors");
    assert_eq!(editors.editors.len(), 2);

    // Casey, not an Editor, opens the request the two review.
    let casey = Session::open(&s.orgs.acme, s.people.casey.token.clone()).await;
    let req = casey
        .wiki_edits()
        .await
        .open_edit_request(
            WIKI.to_string(),
            proposal(&casey, "Say what major means", |p| {
                p.replace(SAMS_LINE, SAMS_EDIT)
            })
            .await,
        )
        .await
        .expect("Casey opens");
    assert_eq!(
        req.status,
        EditStatus::Open,
        "a non-Editor's request was approved"
    );

    alice
        .wiki_edits()
        .await
        .claim_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("Alice claims");
    let err = error_of(
        sam.wiki_edits()
            .await
            .claim_edit_request(WIKI.to_string(), req.id)
            .await,
    );
    assert!(
        err.contains("Refused"),
        "two Editors hold the same claim: {err}"
    );
    let seen = sam
        .wiki_edits()
        .await
        .get_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("get");
    assert_eq!(
        seen.claimed_by,
        id_of(&s.people.alice),
        "the claim is visible to other Editors"
    );

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let after = sam
        .wiki_edits()
        .await
        .get_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("get");
    assert!(
        after.claimed_by.is_empty(),
        "an expired claim is still shown"
    );
    assert_eq!(after.status, EditStatus::Open, "expiry lost the request");
    let claimed = sam
        .wiki_edits()
        .await
        .claim_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("Sam claims after expiry");
    assert_eq!(claimed.claimed_by, id_of(&s.people.sam));
    sam.wiki_edits()
        .await
        .release_edit_request(WIKI.to_string(), req.id)
        .await
        .expect("Sam releases");
}
