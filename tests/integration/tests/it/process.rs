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
//! Every outcome the card offers is then driven the same way — waiting,
//! do it now, a note in your own words, reference, a project, snooze,
//! done, delete, and a suggestion kept or dismissed — each sending the
//! write `inbox_proto::Decision::apply` makes (the function the app's
//! card calls), each walking back from what it made to the capture
//! (`decision::origin_of`), and the three honest ones undone
//! (`decision::undo`) back to exactly the stored item.
//!
//! The maintenance-hour test is the other half of the loop. A task filed with
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
        // t[verify process.capture.marks]
        // Marks are the capture's own words: served as written, for
        // the row to read `#tags`, `@contexts` and people out of.
        assert_eq!(item.body, c.body, "a capture is served verbatim");
        let queued = queue.iter().any(|i| i.id == c.id);
        match (c.status, c.resurface_in_days) {
            ("suggested", _) => {
                assert_eq!(item.status, "suggested");
                assert!(!queued, "a suggestion is not in the trusted queue");
            }
            ("processed", _) => {
                assert_eq!(item.status, "processed");
                assert_eq!(
                    item.processed_into.as_deref(),
                    c.processed_into,
                    "a processed capture names what it became"
                );
                assert!(!queued, "`{}` was decided, not due", c.body);
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
    // t[verify process.capture.verbatim]
    // The walk back: from the task to the thought that started it.
    let all = inbox.list_inbox().await.expect("list the inbox");
    assert_eq!(
        decision::origin_of(&all, &task.path).map(|i| i.id.as_str()),
        Some(captured.id.as_str()),
        "the task walks back to its capture"
    );
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

// ── every outcome, through the shared decision ──────────────────────
//
// What the card does for each outcome, as any client holding a token
// would do it: make the thing on its own lane (when there is one), then
// send the one write `inbox_proto::Decision::apply` says the capture
// gets. The decision is the same function the app's card calls, so a
// pass here is the app's outcome over the wire, not a restatement of it.

use inbox_proto::decision::{self, Decision, Write};
use inbox_proto::{InboxClient, InboxItem, ReviewResponse};

fn day() -> chrono::NaiveDate {
    chrono::Utc::now().date_naive()
}

/// Send the write a decision made.
async fn carry(inbox: &InboxClient, write: Write) {
    match write {
        Write::Save(item) => inbox
            .upsert_inbox_item(item)
            .await
            .expect("save the capture"),
        Write::Remove(id) => inbox
            .delete_inbox_item(id)
            .await
            .expect("delete the capture"),
    }
}

/// Capture `body` the way every surface does, and read back what the
/// inbox stored — the "before" an undo has to restore exactly.
async fn capture(inbox: &InboxClient, body: &str) -> InboxItem {
    let item = InboxItem::capture(
        uuid::Uuid::new_v4().to_string(),
        body,
        "ui",
        chrono::Utc::now().to_rfc3339(),
    );
    inbox
        .upsert_inbox_item(item.clone())
        .await
        .expect("capture");
    inbox
        .get_inbox_item(item.id.clone())
        .await
        .expect("the capture is stored")
}

async fn queued(inbox: &InboxClient, id: &str) -> bool {
    inbox
        .review_queue(today())
        .await
        .expect("today's queue")
        .iter()
        .any(|i| i.id == id)
}

/// The walk back from `what` to the capture that became it, over the
/// wire, asserting the capture kept its words.
async fn walks_back(inbox: &InboxClient, what: &str, body: &str) -> InboxItem {
    let all = inbox.list_inbox().await.expect("list the inbox");
    let origin = decision::origin_of(&all, what)
        .unwrap_or_else(|| panic!("no capture names `{what}` as what it became"))
        .clone();
    assert_eq!(origin.body, body, "the capture's words are kept verbatim");
    origin
}

// t[verify process.outcome.waiting]
/// "Waiting on someone": a task created `waiting`, the capture processed
/// into it and walkable back from it.
#[tokio::test]
async fn waiting_on_someone_is_a_waiting_task_the_capture_names() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let (inbox, tasks) = (alice.inbox().await, alice.tasks().await);

    let body = "Hear back from the mastering house about the vinyl cut @calls";
    let item = capture(&inbox, body).await;
    let mut draft = task_proto::capture(body);
    draft.status = "waiting".into();
    draft.details = "Waiting on the mastering house.".into();
    let task = tasks.create(draft).await.expect("create the waiting task");

    carry(
        &inbox,
        Decision::Into(task.path.clone()).apply(&item, day()),
    )
    .await;

    let made = tasks
        .list()
        .await
        .expect("list tasks")
        .into_iter()
        .find(|t| t.path == task.path)
        .expect("the task the capture became");
    assert_eq!(made.status, "waiting");
    assert!(made.contexts.iter().any(|c| c == "@calls"));
    let origin = walks_back(&inbox, &task.path, body).await;
    assert_eq!(origin.id, item.id);
    assert!(
        !queued(&inbox, &item.id).await,
        "a processed capture is not due"
    );
}

// t[verify process.outcome.now]
/// "Do it now": the task starts `in-progress` with its clock running,
/// and the capture names it.
#[tokio::test]
async fn do_it_now_starts_the_clock_on_the_task_the_capture_became() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let (inbox, tasks) = (alice.inbox().await, alice.tasks().await);

    let body = "Rename the bounce folder to the album title";
    let item = capture(&inbox, body).await;
    let mut draft = task_proto::capture(body);
    draft.status = "in-progress".into();
    draft.time_entries = task_proto::model::TimeEntries(vec![task_proto::model::TimeEntry {
        start_time: chrono::Utc::now(),
        end_time: None,
    }]);
    let task = tasks.create(draft).await.expect("create the task");
    carry(
        &inbox,
        Decision::Into(task.path.clone()).apply(&item, day()),
    )
    .await;

    let made = tasks
        .list()
        .await
        .expect("list tasks")
        .into_iter()
        .find(|t| t.path == task.path)
        .expect("the task");
    assert_eq!(made.status, "in-progress");
    assert!(
        made.time_entries.0.iter().any(|e| e.end_time.is_none()),
        "the clock is running on it"
    );
    walks_back(&inbox, &task.path, body).await;
}

