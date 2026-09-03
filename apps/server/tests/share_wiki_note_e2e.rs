#![allow(clippy::large_futures)]
//! A share link to a wiki page, end to end over a live `task-server`:
//! a note target names its vault, so the same Share panel that mints
//! links for vault notes mints them for wiki pages — and the landing
//! page opens the page in the wiki, not the vault.
//!
//! Boots over the example studio (see `support`), whose `acme-audio`
//! org plants the `music-theory` wiki with `Concepts/Modes.md`.

use share_proto::{NewShareLink, ShareServiceClient, ShareTarget};

// `support` compiles into each test binary separately, so whatever this
// one does not touch reads as dead code here. Same attribute the other
// binaries carry.
#[allow(dead_code)]
mod support;

const WIKI_VAULT: &str = "wiki:music-theory";
const PAGE: &str = "Concepts/Modes.md";

async fn lane<C: vox_core::FromVoxLane>(url: &str) -> C {
    vox::connect_lane(url)
        .establish()
        .await
        .unwrap_or_else(|e| panic!("connect: {e:?}"))
}

fn options() -> NewShareLink {
    NewShareLink {
        label: "modes for the band".into(),
        capabilities: None,
        password: None,
        expires_unix: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wiki_page_shares_by_its_vault_id_and_lands_in_the_wiki() {
    let (url, _tmp) = support::boot_ws().await.expect("boot");
    let http = url
        .replace("ws://", "http://")
        .trim_end_matches("/vox")
        .to_owned();
    let share: ShareServiceClient = lane(&url).await;

    // Mint for the page IN ITS WIKI.
    let link = share
        .create_link(ShareTarget::note(WIKI_VAULT, PAGE), options())
        .await
        .expect("create_link");
    assert_eq!(link.target, ShareTarget::note(WIKI_VAULT, PAGE));

    // The wiki page's Share panel finds it; the vault note of the same
    // path (a different note) does not — and a query that names no
    // vault is the org's own vault, not a wildcard.
    let for_wiki = share
        .links_for_target(ShareTarget::note(WIKI_VAULT, PAGE))
        .await
        .expect("links_for_target");
    assert_eq!(
        for_wiki
            .iter()
            .map(|l| l.token.as_str())
            .collect::<Vec<_>>(),
        vec![link.token.as_str()]
    );
    let for_vault = share
        .links_for_target(ShareTarget::note("default", PAGE))
        .await
        .expect("links_for_target");
    assert!(for_vault.is_empty(), "{for_vault:?}");
    let for_unnamed = share
        .links_for_target(ShareTarget::Note {
            path: PAGE.into(),
            vault_id: String::new(),
        })
        .await
        .expect("links_for_target");
    assert!(for_unnamed.is_empty(), "{for_unnamed:?}");

    // A note of the org's own vault still mints and matches as before,
    // with or without the vault named.
    let vault_link = share
        .create_link(
            ShareTarget::Note {
                path: "Plans.md".into(),
                vault_id: String::new(),
            },
            options(),
        )
        .await
        .expect("create_link (vault)");
    assert_eq!(vault_link.target, ShareTarget::note("default", "Plans.md"));
    let found = share
        .links_for_target(ShareTarget::note("default", "Plans.md"))
        .await
        .expect("links_for_target");
    assert_eq!(found.len(), 1);

    // A vault nobody serves notes from is refused at the mint.
    let bad = share
        .create_link(ShareTarget::note("shares", PAGE), options())
        .await;
    assert!(bad.is_err(), "{bad:?}");

    // The landing page (token-checked, served by the same server)
    // opens the page on the wiki route.
    let landing = reqwest::get(format!("{http}/org/{}/share/{}", support::ORG, link.token))
        .await
        .expect("landing")
        .text()
        .await
        .expect("landing body");
    assert!(
        landing.contains(&format!(
            "/wiki/w/{}/music-theory/page?path=Concepts/Modes.md&share=1",
            support::ORG
        )),
        "{landing}"
    );
    let vault_landing = reqwest::get(format!(
        "{http}/org/{}/share/{}",
        support::ORG,
        vault_link.token
    ))
    .await
    .expect("landing")
    .text()
    .await
    .expect("landing body");
    assert!(
        vault_landing.contains("/vault?path=Plans.md&share=1"),
        "{vault_landing}"
    );
}
