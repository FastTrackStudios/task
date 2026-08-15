//! Auto-sync note `[[wikilinks]]` → typed links, driven by vault change
//! events. Keeps the knowledge graph live as notes are saved — the
//! always-on counterpart to the `links` crate's `sync_vault_links`
//! example (which also handles video clips). Verse refs become
//! `note → verse` links; wikilinks that resolve to another vault page
//! become `note → note` links.

use std::path::{Path, PathBuf};
#[cfg(feature = "plugin-scripture")]
use std::sync::OnceLock;

use links::{Confidence, LinksService, NodeKind, NodeRef, Relation, Store, TypedLink, Visibility};
#[cfg(feature = "plugin-scripture")]
use regex::Regex;
// Verse-ref recognition rides the scripture plugin; a build without it
// still syncs note→note wikilinks, it just never mints `note → verse`
// edges (see `is_verse_ref` / the cfg'd arm in `sync_note`).
#[cfg(feature = "plugin-scripture")]
use scripture::VerseRange;
use tokio::sync::broadcast;
use vault::VaultGraph as _;
use vault_proto::VaultEvent;

/// Provenance tag for links this sync owns — so a re-sync replaces only
/// its own links, never user-authored ones. Matches `sync_vault_links`.
const SOURCE_REF: &str = "vault-link-sync";

/// Mirrors `vault_obsidian::obsidian_parse::LINK_REGEX` (copied so the
/// server doesn't pull the obsidian feature just for the pattern):
/// g1 = target, g2 = `#^block`, g3 = `#heading`, g4 = `|alias`.
#[cfg(feature = "plugin-scripture")]
const LINK_REGEX: &str =
    r"\[\[([^\]\|#\^\r\n]+)(?:#\^([a-zA-Z0-9\-]+)|#([^\]\|\r\n]+))?(?:\|([^\]\r\n]+))?\]\]";

#[cfg(feature = "plugin-scripture")]
fn link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(LINK_REGEX).expect("LINK_REGEX is valid"))
}

fn is_md(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// Delete this note's previously auto-synced links.
fn remove_synced(store: &Store, src: &NodeRef) {
    if let Ok(links) = store.links_for(src.clone()) {
        for l in links {
            if l.provenance.source_ref == SOURCE_REF && &l.source == src {
                let _ = store.delete(&l.id);
            }
        }
    }
}

/// One auto-synced link, provenance-tagged so re-syncs replace it.
fn synced_link(src: &NodeRef, target: NodeRef) -> TypedLink {
    let mut link = TypedLink::new(src.clone(), target, Relation::Mentions, Confidence::Certain);
    link.visibility = Visibility::Private; // prose notes are the private layer
    link.provenance.created_by = SOURCE_REF.to_string();
    link.provenance.source_ref = SOURCE_REF.to_string();
    link.provenance.derived = true;
    link
}

/// Replace a note's auto-synced links from its current content: verse
/// refs from the body text, note→note edges from the vault link graph
/// (which owns Obsidian-style resolution — exact path, basename,
/// aliases). Unresolved wikilinks are skipped; they sync once the
/// target page exists and this note is next saved.
fn sync_note(store: &Store, graph: &vault::GraphBackend, rel_path: &str, content: &str) {
    let src = NodeRef::new(NodeKind::Note, rel_path);
    remove_synced(store, &src);

    #[cfg(feature = "plugin-scripture")]
    {
        let mut seen = std::collections::HashSet::new();
        for cap in link_re().captures_iter(content) {
            // Skip anchored links (`#^block` / `#heading`) — not verse refs.
            if cap.get(2).is_some() || cap.get(3).is_some() {
                continue;
            }
            let target = cap.get(1).map_or("", |m| m.as_str().trim());
            let Ok(range) = VerseRange::parse(target) else {
                continue;
            };
            let osis = range.osis();
            if !seen.insert(osis.clone()) {
                continue;
            }
            let _ = store.create(synced_link(&src, NodeRef::verse(osis)));
        }
    }
    #[cfg(not(feature = "plugin-scripture"))]
    let _ = content;

    // FUTURE: `links()` rebuilds the vault LinkIndex per save — O(vault).
    // Fine at MVP scale; make it incremental if vaults grow.
    let mut seen_notes = std::collections::HashSet::new();
    for l in graph.links(VAULT_ID, rel_path).unwrap_or_default() {
        let Some(resolved) = l.resolved else { continue };
        if resolved == rel_path {
            continue; // self-link
        }
        // Verse refs are already synced above; don't double-edge when a
        // page happens to shadow a verse name (`John 3:16.md`).
        #[cfg(feature = "plugin-scripture")]
        {
            let sans_anchor = l.linkpath.split('#').next().unwrap_or("").trim();
            if VerseRange::parse(sans_anchor).is_ok() {
                continue;
            }
        }
        if !seen_notes.insert(resolved.clone()) {
            continue;
        }
        let _ = store.create(synced_link(&src, NodeRef::new(NodeKind::Note, resolved)));
    }
}

/// The per-org vault id the server mounts everywhere (`GraphBackend::
/// single("default", …)` in `lib.rs`).
const VAULT_ID: &str = "default";

/// Subscribe to the vault's change broadcast and keep `note → verse` +
/// `note → note` links in sync as notes are saved (`Put`) or removed
/// (`Delete`).
pub fn spawn(
    store: Store,
    vault_root: PathBuf,
    graph: vault::GraphBackend,
    mut rx: broadcast::Receiver<VaultEvent>,
) {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(VaultEvent::Put { path, .. }) if is_md(&path) => {
                    if let Ok(content) = std::fs::read_to_string(vault_root.join(&path)) {
                        sync_note(&store, &graph, &path, &content);
                    }
                }
                Ok(VaultEvent::Delete { path }) if is_md(&path) => {
                    remove_synced(&store, &NodeRef::new(NodeKind::Note, path));
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => {}
            }
        }
    });
}

