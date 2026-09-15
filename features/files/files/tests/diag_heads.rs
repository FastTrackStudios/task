//! Diagnostic, opt-in: `FTS_DIAG_REPO=<root>/.fts-files cargo nextest run
//! -p files --test diag_heads --no-capture` prints every view head of a
//! version store with its description and parents — the fastest way to
//! see why a root reads as divergent.

use jj_lib::object_id::ObjectId as _;
use jj_lib::repo::Repo as _;

#[tokio::test(flavor = "multi_thread")]
async fn print_heads() {
    let Ok(path) = std::env::var("FTS_DIAG_REPO") else {
        return;
    };
    tokio::task::spawn_blocking(move || dump(&path))
        .await
        .expect("join");
}

fn dump(path: &str) {
    let repo = files_store::version::repo::open_or_init_repo_blocking(std::path::Path::new(&path))
        .expect("open repo");
    let store = repo.store();
    for head in repo.view().heads() {
        let commit = pollster::block_on(store.get_commit_async(head)).expect("commit");
        println!(
            "head {} parents={:?} desc={:?}",
            head.hex(),
            commit
                .parent_ids()
                .iter()
                .map(|p| p.hex())
                .collect::<Vec<_>>(),
            commit.description()
        );
        for p in commit.parent_ids() {
            if let Ok(pc) = pollster::block_on(store.get_commit_async(p)) {
                println!("   parent {} desc={:?}", p.hex(), pc.description());
            }
        }
    }
}
