//! Envelope — the header-only summary of a message. List views
//! render envelopes; full bodies are fetched lazily via
//! [`crate::EmailSync::fetch_message`].

use facet::Facet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
pub struct Addr {
    pub name: Option<String>,
    pub email: String,
}

/// Message summary returned by
/// [`crate::EmailSync::fetch_envelopes`]. `flags` is a free-form
/// list of strings — IMAP keyword flags and backend custom
/// labels alike. `date` is a unix-ms timestamp; the proto
/// avoids carrying `chrono` types over the wire.
#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct Envelope {
    pub message_id: String,
    pub folder: String,
    pub thread_id: Option<String>,
    pub subject: String,
    pub from: Vec<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub date_ms: i64,
    pub flags: Vec<String>,
    pub has_attachments: bool,
    pub size: u64,
    pub snippet: Option<String>,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{Addr, Envelope};
    unsafe impl vox_types::Reborrow for Addr {
        type Ref<'a> = Addr;
    }
    unsafe impl vox_types::Reborrow for Envelope {
        type Ref<'a> = Envelope;
    }
}
