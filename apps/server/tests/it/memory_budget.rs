//! The task-server's memory budget, held in a test (t_37712151).
//!
//! Prod serves seven small orgs and was OOM-killed at 2 GiB. Nothing
//! said how much memory that deployment *should* take, so nothing could
//! say when a change made it worse. This chapter boots the prod shape in
//! one process — seven orgs, vaults with notes, tasks and an attachment,
//! a File Root with a laptop syncing it — drives it the way agents and
//! devices do, and holds it to three numbers:
//!
//! - **peak heap** over boot and traffic ([`HEAP_BUDGET`]) — what the
//!   program asked the allocator for at its worst moment;
//! - **heap growth** across repeated rounds of identical traffic and
//!   device sweeps ([`GROWTH_BUDGET`]) — a server answering the same
//!   questions over and over must not keep more each time (the shape of
//!   the prod leak: a no-op sync that retained something every cycle);
//! - **anonymous RSS** ([`RSS_ANON_BUDGET_KIB`]) — under the card's 300 MiB,
//!   checked loosely, because RSS also counts what glibc kept, thread
//!   stacks and this harness.
//!
//! Heap is counted by the binary's global allocator
//! (`support::alloc::Counting`), so the first two are exact and
//! allocator-independent: they move when code allocates more, not when
//! the machine has more cores or malloc keeps more arenas.
//!
//! A second test pins one zero-copy fix by itself: `search_vault`
//! matches on paths, and must not read the vault to do it.

use std::time::Duration;

use serde_json::{Value, json};

use crate::support;
use crate::support::alloc;

const TOKEN: &str = "memory-budget-secret";

/// The six orgs seeded beside the example org: seven in all, as prod.
const ORGS: [&str; 6] = [
    "days-to-praise",
    "crossroads",
    "northwind",
    "studio-b",
    "fasttrackstudio",
    "acme-films",
];

/// Peak live heap over boot + traffic + sweeps, seven small orgs.
///
/// Measured at 58.6 MiB on THEBATTLESHIP after t_37712151's fixes,
/// at 49.3 MiB once zero-copy PR 3 (STA-305) had landed, and at
/// 50.5 MiB on dev before zero-copy PR 2 (STA-304). PR 3 tightened it
/// from 128 MiB to 96 MiB and PR 2 to 80 MiB: the measurement plus
/// about 30 MiB of headroom for normal growth of the product, still an
/// order of magnitude under what prod was using. A change that has to
/// raise it should say why on its PR.
const HEAP_BUDGET: usize = 80 << 20;

/// How much more heap may be live after the measured rounds than after
/// the warm-up.
///
/// Identical traffic should retain nothing in steady state. The
/// allowance covers caches that fill on the first use of a code path the
/// warm-up happened not to take, the inbox the rounds themselves grow,
/// and whatever background tasks hold at the moment of reading. Measured
/// at 1.4 MiB after this card. Before it, every device sweep leaked its
/// two connections (see `files_sync::Dialled`), about 0.7 MiB per sweep
/// and 9.1 MiB over these rounds. A leak of that size per round fails
/// here with room to spare. Measured at 0.6 MiB on dev before zero-copy
/// PR 2, which tightened it from 4 MiB.
const GROWTH_BUDGET: usize = 2 << 20;

/// The card's budget for seven small orgs was 300 MiB. Measured at about
/// 101 MiB on THEBATTLESHIP (32 threads, so as many glibc arenas as any
/// runner here has) in zero-copy PR 3, which tightened it to 200 MiB:
/// loose, because RSS also counts allocator slack and thread stacks.
const RSS_ANON_BUDGET_KIB: u64 = 200 * 1024;

/// Rounds of traffic before the baseline is read, and after it.
const WARMUP_ROUNDS: usize = 3;
const MEASURED_ROUNDS: usize = 12;

