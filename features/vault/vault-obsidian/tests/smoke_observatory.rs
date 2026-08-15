//! Smoke test against the user's real Obsidian vault.
//!
//! Only runs when `TASK_OBS_VAULT` env points at a real Obsidian
//! directory.

use std::path::PathBuf;

use vault_obsidian::Vault;

#[test]
fn open_observatory_vault() {
    let Some(root) = std::env::var_os("TASK_OBS_VAULT").map(PathBuf::from) else {
        eprintln!("skipping: set TASK_OBS_VAULT to run");
        return;
    };
    if !root.is_dir() {
        eprintln!(
            "skipping: TASK_OBS_VAULT not a directory: {}",
            root.display()
        );
        return;
    }
    let t0 = std::time::Instant::now();
    let v = Vault::open(&root).expect("open");
    let elapsed = t0.elapsed();
    eprintln!(
        "loaded {} pages, {} bases, {} attachments in {:?}",
        v.pages.len(),
        v.bases.len(),
        v.attachments.len(),
        elapsed
    );
    assert!(!v.pages.is_empty());
    assert!(
        !v.bases.is_empty(),
        "expected to find at least one .base file"
    );
    assert!(elapsed.as_secs() < 10, "load took too long: {elapsed:?}");
}
