//! Org-wide presence — the fixed doc id + timeout for the
//! Discord-style "who's online" channel.
//!
//! Each org has its own `LayerRouter` + [`crate::OrgAppState`], so a
//! single fixed doc id works: every client of an org joins the same
//! `DocPresence` channel, and orgs never share a host. The
//! [`crdt::sync::PresenceHost`] is not coupled to any `CrdtDoc` — it is
//! just a mirror `EphemeralStore` + fan-out, so no document backs this
//! id.
//!
//! NOTE: the constant is **duplicated** in
//! `crates/ui/src/presence.rs` (`ui::presence::PRESENCE_DOC_ID`) — the
//! UI crate can't depend on the server binary and no shared proto crate
//! fits a transport-level constant today. Keep the two in sync.

use crdt::sync::{DocPresence, SyncError};
use uuid::Uuid;

/// The org-wide presence channel's doc id. ASCII `task-org-presenc`
/// as a u128 — stable, greppable, and never colliding with a real
/// (random v4) doc id.
pub const PRESENCE_DOC_ID: Uuid = Uuid::from_u128(0x7461736b_2d6f_7267_2d70_726573656e63);

/// How long a peer's state survives without an update before the host
/// considers it gone (Loro's ephemeral timeout). Mirrors the client's
/// `use_presence_channel(PRESENCE_DOC_ID, 30_000)`.
pub const PRESENCE_TIMEOUT_MS: i64 = 30_000;

/// One mounted `DocPresence` service, two presence universes:
/// the **fixed** org-wide roster channel ([`PRESENCE_DOC_ID`], served
/// by the org's standalone `PresenceHost`) and the **per-vault-file**
/// cursor channels (any other doc id, routed into the vault-collab
/// `DocRegistry`, whose admission hook only accepts ids that
/// `open_collab` registered against a real file).
///
/// Needed because a `LayerRouter` mounts exactly one dispatcher per
/// service descriptor — this enum is that one dispatcher's backend.
#[derive(Clone)]
pub struct PresenceRouter {
    org: crdt::sync::PresenceHost,
    files: crdt::registry::DocRegistry,
}

impl PresenceRouter {
    #[must_use]
    pub fn new(org: crdt::sync::PresenceHost, files: crdt::registry::DocRegistry) -> Self {
        Self { org, files }
    }
}

impl DocPresence for PresenceRouter {
    async fn presence(
        &self,
        doc_id: Uuid,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<Vec<u8>>,
    ) -> Result<(), SyncError> {
        if doc_id == PRESENCE_DOC_ID {
            self.org.presence(doc_id, up, down).await
        } else {
            self.files.presence(doc_id, up, down).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_id_spells_task_org_presenc() {
        assert_eq!(&PRESENCE_DOC_ID.as_bytes()[..], b"task-org-presenc");
    }
}
