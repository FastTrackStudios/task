//! `task recipe import-collection` end to end: the real `task` binary
//! against an embedded server (`TASK_EMBED=1` boots
//! `task_server::AppState` in-process, same as `vox_embedded.rs`),
//! fed a saved listing (`--from-file`) and saved recipe pages
//! (`--pages-dir`) so nothing touches the network.
//!
//! The second run is the point: it must skip everything the first
//! run wrote, matched on the canonical `source:` URL, and leave the
//! files untouched.

use std::path::Path;
use std::process::Output;

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/collection")
}

fn task(data_root: &Path, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_task"))
        .args(args)
        .current_dir(data_root)
        .env("TASK_DATA_ROOT", data_root)
        .env("TASK_EMBED", "1")
        .env_remove("TASK_VOX_URL")
        .env_remove("TASK_SESSION_TOKEN")
        .env_remove("TASK_ORGANIZATION_ID")
        .env_remove("TASK_SERVER_WIKI_ROOT")
        .env_remove("TASK_SERVER_ORG")
        .env_remove("TASK_SERVER_VAULT_ROOT")
        .env_remove("TASK_ORG_ID")
        .env_remove("TASK_USER_ID")
        .env_remove("TASK_VAULT_ROOT")
        .env_remove("ANTHROPIC_API_KEY")
        .env("TASK_SESSION_FILE", data_root.join("session.json"))
        .output()
        .expect("spawn task binary")
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn scratch_org() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    ok(&task(tmp.path(), &["org", "init", "t", "--name", "T"]));
    tmp
}

#[test]
fn recipe_collection_imports_once_and_a_rerun_skips_it() {
    let tmp = scratch_org();
    let listing = fixtures().join("listing.html");
    let pages = fixtures().join("pages");
    let ledger = tmp.path().join("since.json");
    let listing_s = listing.to_string_lossy().into_owned();
    let pages_s = pages.to_string_lossy().into_owned();
    let ledger_s = ledger.to_string_lossy().into_owned();
    let base = [
        "recipe",
        "import-collection",
        "https://www.allrecipes.com/recipes/16791/everyday-cooking/special-collections/food-wishes/",
        "--org",
        "t",
        "--from-file",
        &listing_s,
        "--pages-dir",
        &pages_s,
        "--since-file",
        &ledger_s,
        "--author",
        "Chef John",
        "--no-index",
    ];

    // Dry run: plans both recipes (the round-up and the gallery are
    // not recipe URLs), writes nothing.
    let mut dry = base.to_vec();
    dry.push("--dry-run");
    let out = ok(&task(tmp.path(), &dry));
    assert!(
        out.contains("would import Cookbook/Food Wishes/good-frickin-paprika-chicken.cook"),
        "{out}"
    );
    assert!(
        out.contains("would import Cookbook/Food Wishes/amish-apple-fritter-bread.cook"),
        "{out}"
    );
    assert!(out.contains("(of 2 enumerated); dry-run"), "{out}");
    assert!(!ledger.exists(), "dry-run must not write the ledger");
    let listed = ok(&task(tmp.path(), &["recipe", "list", "--org", "t"]));
    assert!(listed.starts_with("0 recipes"), "{listed}");

    // First real run.
    let out = ok(&task(tmp.path(), &base));
    assert!(
        out.contains("imported 2, skipped 0 present, 0 failed"),
        "{out}"
    );
    assert!(ledger.is_file(), "the ledger is written after a real run");

    let listed = ok(&task(tmp.path(), &["recipe", "list", "--org", "t"]));
    assert!(listed.starts_with("2 recipes"), "{listed}");
    assert!(
        listed.contains("Cookbook/Food Wishes/good-frickin-paprika-chicken.cook"),
        "{listed}"
    );

    // The file carries the resource stamp as `>> key: value` lines.
    let got = ok(&task(
        tmp.path(),
        &[
            "recipe",
            "get",
            "Cookbook/Food Wishes/good-frickin-paprika-chicken.cook",
            "--org",
            "t",
            "--json",
        ],
    ));
    let v: serde_json::Value = serde_json::from_str(&got).expect("json");
    let source = v["source"].as_str().expect("source");
    for want in [
        ">> title: Good Frickin' Paprika Chicken",
        ">> source: https://www.allrecipes.com/recipe/221093/good-frickin-paprika-chicken/",
        ">> source_site: allrecipes",
        ">> collection: Food Wishes",
        ">> author: Chef John",
        ">> tags: resource, food-wishes",
        ">> curated: false",
        ">> servings: 4",
        ">> image: https://img.test/paprika-chicken.jpg",
    ] {
        assert!(source.contains(want), "missing `{want}` in:\n{source}");
    }
    assert!(source.contains(">> imported: 20"), "{source}");
    assert_eq!(
        v["sourceUrl"].as_str(),
        Some("https://www.allrecipes.com/recipe/221093/good-frickin-paprika-chicken/")
    );
    let tags: Vec<&str> = v["tags"]
        .as_array()
        .map(|a| a.iter().filter_map(|t| t.as_str()).collect())
        .unwrap_or_default();
    assert_eq!(tags, vec!["resource", "food-wishes"]);
    assert_eq!(v["ingredients"].as_array().map(Vec::len), Some(3));

    // Second run: everything is present; nothing is rewritten.
    let out = ok(&task(tmp.path(), &base));
    assert!(
        out.contains("imported 0, skipped 2 present, 0 failed"),
        "{out}"
    );
    let listed = ok(&task(tmp.path(), &["recipe", "list", "--org", "t"]));
    assert!(listed.starts_with("2 recipes"), "{listed}");

    // A fresh listing whose links grew tracking parameters still
    // matches — canonical URL is the identity.
    let tracked = tmp.path().join("tracked.txt");
    std::fs::write(
        &tracked,
        "https://www.allrecipes.com/recipe/221093/good-frickin-paprika-chicken/?utm_source=mail\n",
    )
    .unwrap();
    let tracked_s = tracked.to_string_lossy().into_owned();
    let mut again = base.to_vec();
    again[6] = &tracked_s;
    let out = ok(&task(tmp.path(), &again));
    assert!(
        out.contains("imported 0, skipped 1 present, 0 failed"),
        "{out}"
    );

    // A recipe whose page is missing from --pages-dir is a listed
    // failure, not an abort; exit stays 0.
    let missing = tmp.path().join("missing.txt");
    std::fs::write(
        &missing,
        "https://www.allrecipes.com/recipe/1/not-saved/\nhttps://www.allrecipes.com/recipe/221093/good-frickin-paprika-chicken/\n",
    )
    .unwrap();
    let missing_s = missing.to_string_lossy().into_owned();
    let mut with_missing = base.to_vec();
    with_missing[6] = &missing_s;
    let out = ok(&task(tmp.path(), &with_missing));
    assert!(
        out.contains("imported 0, skipped 1 present, 1 failed"),
        "{out}"
    );
    assert!(
        out.contains("failed https://www.allrecipes.com/recipe/1/not-saved/: no saved page"),
        "{out}"
    );
}
