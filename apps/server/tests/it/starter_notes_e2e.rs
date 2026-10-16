#![allow(clippy::large_futures)]
//! Every vault starts with **Home** and an **Example Formatting** note, end to end over
//! a live `task-server`: the org's first boot plants it in the vault, it
//! is served like any note (in the folder index, readable, a tour of the
//! formatting), and a note the person deleted is not planted again when
//! the server boots over the same data (`crate::starter`).

use vault_proto::{IfMatch, VaultSyncClient};

use crate::support;

const PATH: &str = "Example Formatting.md";

async fn lane<C: vox_core::FromVoxLane + vox::MaybeSend>(url: &str) -> C {
    vox::connect_lane(url)
        .establish()
        .await
        .unwrap_or_else(|e| panic!("connect: {e:?}"))
}

async fn has_note(sync: &VaultSyncClient) -> bool {
    let idx = sync
        .folder_index("default".into())
        .await
        .expect("folder index");
    idx.pages.iter().any(|p| p.path == PATH)
}

#[tokio::test(flavor = "multi_thread")]
async fn every_vault_starts_with_example_formatting_once() {
    let (url, tmp) = support::boot_ws().await.expect("boot");
    let sync: VaultSyncClient = lane(&url).await;

    assert!(has_note(&sync).await, "the first boot plants the note");
    // And the home note the vault opens on.
    let idx = sync
        .folder_index("default".into())
        .await
        .expect("folder index");
    assert!(
        idx.pages.iter().any(|p| p.path == "Home.md"),
        "every vault starts with Home.md"
    );
    // And a day shape, so the Calendar has blocks from the first day.
    assert!(
        idx.pages
            .iter()
            .any(|p| p.path == "Projects/Scheduling/templates/weekday.md"),
        "every vault starts with day templates"
    );
    let bytes = sync
        .get_file("default".into(), PATH.into())
        .await
        .expect("read the note");
    let text = String::from_utf8(bytes.0).expect("utf-8");
    for construct in ["## Text", "**Bold**", "- [ ] ", "> [!note]", "| Song", "$$"] {
        assert!(
            text.contains(construct),
            "{construct} missing from the note"
        );
    }

    // The person deletes it; the next boot leaves it deleted.
    sync.delete_file("default".into(), PATH.into(), IfMatch::Force)
        .await
        .expect("delete the note");
    let (again, ()) = support::boot_ws_over(&tmp).await.expect("reboot");
    let sync: VaultSyncClient = lane(&again).await;
    assert!(
        !has_note(&sync).await,
        "a deleted starter note is not planted again"
    );
}
