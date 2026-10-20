//! The decision — what processing a capture writes back to it.
//!
//! Every surface that processes (the inbox deck, process-now from the
//! ✎ capture and quick-add, the CLI, an agent over the wire) carries out
//! the same few outcomes, and each is a pair of ordinary lane calls:
//! make the thing it became, then write the capture back. This module
//! is the second half, once, so no surface can disagree with another
//! about what "snooze" or "done" leaves behind
//! (`docs/spec/capture-and-process.md`, "Outcomes").
//!
//! Three things every decision here holds to:
//!
//! - **Words are kept.** No decision touches `body`
//!   (`process.capture.verbatim`); a note made from a capture carries it
//!   verbatim beneath the rewrite ([`rewritten_note`]).
//! - **The back-link is written last.** [`Decision::Into`] names what the
//!   capture became, and is applied only after that thing exists, so a
//!   failure leaves the capture in the inbox, open.
//! - **Undo is honest.** Only decisions that made nothing elsewhere can
//!   be undone ([`Decision::undoable`]), and undoing writes the item back
//!   exactly as it was ([`undo`]).

use chrono::NaiveDate;

use crate::{InboxItem, ReviewResponse, review};

/// What a person decided a capture is — the write-back half of every
/// outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// It became something elsewhere: a task's path (a task, waiting on
    /// someone, do it now), `project:<id>` ([`project_ref`]), or a
    /// note's vault path (a rewritten note, reference). Written
    /// `processed`, pointing at it.
    // t[impl process.outcome.task]
    // t[impl process.outcome.waiting]
    // t[impl process.outcome.project]
    // t[impl process.outcome.note]
    // t[impl process.outcome.reference]
    Into(String),
    /// Already handled: `processed`, and it became nothing — recorded
    /// rather than deleted.
    // t[impl process.outcome.done]
    Done,
    /// Snoozed by spaced repetition: still `open`, not due until its new
    /// `resurface_on`. A suggestion put off is accepted in passing.
    // t[impl process.outcome.later]
    Later(ReviewResponse),
    /// A suggestion kept: `open`, in the trusted queue.
    // t[impl process.suggestion]
    Keep,
    /// Removed: the item's file goes (a suggestion dismissed, too).
    // t[impl process.outcome.delete]
    Delete,
}

/// The one write a decision makes on the Inbox lane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Write {
    /// `upsert_inbox_item` this item.
    Save(InboxItem),
    /// `delete_inbox_item` this id.
    Remove(String),
}

impl Decision {
    /// The write that carries this decision out on `item`, decided on
    /// `today`. Pure: the caller sends it.
    #[must_use]
    pub fn apply(&self, item: &InboxItem, today: NaiveDate) -> Write {
        let mut next = item.clone();
        match self {
            Self::Into(what) => {
                InboxItem::STATUS_PROCESSED.clone_into(&mut next.status);
                next.processed_into = Some(what.clone());
            }
            Self::Done => {
                InboxItem::STATUS_PROCESSED.clone_into(&mut next.status);
                next.processed_into = None;
            }
            Self::Later(response) => {
                let (interval, ease, resurface_on, reviews) = review(item, *response, today);
                InboxItem::STATUS_OPEN.clone_into(&mut next.status);
                next.interval = interval;
                next.ease = ease;
                next.resurface_on = Some(resurface_on);
                next.reviews = reviews;
            }
            Self::Keep => InboxItem::STATUS_OPEN.clone_into(&mut next.status),
            Self::Delete => return Write::Remove(item.id.clone()),
        }
        Write::Save(next)
    }

    /// Whether this decision offers Undo (`process.undo`): Later, Done
    /// and Delete, which made nothing anyone can see elsewhere. A task,
    /// project or note is work in another place; undoing it would mean
    /// deleting that.
    // t[impl process.undo]
    #[must_use]
    pub fn undoable(&self) -> bool {
        matches!(self, Self::Later(_) | Self::Done | Self::Delete)
    }
}

/// Undo an undoable decision: the item written back exactly as it was
/// before (`before`), recreated if it was deleted.
#[must_use]
pub fn undo(before: &InboxItem) -> Write {
    Write::Save(before.clone())
}

/// The `processed_into` of a capture that became a project.
#[must_use]
pub fn project_ref(id: impl std::fmt::Display) -> String {
    format!("project:{id}")
}

/// The walk back: the capture that became `what` (a task's path,
/// [`project_ref`], a note's path), if one did.
#[must_use]
pub fn origin_of<'a>(items: &'a [InboxItem], what: &str) -> Option<&'a InboxItem> {
    items.iter().find(|i| {
        i.status == InboxItem::STATUS_PROCESSED && i.processed_into.as_deref() == Some(what)
    })
}

/// A note's file name from its title: the title itself, minus what a
/// file name can't hold. A note is found by what it says, so the name is
/// the sentence, not an id.
#[must_use]
pub fn note_file_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if matches!(
                c,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']'
            ) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let squashed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name: String = squashed.chars().take(120).collect();
    if name.is_empty() {
        name = "Untitled note".to_owned();
    }
    name
}

