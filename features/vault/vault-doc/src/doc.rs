//! The document side of the codec: a vault file's Loro document is a
//! `body` text and a `props` map ([`vault_proto::COLLAB_BODY_CONTAINER`],
//! [`vault_proto::COLLAB_PROPS_CONTAINER`]).
//!
//! The three operations every host needs, on the server and on a device
//! alike:
//!
//! - [`render`] — the document as markdown (the server's mirror, the
//!   desktop folder, export, an editor buffer);
//! - [`merge_text`] — a new text version of the file (a folder edit, an
//!   import, an editor keystroke, a service that thinks in whole files)
//!   merged into the document **as a change**: each property by key, the
//!   body character by character, against the version the text was made
//!   from. Never "the file wins": the document keeps every concurrent
//!   change the text did not itself make;
//! - [`migrate_whole_file`] — the one-time conversion of a document made
//!   before this shape (the whole file in one `content` text).
//!
//! Callers commit (and may set a commit message naming the origin first —
//! [`commit_as`]), so a merge and its bookkeeping can land as one change.

use crdt::loro::{LoroDoc, LoroMap, LoroText, LoroValue, ValueOrContainer};
use vault_proto::{COLLAB_BODY_CONTAINER, COLLAB_PROPS_CONTAINER, COLLAB_TEXT_CONTAINER};

use crate::body::{apply_ops_clamped, diff_ops};
use crate::markdown::{self, Props};

/// A new, empty document (for callers that hold no `crdt` dependency of
/// their own).
#[must_use]
pub fn new_doc() -> LoroDoc {
    LoroDoc::new()
}

// t[impl offline.doc.shape] — `body` text + `props` map
/// The document's body text.
#[must_use]
pub fn body_text(doc: &LoroDoc) -> LoroText {
    doc.get_text(COLLAB_BODY_CONTAINER)
}

/// The document's props map.
#[must_use]
pub fn props_map(doc: &LoroDoc) -> LoroMap {
    doc.get_map(COLLAB_PROPS_CONTAINER)
}

/// The body as a string.
#[must_use]
pub fn body(doc: &LoroDoc) -> String {
    body_text(doc).to_string()
}

/// Every property's current value. Child containers (none are written by
/// this codec) are skipped.
#[must_use]
pub fn props(doc: &LoroDoc) -> Props {
    let mut out = Props::new();
    props_map(doc).for_each(|k, v| {
        if let ValueOrContainer::Value(v) = v {
            out.insert(k.to_owned(), v);
        }
    });
    out
}

/// One property, if set.
#[must_use]
pub fn prop(doc: &LoroDoc, key: &str) -> Option<LoroValue> {
    match props_map(doc).get(key)? {
        ValueOrContainer::Value(v) => Some(v),
        ValueOrContainer::Container(_) => None,
    }
}

/// Set one property (a no-op when it already holds `value`). Uncommitted.
pub fn set_prop(doc: &LoroDoc, key: &str, value: LoroValue) {
    if prop(doc, key).as_ref() == Some(&value) {
        return;
    }
    if let Err(e) = props_map(doc).insert(key, value) {
        tracing::warn!(key, "vault-doc: set prop failed: {e}");
    }
}

/// Remove one property. Uncommitted.
pub fn remove_prop(doc: &LoroDoc, key: &str) {
    if prop(doc, key).is_some() {
        if let Err(e) = props_map(doc).delete(key) {
            tracing::warn!(key, "vault-doc: delete prop failed: {e}");
        }
    }
}

/// Whether the document holds nothing authored yet (no body, no props).
#[must_use]
pub fn is_blank(doc: &LoroDoc) -> bool {
    body_text(doc).len_unicode() == 0 && props_map(doc).is_empty()
}

