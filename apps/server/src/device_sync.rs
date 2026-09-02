//! The server pulling its devices — the direction that had no driver.
//!
//! Sync in this system is two pulls ("there is no push", `files_sync`),
//! and until now the server ran neither of them. It *served* the replica
//! lane, so a laptop could take the org's work; nothing on the server
//! ever dialled a laptop, so the work a laptop did stayed there. That is
//! not a degraded sync, it is a one-way sync: a mix bounced on a plane
//! reached the studio when somebody copied it by hand.
//!
//! This module is the other pull. Per org, on a timer, for every peer
//! the org admits: dial it, ask what it holds ([`files_sync::SyncService::roots`]),
//! and reconcile the roots this org already knows.
//!
//! # Two ways in, and the same admitted set
//!
//! The ordinary way is the app: a person signed into the org calls
//! `enroll_device` with their machine's endpoint id, and the org admits
//! it (`files::lane::sync`). The authority is their sign-in, which is
//! the strongest thing available and has a human behind it.
//!
//! This module handles the other way — `orgs/<slug>/admitted-devices`,
//! one endpoint id per line, `#` for a comment. It exists for the cases
//! the app cannot reach: a headless machine with nobody to sign in on
//! it, a first server being brought up, and recovering an org whose app
//! access is exactly what is broken. An operator reads a device's id
//! with `fts-files-daemon id`; the org's own id, which the device needs
//! in return, is in `orgs/<slug>/iroh-endpoint-id`.
//!
//! # Why it does not simply dismiss whatever is unlisted
//!
//! The admitted set is not this file's alone: federation admits server
//! peers into the same table. So this reconciles only its **own**
//! effects, recorded in `admitted-devices.applied` beside it. An id
//! dropped from the file is dismissed on the next pass, including
//! across a restart; an id admitted by anything else is left alone.
//!
//! # A root the server has never seen
//!
//! By default it pulls roots the org **already holds** and logs the
//! rest. A project started on a laptop should reach the server — that is
//! what a server is for — but *where it lands* is a placement decision
//! (`files.scale.capacity`), and a sweep that answered it silently would
//! let any admitted machine create directories in the org's tree by
//! holding a root. `TASK_DEVICE_SYNC_ADOPT=1` turns it on, and the
//! answer is then the org's own files directory: where a project created
//! through the app lands too.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use architect::iroh_link::iroh;
use files_domain::{Content, HostId, Hosting};
use tracing::{debug, info, warn};

use crate::AppState;
use crate::iroh_host::IrohHost;

/// How often each org sweeps its devices.
///
/// A minute is chosen against what the other side does rather than
/// against latency: a device captures on cadence (a session that has
/// gone quiet, ten minutes of debounce), so sweeping faster mostly
/// re-asks a laptop that has nothing new. `TASK_DEVICE_SYNC_SECS`
/// overrides it; `0` turns the sweep off and leaves admission alone.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);

/// The file an operator writes device ids into.
#[must_use]
pub fn admitted_path(org_root: &Path) -> PathBuf {
    org_root.join("admitted-devices")
}

/// Where this module records what it admitted, so it can take it back.
fn applied_path(org_root: &Path) -> PathBuf {
    org_root.join("admitted-devices.applied")
}

/// The endpoint ids listed in `admitted-devices`, comments and blanks
/// dropped.
///
/// A missing file is an empty list, not an error: an org that has
/// admitted no devices is the state every org starts in.
fn listed(org_root: &Path) -> BTreeSet<String> {
    let Ok(raw) = std::fs::read_to_string(admitted_path(org_root)) else {
        return BTreeSet::new();
    };
    raw.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

fn read_applied(org_root: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(applied_path(org_root))
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .map(BTreeSet::from_iter)
        .unwrap_or_default()
}

fn write_applied(org_root: &Path, ids: &BTreeSet<String>) {
    let ids: Vec<&String> = ids.iter().collect();
    match serde_json::to_vec_pretty(&ids) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(applied_path(org_root), bytes) {
                // Losing this costs revocation-across-restart, not
                // correctness now, so it is a warning rather than a stop.
                warn!(error = %e, "device sync: could not record the admissions applied");
            }
        }
        Err(e) => warn!(error = %e, "device sync: could not serialize the applied admissions"),
    }
}