// t[verify process.outcome.note]
// t[verify process.capture.verbatim]
/// "Write a note": the person's words over the capture, kept verbatim
/// as a quote; create-only, so a title already taken fails and the
/// capture stays in the inbox, open — no words lost either way.
#[tokio::test]
async fn a_note_in_your_own_words_keeps_the_capture_beneath_it() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let (inbox, vault) = (alice.inbox().await, alice.vault().await);

    let body = "room mics > close mics on the drums??\nsounded bigger on the demo";
    let item = capture(&inbox, body).await;
    let title = format!("Room mics make the kit {}", &item.id[..8]);
    let path = format!("{}.md", decision::note_file_name(&title));
    let md = decision::rewritten_note(
        "The room carries the kit's size; close mics only add attack.",
        &item.body,
        None,
        &chrono::Utc::now().to_rfc3339(),
    );
    vault
        .put_file(
            "default".into(),
            path.clone(),
            md.into(),
            vault_proto::IfMatch::CreateOnly,
        )
        .await
        .expect("write the note");
    carry(&inbox, Decision::Into(path.clone()).apply(&item, day())).await;

    let written = vault
        .get_file("default".into(), path.clone())
        .await
        .expect("read the note back");
    let text = String::from_utf8(written.0).expect("utf8");
    assert!(text.contains("tags: [atomic]"), "{text}");
    assert!(
        text.contains("> [!quote] Original capture\n> room mics > close mics on the drums??\n> sounded bigger on the demo"),
        "the capture is quoted verbatim under the rewrite: {text}"
    );
    walks_back(&inbox, &path, body).await;

    // A second capture rewritten under the same title: the create-only
    // write is refused, so its decision is never sent and it stays put.
    let again = capture(&inbox, "the room mic thing again").await;
    let clash = vault
        .put_file(
            "default".into(),
            path.clone(),
            decision::rewritten_note("dup", &again.body, None, "2026-10-20").into(),
            vault_proto::IfMatch::CreateOnly,
        )
        .await;
    assert!(clash.is_err(), "a title already taken fails the save");
    assert!(
        queued(&inbox, &again.id).await,
        "the capture stays in the inbox"
    );
}

// t[verify process.outcome.reference]
/// "Keep as reference": the whole capture filed as it stands.
#[tokio::test]
async fn reference_files_the_whole_capture_as_it_stands() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let (inbox, vault) = (alice.inbox().await, alice.vault().await);

    let body = "Gain staging cheat sheet\n-18 dBFS RMS into the plugins, peaks under -6";
    let item = capture(&inbox, body).await;
    let path = decision::reference_path(&item);
    assert!(
        path.starts_with("Wiki/Atomic/gain-staging-cheat-sheet-"),
        "{path}"
    );
    vault
        .put_file(
            "default".into(),
            path.clone(),
            decision::reference_markdown(&item, &chrono::Utc::now().to_rfc3339()).into(),
            vault_proto::IfMatch::CreateOnly,
        )
        .await
        .expect("file the reference");
    carry(&inbox, Decision::Into(path.clone()).apply(&item, day())).await;

    let text = String::from_utf8(
        vault
            .get_file("default".into(), path.clone())
            .await
            .expect("read it")
            .0,
    )
    .expect("utf8");
    assert!(text.contains(body), "the whole capture is kept: {text}");
    walks_back(&inbox, &path, body).await;
}

// t[verify process.outcome.project]
/// "It's a project": a project titled from the first line, and the
/// capture pointing at `project:<id>`.
#[tokio::test]
async fn a_project_capture_links_back_to_its_project() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let (inbox, projects) = (alice.inbox().await, alice.projects().await);

    let body = "Live session video series\nOne song a month, filmed in the A room.";
    let item = capture(&inbox, body).await;
    let (title, _) = decision::split_title_body(body);
    let made = projects
        .create(project::ProjectInfo {
            title: format!("{title} {}", &item.id[..8]),
            project_type: "general".into(),
            ..Default::default()
        })
        .await
        .expect("create the project");
    let link = decision::project_ref(made.id);
    carry(&inbox, Decision::Into(link.clone()).apply(&item, day())).await;

    let origin = walks_back(&inbox, &link, body).await;
    assert_eq!(origin.status, "processed");
}

