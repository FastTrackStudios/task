//! Inbox: capture, list, process.

use std::sync::Arc;

use architect::action::{ActionError, ActionList, ActionMeta, Effect};
use facet::Facet;
use inbox_proto::{Inbox as _, InboxItem};

use super::{Ctx, meta, note_writer, present, register_blocking, who};
use crate::mcp::{backend_err_text, clamp_limit};

pub(super) static CAPTURE: ActionMeta = meta(
    "INBOX_CAPTURE",
    "capture_inbox",
    "Capture to inbox",
    "inbox",
    "Capture a thought, reminder, or follow-up into the inbox — the right home for anything \
     not yet shaped into a task. Captures land as 'suggested' for the user to accept with \
     one tap, so capture freely; pass accepted: true only when the user explicitly asked \
     for it.",
);

pub(super) static LIST: ActionMeta = meta(
    "INBOX_LIST",
    "list_inbox",
    "List inbox",
    "inbox",
    "What's waiting in the inbox for review today — the user's unprocessed captures. Useful \
     for 'what did I say I'd look at'. Pass status 'suggested' to see captures (including \
     your own) still awaiting the user's accept.",
);

pub(super) static PROCESS: ActionMeta = meta(
    "INBOX_PROCESS",
    "process_inbox_item",
    "Process inbox item",
    "inbox",
    "Move an inbox item through its lifecycle: 'open' accepts a suggestion into the trusted \
     queue, 'processed' marks it handled (e.g. after you turned it into a task), 'archived' \
     dismisses it. Get ids from list_inbox.",
);

pub(super) fn register(list: &ActionList, ctx: Option<&Arc<Ctx>>) -> Result<(), ActionError> {
    register_blocking(list, &CAPTURE, Effect::Mutating, ctx, |ctx, args| {
        let id = uuid::Uuid::new_v4().to_string();
        capture(
            ctx,
            args,
            Stamp {
                id,
                now: chrono::Utc::now().to_rfc3339(),
            },
        )
    })?;
    register_blocking(list, &LIST, Effect::ReadOnly, ctx, |ctx, args| {
        list_inbox(
            ctx,
            args,
            &chrono::Local::now().format("%Y-%m-%d").to_string(),
        )
    })?;
    register_blocking(list, &PROCESS, Effect::Mutating, ctx, process)
}

// ── capture_inbox ────────────────────────────────────────────────

#[derive(Facet, Debug)]
pub struct Capture {
    /// The note, verbatim. Markdown.
    body: String,
    /// Skip the suggestion queue and file it as an open item.
    accepted: Option<bool>,
    /// ISO date to bring it back up (a reminder).
    resurface_on: Option<String>,
}

#[derive(Facet, Debug)]
pub struct Captured {
    id: String,
    status: String,
    resurface_on: Option<String>,
    note: String,
}

/// A new item's identity and capture time, minted by the caller.
struct Stamp {
    id: String,
    now: String,
}

fn capture(ctx: &Ctx, args: Capture, stamp: Stamp) -> Result<Captured, String> {
    let body = present(Some(args.body)).ok_or("`body` is required")?;
    // The surface; who captured it is `created_by`.
    let mut item = InboxItem::capture(stamp.id, body, "mcp", stamp.now);
    let accepted = args.accepted.unwrap_or(false);
    if !accepted {
        item.status = InboxItem::STATUS_SUGGESTED.to_string();
    }
    item.resurface_on = present(args.resurface_on);
    crate::attribution::attribute_inbox(&mut item, ctx.creator.as_ref());
    ctx.org
        .inbox
        .upsert_inbox_item(&item)
        .map_err(|e| format!("{e:?}"))?;
    let note = if accepted {
        "filed as an open inbox item"
    } else {
        "captured as a suggestion — the user accepts or dismisses it in their inbox"
    };
    Ok(Captured {
        id: item.id,
        status: item.status,
        resurface_on: item.resurface_on,
        note: note.to_owned(),
    })
}

// ── list_inbox ───────────────────────────────────────────────────

#[derive(Facet, Debug)]
pub struct ListInbox {
    /// Show every item with this status instead of today's review queue, e.g. 'suggested', 'open'.
    status: Option<String>,
    /// Keep items captured by this account — a created_by ref from a listed item ('bot:<uuid>', 'user:<uuid>'), a bare account uuid, 'me', or 'none'. Searches every item, not only today's queue, unless status narrows it.
    created_by: Option<String>,
    /// Keep items a personal assistant bot captured on behalf of this person — 'me', a 'user:<uuid>' ref, a bare uuid, or 'none'. Searches every item, like created_by.
    on_behalf_of: Option<String>,
    /// Max rows (default 50, max 200).
    limit: Option<u64>,
}