// t[impl offline.doc.materialise] — the one conversion every writer of
// markdown uses (server mirror, editor buffer, desktop folder, export)
/// The document as markdown. `previous` is the file as it stands (the
/// mirror on disk, the editor's buffer), whose unchanged frontmatter
/// lines are kept verbatim — so rendering an unchanged document gives
/// back exactly `previous`.
#[must_use]
pub fn render(doc: &LoroDoc, previous: Option<&str>) -> String {
    markdown::materialise(&props(doc), &body(doc), previous)
}

/// What a merge changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Merged {
    pub props: bool,
    pub body: bool,
}

impl Merged {
    #[must_use]
    pub const fn any(self) -> bool {
        self.props || self.body
    }
}

// t[impl offline.doc.materialise] — text coming in becomes a change
/// Merge a new text version of the file into the document, as a change.
///
/// `base` is the text the new version was made from (what the editor
/// buffer held before the keystroke, what the mirror last wrote before
/// the folder edit). Only what differs between `base` and `new` is
/// applied: each property that changed is set (or removed) by key, and
/// the body's edit is applied at its place in the document's current
/// body. Everything else the document holds — concurrent edits from
/// other replicas — stays.
///
/// A new version whose frontmatter does not parse leaves the props as
/// they are (a half-typed key is not "every property deleted"). Not
/// committed — see [`commit_as`].
pub fn merge_text(doc: &LoroDoc, base: &str, new: &str) -> Merged {
    if base == new {
        return Merged::default();
    }
    let b = markdown::parse(base);
    let n = markdown::parse(new);
    let mut merged = Merged::default();

    if let (Some(bp), Some(np)) = (b.props(), n.props()) {
        for (k, v) in &np {
            if bp.get(k) != Some(v) {
                set_prop(doc, k, v.clone());
                merged.props = true;
            }
        }
        for k in bp.keys() {
            if !np.contains_key(k) {
                remove_prop(doc, k);
                merged.props = true;
            }
        }
    }

    if b.body != n.body {
        let ops = diff_ops(b.body, n.body);
        apply_ops_clamped(&body_text(doc), &ops);
        merged.body = true;
    }
    merged
}

/// Two-way variant: make the document read as `new`, merging against
/// what the document renders now. For seeding a blank document from a
/// file, and for adopting a file whose base version is unknown.
pub fn adopt_text(doc: &LoroDoc, new: &str) -> Merged {
    let base = render(doc, Some(new));
    merge_text(doc, &base, new)
}

/// The document was deleted — its tombstone is set
/// (`vault_proto::COLLAB_META_DELETED`).
#[must_use]
pub fn is_deleted(doc: &LoroDoc) -> bool {
    matches!(
        doc.get_map(vault_proto::COLLAB_META_CONTAINER)
            .get(vault_proto::COLLAB_META_DELETED),
        Some(ValueOrContainer::Value(LoroValue::Bool(true)))
    )
}

/// Set or clear the tombstone. A no-op when it already says so, so a
/// delete seen twice (a watcher's duplicate, a peer's echo) commits
/// nothing. Not committed.
pub fn set_deleted(doc: &LoroDoc, deleted: bool) {
    if is_deleted(doc) == deleted {
        return;
    }
    let _ = doc
        .get_map(vault_proto::COLLAB_META_CONTAINER)
        .insert(vault_proto::COLLAB_META_DELETED, deleted);
}

/// The commit message of a document made from a file ([`seed_from_file`]).
pub const SEED_ORIGIN: &str = "made from the vault file";

// t[impl offline.peers] — one file, one document, on every node
/// Make an **empty** document from a file's `text` as the same change on
/// every node that does it from the same bytes: same author (a peer id
/// derived from `doc_id` and the text), same counters, same message, no
/// timestamp. Two nodes that each made the document from the file — the
/// same seed planted on both, a file that arrived by copy — then hold one
/// history rather than two documents of one name, and their documents
/// exchange instead of being refused as foreign (`offline.peers`).
/// Changes made afterwards are the node's own: the author is put back.
///
/// A document with any history is not empty, and gets an ordinary
/// [`adopt_text`]: the same op ids over a different history would be two
/// different changes claiming to be one. Committed either way.
pub fn seed_from_file(doc: &LoroDoc, doc_id: uuid::Uuid, text: &str) {
    let empty = doc.oplog_vv().iter().all(|(_, c)| *c == 0);
    let own = doc.peer_id();
    let seeded = empty && doc.set_peer_id(seed_peer(doc_id, text)).is_ok();
    adopt_text(doc, text);
    commit_as(doc, SEED_ORIGIN);
    if seeded {
        let _ = doc.set_peer_id(own);
    }
}

