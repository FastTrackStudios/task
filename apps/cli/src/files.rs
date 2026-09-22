//! `task files …` — the Files RPC surface (issue #259, ADR 0001):
//! turn a folder into a File Root, browse it, read a file's version
//! chain, checkpoint on demand. Talks to the org's Files lanes over
//! vox — remote server or embedded in-process backend alike, exactly
//! like `task timer …` (see `establish_for_url`). Each command dials the
//! lane it needs (`RootsService`, `TreeService`, `VersionService`, …).
//!
//! Issue #261 adds the curated verbs — `task files version …` (Named
//! Versions), `task files project-version …` (Project Versions), and
//! `task files gc` (the Vault-protected sweep). Those entities are
//! vault pages, so they are equally editable in a text editor; the CLI
//! is the path that also validates the reference against the store.
//!
//! A commit typed on the command line may be any unambiguous prefix; it
//! is expanded to the full commit before it reaches a lane, because a
//! `VersionId` only round-trips a full commit hex (see `expand_commit`).

use clap::{Subcommand, ValueEnum};
use files_proto::id::{ProjectVersionId, VersionId};
use files_proto::service::roots::AdoptRequest;
use files_proto::service::tree::EntryKind;
use files_proto::{
    BrowseEntry, CurationServiceClient, RootFlavor, RootId, RootPath, RootsServiceClient,
    SyncServiceClient, TreeServiceClient, VersionServiceClient,
};

use crate::establish_for_url;
use crate::resolve_org_vox_url;

