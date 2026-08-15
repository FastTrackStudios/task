//! Query-side row types. Kept separate from the proto so the
//! cache can record fields the wire format doesn't expose yet
//! (relative maildir path, content hash, FTS5 rank).

use email_proto::Envelope;
use serde::{Deserialize, Serialize};

/// One envelope as stored. `path` is the relative position
/// inside the account root (`"INBOX/cur/1700000000.M1.host:2,S"`);
/// `None` means the envelope came from a server only and isn't
/// yet mirrored to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEnvelope {
    pub envelope: Envelope,
    pub path: Option<String>,
    pub content_hash: Option<String>,
}

/// FTS5 hit with the matching envelope + the rank (lower = better).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub envelope: StoredEnvelope,
    pub rank: f64,
}
