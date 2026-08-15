//! End-to-end vault scan + block-index round trip on a real
//! temp directory. Exercises walker → loader → `BlockIndex` →
//! lookup as a pipeline, separately from the per-module unit
//! tests.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;

use vault_live::{BlockIndex, Vault};

#[test]
fn vault_scan_and_block_lookup() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("notes/2026")).unwrap();
    fs::write(
        root.join("Home.md"),
        "# Home\n\nWelcome.\n\nA block worth referencing\nid:: 11111111-1111-4111-8111-111111111111\n",
    )
    .unwrap();
    fs::write(
        root.join("notes/2026/howdy.md"),
        "Hello world\nid:: 22222222-2222-4222-8222-222222222222\n",
    )
    .unwrap();
    // Hidden / ignore dirs should be skipped.
    fs::create_dir_all(root.join(".obsidian")).unwrap();
    fs::write(root.join(".obsidian/app.json"), "{}").unwrap();

    let v = Vault::open(root).expect("vault opens");
    assert_eq!(v.pages.len(), 2);
    let home = v.page_by_basename("Home").unwrap();
    assert!(home.raw.contains("Welcome."));
    let howdy = v.page_by_rel_path("notes/2026/howdy.md").unwrap();
    assert_eq!(howdy.folder, "notes/2026");

    let idx = BlockIndex::build(&v);
    assert_eq!(idx.len(), 2);
    let preview = idx
        .lookup_str("22222222-2222-4222-8222-222222222222")
        .map(|loc| {
            let page = &v.pages[loc.page_idx];
            let end = page.raw[loc.anchor..]
                .find('\n')
                .map_or(page.raw.len(), |n| loc.anchor + n);
            page.raw[loc.anchor..end].to_string()
        })
        .unwrap();
    assert_eq!(preview, "Hello world");
}
