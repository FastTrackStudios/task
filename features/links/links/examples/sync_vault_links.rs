//! Sync vault-note wikilinks into the typed-link graph.
//!
//! Walks every vault note, and for each `[[wikilink]]` creates a typed
//! link from the note. `[[John 3:16]]` / `[[John 3:16-18]]` →
//! `note → verse:<osis>` (`mentions`). `[[<video>#4:23-16:23]]` /
//! `[[<video>#1:30]]` (a `mm:ss` anchor) → `note → video:<slug>#t:…`
//! (`cites`).
//! Other wikilinks (note→note, headings, blocks) are left to the wiki
//! backlink machinery. This is what makes `[[…]]` references in prose
//! real edges in `/connections` and the watch view — the in-app
//! alternative to inline editor chips (which live in the Editor repo).
//!
//! Idempotent: replaces prior links stamped `vault-link-sync`.
//!
//! Run: cargo run -p links --example sync_vault_links -- [ORG_ROOT]

use std::collections::HashSet;
use std::path::PathBuf;

use links::Store;
use links_proto::{
    Confidence, LinksService, NodeKind, NodeRef, Relation, TypedLink, Visibility, parse_timecode,
};
use scripture_proto::VerseRange;
use vault_live::refs::Ref;
use vault_obsidian::Vault;

const SOURCE_REF: &str = "vault-link-sync";

/// A `mm:ss` / `mm:ss-mm:ss` heading → a video clip/point node.
fn video_target(slug: &str, heading: &str) -> Option<NodeRef> {
    if let Some((a, b)) = heading.split_once('-') {
        let (s, e) = (parse_timecode(a)?, parse_timecode(b)?);
        return Some(NodeRef::video(slug).clip(s, e));
    }
    parse_timecode(heading).map(|s| NodeRef::video(slug).at(s))
}

/// Resolve a wikilink (target + optional `#heading`) into a typed target.
fn resolve(target: &str, heading: Option<&str>) -> Option<(NodeRef, Relation)> {
    // A timecode anchor ⇒ a video clip reference.
    if let Some(h) = heading {
        if let Some(node) = video_target(target, h) {
            return Some((node, Relation::Cites));
        }
    }
    // Else: does the target name a scripture reference?
    if let Ok(vr) = VerseRange::parse(target) {
        return Some((NodeRef::verse(vr.osis()), Relation::Mentions));
    }
    None
}

fn main() {
    let org: PathBuf = std::env::args().nth(1).map_or_else(
        || PathBuf::from(std::env::var("HOME").unwrap()).join(".task/orgs/codywright"),
        PathBuf::from,
    );

    let vault = Vault::open(&org.join("vault")).expect("open vault");
    let store = Store::open(org.join("links.jsonl"));

    // Idempotent: drop links from a prior sync.
    let mut removed = 0usize;
    for l in store
        .graph(Confidence::Speculative, true)
        .expect("read links")
    {
        if l.provenance.source_ref == SOURCE_REF {
            store.delete(&l.id).expect("delete stale");
            removed += 1;
        }
    }

    let mut seen: HashSet<(String, String, &'static str)> = HashSet::new();
    let mut created = 0usize;
    for page in &vault.pages {
        let src = NodeRef::new(NodeKind::Note, page.rel_path.clone());
        for block in &page.parsed.blocks {
            for r in &block.refs {
                let Ref::Link(link) = r else { continue };
                let Some((target, relation)) =
                    resolve(link.target_linkpath.trim(), link.heading.as_deref())
                else {
                    continue;
                };
                // Dedup identical (note, target, relation) within the run.
                let key = (src.to_token(), target.to_token(), relation.as_str());
                if !seen.insert(key) {
                    continue;
                }
                let mut tl = TypedLink::new(src.clone(), target, relation, Confidence::Certain);
                tl.visibility = Visibility::Private; // prose notes are the private layer
                tl.provenance.created_by = SOURCE_REF.to_string();
                tl.provenance.source_ref = SOURCE_REF.to_string();
                tl.provenance.derived = true;
                store.create(tl).expect("create link");
                created += 1;
            }
        }
    }

    println!(
        "synced {created} note→verse/video links from {} pages (replaced {removed})",
        vault.pages.len()
    );
}
