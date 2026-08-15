//! Exercise the `VaultSync::base_views` backend method against the live
//! vault — proves the server-side base renderer (row projection, cells,
//! grouping) before the UI consumes it.
//!
//! Run: cargo run -p vault-live --example run_base_views -- [VAULT_ROOT] [BASE_PATH]

use std::path::PathBuf;

use vault_live::Backend;
use vault_proto::VaultSync;

fn main() {
    let mut args = std::env::args().skip(1);
    let root: PathBuf = args.next().map_or_else(
        || PathBuf::from(std::env::var("HOME").unwrap()).join(".task/orgs/codywright/vault"),
        PathBuf::from,
    );
    let base = args
        .next()
        .unwrap_or_else(|| "Scripture/Songs.base".to_string());

    // Recipes live under the wiki, not the vault, so a base filtering
    // `type: recipe` needs the cookbook root registered too. Defaults
    // to the sibling `wiki/Knowledge/` of the given vault root, which
    // is the on-disk org layout.
    let recipe_root = std::env::var("TASK_SERVER_WIKI_ROOT").map_or_else(
        |_| {
            root.parent()
                .map_or_else(|| root.join("wiki/Knowledge"), |p| p.join("wiki/Knowledge"))
        },
        PathBuf::from,
    );
    let backend = Backend::single("default", root)
        .expect("open backend")
        .with_recipe_roots([("default".to_string(), recipe_root)].into_iter().collect());
    let views = backend.base_views("default", &base).expect("base_views");

    println!("base: {base} — {} view(s)\n", views.len());
    for v in views {
        println!("▸ {} [{}]  columns: {:?}", v.name, v.view_type, v.columns);
        for g in v.groups {
            let label = if g.label.is_empty() {
                "—".into()
            } else {
                g.label
            };
            for r in g.rows {
                println!("    [{label}] {}  ::  {:?}", r.title, r.cells);
            }
        }
        println!();
    }
}