// t[verify process.outcome.later]
// t[verify process.undo]
/// "Snooze": still open, off today's queue until its date; Undo writes
/// it back exactly as it was and it is due again.
#[tokio::test]
async fn snooze_puts_a_capture_off_and_undo_brings_it_back() {
    let s = Scenario::open().await;
    let inbox = s.as_alice().await.inbox().await;

    let before = capture(&inbox, "Try the ribbon mic on the cello").await;
    assert!(queued(&inbox, &before.id).await, "a fresh capture is due");

    let snooze = Decision::Later(ReviewResponse::Good);
    assert!(snooze.undoable());
    carry(&inbox, snooze.apply(&before, day())).await;
    let later = inbox
        .get_inbox_item(before.id.clone())
        .await
        .expect("still there");
    assert_eq!(later.status, "open");
    assert_eq!(later.body, before.body);
    assert_eq!(later.reviews, before.reviews + 1);
    assert!(
        later
            .resurface_on
            .as_deref()
            .is_some_and(|d| d > today().as_str()),
        "put off past today: {:?}",
        later.resurface_on
    );
    assert!(
        !queued(&inbox, &before.id).await,
        "a snoozed capture is not due"
    );

    carry(&inbox, decision::undo(&before)).await;
    let back = inbox.get_inbox_item(before.id.clone()).await.expect("back");
    assert_eq!(back, before, "undo restores the capture exactly");
    assert!(queued(&inbox, &before.id).await, "and it is due again");
}

// t[verify process.outcome.done]
// t[verify process.undo]
/// "Already done": processed, became nothing — recorded, not deleted.
#[tokio::test]
async fn done_is_recorded_rather_than_deleted_and_can_be_undone() {
    let s = Scenario::open().await;
    let inbox = s.as_alice().await.inbox().await;

    let before = capture(&inbox, "Email the drummer the click tempo").await;
    carry(&inbox, Decision::Done.apply(&before, day())).await;
    let done = inbox
        .get_inbox_item(before.id.clone())
        .await
        .expect("a done capture is kept");
    assert_eq!(done.status, "processed");
    assert_eq!(done.processed_into, None, "it became nothing");
    assert_eq!(done.body, before.body);
    assert!(!queued(&inbox, &before.id).await);

    carry(&inbox, decision::undo(&before)).await;
    assert_eq!(
        inbox.get_inbox_item(before.id.clone()).await.expect("back"),
        before
    );
}

// t[verify process.outcome.delete]
// t[verify process.undo]
/// "Delete": the file goes; Undo recreates it as it was.
#[tokio::test]
async fn delete_removes_the_capture_and_undo_recreates_it() {
    let s = Scenario::open().await;
    let inbox = s.as_alice().await.inbox().await;

    let before = capture(&inbox, "asdf (pocket capture)").await;
    carry(&inbox, Decision::Delete.apply(&before, day())).await;
    assert!(
        inbox.get_inbox_item(before.id.clone()).await.is_err(),
        "a deleted capture is gone"
    );
    assert!(
        !inbox
            .list_inbox()
            .await
            .expect("list")
            .iter()
            .any(|i| i.id == before.id)
    );

    carry(&inbox, decision::undo(&before)).await;
    assert_eq!(
        inbox
            .get_inbox_item(before.id.clone())
            .await
            .expect("recreated"),
        before,
        "undo recreates the capture exactly"
    );
}

// t[verify process.suggestion]
/// A suggestion waits outside the trusted queue: kept, it joins it;
/// dismissed, it is gone.
#[tokio::test]
async fn a_suggestion_is_kept_or_dismissed() {
    let s = Scenario::open().await;
    let inbox = s.as_alice().await.inbox().await;

    let suggest = |body: &str| {
        let mut i = InboxItem::capture(
            uuid::Uuid::new_v4().to_string(),
            body,
            "email",
            chrono::Utc::now().to_rfc3339(),
        );
        InboxItem::STATUS_SUGGESTED.clone_into(&mut i.status);
        i
    };
    let keep = suggest("Confirm the studio booking for the 14th");
    let dismiss = suggest("Unsubscribe from the plugin sale emails");
    for i in [&keep, &dismiss] {
        inbox.upsert_inbox_item(i.clone()).await.expect("suggest");
        assert!(
            !queued(&inbox, &i.id).await,
            "a suggestion is not in the trusted queue"
        );
    }

    carry(&inbox, Decision::Keep.apply(&keep, day())).await;
    assert!(
        queued(&inbox, &keep.id).await,
        "kept, it is due like any capture"
    );
    carry(&inbox, Decision::Delete.apply(&dismiss, day())).await;
    assert!(
        inbox.get_inbox_item(dismiss.id.clone()).await.is_err(),
        "dismissed, it is gone"
    );
}