/// Bring the org's admitted set in line with its `admitted-devices`
/// file. Returns the ids now admitted by this file.
///
/// Idempotent by construction — `admit_host` is — so this runs on every
/// pass rather than only when the file's mtime moves: an operator who
/// edits it expects it to take effect, and watching a file to save a
/// no-op write is machinery with a failure mode of its own.
pub fn apply_admissions(files: &files::FilesBackend, org_root: &Path) -> BTreeSet<String> {
    let want = listed(org_root);
    let had = read_applied(org_root);

    for id in &want {
        if !had.contains(id) {
            info!(device = %id, "device sync: admitting a device");
        }
        // Devices hold the whole thing, structure and content alike.
        files.admit_host(HostId(id.clone()), Hosting::working());
    }
    for gone in had.difference(&want) {
        info!(device = %gone, "device sync: dismissing a device (removed from admitted-devices)");
        files.dismiss_host(&HostId(gone.clone()));
    }
    if want != had {
        write_applied(org_root, &want);
    }
    want
}

// t[impl files.peering.replication] — structure converges across every
// host and content follows placement: the depth below is that split,
// applied to the hosts an org admits rather than only to the ones it
// dials for federation
/// One sweep of one org: dial every admitted peer and reconcile what it
/// holds and this org knows.
///
/// Errors are per-peer and logged, never propagated: a laptop that is
/// shut is the ordinary case, and one unreachable device must not stop
/// the sweep reaching the next.
pub async fn sweep(state: &AppState, slug: &str, endpoint: &iroh::Endpoint) {
    let Some(org) = state.org(slug) else { return };
    let org_root = state.data_root.org(slug).path().to_path_buf();
    apply_admissions(&org.files, &org_root);
    // A wiki created or a source subscribed since the last pass becomes
    // a root here, so it is in the next `roots` answer a device pulls.
    match crate::org_roots::adopt_knowledge_roots(&org.files, &org.org_root).await {
        0 => {}
        n => info!(%slug, adopted = n, "device sync: new knowledge trees adopted as roots"),
    }

    for (host, hosting) in org.files.admitted_hosts() {
        // Never dial ourselves: an org's own endpoint id can legitimately
        // be in the admitted set (a second process hosting the same org),
        // and pulling from yourself is a round trip to import what you
        // already have.
        if host.0 == endpoint.id().to_string() {
            continue;
        }
        if let Err(e) = sweep_peer(&org.files, endpoint, &host, hosting, &org_root).await {
            debug!(peer = %host.0, %slug, error = %e, "device sync: peer not reached this pass");
        }
    }
}

/// Whether the server takes on a root it has never seen, offered by a
/// device.
///
/// Off by default, and the default is the interesting half. A project
/// started on a laptop *should* end up on the server — that is the whole
/// point of a server — but "where does it land" is a placement decision
/// (`files.scale.capacity`), and a sweep that answers it silently would
/// let any admitted machine create directories in the org's tree by
/// holding a root. Turned on, the answer is the org's own files
/// directory, which is where a root created through the app lands too.
fn adopts_offered_roots() -> bool {
    std::env::var("TASK_DEVICE_SYNC_ADOPT").is_ok_and(|v| v == "1")
}

/// Adopt a device's root into this org, if the operator allows it.
///
/// Returns whether the root is now this org's to reconcile.
fn adopt_offered(
    files: &files::FilesBackend,
    org_root: &Path,
    host: &HostId,
    root: &files_sync::WireRoot,
) -> bool {
    if !adopts_offered_roots() {
        debug!(
            peer = %host.0,
            root = %root.id,
            name = %root.name,
            "device sync: peer offers a root this org does not hold — set TASK_DEVICE_SYNC_ADOPT=1 to take it"
        );
        return false;
    }

    // Beside every other root of this org. `create_root` would refuse a
    // path outside this boundary, and adopting one *at* it is the same
    // placement the app makes when a person creates a project here.
    let tree = org_root.join("files").join(sanitize(&root.name));
    if let Err(e) = std::fs::create_dir_all(&tree) {
        warn!(peer = %host.0, name = %root.name, error = %e, "device sync: cannot make room for an offered root");
        return false;
    }
    let Some(path) = tree.to_str() else {
        warn!(peer = %host.0, name = %root.name, "device sync: that root's path is not utf-8");
        return false;
    };
    match files.adopt_replica(root.id, &root.name, path, root.flavor) {
        Ok(_) => {
            info!(
                peer = %host.0,
                root = %root.id,
                name = %root.name,
                path,
                "device sync: adopted a root a device brought"
            );
            true
        }
        Err(e) => {
            warn!(peer = %host.0, name = %root.name, error = %e, "device sync: could not adopt an offered root");
            false
        }
    }
}

