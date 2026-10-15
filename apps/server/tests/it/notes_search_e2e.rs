#![allow(clippy::large_futures)]
//! The phone's notes search, end to end over a live `task-server`:
//! `VaultSync::search_text` finds what pages *say*, in the org's vault
//! and in a wiki (a wiki root is the vault `wiki:<slug>`), ranks a
//! title match first, never matches a property, and answers an empty
//! query with nothing.
//!
//! Boots over the example studio (see `support`), whose `acme-audio`
//! org plants the `music-theory` wiki.

use vault_proto::{IfMatch, VaultSyncClient};

use crate::support;

const WIKI_VAULT: &str = "wiki:music-theory";

async fn lane<C: vox_core::FromVoxLane + vox::MaybeSend>(url: &str) -> C {
    vox::connect_lane(url)
        .establish()
        .await
        .unwrap_or_else(|e| panic!("connect: {e:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn notes_and_wikis_are_searched_by_what_they_say() {
    let (url, _tmp) = support::boot_ws().await.expect("boot");
    let sync: VaultSyncClient = lane(&url).await;

    let put = |vault: &'static str, path: &'static str, body: &'static str| {
        let sync = sync.clone();
        async move {
            sync.put_file(
                vault.to_owned(),
                path.to_owned(),
                body.as_bytes().to_vec(),
                IfMatch::CreateOnly,
            )
            .await
            .unwrap_or_else(|e| panic!("put {path}: {e:?}"));
        }
    };
    put(
        "default",
        "Gig notes.md",
        "---\nvenue: Zanzibar\n---\n\nSoundcheck at four.\nThe zanzibar crowd wants the slow ones.\n",
    )
    .await;
    put("default", "Zanzibar.md", "Load-in through the side door.\n").await;
    put(
        WIKI_VAULT,
        "Concepts/Lydian.md",
        "# Lydian\n\nThe raised fourth gives the zanzibar shimmer.\n",
    )
    .await;

    // The vault: the page named for it first, then the one that says it
    // in its body — and the body line, not the `venue` property.
    let hits = sync
        .search_text("default".into(), "zanzibar".into(), 20)
        .await
        .expect("search the vault");
    let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
    assert_eq!(paths[..2], ["Zanzibar.md", "Gig notes.md"], "{paths:?}");
    let gig = &hits[1];
    assert_eq!(gig.excerpt, "The zanzibar crowd wants the slow ones.");
    assert_eq!(gig.matches, 1, "the property line is not text");

    // A wiki, through the same door.
    let wiki = sync
        .search_text(WIKI_VAULT.into(), "raised ZANZIBAR".into(), 20)
        .await
        .expect("search the wiki");
    assert_eq!(wiki.len(), 1, "{wiki:?}");
    assert_eq!(wiki[0].path, "Concepts/Lydian.md");
    assert_eq!(wiki[0].title, "Lydian");

    // Every word must match; an empty query finds nothing.
    let none = sync
        .search_text("default".into(), "zanzibar trombone".into(), 20)
        .await
        .unwrap();
    assert!(none.is_empty(), "{none:?}");
    let empty = sync
        .search_text("default".into(), "   ".into(), 20)
        .await
        .unwrap();
    assert!(empty.is_empty());
}