/// [`MEASURED_ROUNDS`], or `TASK_MEMORY_BUDGET_ROUNDS` for a diagnostic
/// run: under heaptrack, a long run against a short one shows what
/// accumulates. A run with any other count reports and asserts nothing.
fn measured_rounds() -> usize {
    std::env::var("TASK_MEMORY_BUDGET_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MEASURED_ROUNDS)
}

/// A vault attachment of this size sits in every org: a bounce, a PDF.
/// Anything that reads whole files to answer a question about names or
/// hashes shows up in the peak once per org it touches.
const ATTACHMENT_BYTES: usize = 4 << 20;

/// Seed one org's vault: notes, tasks and an attachment.
fn seed_vault(vault: &std::path::Path) {
    std::fs::create_dir_all(vault.join("Notes")).unwrap();
    std::fs::create_dir_all(vault.join("Tasks")).unwrap();
    std::fs::create_dir_all(vault.join("Attachments")).unwrap();
    let para = "A paragraph of an ordinary working note, the kind a studio keeps. ".repeat(24);
    for i in 0..30 {
        std::fs::write(
            vault.join("Notes").join(format!("Note {i:02}.md")),
            format!("---\ntags: [note, studio]\nstatus: draft\n---\n\n# Note {i}\n\n{para}\n"),
        )
        .unwrap();
    }
    for i in 0..8 {
        std::fs::write(
            vault.join("Tasks").join(format!("Task {i:02}.md")),
            format!("---\ntype: task\ntitle: Task {i}\nstatus: open\npriority: normal\n---\n\nDo thing {i}.\n"),
        )
        .unwrap();
    }
    std::fs::write(
        vault.join("Attachments").join("bounce.wav"),
        vec![0x5a; ATTACHMENT_BYTES],
    )
    .unwrap();
}

/// Plant the six extra orgs (and seed the example org's vault too).
fn plant(example: &org_proto::OrgRoot) {
    seed_vault(&example.vault_dir());
    let data_root = example
        .path()
        .parent()
        .and_then(std::path::Path::parent)
        .expect("orgs/<slug> sits under the data root")
        .to_path_buf();
    let data_root = org_proto::DataRoot::new(data_root);
    for slug in ORGS {
        let org = data_root
            .init_org(slug, slug, false)
            .unwrap_or_else(|e| panic!("scaffold {slug}: {e}"));
        seed_vault(&org.vault_dir());
    }
}

async fn call_tool(client: &reqwest::Client, base: &str, org: &str, name: &str, args: Value) {
    let res: Value = client
        .post(format!("{base}/org/{org}/mcp"))
        .bearer_auth(TOKEN)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        }))
        .send()
        .await
        .expect("POST /mcp")
        .json()
        .await
        .expect("json-rpc body");
    assert!(
        !res["result"].is_null() && res["result"]["isError"] != json!(true),
        "{org}: tools/call {name} failed: {res}"
    );
}

/// One round of what agents do to every org.
async fn round(client: &reqwest::Client, base: &str, orgs: &[&str], n: usize) {
    for org in orgs {
        call_tool(client, base, org, "task_context", json!({})).await;
        call_tool(client, base, org, "list_tasks", json!({})).await;
        call_tool(client, base, org, "list_projects", json!({})).await;
        call_tool(
            client,
            base,
            org,
            "search_vault",
            json!({ "query": "note" }),
        )
        .await;
        call_tool(client, base, org, "list_inbox", json!({})).await;
        call_tool(
            client,
            base,
            org,
            "read_note",
            json!({ "path": "Notes/Note 07.md" }),
        )
        .await;
        call_tool(
            client,
            base,
            org,
            "capture_inbox",
            json!({ "body": format!("budget round {n}") }),
        )
        .await;
    }
}

/// Serve `state` over HTTP on an ephemeral port; the base URL.
async fn serve(state: task_server::AppState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = task_server::router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{port}")
}

/// Bind an endpoint into a shared book (as `device_sync_e2e`).
async fn bind(book: &files::AddressBook) -> architect::iroh_link::iroh::Endpoint {
    let endpoint = files::bind_endpoint(
        architect::iroh_link::iroh::SecretKey::generate(),
        Some(book.clone()),
    )
    .await
    .expect("bind an endpoint");
    book.add_endpoint_info(endpoint.addr());
    endpoint
}