// Both tests assert the verse-edge half, which needs the scripture
// plugin compiled in.
#[cfg(all(test, feature = "plugin-scripture"))]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn targets(store: &Store, src: &NodeRef) -> Vec<String> {
        let mut t: Vec<String> = store
            .links_for(src.clone())
            .unwrap()
            .iter()
            .map(|l| l.target.to_token())
            .collect();
        t.sort();
        t
    }

    #[test]
    fn syncs_verse_wikilinks_deduped_and_replaceable() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("vault");
        std::fs::create_dir_all(&vault_root).unwrap();
        let graph = vault::GraphBackend::single(VAULT_ID, vault_root);
        let store = Store::open(dir.path().join("links.jsonl"));
        let src = NodeRef::new(NodeKind::Note, "Notes/study.md");

        sync_note(
            &store,
            &graph,
            "Notes/study.md",
            "See [[John 3:16]] and again [[John 3:16]], plus [[Romans 5:8]], \
             a plain [[Some Note]], and a block ref [[John 3:16#^abc]].",
        );
        // Deduped, verses only ([[Some Note]] resolves to nothing in an
        // empty vault; block-anchored ref dropped).
        assert_eq!(
            targets(&store, &src),
            vec!["verse:John.3.16", "verse:Rom.5.8"]
        );

        // Re-sync with fewer refs replaces the prior set (stale removed).
        sync_note(&store, &graph, "Notes/study.md", "Only [[John 3:16]] now.");
        let after = store.links_for(src).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].target.to_token(), "verse:John.3.16");
    }

    #[test]
    fn syncs_note_wikilinks_resolved_against_vault() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("vault");
        let content = "Links [[Some Note]] twice: [[Some Note|alias]], itself \
                       [[study]], a heading [[Some Note#Intro]], a ghost \
                       [[No Such Page]], and [[John 3:16]].";
        write(&vault_root, "Notes/study.md", content);
        write(&vault_root, "Pages/Some Note.md", "target");
        let graph = vault::GraphBackend::single(VAULT_ID, vault_root);
        let store = Store::open(dir.path().join("links.jsonl"));
        let src = NodeRef::new(NodeKind::Note, "Notes/study.md");

        sync_note(&store, &graph, "Notes/study.md", content);
        // One note edge (deduped across alias + heading forms), self-link
        // and unresolved ghost dropped, verse still synced as a verse.
        assert_eq!(
            targets(&store, &src),
            vec!["note:Pages/Some Note.md", "verse:John.3.16"]
        );
    }
}
