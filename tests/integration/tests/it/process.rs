//! Capture, then process — the centre of the app.
//!
//! Every capture lands in one place, the inbox (`Records/inbox/<id>.md`
//! in the org's vault), and comes back once to be decided: a task with
//! somewhere to live, a note, a project, a later, or nothing. The rules
//! are `docs/spec/capture-and-process.md`; this chapter holds the ones
//! a wire can check.
//!
//! The decision itself is made in the app (`crates/ui/src/process.rs`)
//! and carried out as ordinary calls on two lanes — create a task on
//! the Task lane, then write the item back on the Inbox lane as
//! processed, pointing at what it became. So that is exactly what this
//! chapter does, as Alice, over iroh, through the router a deployment
//! mounts: if the app can do it, so can any client holding a token.
//!
//! The last test is the other half of the loop. A task filed with
//! `@errands` is hidden by its gate most of the day — and brought
//! forward, first, during a maintenance hour, because the hour's
//! `focus` opens the gate and ranks what matches it.

use integration::scenario::Scenario;
use task_proto::{RelevanceContext, TaskListFilter};

/// What a maintenance hour brings forward — read from the scheduling
/// vocabulary, which is what the app reads, rather than restated.
fn maintenance_hour() -> RelevanceContext {
    RelevanceContext {
        focus: scheduling_proto::BlockCategory::Maintenance
            .surfaces()
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        ..Default::default()
    }
}

fn today() -> String {
    chrono::Utc::now().date_naive().to_string()
}

// t[verify process.due]
/// The planted inbox, as the Inbox lane serves it: today's queue holds
/// the open captures and leaves out the one put off into the future;
/// the agent's suggestion is in the inbox but awaits a yes or no
/// rather than sitting in the trusted queue.
#[tokio::test]
async fn the_planted_inbox_is_served_with_its_snooze_and_its_suggestion() {
    let s = Scenario::open().await;
    let inbox = s.as_alice().await.inbox().await;

    let all = inbox.list_inbox().await.expect("list the inbox");
    let queue = inbox.review_queue(today()).await.expect("today's queue");

    let declared: Vec<_> = task_server::example_org::inbox_of("acme-audio").collect();
    assert!(!declared.is_empty(), "the seed declares captures for ACME");
    for c in &declared {
        let item = all
            .iter()
            .find(|i| i.id == c.id)
            .unwrap_or_else(|| panic!("`{}` was planted but is not served", c.body));
        let queued = queue.iter().any(|i| i.id == c.id);
        match (c.status, c.resurface_in_days) {
            ("suggested", _) => {
                assert_eq!(item.status, "suggested");
                assert!(!queued, "a suggestion is not in the trusted queue");
            }
            (_, Some(_)) => assert!(!queued, "`{}` is snoozed, not due", c.body),
            _ => assert!(queued, "`{}` is due today and missing", c.body),
        }
    }
}

// t[verify process.capture.single-path]
// t[verify process.outcome.task]
// t[verify process.capture.verbatim]
/// A capture becomes a task: the item is processed, says what it
/// became, and keeps its words; the task carries what the capture's
/// quick-add syntax said.
#[tokio::test]
async fn a_capture_processed_into_a_task_links_back_to_it() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let inbox = alice.inbox().await;
    let tasks = alice.tasks().await;

    // Capture: no questions asked, straight into the inbox.
    let body = "Drop the broken DI box at the repair shop @errands";
    let captured = inbox_proto::InboxItem::capture(
        uuid::Uuid::new_v4().to_string(),
        body,
        "ui",
        chrono::Utc::now().to_rfc3339(),
    );
    inbox
        .upsert_inbox_item(captured.clone())
        .await
        .expect("capture");
    let queued = inbox.review_queue(today()).await.expect("queue");
    assert!(
        queued.iter().any(|i| i.id == captured.id),
        "a fresh capture is due at once"
    );

    // Process: "make it a task" — the capture parsed the way quick-add
    // parses it, created on the Task lane …
    let draft = task_proto::capture(body);
    assert_eq!(draft.contexts.0, vec!["@errands".to_owned()]);
    let task = tasks.create(draft).await.expect("create the task");
    assert!(!task.path.is_empty(), "the backend gave the task no path");

    // … then the item written back as processed, pointing at it.
    let mut done = captured.clone();
    done.status = inbox_proto::InboxItem::STATUS_PROCESSED.to_owned();
    done.processed_into = Some(task.path.clone());
    inbox.upsert_inbox_item(done).await.expect("mark processed");

    let item = inbox
        .get_inbox_item(captured.id.clone())
        .await
        .expect("the item is still there");
    assert_eq!(item.status, "processed");
    assert_eq!(
        item.processed_into.as_deref(),
        Some(task.path.as_str()),
        "the item names what it became"
    );
    assert_eq!(item.body, body, "the capture's words are kept verbatim");
    assert!(
        !inbox
            .review_queue(today())
            .await
            .expect("queue")
            .iter()
            .any(|i| i.id == captured.id),
        "a processed item is not due again"
    );

    let listed = tasks.list().await.expect("list tasks");
    let made = listed
        .iter()
        .find(|t| t.path == task.path)
        .expect("the task the item points at exists");
    assert_eq!(made.title, "Drop the broken DI box at the repair shop");
    assert!(
        made.contexts.iter().any(|c| c == "@errands"),
        "the task kept its context: {:?}",
        made.contexts
    );
}

// t[verify process.focus.opens]
// t[verify process.focus.ranks]
/// A maintenance hour shows `@errands` work, and shows it first.
///
/// Outside the hour the errand's gate is shut — nothing says Alice is
/// out — so a relevance query leaves it out. During the hour the
/// block's focus opens the gate and ranks the errand ahead of ordinary
/// work that would otherwise lead (its path sorts first).
#[tokio::test]
async fn a_maintenance_hour_brings_errands_forward() {
    let s = Scenario::open().await;
    let tasks = s.as_alice().await.tasks().await;

    let ordinary = tasks
        .create(task_proto::capture("Archive the tracking notes #album"))
        .await
        .expect("an ordinary task");
    let errand = tasks
        .create(task_proto::capture("Return the hired mic stands @errands"))
        .await
        .expect("an errand");
    assert!(
        ordinary.path < errand.path,
        "the ordinary task should lead by path, so a rank is what moves the errand"
    );

    let relevant = |ctx: RelevanceContext| {
        let tasks = tasks.clone();
        async move {
            tasks
                .query(TaskListFilter {
                    open_only: true,
                    relevance: Some(ctx),
                    ..Default::default()
                })
                .await
                .expect("a relevance query")
        }
    };

    let elsewhere = relevant(RelevanceContext::default()).await;
    assert!(
        elsewhere.iter().any(|t| t.id == ordinary.id),
        "ungated work is always relevant"
    );
    assert!(
        !elsewhere.iter().any(|t| t.id == errand.id),
        "outside the hour, nothing opens the @errands gate"
    );

    let hour = relevant(maintenance_hour()).await;
    let pos = |id| hour.iter().position(|t| t.id == id);
    let (Some(e), Some(o)) = (pos(errand.id), pos(ordinary.id)) else {
        panic!(
            "both tasks are relevant in the hour: {:?}",
            hour.iter().map(|t| &t.title).collect::<Vec<_>>()
        );
    };
    assert!(
        e < o,
        "the errand leads the ordinary task in a maintenance hour: {:?}",
        hour.iter().map(|t| &t.title).collect::<Vec<_>>()
    );
}