/// The markdown for a rewritten note (`process.outcome.note`): tagged
/// `atomic`, placed under its parent (`folder`), the person's words, and
/// the original capture kept beneath, verbatim, as a quote — so the
/// source is never lost.
#[must_use]
pub fn rewritten_note(body: &str, original: &str, parent: Option<&str>, created: &str) -> String {
    let mut out = format!(
        "---\ntags: [atomic]\ncreated: {}\n",
        created.get(..10).unwrap_or(created)
    );
    if let Some(p) = parent {
        out.push_str(&format!("folder: \"[[{p}]]\"\n"));
    }
    out.push_str("---\n\n");
    let body = body.trim();
    if !body.is_empty() {
        out.push_str(body);
        out.push_str("\n\n");
    }
    out.push_str("> [!quote] Original capture\n");
    for l in original.lines() {
        out.push_str("> ");
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// Kebab-case a title into a vault-safe file stem.
#[must_use]
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let capped: String = out.trim_matches('-').chars().take(60).collect();
    let capped = capped.trim_matches('-').to_string();
    if capped.is_empty() {
        "note".to_string()
    } else {
        capped
    }
}

/// First non-empty line (capped) as the title; the remainder as the
/// body.
#[must_use]
pub fn split_title_body(body: &str) -> (String, String) {
    let trimmed = body.trim();
    let (first, rest) = trimmed.split_once('\n').unwrap_or((trimmed, ""));
    let title: String = first.trim().chars().take(120).collect();
    let title = if title.is_empty() {
        "Untitled".to_string()
    } else {
        title
    };
    (title, rest.trim().to_string())
}

/// Where "keep as reference" files a capture
/// (`process.outcome.reference`): `Wiki/Atomic/<slug>-<id6>.md`.
#[must_use]
pub fn reference_path(item: &InboxItem) -> String {
    let (title, _) = split_title_body(&item.body);
    format!(
        "Wiki/Atomic/{}-{}.md",
        slugify(&title),
        item.id.get(..6).unwrap_or("note")
    )
}

/// The markdown "keep as reference" writes: frontmatter over the whole
/// capture, verbatim.
#[must_use]
pub fn reference_markdown(item: &InboxItem, created: &str) -> String {
    let (title, _) = split_title_body(&item.body);
    let esc = title.replace('"', "'");
    format!(
        "---\ntitle: \"{esc}\"\ntype: atomic\ntags:\n  - atomic\ncreated: {created}\n---\n\n{}\n",
        item.body
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()
    }

    fn item() -> InboxItem {
        InboxItem::capture(
            "abcdef12-0000",
            "Call the tuner\nabout Thursday",
            "ui",
            "2026-10-19T09:00:00Z",
        )
    }

    fn saved(w: Write) -> InboxItem {
        match w {
            Write::Save(i) => i,
            Write::Remove(id) => panic!("removed {id}"),
        }
    }

    #[test]
    fn every_decision_keeps_the_words() {
        let it = item();
        for d in [
            Decision::Into("Tasks/x.md".into()),
            Decision::Done,
            Decision::Later(ReviewResponse::Good),
            Decision::Keep,
        ] {
            assert_eq!(saved(d.apply(&it, day())).body, it.body, "{d:?}");
        }
    }

    #[test]
    fn into_links_back_and_done_links_nothing() {
        let into = saved(Decision::Into("project:1".into()).apply(&item(), day()));
        assert_eq!(into.status, "processed");
        assert_eq!(into.processed_into.as_deref(), Some("project:1"));
        assert!(!into.is_due("2026-10-20"));
        let done = saved(Decision::Done.apply(&item(), day()));
        assert_eq!(done.status, "processed");
        assert_eq!(done.processed_into, None);
        assert_eq!(origin_of(&[into.clone(), done], "project:1"), Some(&into));
    }

    #[test]
    fn later_puts_it_off_and_accepts_a_suggestion() {
        let mut s = item();
        s.status = InboxItem::STATUS_SUGGESTED.into();
        let later = saved(Decision::Later(ReviewResponse::Easy).apply(&s, day()));
        assert_eq!(later.status, "open");
        assert!(later.resurface_on.as_deref().unwrap() > "2026-10-20");
        assert!(!later.is_due("2026-10-20"));
        assert_eq!(later.reviews, 1);
    }

    #[test]
    fn delete_removes_and_undo_restores_exactly() {
        let it = item();
        assert_eq!(
            Decision::Delete.apply(&it, day()),
            Write::Remove(it.id.clone())
        );
        assert_eq!(undo(&it), Write::Save(it));
        assert!(Decision::Delete.undoable() && Decision::Done.undoable());
        assert!(!Decision::Into("t".into()).undoable() && !Decision::Keep.undoable());
    }

    #[test]
    fn a_rewritten_note_keeps_the_original_and_names_its_parent() {
        let md = rewritten_note(
            "You can't write what you haven't read.",
            "writer's block = reader's block?",
            Some("Writing"),
            "2026-10-15T09:00:00Z",
        );
        assert!(md.contains("tags: [atomic]"));
        assert!(md.contains("folder: \"[[Writing]]\""), "{md}");
        assert!(md.contains("You can't write what you haven't read."));
        assert!(md.contains("> [!quote] Original capture\n> writer's block = reader's block?"));
        assert!(!rewritten_note("", "o", None, "2026-10-15").contains("folder:"));
    }

    #[test]
    fn a_note_is_named_by_what_it_says() {
        assert_eq!(
            note_file_name("Writer's block is reader's block"),
            "Writer's block is reader's block"
        );
        assert_eq!(note_file_name("a/b: c?"), "a b c");
        assert_eq!(note_file_name("   "), "Untitled note");
    }

    #[test]
    fn a_reference_carries_the_whole_capture() {
        let it = item();
        assert_eq!(reference_path(&it), "Wiki/Atomic/call-the-tuner-abcdef.md");
        assert!(reference_markdown(&it, "2026-10-20").contains(&it.body));
    }
}
