//! `task-server admin rename-org`, end to end against a real data root.
//!
//! The slug is the join key between four things — the org's directory,
//! its manifest, every File Root path it owns, and its membership rows
//! — and this is the one command that moves all four. Each half of a
//! rename is easy to test alone and easy to get wrong together, so this
//! drives the verb as an operator would and then reads every place the
//! old slug used to live.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use files_proto::{FileRootInfo, RootFlavor};
use task_server::admin_cli::rename_org;
use task_server::memberships::Memberships;

/// A data root with a home org and the org under test, pointed at by
/// `TASK_DATA_ROOT` — which is the whole process's, so the caller holds
/// the returned guard for as long as the test reads it.
async fn data_root(
    with: &[(&str, &str, bool)],
) -> (
    tempfile::TempDir,
    org_proto::DataRoot,
    tokio::sync::MutexGuard<'static, ()>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let env_guard = crate::support::env_lock().await;
    // SAFETY: held under the binary's one `ENV_LOCK`.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::remove_var("TASK_CENTRAL_AUTH_URL");
    }
    let root = org_proto::DataRoot::from_env().unwrap();
    root.ensure().unwrap();
    for (slug, name, is_home) in with {
        root.init_org(slug, name, *is_home).unwrap();
    }
    (tmp, root, env_guard)
}

fn args(pairs: &[(&str, &str)]) -> Vec<String> {
    pairs
        .iter()
        .flat_map(|(k, v)| [(*k).to_owned(), (*v).to_owned()])
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rename_moves_the_directory_manifest_roots_and_memberships() {
    let (_tmp, root, _env) = data_root(&[
        ("home", "Home", true),
        ("fasttrackstudios", "FastTrackStudios", false),
    ])
    .await;
    let old = root.org("fasttrackstudios");
    let home = root.org("home");

    // Two File Roots under the old directory, one elsewhere.
    let files_dir = old.path().join("files");
    let registry = files::registry::Registry::open(&files_dir).unwrap();
    let mk = |name: &str, path: &Path| FileRootInfo {
        id: uuid::Uuid::new_v4(),
        name: name.to_owned(),
        path: Some(path.to_string_lossy().into_owned()),
        flavor: RootFlavor::Media,
        created_at: chrono::Utc::now(),
        project_version: None,
    };
    registry
        .insert(mk("Vault", &old.path().join("vault")))
        .unwrap();
    registry
        .insert(mk("Wiki — docs", &old.path().join("wikis/docs")))
        .unwrap();
    registry
        .insert(mk(
            "Media",
            Path::new("/mnt/storage/Task/fasttrackstudios/Projects"),
        ))
        .unwrap();
    drop(registry);

    // A local member and a mirrored one.
    let members = Memberships::open(&home.memberships_db()).await.unwrap();
    let cody = uuid::Uuid::new_v4();
    let tom = uuid::Uuid::new_v4();
    members
        .upsert(cody, "fasttrackstudios", Some("owner"))
        .await
        .unwrap();
    members
        .sync_from_issuer(
            tom,
            &[("fasttrackstudios".to_owned(), Some("member".to_owned()))],
        )
        .await
        .unwrap();
    drop(members);

    rename_org(&args(&[
        ("--from", "fasttrackstudios"),
        ("--to", "fasttrackstudio"),
        ("--display-name", "FastTrackStudio"),
    ]))
    .await
    .expect("rename");

    // 1. The directory moved, and only that directory.
    assert!(!old.path().exists(), "old directory is gone");
    let new = root.org("fasttrackstudio");
    assert!(new.path().is_dir(), "new directory exists");
    assert!(home.path().is_dir(), "the home org was not touched");

    // 2. The manifest names the new slug — which is what lets the
    //    loader accept the directory at all — and the new display name.
    let orgs = root.scan_orgs().expect("the data root still scans");
    let (_, manifest) = orgs
        .iter()
        .find(|(r, _)| r.slug() == "fasttrackstudio")
        .expect("renamed org loads under its new slug");
    assert_eq!(manifest.slug, "fasttrackstudio");
    assert_eq!(manifest.display_name, "FastTrackStudio");
    assert!(
        orgs.iter().all(|(r, _)| r.slug() != "fasttrackstudios"),
        "nothing loads under the old slug"
    );

    // 3. Roots under the old directory follow it; the one on the media
    //    tree does not — that tree is the deployment's, and the command
    //    says so rather than guessing.
    let registry = files::registry::Registry::open(&new.path().join("files")).unwrap();
    let mut paths: Vec<String> = registry.list().into_iter().filter_map(|r| r.path).collect();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            "/mnt/storage/Task/fasttrackstudios/Projects".to_owned(),
            new.path().join("vault").to_string_lossy().into_owned(),
            new.path().join("wikis/docs").to_string_lossy().into_owned(),
        ]
    );

    // 4. Every membership row moved, whoever granted it.
    let members = Memberships::open(&home.memberships_db()).await.unwrap();
    assert!(
        members
            .role_for(cody, "fasttrackstudios")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        members
            .role_for(cody, "fasttrackstudio")
            .await
            .unwrap()
            .unwrap()
            .role,
        Some("owner".into())
    );
    assert!(
        members
            .role_for(tom, "fasttrackstudio")
            .await
            .unwrap()
            .is_some()
    );
}

/// The home org is the identity authority and its slug is baked into
/// where the memberships store lives. Renaming it is not a small change,
/// so the command refuses rather than doing half of one.
#[tokio::test(flavor = "multi_thread")]
async fn the_home_org_is_refused() {
    let (_tmp, root, _env) = data_root(&[("home", "Home", true)]).await;
    let err = rename_org(&args(&[("--from", "home"), ("--to", "casa")]))
        .await
        .expect_err("home org must be refused");
    assert!(err.to_string().contains("home org"), "{err}");
    assert!(root.org("home").path().is_dir(), "nothing moved");
    assert!(!root.org("casa").path().exists());
}

/// Renaming onto a slug that exists would merge two orgs' directories.
/// Refused before anything is touched.
#[tokio::test(flavor = "multi_thread")]
async fn an_existing_target_is_refused() {
    let (_tmp, root, _env) = data_root(&[
        ("home", "Home", true),
        ("alpha", "Alpha", false),
        ("beta", "Beta", false),
    ])
    .await;
    let err = rename_org(&args(&[("--from", "alpha"), ("--to", "beta")]))
        .await
        .expect_err("collision must be refused");
    assert!(err.to_string().contains("already exists"), "{err}");
    assert!(root.org("alpha").path().is_dir());
    assert!(root.org("beta").path().is_dir());
}

/// A slug is lowercase ASCII, digits and dashes — the manifest loader
/// enforces the same, so accepting anything else here would produce a
/// directory the server then refuses to load.
#[tokio::test(flavor = "multi_thread")]
async fn a_bad_slug_is_refused_before_anything_moves() {
    let (_tmp, root, _env) = data_root(&[("home", "Home", true), ("alpha", "Alpha", false)]).await;
    for bad in ["Alpha", "al pha", "alpha_2", ""] {
        let err = rename_org(&args(&[("--from", "alpha"), ("--to", bad)]))
            .await
            .expect_err(bad);
        assert!(err.to_string().contains("not a slug"), "{bad:?}: {err}");
    }
    assert!(root.org("alpha").path().is_dir(), "nothing moved");
}
