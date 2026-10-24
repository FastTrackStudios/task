//! Inbox actions: `capture_inbox`, `list_inbox`, `process_inbox_item`.
//!
//! Backed by the org's `inbox_proto::Inbox`, a per-org rpc trait, so
//! registered by hand (see the module docs of [`super`]).

use architect::action::{ActionError, ActionList, ActionMeta, Effect};
use facet::Facet;
use serde_json::{Value, json};

use super::{CallContext, limit, meta, required, text, who};
use crate::mcp::{ToolFailure, backend_err};

/// `capture_inbox`.
pub static CAPTURE: ActionMeta = meta(
    "INBOX_CAPTURE",
    "capture",
    "Capture to the inbox",
    "Inbox",
    "Capture a thought, reminder, or follow-up into the inbox — the right home for anything \
     not yet shaped into a task. Captures land as 'suggested' for the user to accept with one \
     tap, so capture freely; pass accepted: true only when the user explicitly asked for it.",
);

/// `list_inbox`.
pub static LIST: ActionMeta = meta(
    "INBOX_LIST",
    "list",
    "List the inbox",
    "Inbox",
    "What's waiting in the inbox for review today — the user's unprocessed captures. Useful \
     for 'what did I say I'd look at'. Pass status 'suggested' to see captures (including your \
     own) still awaiting the user's accept.",
);

/// `process_inbox_item`.
pub static PROCESS: ActionMeta = meta(
    "INBOX_PROCESS",
    "process",
    "Process an inbox item",
    "Inbox",
    "Move an inbox item through its lifecycle: 'open' accepts a suggestion into the trusted \
     queue, 'processed' marks it handled (e.g. after you turned it into a task), 'archived' \
     dismisses it. Get ids from list_inbox.",
);

/// A capture.
#[derive(Facet)]
pub struct CaptureArgs {
    /// The note, verbatim. Markdown.
    pub body: String,
    /// Skip the suggestion queue and file it as an open item.
    pub accepted: Option<bool>,
    /// ISO date to bring it back up (a reminder).
    pub resurface_on: Option<String>,
}

/// Which inbox items to list.
#[derive(Facet)]
pub struct ListArgs {
    /// Show every item with this status instead of today's review queue, e.g. 'suggested', 'open'.
    pub status: Option<String>,
    /// Keep items captured by this account — a created_by ref from a listed item ('bot:<uuid>', 'user:<uuid>'), a bare account uuid, 'me', or 'none'. Searches every item, not only today's queue, unless status narrows it.
    pub created_by: Option<String>,
    /// Keep items a personal assistant bot captured on behalf of this person — 'me', a 'user:<uuid>' ref, a bare uuid, or 'none'. Searches every item, like created_by.
    pub on_behalf_of: Option<String>,
    /// Max rows (default 50, max 200).
    pub limit: Option<u64>,
}

/// An item's next state.
#[derive(Facet)]
pub struct ProcessArgs {
    /// Inbox item id from list_inbox.
    pub id: String,
    /// 'open' | 'processed' | 'archived'.
    pub status: String,
}

/// Put the inbox actions on `list`.
///
/// # Errors
///
/// [`ActionError::Duplicate`] when one is already there.
pub fn register(list: &ActionList) -> Result<(), ActionError> {
    super::register(list, &CAPTURE, Effect::Mutating, capture)?;
    super::register(list, &LIST, Effect::ReadOnly, list_inbox)?;
    super::register(list, &PROCESS, Effect::Mutating, process)
}

fn capture(ctx: &CallContext, args: CaptureArgs) -> Result<Value, ToolFailure> {
    use inbox_proto::Inbox as _;
    let body = required(args.body, "body")?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut item = inbox_proto::InboxItem::capture(
        uuid::Uuid::new_v4().to_string(),
        body,
        // The surface; who captured it is `created_by`.
        "mcp",
        now,
    );
    if !args.accepted.unwrap_or(false) {
        item.status = inbox_proto::InboxItem::STATUS_SUGGESTED.to_string();
    }
    item.resurface_on = text(args.resurface_on);
    crate::attribution::attribute_inbox(&mut item, ctx.creator.as_ref());
    ctx.org
        .inbox
        .upsert_inbox_item(&item)
        .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
    Ok(json!({
        "id": item.id,
        "status": item.status,
        "resurface_on": item.resurface_on,
        "note": if item.status == inbox_proto::InboxItem::STATUS_SUGGESTED {
            "captured as a suggestion — the user accepts or dismisses it in their inbox"
        } else {
            "filed as an open inbox item"
        },
    }))
}

fn list_inbox(ctx: &CallContext, args: ListArgs) -> Result<Value, ToolFailure> {
    use inbox_proto::Inbox as _;
    let org = &ctx.org;
    // The review queue is "what surfaces today", which by design
    // excludes suggestions — so an agent couldn't see its own captures.
    // `status` opens the full list, and so does `created_by` (whose
    // captures, whatever their state).
    let created_by = who(args.created_by, ctx);
    let on_behalf_of = who(args.on_behalf_of, ctx);
    let narrowed = created_by.is_some() || on_behalf_of.is_some();
    let rows: Vec<inbox_proto::InboxItem> = match (text(args.status), narrowed) {
        (Some(status), _) => {
            let all = org
                .inbox
                .list_inbox()
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            all.into_iter()
                .filter(|i| i.status.eq_ignore_ascii_case(&status))
                .collect()
        }
        (None, true) => org
            .inbox
            .list_inbox()
            .map_err(|e| ToolFailure::Message(format!("{e:?}")))?,
        (None, false) => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            org.inbox
                .review_queue(today)
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?
        }
    };
    let out: Vec<Value> = rows
        .iter()
        .filter(|i| {
            created_by
                .as_deref()
                .is_none_or(|who| i.created_by_matches(who))
                && on_behalf_of
                    .as_deref()
                    .is_none_or(|who| i.on_behalf_of_matches(who))
        })
        .take(limit(args.limit))
        .map(|i| {
            json!({
                "id": i.id,
                "body": i.body,
                "kind": i.kind,
                "status": i.status,
                "created": i.created,
                "source": i.source,
                "created_by": i.created_by,
                "created_by_name": i.created_by_name,
                "on_behalf_of": i.on_behalf_of,
                "on_behalf_of_name": i.on_behalf_of_name,
            })
        })
        .collect();
    Ok(json!({ "count": out.len(), "items": out }))
}

fn process(ctx: &CallContext, args: ProcessArgs) -> Result<Value, ToolFailure> {
    use inbox_proto::Inbox as _;
    let id = required(args.id, "id")?;
    let status = required(args.status, "status")?.to_lowercase();
    let allowed = [
        inbox_proto::InboxItem::STATUS_OPEN,
        inbox_proto::InboxItem::STATUS_PROCESSED,
        inbox_proto::InboxItem::STATUS_ARCHIVED,
    ];
    if !allowed.contains(&status.as_str()) {
        return Err(ToolFailure::Message(format!(
            "`status` must be one of open, processed, archived — got `{status}`"
        )));
    }
    let mut item = ctx
        .org
        .inbox
        .get_inbox_item(&id)
        .map_err(|e| backend_err("inbox item", &id, &e))?;
    let was = item.status.clone();
    item.status = status;
    ctx.org
        .inbox
        .upsert_inbox_item(&item)
        .map_err(|e| backend_err("inbox item", &id, &e))?;
    Ok(json!({ "id": item.id, "status": item.status, "was": was }))
}