#[tokio::test(flavor = "multi_thread")]
async fn seven_small_orgs_stay_inside_the_memory_budget() {
    if !alloc::counters_are_this_tests() {
        return;
    }
    alloc::reset_peak();
    let before_boot = alloc::live();
    let (state, _tmp) = support::boot_app_state_with_env(plant, &[("TASK_MCP_TOKEN", TOKEN)])
        .await
        .expect("boot seven orgs");
    let after_boot = alloc::live();

    // ── a File Root with a laptop syncing it, in the home org ────────
    let mut org = state.org(support::ORG).expect("the example org");
    let book = files::AddressBook::new();
    let server_endpoint = bind(&book).await;
    task_server::attach_peering(
        &mut org,
        server_endpoint.id().to_string(),
        files::IrohRemotes::port(server_endpoint.clone()),
    );
    {
        let (org, gate, serving) = (
            org.clone(),
            state.write_gate.clone(),
            server_endpoint.clone(),
        );
        tokio::spawn(async move {
            task_server::serve_org_iroh(org, gate, &serving).await;
        });
    }
    let project = state
        .data_root
        .org(support::ORG)
        .path()
        .join("files")
        .join("Album");
    std::fs::create_dir_all(&project).unwrap();
    for i in 0..5 {
        std::fs::write(
            project.join(format!("stem-{i}.wav")),
            vec![i as u8; 64 << 10],
        )
        .unwrap();
    }
    let root = support::adopt_root(&org.files, &project, "Album", files::RootFlavor::Media)
        .await
        .expect("adopt the album");
    support::checkpoint(&org.files, root.id)
        .await
        .expect("checkpoint the album");

    let laptop_dir = tempfile::tempdir().expect("laptop dir");
    let laptop_store = files::FilesBackend::new(laptop_dir.path(), laptop_dir.path().join("vault"))
        .expect("laptop backend");
    let laptop = files_daemon::SyncDaemon::open(laptop_store, laptop_dir.path().join("daemon"))
        .expect("laptop daemon");
    let laptop_id = laptop.attach_endpoint(bind(&book).await);
    files::service::sync::SyncService::enroll_device(
        &org.files,
        laptop_id.clone(),
        "the budget laptop".into(),
    )
    .await
    .expect("enrol the laptop");
    let coordinator = files::service::sync::SyncService::coordinator(&org.files)
        .await
        .expect("the org's endpoint id");
    laptop.admit_peer(&coordinator);
    laptop
        .sync_from_peer(
            &server_endpoint.id().to_string(),
            root.id,
            vec![],
            laptop_dir.path(),
        )
        .await
        .expect("the laptop takes the album");
    laptop.tick().await;

    // Plain references, copied into each future: a closure returning a
    // future that borrowed the closure's own captures would not outlive
    // the call that made it.
    let (state_ref, endpoint_ref) = (&state, &server_endpoint);
    let sweep = move || async move {
        tokio::time::timeout(
            Duration::from_secs(30),
            task_server::device_sync::sweep(state_ref, support::ORG, endpoint_ref),
        )
        .await
        .expect("a device sweep did not finish");
    };

    // ── traffic: warm up, read a baseline, then the same again ───────
    let base = serve(state.clone()).await;
    let client = reqwest::Client::new();
    let mut orgs = vec![support::ORG];
    orgs.extend(ORGS);
    for n in 0..WARMUP_ROUNDS {
        round(&client, &base, &orgs, n).await;
        sweep().await;
    }
    let baseline = alloc::live();
    // Live heap after each round's traffic and after its sweep, so a
    // failure says which half grows and whether it climbs every round (a
    // leak) or steps once (a cache filling).
    let rounds = measured_rounds();
    let mut series = Vec::with_capacity(rounds);
    for n in WARMUP_ROUNDS..WARMUP_ROUNDS + rounds {
        round(&client, &base, &orgs, n).await;
        let traffic = alloc::live();
        sweep().await;
        series.push((traffic, alloc::live()));
    }
    let after = alloc::live();
    println!(
        "live heap per measured round (after traffic / after sweep), from {}: {}",
        alloc::mib(baseline),
        series
            .iter()
            .map(|(t, s)| format!("{:.1}/{:.1}", *t as f64 / 1048576.0, *s as f64 / 1048576.0))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let peak = alloc::peak();
    let rss_anon = alloc::rss_anon_kib();

    let growth = after.saturating_sub(baseline);
    // The run's numbers, for the PR and for whoever tightens the budget.
    println!(
        "memory budget, seven orgs: boot {} → {}, warm {}, after {} rounds {} (growth {}), \
         peak {} (budget {}), RssAnon {:?} KiB",
        alloc::mib(before_boot),
        alloc::mib(after_boot),
        alloc::mib(baseline),
        rounds,
        alloc::mib(after),
        alloc::mib(growth),
        alloc::mib(peak),
        alloc::mib(HEAP_BUDGET),
        rss_anon,
    );
    if rounds != MEASURED_ROUNDS {
        // A diagnostic run (`TASK_MEMORY_BUDGET_ROUNDS`): the budgets are
        // for the standard scenario, so report and stop. With
        // `TASK_MEMORY_BUDGET_SETTLE_SECS`, first watch the heap idle:
        // what falls back was in flight (a connection waiting out its
        // idle timeout), and what stays is retained.
        let settle: u64 = std::env::var("TASK_MEMORY_BUDGET_SETTLE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        for t in (10..=settle).step_by(10) {
            tokio::time::sleep(Duration::from_secs(10)).await;
            println!("idle {t:>3}s: live {}", alloc::mib(alloc::live()));
        }
        return;
    }

    assert!(
        peak <= HEAP_BUDGET,
        "peak heap {} is over the {} budget for seven small orgs",
        alloc::mib(peak),
        alloc::mib(HEAP_BUDGET)
    );
    assert!(
        growth <= GROWTH_BUDGET,
        "{} rounds of identical traffic and device sweeps left {} more heap live \
         (budget {}): something is retained per request or per sweep",
        MEASURED_ROUNDS,
        alloc::mib(growth),
        alloc::mib(GROWTH_BUDGET)
    );
    if let Some(kib) = rss_anon {
        assert!(
            kib <= RSS_ANON_BUDGET_KIB,
            "anonymous RSS {kib} KiB is over the {RSS_ANON_BUDGET_KIB} KiB budget"
        );
    }
}

/// How much heap one call may take on top of what was live, however big
/// the vault's files are. A path search reads directory entries; a
/// megabyte covers the listing, the JSON-RPC envelope and the router.
const SEARCH_CALL_BUDGET: usize = 2 << 20;

#[tokio::test(flavor = "multi_thread")]
async fn a_path_search_reads_no_file_in_the_vault() {
    if !alloc::counters_are_this_tests() {
        return;
    }
    const BIG: usize = 32 << 20;
    let (state, _tmp) = support::boot_app_state_with_env(
        |org| {
            let dir = org.vault_dir().join("Mixes");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("final bounce.wav"), vec![0u8; BIG]).unwrap();
        },
        &[("TASK_MCP_TOKEN", TOKEN)],
    )
    .await
    .expect("boot");
    let base = serve(state).await;
    let client = reqwest::Client::new();
    // Once to warm every lazy path (router, permits, the MCP catalogue).
    call_tool(
        &client,
        &base,
        support::ORG,
        "search_vault",
        json!({ "query": "bounce" }),
    )
    .await;

    alloc::reset_peak();
    let before = alloc::live();
    call_tool(
        &client,
        &base,
        support::ORG,
        "search_vault",
        json!({ "query": "bounce" }),
    )
    .await;
    let took = alloc::peak().saturating_sub(before);
    println!(
        "search_vault over a vault holding a {} file: peak +{}",
        alloc::mib(BIG),
        alloc::mib(took)
    );
    assert!(
        took <= SEARCH_CALL_BUDGET,
        "search_vault took {} of heap with a {} file in the vault: it is reading files \
         to match on their names",
        alloc::mib(took),
        alloc::mib(BIG)
    );
}
