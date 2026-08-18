//! Chapter eight — the work around the files.
//!
//! A session folder is not a project. The project is the thing with a
//! deadline, a lead, a list of what is left to do — and in this product
//! it is a markdown page in the org's vault, which is why it can be
//! edited in Obsidian and why it survives the server that served it.
//!
//! This chapter is short on purpose. It is not a test of the project
//! feature, which has its own; it asks the three things only an
//! integration suite can: that a signed-in person can reach it over the
//! wire, that what they create lands on disk as a page, and that the
//! claim primitive two agents race for is actually atomic.
//!
//! It also records something uncomfortable, in the last test.

use integration::client::Session;
use integration::scenario::Scenario;

/// A project draft. `id` nil and `path` empty means "you assign them" —
/// the backend picks `Projects/<slug>.md`.
fn draft(title: &str) -> project::ProjectInfo {
    project::ProjectInfo {
        title: title.into(),
        project_type: "general".into(),
        ..Default::default()
    }
}

/// The album, as a project, created by the person who runs the label.
#[tokio::test]
async fn a_project_created_over_the_wire_comes_back_in_the_list() {
    let s = Scenario::open().await;
    let projects = s.as_alice().await.projects().await;

    let made = projects
        .create(draft("Album — mix and master"))
        .await
        .expect("create a project");
    assert_ne!(made.id, uuid::Uuid::nil(), "the backend assigned no id");
    assert!(!made.path.is_empty(), "the backend assigned no path");

    let listed = projects.list().await.expect("list");
    assert!(
        listed.iter().any(|p| p.id == made.id),
        "the project did not come back: {listed:?}"
    );
}

/// And it is a page on disk, not a row in a database.
///
/// This is the local-first claim, and it is the one an integration test
/// can check that a unit test cannot: the file is under the org's vault,
/// where Obsidian would find it, put there by a call that arrived over
/// a network.
#[tokio::test]
async fn a_project_is_a_markdown_page_in_the_vault() {
    let s = Scenario::open().await;
    let made = s
        .as_alice()
        .await
        .projects()
        .await
        .create(draft("Album — mix and master"))
        .await
        .expect("create");

    let page = s.orgs.acme.backend.vault_root().join(&made.path);
    let text = std::fs::read_to_string(&page).unwrap_or_else(|e| panic!("{}: {e}", page.display()));
    assert!(
        text.contains("Album — mix and master"),
        "the page does not carry its own title: {text:.300}"
    );
}

/// Exactly one of two racing agents wins a task.
///
/// `try_claim` is the parallel-agent primitive, and its whole value is
/// that the read-check-write happens under a lock the callers cannot
/// interleave. A client-side check-then-set has a window; this is here
/// to prove this one does not — over the wire, where two agents actually
/// are two callers.
#[tokio::test]
async fn two_agents_race_for_a_task_and_exactly_one_wins() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let task = task::parse_str(
        "tasks/master-the-album.md",
        "master-the-album",
        "---\ntype: task\ntitle: Master the album\n---\n",
    )
    .expect("a minimal task parses");
    let made = alice
        .tasks()
        .await
        .create(task)
        .await
        .expect("create a task");

    // Two clients, two connections, one task. `agent` is a JSON-encoded
    // `AgentRef`, internally tagged — a bare string is refused, which is
    // the lane declining to record a claimant it cannot name.
    let (one, two) = (alice.tasks().await, s.as_alice().await.tasks().await);
    let agent = |name: &str| format!(r#"{{"kind":"agent","name":"{name}"}}"#);
    let (first, second) = tokio::join!(
        one.try_claim(made.id, agent("agent-one"), false),
        two.try_claim(made.id, agent("agent-two"), false),
    );

    let won = [first.expect("claim one"), second.expect("claim two")]
        .into_iter()
        .filter(|r| matches!(r, task_proto::service::ClaimResult::Won))
        .count();
    assert_eq!(won, 1, "a task was claimed by both agents, or by neither");
}

/// **The client account is a full member of everything that is not
/// Files.**
///
/// Casey was invited to look at one folder of deliverables. The Files
/// lanes enforce that — `people.rs` shows them refused at the session
/// folder — because Files has a per-path grant table and a lane that
/// consults it. Nothing else does. Every other service on the org router
/// is gated only by the coarse permit table, and that asks "is this a
/// validated user", to which the answer for any signed-up account is
/// yes (`DEFAULT_ORG_ROLE`).
///
/// So the client can read the label's projects, and would be able to
/// read its tasks, notes and wiki. That is not a bug in a lane; it is
/// the shape of the system: `files.access.granularity` was specified for
/// Files and the same idea has no counterpart elsewhere yet.
///
/// This test asserts the current behaviour deliberately, so that closing
/// the gap is a decision someone makes and sees fail here, rather than a
/// surprise nobody wrote down.
#[tokio::test]
async fn a_client_account_can_read_everything_that_is_not_files() {
    let s = Scenario::open().await;
    s.as_alice()
        .await
        .projects()
        .await
        .create(draft("Album — mix and master"))
        .await
        .expect("create");

    let casey = Session::open(&s.orgs.acme, s.people.casey.token.clone()).await;
    let seen = casey
        .projects()
        .await
        .list()
        .await
        .expect("the client can list the label's projects");

    assert!(
        seen.iter().any(|p| p.title == "Album — mix and master"),
        "if this fails, per-path access reached the project lane — good, \
         and this test needs rewriting to match"
    );
}
