#![allow(clippy::large_futures)]
//! Cross-org node references over the wire — ADR 0003's read path.
//!
//! The claim the ADR makes is narrow and worth pinning exactly: a
//! qualified reference *addresses* a node in another organisation and
//! grants nothing. So this boots a real server and asks
//! `LinksService::resolve_nodes` the four questions that matter —
//! local, unknown domain, known org without a subscription, and a
//! reference the reader may actually follow — and checks that the
//! refusals are told apart rather than collapsed into "missing".
//!
//! It also pins the property that makes a cross-org setlist usable at
//! all: one unreachable item does not fail the batch.

#[allow(dead_code)]
mod support;

use links_proto::{LinksServiceClient, NodeKind, NodeRef, Reach};

/// The other organisation the example seed plants, and the domain it
/// answers to (`wiki_domains` gives every seeded org `<name>.test`).
const GUEST_ORG: &str = "vnt-video";
const GUEST_DOMAIN: &str = "vnt.test";

#[tokio::test(flavor = "multi_thread")]
async fn a_qualified_reference_addresses_without_granting() {
    let (url, tmp) = support::boot_ws().await.unwrap();
    let links: LinksServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    // Put a real chart in the *other* org, so "not permitted" cannot be
    // confused with "not there".
    let charts = tmp
        .path()
        .join("orgs")
        .join(GUEST_ORG)
        .join("resources/charts");
    std::fs::create_dir_all(&charts).unwrap();
    std::fs::write(charts.join("hosanna.kf"), "| C  G | Am F |\n").unwrap();

    let local = NodeRef::new(NodeKind::Chart, "doxology");
    let stranger = NodeRef::new(NodeKind::Chart, "hosanna").in_domain("nobody.example");
    let unsubscribed = NodeRef::new(NodeKind::Chart, "hosanna").in_domain(GUEST_DOMAIN);
    let own_domain = NodeRef::new(NodeKind::Chart, "doxology").in_domain("acme.test");

    let answers = links
        .resolve_nodes(vec![
            local.clone(),
            stranger.clone(),
            unsubscribed.clone(),
            own_domain.clone(),
        ])
        .await
        .expect("resolve_nodes");

    // Positional and total: one answer per question, in order, and a
    // refusal is never an error.
    assert_eq!(answers.len(), 4);
    assert_eq!(answers[0].node, local);
    assert_eq!(answers[0].reach, Reach::Local);

    assert_eq!(answers[1].reach, Reach::UnknownDomain);

    // The chart is right there on disk, and an unsubscribed reader is
    // told nothing about that — the refusal is `NotPermitted`, not
    // `NotFound`, and carries no path.
    assert_eq!(answers[2].reach, Reach::NotPermitted);
    assert_eq!(answers[2].rel_path, "");
    assert!(!answers[2].is_reachable());

    // An org's own domain is its own business, however the reference
    // was written.
    assert_eq!(answers[3].reach, Reach::Local);
}

/// A setlist is the real caller, and it holds a mix. One song nobody can
/// reach must not cost the reader the other two.
#[tokio::test(flavor = "multi_thread")]
async fn one_unreachable_item_does_not_fail_the_batch() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let links: LinksServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    let answers = links
        .resolve_nodes(vec![
            NodeRef::song("doxology"),
            NodeRef::song("hosanna").in_domain("nobody.example"),
            NodeRef::song("be-thou-my-vision"),
        ])
        .await
        .expect("resolve_nodes");

    assert_eq!(answers.len(), 3);
    assert!(answers[0].is_reachable());
    assert!(!answers[1].is_reachable());
    assert!(answers[2].is_reachable());
    // And the two that resolved say which org they belong to only when
    // one was found — a local node's org is the caller's own.
    assert_eq!(answers[1].org, "");
}

/// Nothing outside the library kinds crosses an org boundary: a note
/// lives in a vault, and a vault is not subscribable
/// (`wiki.boundary.no-subscribe`). Asking for one by domain is refused
/// rather than reaching into another org's tree.
#[tokio::test(flavor = "multi_thread")]
async fn a_vault_note_does_not_cross_an_org_boundary() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let links: LinksServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    let answers = links
        .resolve_nodes(vec![
            NodeRef::new(NodeKind::Note, "Projects/Album.md").in_domain(GUEST_DOMAIN),
        ])
        .await
        .expect("resolve_nodes");

    assert_eq!(answers[0].reach, Reach::NotPermitted);
    assert_eq!(answers[0].rel_path, "");
}
