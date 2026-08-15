fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::PathBuf;
    use std::time::Instant;
    let root = PathBuf::from(std::env::var("HOME")?).join("Documents/The Observatory");
    let t0 = Instant::now();
    let vault = vault_live::Vault::open(&root)?;
    let t_open = t0.elapsed();
    let t1 = Instant::now();
    let idx = vault_live::BlockIndex::build(&vault);
    let t_index = t1.elapsed();
    let bytes: usize = vault.pages.iter().map(|p| p.raw.len()).sum();
    println!("pages:           {}", vault.pages.len());
    println!("bases:           {}", vault.bases.len());
    println!("bytes:           {:.1} MiB", bytes as f64 / 1024.0 / 1024.0);
    println!("block ids:       {}", idx.len());
    println!("property hints:  {}", vault.property_types.map.len());
    println!("open:            {t_open:>7.2?}");
    println!("index:           {t_index:>7.2?}");
    // List unique folders, top 10 by page count.
    let mut by_folder = std::collections::HashMap::<String, usize>::new();
    for p in &vault.pages {
        *by_folder.entry(p.folder.clone()).or_default() += 1;
    }
    let mut v: Vec<_> = by_folder.into_iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!("\ntop folders:");
    for (f, n) in v.iter().take(10) {
        println!("  {n:>5}  {}", if f.is_empty() { "<root>" } else { f });
    }
    Ok(())
}