/// FNV-1a over the doc id and the text: stable across machines, builds
/// and platforms (no `std` hasher is). The top bit is cleared so it is
/// never `PeerID::MAX`, which Loro reserves.
fn seed_peer(doc_id: uuid::Uuid, text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in doc_id.as_bytes().iter().chain(text.as_bytes()) {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h >> 1
}

/// Commit pending changes under a message naming where they came from
/// (`"folder"`, `"import"`, `"task-service"`, …) — the attribution the
/// document's history shows. A no-op when nothing is pending.
pub fn commit_as(doc: &LoroDoc, origin: &str) {
    doc.set_next_commit_message(origin);
    doc.commit();
}

/// Convert a document made before the `body` + `props` shape — the whole
/// file in one `content` text — into the current shape, once. The legacy
/// text is parsed as a file; its frontmatter becomes props, its body the
/// body; the legacy text is then emptied so a stale client typing into
/// it changes nothing that is rendered. Returns whether it converted.
/// Not committed.
pub fn migrate_whole_file(doc: &LoroDoc) -> bool {
    let legacy = doc.get_text(COLLAB_TEXT_CONTAINER);
    if legacy.len_unicode() == 0 || !is_blank(doc) {
        return false;
    }
    let whole = legacy.to_string();
    adopt_text(doc, &whole);
    if let Err(e) = legacy.delete(0, legacy.len_unicode()) {
        tracing::warn!("vault-doc: clearing legacy content failed: {e}");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\ntitle: Mix\nstatus: open\n---\nLine one\n";

    fn doc_from(text: &str) -> LoroDoc {
        let doc = LoroDoc::new();
        adopt_text(&doc, text);
        commit_as(&doc, "seed");
        doc
    }

    // t[verify offline.peers] — two nodes that make a document from the
    // same file make one document; different files stay two
    #[test]
    fn the_same_file_seeds_the_same_document_everywhere() {
        let id = uuid::Uuid::new_v4();
        let (a, b) = (LoroDoc::new(), LoroDoc::new());
        seed_from_file(&a, id, NOTE);
        seed_from_file(&b, id, NOTE);
        assert_eq!(a.oplog_vv(), b.oplog_vv(), "one history, not two");
        assert!(!crate::is_foreign_lineage(&a.oplog_vv(), &b.oplog_vv()));
        // Importing one into the other is a no-op, not a second copy.
        a.import(&b.export(crdt::loro::ExportMode::Snapshot).unwrap())
            .unwrap();
        assert_eq!(render(&a, Some(NOTE)), NOTE);

        // What either does next is its own, and merges.
        let own_a = a.peer_id();
        merge_text(&a, NOTE, &NOTE.replace("Line one\n", "Line one\nfrom a\n"));
        commit_as(&a, "a");
        assert_ne!(
            a.oplog_vv().get(&own_a).copied().unwrap_or(0),
            0,
            "the edit was made as the node, not as the seed"
        );
        b.import(&a.export(crdt::loro::ExportMode::Snapshot).unwrap())
            .unwrap();
        assert_eq!(render(&b, None), render(&a, None));

        // Another file is another document.
        let c = LoroDoc::new();
        seed_from_file(&c, id, "---\ntitle: Other\n---\nx\n");
        assert!(crate::is_foreign_lineage(&c.oplog_vv(), &b.oplog_vv()));
    }

    // t[verify offline.doc.shape] — a file becomes body + props, and
    // renders back unchanged
    #[test]
    fn a_seeded_document_renders_its_file() {
        let doc = doc_from(NOTE);
        assert_eq!(body(&doc), "Line one\n");
        assert_eq!(prop(&doc, "status"), Some(LoroValue::from("open")));
        assert_eq!(render(&doc, Some(NOTE)), NOTE);
        // With no file to keep the order of, keys come out sorted.
        assert_eq!(
            render(&doc, None),
            "---\nstatus: open\ntitle: Mix\n---\nLine one\n"
        );
    }

    // t[verify offline.doc.materialise] — text coming in is a change:
    // only what it changed is applied, concurrent edits stay
    #[test]
    fn an_outside_edit_merges_as_a_change() {
        let doc = doc_from(NOTE);
        // Meanwhile, in the app: someone appends to the body and renames.
        body_text(&doc).push_str("Line two\n").unwrap();
        set_prop(&doc, "title", LoroValue::from("Mix v2"));
        doc.commit();
        // The folder copy was made from NOTE: it changes the status and
        // prepends a line.
        let edited = "---\ntitle: Mix\nstatus: done\n---\nIntro\nLine one\n";
        let m = merge_text(&doc, NOTE, edited);
        assert!(m.props && m.body);
        commit_as(&doc, "folder");
        assert_eq!(
            render(&doc, Some(edited)),
            "---\ntitle: Mix v2\nstatus: done\n---\nIntro\nLine one\nLine two\n"
        );
    }

    #[test]
    fn a_broken_frontmatter_loses_no_text_and_heals() {
        let doc = doc_from(NOTE);
        let broken = "---\ntitle: [Mix\nstatus: open\n---\nLine one!\n";
        merge_text(&doc, NOTE, broken);
        doc.commit();
        // Every character is still there (as body) and renders back.
        assert_eq!(render(&doc, Some(broken)), broken);
        // Fixed, the props come back and the body is the body again.
        let fixed = "---\ntitle: Mix\nstatus: open\n---\nLine one!\n";
        merge_text(&doc, broken, fixed);
        doc.commit();
        assert_eq!(prop(&doc, "title"), Some(LoroValue::from("Mix")));
        assert_eq!(body(&doc), "Line one!\n");
        assert_eq!(render(&doc, Some(fixed)), fixed);
    }

    #[test]
    fn two_replicas_merge_props_by_key_and_body_by_character() {
        let a = doc_from(NOTE);
        let b = LoroDoc::new();
        b.import(&a.export(crdt::loro::ExportMode::Snapshot).unwrap())
            .unwrap();
        // Offline on both: A ticks the status, B renames and both type.
        merge_text(
            &a,
            NOTE,
            "---\ntitle: Mix\nstatus: done\n---\nLine one\nfrom A\n",
        );
        a.commit();
        merge_text(
            &b,
            NOTE,
            "---\ntitle: Final mix\nstatus: open\n---\nfrom B\nLine one\n",
        );
        b.commit();
        a.import(&b.export(crdt::loro::ExportMode::Snapshot).unwrap())
            .unwrap();
        b.import(&a.export(crdt::loro::ExportMode::Snapshot).unwrap())
            .unwrap();
        let (ra, rb) = (render(&a, None), render(&b, None));
        assert_eq!(ra, rb);
        assert_eq!(
            ra,
            "---\nstatus: done\ntitle: Final mix\n---\nfrom B\nLine one\nfrom A\n"
        );
    }

    #[test]
    fn a_whole_file_document_converts_once() {
        let doc = LoroDoc::new();
        doc.get_text(COLLAB_TEXT_CONTAINER).insert(0, NOTE).unwrap();
        doc.commit();
        assert!(migrate_whole_file(&doc));
        doc.commit();
        assert_eq!(render(&doc, Some(NOTE)), NOTE);
        assert_eq!(doc.get_text(COLLAB_TEXT_CONTAINER).to_string(), "");
        assert!(!migrate_whole_file(&doc), "only once");
    }
}