#[derive(Facet, Debug)]
pub struct InboxList {
    count: usize,
    items: Vec<InboxRow>,
}

#[derive(Facet, Debug)]
pub struct InboxRow {
    id: String,
    body: String,
    kind: String,
    status: String,
    created: String,
    source: String,
    created_by: Option<String>,
    created_by_name: Option<String>,
    on_behalf_of: Option<String>,
    on_behalf_of_name: Option<String>,
}

impl From<InboxItem> for InboxRow {
    fn from(i: InboxItem) -> Self {
        Self {
            id: i.id,
            body: i.body,
            kind: i.kind,
            status: i.status,
            created: i.created,
            source: i.source,
            created_by: i.created_by,
            created_by_name: i.created_by_name,
            on_behalf_of: i.on_behalf_of,
            on_behalf_of_name: i.on_behalf_of_name,
        }
    }
}

fn list_inbox(ctx: &Ctx, args: ListInbox, today: &str) -> Result<InboxList, String> {
    let creator = ctx.creator.as_ref();
    let created_by = who(args.created_by, creator);
    let on_behalf_of = who(args.on_behalf_of, creator);
    let rows = inbox_rows(
        ctx,
        present(args.status),
        created_by.is_some() || on_behalf_of.is_some(),
        today,
    )?;
    let items: Vec<InboxRow> = rows
        .into_iter()
        .filter(|i| {
            created_by
                .as_deref()
                .is_none_or(|w| i.created_by_matches(w))
        })
        .filter(|i| {
            on_behalf_of
                .as_deref()
                .is_none_or(|w| i.on_behalf_of_matches(w))
        })
        .take(clamp_limit(args.limit))
        .map(InboxRow::from)
        .collect();
    Ok(InboxList {
        count: items.len(),
        items,
    })
}

/// The items a listing starts from. The review queue is "what surfaces
/// today", which by design excludes suggestions — so an agent couldn't
/// see its own captures. A `status` opens the full list, and so does
/// asking whose captures (`narrowed`), whatever their state.
fn inbox_rows(
    ctx: &Ctx,
    status: Option<String>,
    narrowed: bool,
    today: &str,
) -> Result<Vec<InboxItem>, String> {
    let inbox = &ctx.org.inbox;
    let all = || inbox.list_inbox().map_err(|e| format!("{e:?}"));
    match (status, narrowed) {
        (Some(status), _) => Ok(all()?
            .into_iter()
            .filter(|i| i.status.eq_ignore_ascii_case(&status))
            .collect()),
        (None, true) => all(),
        (None, false) => inbox
            .review_queue(today.to_owned())
            .map_err(|e| format!("{e:?}")),
    }
}

// ── process_inbox_item ───────────────────────────────────────────

#[derive(Facet, Debug)]
pub struct Process {
    /// Inbox item id from list_inbox.
    id: String,
    /// 'open' | 'processed' | 'archived'.
    status: String,
}

#[derive(Facet, Debug)]
pub struct Processed {
    id: String,
    status: String,
    was: String,
}

fn process(ctx: &Ctx, args: Process) -> Result<Processed, String> {
    note_writer(ctx, "task:new".to_owned());
    let id = present(Some(args.id)).ok_or("`id` is required")?;
    let status = present(Some(args.status))
        .ok_or("`status` is required")?
        .to_lowercase();
    let allowed = [
        InboxItem::STATUS_OPEN,
        InboxItem::STATUS_PROCESSED,
        InboxItem::STATUS_ARCHIVED,
    ];
    if !allowed.contains(&status.as_str()) {
        return Err(format!(
            "`status` must be one of open, processed, archived — got `{status}`"
        ));
    }
    let inbox = &ctx.org.inbox;
    let mut item = inbox
        .get_inbox_item(&id)
        .map_err(|e| backend_err_text("inbox item", &id, &e))?;
    let was = std::mem::replace(&mut item.status, status);
    inbox
        .upsert_inbox_item(&item)
        .map_err(|e| backend_err_text("inbox item", &id, &e))?;
    Ok(Processed {
        id: item.id,
        status: item.status,
        was,
    })
}
