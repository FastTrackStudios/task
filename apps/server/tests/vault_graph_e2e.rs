#![allow(clippy::large_futures)]
//! End-to-end check for the `VaultGraph` architect-rpc service
//! against a live `task-server`: boots `AppState` on an
//! ephemeral TCP port over the repo's example studio (see
//! `support`), seeds linked pages through `VaultSync::put_file`,
//! and exercises backlinks / links / orphans / unresolved /
//! deadends / tags over the same socket.
//!
//! The example vault seeds one page of its own — link-less, tag-less —
//! so it appears in `orphans` and `deadends` beside the page this test
//! plants for the purpose, and nowhere else.
//!
//! Uses `vault_id = "default"` throughout — the server mounts
//! one vault per org under that id (`Backend::single`), and the
//! graph backend is constructed over the same root.

use vault_proto::{IfMatch, VaultGraphClient, VaultSyncClient};

#[allow(dead_code)]
mod support;

async fn boot_server() -> eyre::Result<(String, tempfile::TempDir)> {
    support::boot_ws().await
}

async fn put(sync: &VaultSyncClient, path: &str, body: &str) {
    sync.put_file(
        "default".to_string(),
        path.to_string(),
        body.as_bytes().to_vec(),
        IfMatch::CreateOnly,
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn graph_queries_over_seeded_vault() {
    let (url, _tmp) = boot_server().await.unwrap();
    let sync: VaultSyncClient = vox::connect_lane(&url).establish().await.unwrap();
    let graph: VaultGraphClient = vox::connect_lane(&url).establish().await.unwrap();

    put(
        &sync,
        "Wisdom.md",
        "---\ntitle: Wisdom\ntags: [philosophy]\n---\nSee [[Plans]] and [[Nowhere]].\n",
    )
    .await;
    put(
        &sync,
        "Plans.md",
        "---\ntags: [philosophy, planning]\n---\nBack to [[Wisdom]]. Also #inline/tag.\n",
    )
    .await;
    put(&sync, "Loose.md", "No links at all.\n").await;

    // Backlinks both ways; unknown paths are empty, not errors.
    let back = graph
        .backlinks("default".to_string(), "Wisdom.md".to_string())
        .await
        .unwrap();
    assert_eq!(back, vec!["Plans.md".to_string()]);
    assert!(
        graph
            .backlinks("default".to_string(), "Missing.md".to_string())
            .await
            .unwrap()
            .is_empty()
    );

    // Outgoing links resolve (or not) per target.
    let links = graph
        .links("default".to_string(), "Wisdom.md".to_string())
        .await
        .unwrap();
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].resolved.as_deref(), Some("Plans.md"));
    assert_eq!(links[1].resolved, None);

    // Orphans + deadends: the unlinked, link-less pages. Asserted by
    // membership, not as the whole list — every seeded page that happens
    // to carry no links is also an orphan, so pinning the list makes
    // adding an unrelated example file fail here, which is a confusing
    // place to hear about it.
    //
    // What matters is that a page this test planted with no links *is*
    // one, and a page it planted *with* links is not.
    for page in ["Loose.md", support::EXAMPLE_PAGE] {
        let orphans = graph.orphans("default".to_string()).await.unwrap();
        assert!(orphans.contains(&page.to_string()), "{page}: {orphans:?}");
        let deadends = graph.deadends("default".to_string()).await.unwrap();
        assert!(deadends.contains(&page.to_string()), "{page}: {deadends:?}");
    }
    // The other half of the claim: a page WITH links is neither.
    let orphans = graph.orphans("default".to_string()).await.unwrap();
    assert!(
        !orphans.contains(&"Wisdom.md".to_string()),
        "a linked page is not an orphan: {orphans:?}"
    );

    // Unresolved carries (source, linkpath).
    let unresolved = graph.unresolved("default".to_string()).await.unwrap();
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].source, "Wisdom.md");
    assert_eq!(unresolved[0].linkpath, "Nowhere");

    // Tags: frontmatter + inline, pages counted once, sorted by
    // count desc then tag asc.
    let tags = graph.tags("default".to_string()).await.unwrap();
    let get = |name: &str| tags.iter().find(|t| t.tag == name).map(|t| t.count);
    assert_eq!(get("philosophy"), Some(2));
    assert_eq!(get("planning"), Some(1));
    assert_eq!(get("inline/tag"), Some(1));
    assert_eq!(tags[0].tag, "philosophy");
}
