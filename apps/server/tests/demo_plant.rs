//! `admin demo` plants a world the app can stand on — asserted, not
//! assumed.
//!
//! The demo is the seeded half of the repo's representation policy
//! (every feature lives in the suite AND in the planted world), and
//! this is the suite's grip on it: the declared projects become File
//! Roots named after themselves (what the review surface mounts on),
//! the video deliverables are generated INSIDE those roots, and a
//! replant upgrades in place rather than duplicating.
//!
//! The plant runs as a SUBPROCESS (the real `admin demo` CLI), not as
//! an in-process call: the seeder's own `FilesBackend` keeps its
//! version stores open for the life of its process, and a second
//! backend over the same root in the same process deadlocks on the
//! store lock the moment it walks history (`chain`). The subprocess is
//! also simply more honest — it is exactly what `just demo` runs.

use files::service::roots::RootsService;

/// Run `task-server admin demo --org <slug>` against `data_root`.
fn plant(data_root: &std::path::Path, slug: &str) -> eyre::Result<()> {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_task-server"))
        .args(["admin", "demo", "--org", slug])
        .env("TASK_DATA_ROOT", data_root)
        .stdout(std::process::Stdio::null())
        .status()?;
    eyre::ensure!(status.success(), "admin demo --org {slug} failed");
    Ok(())
}

fn ffmpeg_on_path() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// Multi-thread on purpose: planting checkpoints roots, and the Files
// lane parks blocking work that must not starve a lone runtime worker
// (the CLI runs on the default multi-thread runtime too).
#[tokio::test(flavor = "multi_thread")]
async fn demo_plants_adopted_roots_with_video_deliverables() -> eyre::Result<()> {
    let tmp = tempfile::tempdir()?;
    let slug = "acme-audio";
    plant(tmp.path(), slug)?;

    // SAFETY: single-test binary; only the DataRoot construction below
    // reads it.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::remove_var("TASK_SERVER_VAULT_ROOT");
        std::env::remove_var("TASK_SERVER_ORG");
    }
    let root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    let org = root.org(slug);
    let backend = files::FilesBackend::new(org.path().join("files"), org.vault_dir())
        .map_err(|e| eyre::eyre!("files backend: {e}"))?;

    // Every declared project is an adopted root, named after itself —
    // the name is the convention the project page resolves its
    // deliverables through (`locate_in_root`).
    let roots = RootsService::list(&backend)
        .await
        .map_err(|e| eyre::eyre!("list roots: {e}"))?;
    for declared in task_server::example_org::declared_of(slug) {
        assert!(
            roots.iter().any(|r| r.name == declared.title),
            "{}: not adopted as a root (have: {:?})",
            declared.title,
            roots.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()
        );
    }

    // Video deliverables live INSIDE the project's root — the review
    // player streams renditions from a root, so `resources/` would be
    // the wrong side of the boundary. Generated, so only asserted when
    // the machine can generate (CI's dev shell carries ffmpeg; a bare
    // machine gets the honest outstanding state instead).
    if ffmpeg_on_path() {
        for declared in task_server::example_org::declared_of(slug) {
            for (name, medium, _, _) in declared.deliverables {
                if *medium != project::Medium::Video {
                    continue;
                }
                let dest = org
                    .path()
                    .join("files")
                    .join("Projects")
                    .join(declared.dir)
                    .join("Deliverables")
                    .join(format!("{name}.mp4"));
                assert!(
                    dest.is_file(),
                    "{}: \"{name}\" not generated at {}",
                    declared.title,
                    dest.display()
                );
            }
        }
    }

    // Audio masters are delivered into the root too (the review
    // surface's waveform lane), alongside their `resources/` song copy
    // (the global player's lane). Same bytes, two conventions.
    for declared in task_server::example_org::declared_of(slug) {
        for (name, medium, scope, _) in declared.deliverables {
            if *medium != project::Medium::Audio || *scope != project::Scope::WholeProject {
                continue;
            }
            let dest = org
                .path()
                .join("files")
                .join("Projects")
                .join(declared.dir)
                .join("Deliverables")
                .join(format!("{name}.wav"));
            assert!(
                dest.is_file(),
                "{}: \"{name}\" not delivered at {}",
                declared.title,
                dest.display()
            );
        }
    }

    // A fresh video arrives as HISTORY, not a file: rough cut
    // checkpointed, final rendered over it, checkpointed again — the
    // review screen's version switcher and compare depend on there
    // being at least two commits to show.
    if ffmpeg_on_path() {
        for declared in task_server::example_org::declared_of(slug) {
            for (name, medium, _, _) in declared.deliverables {
                if *medium != project::Medium::Video {
                    continue;
                }
                let root = roots
                    .iter()
                    .find(|r| r.name == declared.title)
                    .expect("adopted above");
                let chain = files::FilesService::chain(
                    &backend,
                    root.id,
                    format!("Deliverables/{name}.mp4"),
                )
                .await
                .unwrap_or_else(|e| panic!("chain of \"{name}\": {e}"));
                assert!(
                    chain.len() >= 2,
                    "{}: \"{name}\" has {} version(s) on record — the demo \
                     promises a rough cut AND a final",
                    declared.title,
                    chain.len()
                );
            }
        }
    }

    // Replanting is an upgrade, not a duplication: same roots, same
    // count. (This is the path an old demo root takes to gain whatever
    // the seeder has since learned.)
    plant(tmp.path(), slug)?;
    let again = RootsService::list(&backend)
        .await
        .map_err(|e| eyre::eyre!("list roots: {e}"))?;
    assert_eq!(
        roots.len(),
        again.len(),
        "replant changed the root count: {:?}",
        again.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()
    );
    Ok(())
}
