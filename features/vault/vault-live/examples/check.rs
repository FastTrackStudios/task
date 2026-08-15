// One-off sanity check: open the example vault and walk the
// resolution paths the editor will use at runtime.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::PathBuf;
    let root = PathBuf::from(std::env::var("HOME")?).join("Documents/Task");
    let vault = vault_live::Vault::open(&root)?;
    let idx = vault_live::BlockIndex::build(&vault);
    println!("pages: {}", vault.pages.len());
    for p in &vault.pages {
        println!("  - {} ({} bytes)", p.basename, p.raw.len());
    }
    println!("indexed block ids: {}", idx.len());
    // Try one specific lookup that the seed mentions.
    let uuid = "01950000-0000-7000-8000-000000000001";
    let view = vault_live::VaultLookupView::new(&vault, &idx);
    use editor_state::markdown::VaultLookup;
    match view.lookup_block(uuid) {
        Some(hit) => println!("  ✓ ((:{uuid})) → {} : {}", hit.page, hit.preview),
        None => println!("  ✗ {uuid} not found"),
    }
    match view.lookup_page("Project Roadmap") {
        Some(hit) => println!(
            "  ✓ [[Project Roadmap]] → preview {:?}…",
            &hit.preview[..hit.preview.len().min(60)]
        ),
        None => println!("  ✗ Project Roadmap not found"),
    }
    match view.lookup_section("Project Roadmap", "Goals") {
        Some(body) => println!("  ✓ ![[Project Roadmap#Goals]] → {} chars", body.len()),
        None => println!("  ✗ Goals section not found"),
    }
    Ok(())
}
