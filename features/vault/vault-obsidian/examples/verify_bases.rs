//! Verify the Scripture `.base` files parse + execute against the live
//! vault — proving the `kind:` discriminator and the `order:`/`filters:`
//! schema are right.
//!
//! Run:  cargo run -p vault-obsidian --example verify_bases -- [VAULT_ROOT]
//!   VAULT_ROOT defaults to ~/.task/orgs/codywright/vault

use std::path::PathBuf;

use vault_obsidian::Vault;
use vault_obsidian::base_query::query_all_views;

fn main() {
    let root: PathBuf = std::env::args().nth(1).map_or_else(
        || {
            let home = std::env::var("HOME").expect("HOME");
            PathBuf::from(home).join(".task/orgs/codywright/vault")
        },
        PathBuf::from,
    );

    let vault = Vault::open(&root).expect("open vault");
    println!(
        "vault: {} pages, {} bases\n",
        vault.pages.len(),
        vault.bases.len()
    );

    for base in [
        "Scripture/Songs.base",
        "Scripture/Sermons.base",
        "Scripture/Scripture Studies.base",
    ] {
        println!("══ {base} ══");
        match query_all_views(&vault, base) {
            Ok(views) => {
                for (name, ev) in views {
                    let total: usize = ev.groups.iter().map(|(_, rows)| rows.len()).sum();
                    println!("  view '{name}': {total} rows");
                    for (group, rows) in &ev.groups {
                        let label = if group.is_empty() {
                            "—".to_string()
                        } else {
                            group.clone()
                        };
                        for r in rows {
                            println!("    [{label}] {}", r.basename);
                        }
                    }
                }
            }
            Err(e) => println!("  ERROR: {e:?}"),
        }
        println!();
    }
}
