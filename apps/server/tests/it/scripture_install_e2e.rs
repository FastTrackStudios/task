#![allow(clippy::large_futures)]
//! Scripture installed while the server runs is served without a
//! restart.
//!
//! `task-server admin bible install|lexicon|original` is a separate
//! process writing into the org's `resources/`. This boots the example
//! org, then plays that process's part — a Strong's lexicon entry and a
//! whole edition land on disk — and reads both back over vox the way
//! the reader does, waiting no longer than the store's re-check
//! interval allows.

use std::time::{Duration, Instant};

use scripture_proto::ScriptureServiceClient;

use crate::support;

/// A code past the end of Strong's Greek (G5624), so the example org's
/// own corpus can never already answer it.
const CODE: &str = "G9999";

const EDITION: &str = "TST";

const JOHN: &str = r"\id JHN test edition
\c 3
\p
\v 16 \w For|strong=G1063\w* God so loved the world.
";

/// Poll `check` until it holds or the store has had several chances to
/// re-check its directories.
async fn eventually(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("{what} was not served within 20s of being installed");
}

#[tokio::test(flavor = "multi_thread")]
async fn scripture_installed_while_running_is_served_without_a_restart() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let scripture: ScriptureServiceClient = vox::connect_lane(&url).establish().await.unwrap();
    let resources = org_proto::DataRoot::from_env()
        .unwrap()
        .org(support::ORG)
        .resources_dir();

    assert!(
        scripture.lexicon(CODE.to_owned()).await.is_err(),
        "the example org must not already know {CODE}"
    );
    assert!(
        !scripture
            .translations()
            .await
            .unwrap()
            .iter()
            .any(|t| t.id == EDITION),
        "the example org must not already hold {EDITION}"
    );

    // What `admin bible lexicon` writes.
    let lexicon = resources.join("lexicon").join("strongs");
    std::fs::create_dir_all(&lexicon).unwrap();
    let greek = lexicon.join("greek.json");
    let mut entries: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&greek)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    entries.insert(
        CODE.to_owned(),
        serde_json::json!({ "lemma": "δοκιμή", "translit": "dokimḗ", "kjv_def": "test" }),
    );
    std::fs::write(&greek, serde_json::to_string(&entries).unwrap()).unwrap();

    // What `admin bible install` writes.
    let edition = resources.join("bible").join(EDITION);
    std::fs::create_dir_all(&edition).unwrap();
    std::fs::write(edition.join("44-JHN.usfm"), JOHN).unwrap();

    eventually("the lexicon entry", async || {
        scripture
            .lexicon(CODE.to_owned())
            .await
            .is_ok_and(|e| e.kjv_def == "test")
    })
    .await;
    eventually("the edition", async || {
        scripture
            .translations()
            .await
            .is_ok_and(|ts| ts.iter().any(|t| t.id == EDITION))
    })
    .await;

    let john = scripture
        .chapter(EDITION.to_owned(), "John".to_owned(), 3)
        .await
        .unwrap();
    assert!(john.verses[0].text.contains("God so loved the world"));
}