/// A directory name from a name a device chose.
///
/// The name crosses a trust boundary — it is whatever string the far
/// side put in its registry — and it is about to be joined onto a path.
/// Separators and parent references are what turn that into writing
/// somewhere else entirely, so they do not survive.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if std::path::is_separator(c) { '-' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    if cleaned.is_empty() {
        "device-root".to_string()
    } else {
        cleaned
    }
}

async fn sweep_peer(
    files: &files::FilesBackend,
    endpoint: &iroh::Endpoint,
    host: &HostId,
    hosting: Hosting,
    org_root: &Path,
) -> Result<(), files_sync::SyncError> {
    let peer = files_sync::dial_peer(endpoint, &host.0).await?;
    let offered = peer
        .roots()
        .await
        .map_err(|e| files_sync::SyncError::Io(format!("roots rpc: {e}")))?;

    // A host that keeps no content converges structure and stops there —
    // `files.peering.replication` in one line, and the difference
    // between a cheap second host and an expensive one.
    let depth = match hosting.content {
        Content::None => files_sync::Depth::Structure,
        _ => files_sync::Depth::Content,
    };

    for root in offered {
        if files::FilesService::get_root(files, root.id).await.is_err()
            && !adopt_offered(files, org_root, host, &root)
        {
            continue;
        }
        match files_sync::reconcile_at(files, &peer, root.id, depth, &NoProgress).await {
            Ok(report) => {
                if report.heads_imported > 0 {
                    info!(
                        peer = %host.0,
                        root = %root.id,
                        name = %root.name,
                        heads = report.heads_imported,
                        chunks = report.chunks_fetched,
                        "device sync: pulled a peer's work"
                    );
                }
            }
            Err(e) => warn!(
                peer = %host.0,
                root = %root.id,
                error = %e,
                "device sync: reconcile failed; retrying next pass"
            ),
        }
    }
    Ok(())
}

/// The observer the server pulls with: none. Per-file progress is the
/// daemon's surface, where a person is watching a bar; here the pull is
/// a background sweep whose interesting events are already logged.
struct NoProgress;
impl files_sync::SyncObserver for NoProgress {}

