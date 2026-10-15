//! One document editor — what a task's description survives.
//!
//! The issue page edits a task's description with the same markdown
//! editor a vault note uses (`crates/ui/src/doc_editor.rs`), and commits
//! it as an ordinary Task-lane update: the whole record, `details`
//! changed. The editor holds the body only; the frontmatter is the task
//! service's. So the claim worth checking over the wire is that the body
//! the editor wrote is the body that comes back — byte for byte, with no
//! blank line gained per save — and that the record's properties ride
//! through untouched. `docs/spec/documents.md` holds the rules.

use integration::scenario::Scenario;

// t[verify doc.editor.body-only]
// t[verify doc.editor.one]
/// A description with every structure the slash menu inserts survives
/// update → get, twice, and the properties beside it are unchanged.
#[tokio::test]
async fn a_description_written_in_the_editor_comes_back_as_written() {
    let s = Scenario::open().await;
    let tasks = s.as_alice().await.tasks().await;

    let made = tasks
        .create(task_proto::capture("Prep the label call #album"))
        .await
        .expect("create");
    let body = "## Before the call\n\n- [ ] Bounce the masters\n- [x] Book the room\n\n> [!note]- Folded\n> Body\n\n```mermaid\ngraph TD\n```\n\n---\n\n1. One\n2. Two\n";

    for pass in 0..2 {
        let mut next = tasks.get(made.id).await.expect("read");
        next.details = body.to_owned();
        let saved = tasks.update(next).await.expect("write the description");
        assert_eq!(saved.details, body, "pass {pass}: the update's answer");
        let back = tasks.get(made.id).await.expect("read back");
        assert_eq!(back.details, body, "pass {pass}: the stored body");
        assert_eq!(back.title, made.title, "the title rode through");
        assert_eq!(back.tags, made.tags, "the labels rode through");
        assert_eq!(back.status, made.status, "the status rode through");
    }
}