#[derive(Subcommand)]
pub(crate) enum FilesCmd {
    /// File Root CRUD (create / list / get).
    #[command(subcommand)]
    Root(FilesRootCmd),
    /// The machines that sync this org's files.
    #[command(subcommand)]
    Device(FilesDeviceCmd),
    /// Root-scoped directory listing — the marker file and version
    /// store are hidden. Empty `subpath` lists the root itself.
    Browse {
        root_id: uuid::Uuid,
        #[arg(default_value = "")]
        subpath: String,
        #[arg(long)]
        json: bool,
    },
    /// Rootless directory listing ("Drive" browsing — loose files
    /// outside any root, per the glossary). Shows everything,
    /// including a root's own internals if `path` happens to be one.
    DriveBrowse {
        path: String,
        #[arg(long)]
        json: bool,
    },
    /// A file's version chain (newest first), following recorded
    /// renames.
    Chain {
        root_id: uuid::Uuid,
        path: String,
        #[arg(long)]
        json: bool,
    },
    /// Certify a Session checkpoint right now: full-scan the root's
    /// live tree, diff against the current head, write one commit.
    /// Ends the root's open session.
    Checkpoint {
        root_id: uuid::Uuid,
        /// Defaults to "checkpoint now".
        #[arg(long)]
        message: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// The root's auto-snapshots (newest first) — the ephemeral
    /// mid-session captures. Never version-chain entries.
    Snapshots {
        root_id: uuid::Uuid,
        #[arg(long)]
        json: bool,
    },
    /// Tell the cadence engine these root-relative paths were just
    /// written — what a watcher does, for a machine the server can't
    /// watch itself.
    Hint {
        root_id: uuid::Uuid,
        /// Root-relative paths.
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// The root's Ignore set (patterns neither versioned nor synced).
    #[command(subcommand)]
    Ignore(FilesIgnoreCmd),
    /// Replace a file's live-tree content with a pointer stub. The
    /// content stays in the version store; listings keep its logical
    /// size and identity. Refused when the file has unversioned
    /// changes — checkpoint first.
    Dehydrate {
        root_id: uuid::Uuid,
        /// Root-relative path.
        path: String,
        #[arg(long)]
        json: bool,
    },
    /// Restore a stub's exact content from the version store,
    /// verified by FileId.
    Hydrate {
        root_id: uuid::Uuid,
        /// Root-relative path.
        path: String,
        #[arg(long)]
        json: bool,
    },
    /// The root's hydration policy: paths MATCHING these patterns are
    /// kept hydrated by `apply`; everything else is kept dehydrated.
    /// Empty policy = touch nothing (opt-in).
    #[command(subcommand)]
    HydrationPolicy(FilesHydrationPolicyCmd),
    /// Named Versions — curated labels on top of the automatic chain
    /// ("v3 for client"). Vault entities, not store constructs.
    #[command(subcommand)]
    Version(FilesVersionCmd),
    /// Project Versions — whole-project iterations of one root,
    /// auto-numbered, with the folder name never changing.
    #[command(subcommand)]
    ProjectVersion(FilesProjectVersionCmd),
    /// Sweep a root's version store. Everything the Vault references —
    /// Named Versions, Project Version starts — is immortal.
    Gc {
        root_id: uuid::Uuid,
        /// Refuse to sweep anything written in the last N seconds
        /// (the concurrent-writer guard). Defaults to 60.
        #[arg(long)]
        keep_newer_secs: Option<u64>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum FilesVersionCmd {
    /// Name a checkpoint as a deliverable.
    Name {
        root_id: uuid::Uuid,
        /// Hex commit id — the full id, or any unambiguous prefix
        /// (`task files chain` prints the first twelve characters).
        commit_id: String,
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Every Named Version, newest first.
    List {
        /// Limit to one root.
        #[arg(long)]
        root_id: Option<uuid::Uuid>,
        #[arg(long)]
        json: bool,
    },
    /// What a Named Version points at right now — the resolution a
    /// share link targeting it performs.
    Resolve {
        id: uuid::Uuid,
        #[arg(long)]
        json: bool,
    },
    /// Drop a Named Version's curation. The automatic chain is
    /// untouched; its content stops being immortal at the next `gc`.
    Remove { id: uuid::Uuid },
}

#[derive(Subcommand)]
pub(crate) enum FilesProjectVersionCmd {
    /// Start the next Project Version of a root, from its current
    /// checkpoint head.
    Start {
        root_id: uuid::Uuid,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Every Project Version of a root, oldest first.
    List {
        root_id: uuid::Uuid,
        #[arg(long)]
        json: bool,
    },
    /// Restart the root as a new Project Version: checkpoint the old
    /// iteration, reshape the live tree, and start the new lineage.
    /// Exactly one of --empty / --template / --carry-forward picks the
    /// starting mode; --carry-forward with no paths carries everything
    /// (a pure lineage cut). The new iteration keeps the label of the
    /// one it restarts.
    Restart {
        root_id: uuid::Uuid,
        /// The iteration to restart, by number. Defaults to the root's
        /// current lineage (its highest-numbered Project Version).
        #[arg(long)]
        from: Option<u32>,
        /// Start with an empty tree.
        #[arg(long, conflicts_with_all = ["template", "carry_forward"])]
        empty: bool,
        /// Start from this template folder's contents.
        #[arg(long, conflicts_with = "carry_forward")]
        template: Option<String>,
        /// Carry these root-relative paths forward (repeatable); with
        /// no paths, carries everything minus the Ignore set.
        #[arg(long, num_args = 0..)]
        carry_forward: Option<Vec<String>>,
        #[arg(long)]
        json: bool,
    },
    /// Browse an old iteration read-only at a commit (time travel).
    BrowseAt {
        root_id: uuid::Uuid,
        commit_id: String,
        #[arg(default_value = "")]
        subpath: String,
        #[arg(long)]
        json: bool,
    },
    /// Copy chosen files out of an old commit into the live tree.
    CopyForward {
        root_id: uuid::Uuid,
        commit_id: String,
        #[arg(required = true)]
        paths: Vec<String>,
        #[arg(long)]
        json: bool,
    },
}

/// A root's versioning flavor, chosen at creation (ADR 0001). `media`
/// is the default; `software` makes the root a colocated git repository
/// (issue #273) so git, CI, and IDEs see an ordinary checkout while
/// Files versions the same history.
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub(crate) enum FlavorArg {
    #[default]
    Media,
    Software,
}

impl From<FlavorArg> for RootFlavor {
    fn from(arg: FlavorArg) -> Self {
        match arg {
            FlavorArg::Media => Self::Media,
            FlavorArg::Software => Self::Software,
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum FilesHydrationPolicyCmd {
    /// Show the root's hydration-policy patterns.
    Show {
        root_id: uuid::Uuid,
        #[arg(long)]
        json: bool,
    },
    /// Replace the root's hydration-policy patterns (gitignore
    /// syntax; matching = kept hydrated). Storing changes no file —
    /// run `apply` to enact it.
    Set {
        root_id: uuid::Uuid,
        #[arg(required = true)]
        patterns: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Run the policy over the live tree now: hydrate matching stubs,
    /// dehydrate clean non-matching files. Dirty files are skipped and
    /// reported.
    Apply {
        root_id: uuid::Uuid,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum FilesIgnoreCmd {
    /// Show the root's Ignore set.
    Show {
        root_id: uuid::Uuid,
        #[arg(long)]
        json: bool,
    },
    /// Replace the root's Ignore set with these patterns.
    Set {
        root_id: uuid::Uuid,
        #[arg(required = true)]
        patterns: Vec<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum FilesRootCmd {
    /// Turn an existing folder into a File Root.
    Create {
        path: String,
        /// Defaults to the folder's own name.
        #[arg(long)]
        name: Option<String>,
        /// Versioning flavor. `software` adopts (or creates) a real git
        /// repo in the folder — colocated, so git tooling is unaffected.
        #[arg(long, value_enum, default_value_t = FlavorArg::Media)]
        flavor: FlavorArg,
        #[arg(long)]
        json: bool,
    },
    /// Every File Root known to this org.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One root by id.
    Get {
        id: uuid::Uuid,
        #[arg(long)]
        json: bool,
    },
}

/// `task files device …` — pairing a machine with this org.
///
/// The desktop app does this for itself when somebody signs in. This is
/// the same exchange for machines with no app to sign into: a studio
/// rig, a build box, a server. The authority is the CLI's own stored
/// session, which is the point — a machine does not admit itself.
#[derive(Subcommand)]
pub(crate) enum FilesDeviceCmd {
    /// Pair this machine: enrol the local sync agent's endpoint id with
    /// the org, and print the coordinator to point it at.
    ///
    /// Needs `fts-files-daemon` on PATH (or `--endpoint` to name an id
    /// read from another machine's `fts-files-daemon id`).
    Pair {
        /// The endpoint id to enrol. Defaults to this machine's.
        #[arg(long)]
        endpoint: Option<String>,
        /// How the org's device list should name it. Defaults to this
        /// machine's hostname.
        #[arg(long)]
        name: Option<String>,
        /// Print the install command instead of running it — for
        /// pairing a machine that is not this one.
        #[arg(long)]
        no_install: bool,
    },
    /// The machines this org syncs with.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Cut a machine off: it is refused further sync and wipes its copy
    /// of org content on next contact.
    Revoke { device_id: uuid::Uuid },
    /// Enrol this machine with EVERY org your account is a member of, in
    /// one call, and print each org's endpoint id. What the sync agent's
    /// `sign-in` does on a cadence, for a script or a machine without the
    /// desktop app.
    ///
    /// Needs `fts-files-daemon` on PATH (or `--endpoint`).
    EnrollAll {
        /// The endpoint id to enrol. Defaults to this machine's.
        #[arg(long)]
        endpoint: Option<String>,
        /// How the orgs' device lists should name it. Defaults to this
        /// machine's hostname.
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// Ask the local agent a one-line question.
fn agent_says(args: &[&str]) -> eyre::Result<String> {
    let out = std::process::Command::new("fts-files-daemon")
        .args(args)
        .output()
        .map_err(|e| eyre::eyre!("running fts-files-daemon: {e} (is the sync agent installed?)"))?;
    if !out.status.success() {
        eyre::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn run_files_device(cmd: FilesDeviceCmd, slug: &str, vox_url: &str) -> eyre::Result<()> {
    let sync: SyncServiceClient = establish_for_url(vox_url).await?;
    match cmd {
        FilesDeviceCmd::Pair {
            endpoint,
            name,
            no_install,
        } => {
            let endpoint = match endpoint {
                Some(id) => id,
                None => agent_says(&["id"])?,
            };
            let name = name.unwrap_or_else(|| {
                std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "this machine".into())
            });

            let device = sync
                .enroll_device(endpoint.clone(), name)
                .await
                .map_err(|e| eyre::eyre!("enrolling {endpoint}: {e}"))?;
            let coordinator = sync
                .coordinator()
                .await
                .map_err(|e| eyre::eyre!("asking {slug} for its endpoint id: {e}"))?;

            println!("machine      {endpoint}");
            println!("enrolled as  {} ({})", device.name, device.id);
            println!("coordinator  {coordinator}");
            if no_install {
                println!();
                println!("on that machine, run:");
                println!("    fts-files-daemon install --coordinator {coordinator}");
                return Ok(());
            }
            // The other half: point the local agent at the org and make
            // it a background service. Pairing that stops at "enrolled"
            // leaves a machine the org admits and that syncs nothing.
            let installed = std::process::Command::new("fts-files-daemon")
                .args(["install", "--coordinator", &coordinator])
                .status()
                .map_err(|e| eyre::eyre!("installing the agent's service: {e}"))?;
            if !installed.success() {
                eyre::bail!("the agent's installer exited with {installed}");
            }
        }
        FilesDeviceCmd::List { json } => {
            let devices = sync.devices().await?;
            if json {
                // `facet_json`, like every other `--json` here: the wire
                // types are facet types, and a second serializer would
                // be a second shape for the same data.
                println!(
                    "{}",
                    facet_json::to_string(&devices).map_err(|e| eyre::eyre!("{e}"))?
                );
                return Ok(());
            }
            for d in devices {
                println!(
                    "{}  {:<24}  {}{}",
                    d.id,
                    d.name,
                    d.endpoint.as_deref().unwrap_or("(no endpoint)"),
                    if d.revoked { "  REVOKED" } else { "" }
                );
            }
        }
        FilesDeviceCmd::Revoke { device_id } => {
            let device = sync
                .revoke_device(files_proto::id::DeviceId::new(device_id))
                .await?;
            println!("revoked {} ({})", device.name, device.id);
            println!("it is refused further sync and wipes its copy on next contact.");
        }
        FilesDeviceCmd::EnrollAll {
            endpoint,
            name,
            json,
        } => {
            let endpoint = match endpoint {
                Some(id) => id,
                None => agent_says(&["id"])?,
            };
            let name = name.unwrap_or_else(machine_name);
            // The server lane takes the session token as an argument
            // rather than as the lane's identity, because this call acts
            // across orgs — the token is what says which orgs.
            //
            // And the session names the server the account lives on, so
            // that is the server to enrol with. Dialling the default lane
            // instead reached a local server that was not running while
            // the session pointed at production.
            let no_session = || {
                crate::errors::usage("enroll everywhere")
                    .cause("no stored session")
                    .hint("run `task auth login` first")
                    .report()
            };
            let session = crate::session_store::load()?.ok_or_else(no_session)?;
            let entry = session.active_server().ok_or_else(no_session)?;
            let token = entry.token.clone();
            if token.trim().is_empty() {
                return Err(no_session());
            }
            let server = (entry.url != crate::session_store::LOCAL_URL).then(|| entry.url.clone());
            let (enrollment, _at): (files_proto::DeviceEnrollmentServiceClient, String) =
                crate::establish_server_client(server.as_deref()).await?;
            let enrolled = enrollment
                .enroll_everywhere(token, endpoint.clone(), name)
                .await
                .map_err(|e| eyre::eyre!("enroll everywhere: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&enrolled).map_err(|e| eyre::eyre!("{e}"))?
                );
                return Ok(());
            }
            println!("machine      {endpoint}");
            if enrolled.is_empty() {
                println!("your account is in no org this server hosts");
            }
            for org in &enrolled {
                let coordinator = if org.org_endpoint_id.is_empty() {
                    "(no peering endpoint yet)"
                } else {
                    org.org_endpoint_id.as_str()
                };
                println!("{:<24} {coordinator}", org.slug);
            }
            println!();
            println!("the sync agent takes it from here once it is signed in:");
            println!("    fts-files-daemon sign-in");
        }
    }
    Ok(())
}

pub(crate) async fn run_files(cmd: FilesCmd, org_override: Option<&str>) -> eyre::Result<()> {
    let slug = crate::resolve_slug(org_override)?;
    let vox_url = resolve_org_vox_url(None, &slug);

    // Each arm dials only the lane it uses, so pairing a machine with no
    // roots yet waits on no handshake but the sync lane's.
    match cmd {
        FilesCmd::Device(cmd) => run_files_device(cmd, &slug, &vox_url).await?,
        FilesCmd::Root(cmd) => run_files_root(cmd, &vox_url).await?,
        FilesCmd::Browse {
            root_id,
            subpath,
            json,
        } => {
            let tree: TreeServiceClient = establish_for_url(&vox_url).await?;
            let entries = tree
                .browse(RootId::new(root_id), root_path(&subpath)?)
                .await
                .map_err(|e| eyre::eyre!("browse: {e}"))?;
            print_entries(&entries, json)?;
        }
        FilesCmd::DriveBrowse { path, json } => {
            let roots: RootsServiceClient = establish_for_url(&vox_url).await?;
            let entries = roots
                .browse_area(path)
                .await
                .map_err(|e| eyre::eyre!("browse_area: {e}"))?;
            print_entries(&entries, json)?;
        }
        FilesCmd::Chain {
            root_id,
            path,
            json,
        } => {
            let versions: VersionServiceClient = establish_for_url(&vox_url).await?;
            let chain = versions
                .chain(RootId::new(root_id), root_path(&path)?)
                .await
                .map_err(|e| eyre::eyre!("chain: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&chain).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                for entry in chain {
                    let renamed = entry
                        .renamed_from
                        .map(|p| format!(" (renamed from {p})"))
                        .unwrap_or_default();
                    let named = if entry.names.is_empty() {
                        String::new()
                    } else {
                        format!("  [{}]", entry.names.join(", "))
                    };
                    println!(
                        "{}  {}{}{}",
                        short(&entry.commit_id),
                        entry.path,
                        renamed,
                        named
                    );
                }
            }
        }
        FilesCmd::Checkpoint {
            root_id,
            message,
            json,
        } => {
            let versions: VersionServiceClient = establish_for_url(&vox_url).await?;
            let info = versions
                .checkpoint(RootId::new(root_id), message)
                .await
                .map_err(|e| eyre::eyre!("checkpoint: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&info).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!(
                    "{}  {} ({} paths changed{}{})",
                    short(&info.commit_id),
                    info.description,
                    info.changed_paths.len(),
                    if info.requeued_paths.is_empty() {
                        String::new()
                    } else {
                        format!(", {} requeued", info.requeued_paths.len())
                    },
                    if info.save_points.is_empty() {
                        String::new()
                    } else {
                        format!(", {} save points", info.save_points.len())
                    },
                );
            }
        }
        FilesCmd::Snapshots { root_id, json } => {
            let versions: VersionServiceClient = establish_for_url(&vox_url).await?;
            let snapshots = versions
                .snapshots(RootId::new(root_id), None)
                .await
                .map_err(|e| eyre::eyre!("snapshots: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&snapshots).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                for s in snapshots {
                    let saves: Vec<&str> = s.save_points.iter().map(|p| p.path.as_str()).collect();
                    println!(
                        "{}  {}  {} paths{}",
                        short(&s.snapshot_id),
                        s.at.to_rfc3339(),
                        s.changed_paths.len(),
                        if saves.is_empty() {
                            String::new()
                        } else {
                            format!("  save points: {}", saves.join(", "))
                        },
                    );
                }
            }
        }
        FilesCmd::Version(cmd) => run_files_version(cmd, &vox_url).await?,
        FilesCmd::ProjectVersion(cmd) => run_files_project_version(cmd, &vox_url).await?,
        FilesCmd::Hint { root_id, paths } => {
            let versions: VersionServiceClient = establish_for_url(&vox_url).await?;
            let paths = paths
                .iter()
                .map(|p| root_path(p))
                .collect::<eyre::Result<Vec<_>>>()?;
            let accepted = versions
                .hint_activity(RootId::new(root_id), paths)
                .await
                .map_err(|e| eyre::eyre!("hint_activity: {e}"))?;
            println!("{accepted} hints accepted (the rest are in the Ignore set)");
        }
        FilesCmd::Ignore(FilesIgnoreCmd::Show { root_id, json }) => {
            let sync: SyncServiceClient = establish_for_url(&vox_url).await?;
            let set = sync
                .ignore_set(RootId::new(root_id))
                .await
                .map_err(|e| eyre::eyre!("ignore_set: {e}"))?;
            // The root's own layer — the one `set` replaces. The platform
            // and capability layers are not this root's to change.
            print_patterns(&set.project, json)?;
        }
        FilesCmd::Ignore(FilesIgnoreCmd::Set {
            root_id,
            patterns,
            json,
        }) => {
            let sync: SyncServiceClient = establish_for_url(&vox_url).await?;
            let stored = sync
                .set_project_ignores(RootId::new(root_id), patterns)
                .await
                .map_err(|e| eyre::eyre!("set_project_ignores: {e}"))?;
            print_patterns(&stored.project, json)?;
        }
        FilesCmd::Dehydrate {
            root_id,
            path,
            json,
        } => {
            let entry = set_residency_of(&vox_url, root_id, &path, false).await?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&entry).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!(
                    "{} dehydrated ({} bytes stay addressable in the store)",
                    entry.name,
                    entry.size.unwrap_or(0)
                );
            }
        }
        FilesCmd::Hydrate {
            root_id,
            path,
            json,
        } => {
            let entry = set_residency_of(&vox_url, root_id, &path, true).await?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&entry).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!(
                    "{} hydrated ({} bytes resident, verified by FileId)",
                    entry.name,
                    entry.size.unwrap_or(0)
                );
            }
        }
        FilesCmd::HydrationPolicy(FilesHydrationPolicyCmd::Show { root_id, json }) => {
            let sync: SyncServiceClient = establish_for_url(&vox_url).await?;
            let patterns = sync
                .residency(RootId::new(root_id))
                .await
                .map_err(|e| eyre::eyre!("residency: {e}"))?;
            print_patterns(&patterns, json)?;
        }
        FilesCmd::HydrationPolicy(FilesHydrationPolicyCmd::Set {
            root_id,
            patterns,
            json,
        }) => {
            let sync: SyncServiceClient = establish_for_url(&vox_url).await?;
            let stored = sync
                .set_residency(RootId::new(root_id), patterns)
                .await
                .map_err(|e| eyre::eyre!("set_residency: {e}"))?;
            print_patterns(&stored, json)?;
        }
        FilesCmd::HydrationPolicy(FilesHydrationPolicyCmd::Apply { root_id, json }) => {
            let sync: SyncServiceClient = establish_for_url(&vox_url).await?;
            let report = sync
                .apply_residency(RootId::new(root_id))
                .await
                .map_err(|e| eyre::eyre!("apply_residency: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&report).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!(
                    "hydrated {}, dehydrated {}, skipped {} dirty",
                    report.hydrated.len(),
                    report.dehydrated.len(),
                    report.skipped_dirty.len()
                );
                for path in &report.skipped_dirty {
                    println!("  dirty (checkpoint first): {path}");
                }
            }
        }
        FilesCmd::Gc {
            root_id,
            keep_newer_secs,
            json,
        } => {
            let versions: VersionServiceClient = establish_for_url(&vox_url).await?;
            let report = versions
                .collect(RootId::new(root_id), keep_newer_secs)
                .await
                .map_err(|e| eyre::eyre!("collect: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&report).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!(
                    "{} objects, {} manifests swept; {} vault-protected commits",
                    report.objects_swept, report.manifests_swept, report.protected_commits
                );
            }
        }
    }
    Ok(())
}

async fn run_files_root(cmd: FilesRootCmd, vox_url: &str) -> eyre::Result<()> {
    let roots: RootsServiceClient = establish_for_url(vox_url).await?;
    match cmd {
        FilesRootCmd::Create {
            path,
            name,
            flavor,
            json,
        } => {
            let name = name.unwrap_or_else(|| {
                std::path::Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone())
            });
            let root = roots
                .adopt(AdoptRequest {
                    path,
                    name,
                    flavor: flavor.into(),
                    hash_content: true,
                })
                .await
                .map_err(|e| eyre::eyre!("adopt: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&root).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!("{} ({})", root.id, placement(&root));
            }
        }
        FilesRootCmd::List { json } => {
            let roots = roots
                .list()
                .await
                .map_err(|e| eyre::eyre!("list roots: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&roots).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                for r in roots {
                    println!(
                        "{}  {:?}  {}  {}{}",
                        r.id,
                        r.flavor,
                        r.name,
                        placement(&r),
                        project_version_suffix(&r)
                    );
                }
            }
        }
        FilesRootCmd::Get { id, json } => {
            let root = roots
                .get(RootId::new(id))
                .await
                .map_err(|e| eyre::eyre!("get root: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&root).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!(
                    "{} [{:?}] ({}){}",
                    root.name,
                    root.flavor,
                    placement(&root),
                    project_version_suffix(&root)
                );
            }
        }
    }
    Ok(())
}

async fn run_files_version(cmd: FilesVersionCmd, vox_url: &str) -> eyre::Result<()> {
    let curation: CurationServiceClient = establish_for_url(vox_url).await?;
    match cmd {
        FilesVersionCmd::Name {
            root_id,
            commit_id,
            name,
            json,
        } => {
            let version = expand_commit(vox_url, root_id, &commit_id).await?;
            let named = curation
                .name_version(RootId::new(root_id), version, name)
                .await
                .map_err(|e| eyre::eyre!("name_version: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&named).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!(
                    "{}  {}  {}  ({})",
                    named.id,
                    short(&named.commit_id),
                    named.name,
                    named.path
                );
            }
        }
        FilesVersionCmd::List { root_id, json } => {
            let versions = curation
                .named_versions(root_id.map(RootId::new), None)
                .await
                .map_err(|e| eyre::eyre!("named_versions: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&versions).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                for v in versions {
                    println!("{}  {}  {}", v.id, short(&v.commit_id), v.name);
                }
            }
        }
        FilesVersionCmd::Resolve { id, json } => {
            let target = curation
                .named_version(id)
                .await
                .map_err(|e| eyre::eyre!("named_version: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&target).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!(
                    "root {}  change {}  commit {}",
                    target.root_id,
                    short(&target.change_id),
                    target.commit_id
                );
            }
        }
        FilesVersionCmd::Remove { id } => {
            // Unnaming is addressed by root; the entity says which.
            let named = curation
                .named_version(id)
                .await
                .map_err(|e| eyre::eyre!("named_version: {e}"))?;
            curation
                .unname_version(RootId::new(named.root_id), VersionId::new(id))
                .await
                .map_err(|e| eyre::eyre!("unname_version: {e}"))?;
            println!("removed {id}");
        }
    }
    Ok(())
}

async fn run_files_project_version(cmd: FilesProjectVersionCmd, vox_url: &str) -> eyre::Result<()> {
    match cmd {
        FilesProjectVersionCmd::Start {
            root_id,
            label,
            json,
        } => {
            let curation: CurationServiceClient = establish_for_url(vox_url).await?;
            let pv = curation
                .start_project_version(RootId::new(root_id), label.unwrap_or_default())
                .await
                .map_err(|e| eyre::eyre!("start_project_version: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&pv).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!("v{}{}  ({})", pv.number, label_suffix(&pv.label), pv.path);
            }
        }
        FilesProjectVersionCmd::List { root_id, json } => {
            let curation: CurationServiceClient = establish_for_url(vox_url).await?;
            let mut versions = curation
                .project_versions(RootId::new(root_id))
                .await
                .map_err(|e| eyre::eyre!("project_versions: {e}"))?;
            // Oldest first, as this listing always read.
            versions.sort_by_key(|v| v.number);
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&versions).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                for v in versions {
                    println!(
                        "v{}{}  {}  {}",
                        v.number,
                        label_suffix(&v.label),
                        short(&v.commit_id),
                        v.id
                    );
                }
            }
        }
        FilesProjectVersionCmd::Restart {
            root_id,
            from,
            empty,
            template,
            carry_forward,
            json,
        } => {
            let mode = match (empty, template, carry_forward) {
                (true, None, None) => files_proto::RestartMode::Empty,
                (false, Some(source_path), None) => {
                    files_proto::RestartMode::Template { source_path }
                }
                (false, None, Some(paths)) => files_proto::RestartMode::CarryForward { paths },
                _ => eyre::bail!("pick exactly one of --empty / --template / --carry-forward"),
            };
            let curation: CurationServiceClient = establish_for_url(vox_url).await?;
            let root = RootId::new(root_id);
            let versions = curation
                .project_versions(root)
                .await
                .map_err(|e| eyre::eyre!("project_versions: {e}"))?;
            // The iteration being restarted: the one asked for, else the
            // root's current lineage (its highest number).
            let target = match from {
                Some(number) => versions.iter().find(|v| v.number == number),
                None => versions.iter().max_by_key(|v| v.number),
            }
            .ok_or_else(|| match from {
                Some(number) => eyre::eyre!("root {root_id} has no project version v{number}"),
                None => eyre::eyre!(
                    "root {root_id} has no project version to restart — \
                     `task files project-version start` begins one"
                ),
            })?;
            let pv = curation
                .restart_project_version(root, ProjectVersionId::new(target.id), mode)
                .await
                .map_err(|e| eyre::eyre!("restart_project_version: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&pv).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!(
                    "restarted as v{}{} at {}",
                    pv.number,
                    label_suffix(&pv.label),
                    short(&pv.commit_id)
                );
            }
        }
        FilesProjectVersionCmd::BrowseAt {
            root_id,
            commit_id,
            subpath,
            json,
        } => {
            let version = expand_commit(vox_url, root_id, &commit_id).await?;
            let versions: VersionServiceClient = establish_for_url(vox_url).await?;
            let entries = versions
                .browse_at(RootId::new(root_id), root_path(&subpath)?, version)
                .await
                .map_err(|e| eyre::eyre!("browse_at: {e}"))?;
            print_entries(&entries, json)?;
        }
        FilesProjectVersionCmd::CopyForward {
            root_id,
            commit_id,
            paths,
            json,
        } => {
            let version = expand_commit(vox_url, root_id, &commit_id).await?;
            let versions: VersionServiceClient = establish_for_url(vox_url).await?;
            let paths = paths
                .iter()
                .map(|p| root_path(p))
                .collect::<eyre::Result<Vec<_>>>()?;
            let written = versions
                .copy_forward(RootId::new(root_id), version, paths)
                .await
                .map_err(|e| eyre::eyre!("copy_forward: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&written).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                for path in &written {
                    println!("{path}");
                }
                println!("{} file(s) copied forward", written.len());
            }
        }
    }
    Ok(())
}

/// A root-relative path as typed, validated the way the lanes will.
fn root_path(raw: &str) -> eyre::Result<RootPath> {
    RootPath::parse(raw).map_err(|e| eyre::eyre!("`{raw}`: {e}"))
}

/// Fetch or release one path's bytes, then read its entry back for the
/// report — the hydration verb answers with the paths it moved, and the
/// listing is what carries the size and the stub flag.
async fn set_residency_of(
    vox_url: &str,
    root_id: uuid::Uuid,
    raw: &str,
    resident: bool,
) -> eyre::Result<BrowseEntry> {
    let root = RootId::new(root_id);
    let path = root_path(raw)?;
    let sync: SyncServiceClient = establish_for_url(vox_url).await?;
    sync.hydrate(root, vec![path.clone()], resident)
        .await
        .map_err(|e| eyre::eyre!("{}: {e}", if resident { "hydrate" } else { "dehydrate" }))?;
    let (parent, name) = match raw.trim_matches('/').rsplit_once('/') {
        Some((parent, name)) => (parent.to_string(), name.to_string()),
        None => (String::new(), raw.trim_matches('/').to_string()),
    };
    let tree: TreeServiceClient = establish_for_url(vox_url).await?;
    tree.browse(root, root_path(&parent)?)
        .await
        .map_err(|e| eyre::eyre!("browse: {e}"))?
        .into_iter()
        .find(|e| e.name == name)
        .ok_or_else(|| eyre::eyre!("{path} is not in the root's listing"))
}

/// A commit as a person types it — any unambiguous prefix of the hex
/// `task files chain` prints — expanded to the version the lanes address.
///
/// A [`VersionId`] only round-trips a full commit hex (its first 32
/// characters), so a short prefix has to be matched against commits the
/// root actually has: its curated versions, snapshots and divergences
/// first, and only if none of those match, the chain of each file in the
/// catalogue until one does.
async fn expand_commit(vox_url: &str, root_id: uuid::Uuid, typed: &str) -> eyre::Result<VersionId> {
    let prefix = typed.trim().to_ascii_lowercase();
    if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
        eyre::bail!("`{typed}` is not a hex commit id");
    }
    if prefix.len() >= 32 {
        return Ok(VersionId::from_commit_hex(&prefix));
    }
    let root = RootId::new(root_id);
    let mut matches = std::collections::BTreeSet::new();
    let consider = |commit: &str, matches: &mut std::collections::BTreeSet<String>| {
        if commit.starts_with(&prefix) {
            matches.insert(commit.to_string());
        }
    };

    let versions: VersionServiceClient = establish_for_url(vox_url).await?;
    let curation: CurationServiceClient = establish_for_url(vox_url).await?;
    for v in curation
        .named_versions(Some(root), None)
        .await
        .map_err(|e| eyre::eyre!("named_versions: {e}"))?
    {
        consider(&v.commit_id, &mut matches);
    }
    for v in curation
        .project_versions(root)
        .await
        .map_err(|e| eyre::eyre!("project_versions: {e}"))?
    {
        consider(&v.commit_id, &mut matches);
    }
    for s in versions
        .snapshots(root, None)
        .await
        .map_err(|e| eyre::eyre!("snapshots: {e}"))?
    {
        consider(&s.snapshot_id, &mut matches);
    }
    for d in versions
        .divergences(root)
        .await
        .map_err(|e| eyre::eyre!("divergences: {e}"))?
    {
        for side in &d.sides {
            consider(&side.commit_id, &mut matches);
        }
    }

    // Nothing curated matched: walk the catalogue, reading each file's
    // chain, and stop at the first file whose history holds it.
    if matches.is_empty() {
        let tree: TreeServiceClient = establish_for_url(vox_url).await?;
        let mut cursor = None;
        'walk: loop {
            let page = tree
                .catalogue(root, cursor)
                .await
                .map_err(|e| eyre::eyre!("catalogue: {e}"))?;
            for entry in &page.changed {
                if entry.kind != EntryKind::File {
                    continue;
                }
                let chain = versions
                    .chain(root, entry.path.clone())
                    .await
                    .map_err(|e| eyre::eyre!("chain {}: {e}", entry.path))?;
                for c in &chain {
                    consider(&c.commit_id, &mut matches);
                }
                if !matches.is_empty() {
                    break 'walk;
                }
            }
            if !page.more {
                break;
            }
            cursor = Some(page.cursor);
        }
    }

    let mut found = matches.into_iter();
    match (found.next(), found.next()) {
        (Some(full), None) => Ok(VersionId::from_commit_hex(&full)),
        (None, _) => eyre::bail!("no commit in root {root_id} starts with `{typed}`"),
        (Some(a), Some(b)) => eyre::bail!(
            "`{typed}` is ambiguous in root {root_id} ({}, {}, …) — type more of it",
            short(&a),
            short(&b)
        ),
    }
}

/// Hex ids are long and only their prefix is ever typed back.
fn short(hex: &str) -> &str {
    &hex[..12.min(hex.len())]
}

fn label_suffix(label: &Option<String>) -> String {
    label
        .as_deref()
        .map(|l| format!(" — {l}"))
        .unwrap_or_default()
}

/// The root's current lineage (its highest-numbered Project Version),
/// as a printable suffix — empty for a root that has never been
/// restarted (issue #266).
fn project_version_suffix(root: &files_proto::FileRootInfo) -> String {
    match &root.project_version {
        Some(pv) => format!("  [v{}{}]", pv.number, label_suffix(&pv.label)),
        None => String::new(),
    }
}

fn print_entries(entries: &[files_proto::BrowseEntry], json: bool) -> eyre::Result<()> {
    if json {
        println!(
            "{}",
            facet_json::to_string(entries).map_err(|e| eyre::eyre!("{e}"))?
        );
        return Ok(());
    }
    for e in entries {
        let kind = if e.is_dir { "dir " } else { "file" };
        let size = e.size.map(|s| s.to_string()).unwrap_or_default();
        // Same badges the explorer renders (issue #266): a pointer stub
        // is tracked but not resident here, a divergent entry has
        // concurrent saves waiting to be resolved.
        let mut badges = String::new();
        if e.stub {
            badges.push_str("  [stub]");
        }
        if e.divergent {
            badges.push_str("  [divergent]");
        }
        println!("{kind}  {size:>10}  {}{badges}", e.name);
    }
    Ok(())
}

fn print_patterns(patterns: &[String], json: bool) -> eyre::Result<()> {
    if json {
        println!(
            "{}",
            facet_json::to_string(patterns).map_err(|e| eyre::eyre!("{e}"))?
        );
        return Ok(());
    }
    for p in patterns {
        println!("{p}");
    }
    Ok(())
}

/// A root's local tree, or a word for not having one.
///
/// A host may hold an org's structure and none of its content
/// (`files.peering.replication`), and printing an empty column for that
/// reads as a bug rather than as the answer.
fn placement(root: &files_proto::model::FileRootInfo) -> &str {
    root.path.as_deref().unwrap_or("(structure only)")
}

/// This machine's name for an org's device list.
fn machine_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this machine".into())
}