/// Start the sweep loop for every org this process serves over iroh.
///
/// Takes the endpoints from [`IrohHost`] rather than binding its own:
/// the id a device admitted is the org's, so the pull has to come *from*
/// that endpoint or the far gate sees a machine it never admitted.
pub fn start(state: &AppState, host: &IrohHost) {
    let interval = match std::env::var("TASK_DEVICE_SYNC_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        Some(0) => {
            info!("device sync: disabled by TASK_DEVICE_SYNC_SECS=0");
            return;
        }
        Some(secs) => Duration::from_secs(secs),
        None => DEFAULT_INTERVAL,
    };

    for (org_slug, endpoint) in &host.endpoints {
        let (state, slug, endpoint) = (state.clone(), org_slug.clone(), endpoint.clone());
        tokio::spawn(async move {
            // A first pass immediately, so an operator who has just
            // pasted an id does not wait a minute to see it take.
            let mut timer = tokio::time::interval(interval);
            loop {
                timer.tick().await;
                sweep(&state, &slug, &endpoint).await;
            }
        });
        info!(slug = %org_slug, secs = interval.as_secs(), "device sync: sweeping this org's devices");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("org root")
    }

    fn backend(dir: &Path) -> files::FilesBackend {
        files::FilesBackend::new(dir.join("files"), dir.join("vault")).expect("backend")
    }

    #[test]
    fn ids_are_read_with_comments_and_blanks_dropped() {
        let dir = org_root();
        std::fs::write(
            admitted_path(dir.path()),
            "# the studio imac\nabc123\n\n  def456  # cody's macbook\n",
        )
        .unwrap();
        let ids = listed(dir.path());
        assert_eq!(
            ids,
            ["abc123".to_string(), "def456".to_string()]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn a_missing_file_admits_nobody() {
        let dir = org_root();
        let files = backend(dir.path());
        assert!(apply_admissions(&files, dir.path()).is_empty());
        assert!(files.admitted_hosts().is_empty());
    }

    #[test]
    fn removing_a_line_dismisses_that_device_and_leaves_the_others() {
        let dir = org_root();
        let files = backend(dir.path());
        std::fs::write(admitted_path(dir.path()), "laptop\nimac\n").unwrap();
        apply_admissions(&files, dir.path());
        assert_eq!(files.admitted_hosts().len(), 2);

        std::fs::write(admitted_path(dir.path()), "imac\n").unwrap();
        apply_admissions(&files, dir.path());
        assert_eq!(
            files.admits(&HostId("laptop".into())),
            None,
            "not dismissed"
        );
        assert!(files.admits(&HostId("imac".into())).is_some(), "collateral");
    }

    /// The property the `.applied` sidecar exists for: a host admitted
    /// by something else — federation admits server peers into the same
    /// table — is not swept away by this file's reconcile.
    #[test]
    fn a_peer_this_file_never_admitted_is_left_alone() {
        let dir = org_root();
        let files = backend(dir.path());
        files.admit_host(HostId("eu-west-server".into()), Hosting::structure_only());

        std::fs::write(admitted_path(dir.path()), "laptop\n").unwrap();
        apply_admissions(&files, dir.path());
        std::fs::write(admitted_path(dir.path()), "").unwrap();
        apply_admissions(&files, dir.path());

        assert_eq!(files.admits(&HostId("laptop".into())), None);
        assert!(
            files.admits(&HostId("eu-west-server".into())).is_some(),
            "the sweep dismissed a host it never admitted"
        );
    }

    /// A name from a device is a string from another machine, and it is
    /// about to be joined onto a path. Separators and parent references
    /// are how that becomes a write somewhere else entirely.
    #[test]
    fn a_devices_root_name_cannot_climb_out_of_the_org_directory() {
        for hostile in [
            "../../etc",
            "..",
            "/etc/passwd",
            "Album/../../..",
            ".",
            "   ",
            "",
        ] {
            let safe = sanitize(hostile);
            let joined = Path::new("/orgs/acme/files").join(&safe);
            assert!(
                joined.starts_with("/orgs/acme/files"),
                "{hostile:?} became {safe:?} → {}",
                joined.display()
            );
            assert!(
                !safe.is_empty() && safe != "." && safe != "..",
                "{hostile:?} became {safe:?}"
            );
        }
    }

    /// And an ordinary name is left alone — a project called
    /// "First Single - Example Client" must keep its name on the server.
    #[test]
    fn an_ordinary_name_survives_sanitizing() {
        assert_eq!(
            sanitize("First Single - Example Client"),
            "First Single - Example Client"
        );
        assert_eq!(sanitize("Album #2 (2026)"), "Album #2 (2026)");
    }

    /// Adoption is off unless an operator asked for it: an admitted
    /// machine must not be able to create directories in the org's tree
    /// merely by holding a root.
    #[test]
    fn an_offered_root_is_refused_unless_adoption_is_enabled() {
        let dir = org_root();
        let files = backend(dir.path());
        let root = files_sync::WireRoot {
            id: uuid::Uuid::new_v4(),
            name: "Laptop Project".into(),
            flavor: files::RootFlavor::Media,
            place: None,
            read_only: false,
        };
        // No `TASK_DEVICE_SYNC_ADOPT` in this process's environment —
        // asserted rather than assumed, since the default is the point.
        assert!(!adopts_offered_roots());
        assert!(!adopt_offered(
            &files,
            dir.path(),
            &HostId("laptop".into()),
            &root
        ));
        assert!(
            !dir.path().join("files").join("Laptop Project").exists(),
            "a refused adoption still made room for itself"
        );
    }

    /// Revocation has to outlive the process that issued it, or an
    /// operator's removal is undone by a restart.
    #[test]
    fn what_was_applied_is_remembered_across_a_restart() {
        let dir = org_root();
        std::fs::write(admitted_path(dir.path()), "laptop\n").unwrap();
        {
            let files = backend(dir.path());
            apply_admissions(&files, dir.path());
        }
        std::fs::write(admitted_path(dir.path()), "").unwrap();
        {
            // A fresh backend over the same dir — the restart.
            let files = backend(dir.path());
            apply_admissions(&files, dir.path());
            assert_eq!(files.admits(&HostId("laptop".into())), None);
        }
    }
}
