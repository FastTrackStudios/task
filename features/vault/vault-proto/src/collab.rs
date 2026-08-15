//! Per-file CRDT collaboration identity — **the** scheme mapping a
//! vault file to its collaborative doc id.
//!
//! Every vault file that is collaboratively edited gets exactly one
//! CRDT doc, addressed by a **UUID v5 of `(vault_id, path)`** under
//! the fixed [`VAULT_DOC_NAMESPACE`]. Both sides derive the same id
//! independently:
//!
//! - the **client** calls [`crate::VaultSync::open_collab`] (which
//!   validates the path and registers the reverse mapping) and gets
//!   the id back in a [`CollabAck`];
//! - the **server** uses [`collab_doc_id`] to reverse-route inbound
//!   file events (watcher / `put_file` by non-CRDT clients) to the
//!   matching open doc.
//!
//! ## Identity rules (v1)
//!
//! - The id is a pure function of `(vault_id, path)` — stable across
//!   restarts, machines, and transports. No table required to *mint*
//!   ids; the server-side registration map exists only so the doc
//!   registry's admission hook (which sees nothing but a `Uuid`) can
//!   reverse-resolve to a real file.
//! - **Renames move content to a new doc id.** A renamed file is a
//!   different `(vault_id, path)` and therefore a different doc; the
//!   old doc's history stays parked under the old id. Carrying CRDT
//!   history across renames is a separate issue (it needs a rename
//!   event on the `VaultSync` wire, which doesn't exist yet — the
//!   watcher reports renames as delete + create).
//! - Paths are compared byte-for-byte, exactly as the rest of the
//!   `VaultSync` surface treats them (`notes/a.md` ≠ `Notes/a.md`).
//!
//! ## Doc layout
//!
//! The doc is a *raw text doc*: the file's full UTF-8 content lives
//! in the root `LoroText` container [`COLLAB_TEXT_CONTAINER`].
//! Server-side bookkeeping (the write-behind's last-flushed sha)
//! lives in the root `LoroMap` [`COLLAB_META_CONTAINER`] — clients
//! ignore it.

use facet::Facet;
use uuid::Uuid;

/// Namespace for vault-file doc ids — the ASCII bytes
/// `task-vault-docid`. Fixed forever: changing it would silently remap
/// every file to a fresh (empty) doc.
pub const VAULT_DOC_NAMESPACE: Uuid = Uuid::from_bytes(*b"task-vault-docid");

/// Root `LoroText` container holding the file's full text. Both the
/// client editor bridge and the server write-behind address this
/// container by name.
pub const COLLAB_TEXT_CONTAINER: &str = "content";

/// Root `LoroMap` for server-side bookkeeping (not user content).
pub const COLLAB_META_CONTAINER: &str = "meta";

/// Key in [`COLLAB_META_CONTAINER`]: sha256 (hex) of the file content
/// the server write-behind last reconciled with disk. Persisted in
/// the doc so a re-opened doc can tell "file changed externally while
/// I was evicted" from "my last flush is still current".
pub const COLLAB_META_FLUSHED_SHA: &str = "flushed_sha";

/// The doc id for `(vault_id, path)` — UUID v5 under
/// [`VAULT_DOC_NAMESPACE`]. `vault_id` and `path` are joined with an
/// ASCII unit separator (`0x1f`, which is not legal in either) so
/// `("ab", "c.md")` and `("a", "bc.md")` can never collide.
#[must_use]
pub fn collab_doc_id(vault_id: &str, path: &str) -> Uuid {
    let mut name = Vec::with_capacity(vault_id.len() + 1 + path.len());
    name.extend_from_slice(vault_id.as_bytes());
    name.push(0x1f);
    name.extend_from_slice(path.as_bytes());
    Uuid::new_v5(&VAULT_DOC_NAMESPACE, &name)
}

/// Ack for [`crate::VaultSync::open_collab`]: the per-file doc id to
/// hand to `use_synced_doc_keyed` / the `DocSync` service, plus the
/// file's current sha (the content the doc was — or will be — seeded
/// from), so the caller can keep its sha bookkeeping coherent without
/// a second round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
pub struct CollabAck {
    pub doc_id: Uuid,
    /// sha256 (hex) of the file's bytes at registration time.
    pub sha256: String,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::CollabAck;
    unsafe impl vox_types::Reborrow for CollabAck {
        type Ref<'a> = CollabAck;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_spells_task_vault_docid() {
        assert_eq!(&VAULT_DOC_NAMESPACE.as_bytes()[..], b"task-vault-docid");
    }

    #[test]
    fn doc_id_is_stable() {
        // Pinned: this exact value is what every deployed peer derives
        // for ("default", "notes/a.md"). If this assertion ever needs
        // updating, existing docs are being silently orphaned.
        let id = collab_doc_id("default", "notes/a.md");
        assert_eq!(id, collab_doc_id("default", "notes/a.md"));
        assert_eq!(id.get_version(), Some(uuid::Version::Sha1));
        assert_eq!(id.to_string(), "224f49be-7b65-58aa-ba8a-7475177598d5");
    }

    #[test]
    fn doc_id_varies_by_vault_and_path() {
        let a = collab_doc_id("default", "a.md");
        assert_ne!(a, collab_doc_id("default", "b.md"));
        assert_ne!(a, collab_doc_id("other", "a.md"));
        // Rename ⇒ new identity (documented v1 behavior).
        assert_ne!(a, collab_doc_id("default", "renamed/a.md"));
    }

    #[test]
    fn separator_prevents_boundary_ambiguity() {
        assert_ne!(collab_doc_id("ab", "c.md"), collab_doc_id("a", "bc.md"));
    }
}
