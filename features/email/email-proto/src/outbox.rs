//! Outbox — the staged-send state machine (the human-in-the-loop
//! gate). Drafts are *staged* into the outbox; only an explicit
//! approval releases delivery, and delivery itself happens
//! asynchronously in a server-side poller. This is what lets an
//! agent DRAFT mail while only the user SENDS it.
//!
//! ```text
//! Draft ──submit──▶ PendingApproval ──approve──▶ Approved
//!                        │                          │ poller
//!                        └──cancel──▶ Cancelled     ▼
//!                                                Sending ──▶ Sent
//!                                                   │
//!                                                   └──▶ Failed(reason, retries)
//!                                                          │ backoff
//!                                                          └──▶ Sending (retry)
//! ```

use facet::Facet;
use serde::{Deserialize, Serialize};

/// Where one outbox entry sits in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum OutboxStatus {
    /// Staged but not yet submitted for approval. (Reserved —
    /// `submit_draft` currently creates entries directly in
    /// `PendingApproval`.)
    Draft,
    /// Waiting for a human to approve or cancel.
    PendingApproval,
    /// Approved; the delivery poller will pick it up.
    Approved,
    /// A poller pass is actively delivering it.
    Sending,
    /// Delivered — `sent_message_id` names the sent copy.
    Sent,
    /// Delivery failed; `last_error` + `retries` say why and how
    /// often. Retried with backoff until the retry cap, then it
    /// stays `Failed` (visible, re-approvable).
    Failed,
    /// Withdrawn before delivery.
    Cancelled,
}

impl OutboxStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::PendingApproval => "pending_approval",
            Self::Approved => "approved",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "draft" => Self::Draft,
            "pending_approval" => Self::PendingApproval,
            "approved" => Self::Approved,
            "sending" => Self::Sending,
            "sent" => Self::Sent,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

/// One staged outgoing message.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
pub struct OutboxEntry {
    /// Store-assigned id, unique per account.
    pub id: u64,
    /// Account the entry belongs to.
    pub account: String,
    pub status: OutboxStatus,
    /// The message to deliver.
    pub draft: crate::Draft,
    /// Who staged it (`"user"`, `"agent:<name>"`, …) — free-form
    /// provenance shown in the approval UI.
    pub origin: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    /// Delivery attempts so far.
    pub retries: u32,
    /// Unix-ms before which the poller won't retry a `Failed`
    /// entry. 0 = due immediately.
    pub next_attempt_ms: i64,
    pub last_error: Option<String>,
    /// Message-ID of the sent copy once `Sent`.
    pub sent_message_id: Option<String>,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{OutboxEntry, OutboxStatus};
    unsafe impl vox_types::Reborrow for OutboxStatus {
        type Ref<'a> = OutboxStatus;
    }
    unsafe impl vox_types::Reborrow for OutboxEntry {
        type Ref<'a> = OutboxEntry;
    }
}
