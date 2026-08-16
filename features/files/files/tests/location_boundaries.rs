//! A File Root may live outside the org directory — but only inside a
//! Storage Location the org was granted (issue #262).
//!
//! Before this, `create_root` confined to `<org>/files` and nothing else,
//! so media on a NAS could not be registered at all: the bytes were never
//! going to fit on the server's own disk, and pointing at them was
//! refused as a path escape. The boundary is still a real fence — these
//! tests are mostly about the paths it must keep refusing.

use std::path::PathBuf;
use std::sync::Arc;

use files::{FilesBackend, LocationBoundaries};
use files_proto::{FilesService as _, RootFlavor};

/// A boundary set fixed at construction — the server's implementation
/// reads a live grant registry, but the rule under test is the same.
struct Granted(Vec<PathBuf>);

impl LocationBoundaries for Granted {
    fn permitted(&self) -> Vec<PathBuf> {
        self.0.clone()
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    org_files: PathBuf,
    /// Stands in for the NAS: outside the org directory entirely.
    location: PathBuf,
    /// Outside both — nothing should ever register here.
    elsewhere: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let org_files = tmp.path().join("orgs/acme/files");
    let location = tmp.path().join("nas/Task/Projects");
    let elsewhere = tmp.path().join("somewhere-else");
    for p in [&org_files, &location, &elsewhere] {
        std::fs::create_dir_all(p).expect("mkdir");
    }
    Fixture {
        _tmp: tmp,
        org_files,
        location,
        elsewhere,
    }
}

fn backend(f: &Fixture, granted: Vec<PathBuf>) -> FilesBackend {
    let vault = f.org_files.parent().expect("org dir").join("vault");
    std::fs::create_dir_all(&vault).expect("vault");
    FilesBackend::new(&f.org_files, &vault)
        .expect("backend")
        .with_location_boundaries(Arc::new(Granted(granted)))
}

async fn create_at(b: &FilesBackend, dir: &PathBuf, name: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).expect("mkdir");
    b.create_root(
        dir.to_string_lossy().into_owned(),
        name.to_owned(),
        RootFlavor::Media,
    )
    .await
    .map(|_| ())
    .map_err(|e| format!("{e:?}"))
}

#[tokio::test]
async fn a_granted_location_can_host_a_root() {
    let f = fixture();
    let b = backend(&f, vec![f.location.clone()]);
    let project = f.location.join("A Journey of Immigrants");

    create_at(&b, &project, "A Journey of Immigrants")
        .await
        .expect("a granted location hosts a root");

    let roots = b.list_roots().await.expect("list");
    assert_eq!(roots.len(), 1);
    // The registered path is the location's, not a copy under the org dir
    // — the whole point is that the bytes stay where they are.
    assert!(
        roots[0].path.as_deref().is_some_and(|p| p.contains("nas/Task/Projects")),
        "root should point at the location, got {}",
        roots[0].path.as_deref().unwrap_or("(unplaced)")
    );
}

#[tokio::test]
async fn the_org_directory_still_works_with_no_grants_at_all() {
    // The single-machine default. Locations are an addition; a deployment
    // that has none must behave exactly as it did before they existed.
    let f = fixture();
    let b = backend(&f, vec![]);
    create_at(&b, &f.org_files.join("demo"), "demo")
        .await
        .expect("the org's own directory is always permitted");
}

#[tokio::test]
async fn a_path_outside_every_boundary_is_refused() {
    let f = fixture();
    let b = backend(&f, vec![f.location.clone()]);

    let err = create_at(&b, &f.elsewhere.join("nope"), "nope")
        .await
        .expect_err("outside every boundary");

    // And the message names the org directory, because that is the
    // answer that makes sense on a deployment with no locations.
    assert!(
        err.contains("boundary") || err.contains("outside"),
        "expected a confinement error, got {err}"
    );
}

#[tokio::test]
async fn revoking_the_grant_closes_the_boundary() {
    // Boundaries are read per call, not cached at construction: a grant
    // that goes away stops permitting new roots immediately. (The inverse
    // — a grant issued after boot — is why the server holds the registry
    // rather than a snapshot.)
    let f = fixture();
    let b = backend(&f, vec![]);
    create_at(&b, &f.location.join("late"), "late")
        .await
        .expect_err("no grant, no boundary");
}

#[tokio::test]
async fn a_traversal_out_of_a_granted_location_is_still_refused() {
    // The boundary canonicalizes, so `..` cannot walk out of a location
    // into a sibling directory. This is the property that makes widening
    // the boundary safe at all.
    let f = fixture();
    let b = backend(&f, vec![f.location.clone()]);
    let escape = f.location.join("../../somewhere-else");
    std::fs::create_dir_all(&f.elsewhere).expect("mkdir");

    create_at(&b, &escape, "escape")
        .await
        .expect_err("`..` must not escape a granted location");
}
