//! `task files …` — the Files RPC surface (issue #259, ADR 0001):
//! turn a folder into a File Root, browse it, read a file's version
//! chain, checkpoint on demand. Talks to the org's `FilesService` over
//! vox — remote server or embedded in-process backend alike, exactly
//! like `task timer …` (see `establish_for_url`).
//!
//! Issue #261 adds the curated verbs — `task files version …` (Named
//! Versions), `task files project-version …` (Project Versions), and
//! `task files gc` (the Vault-protected sweep). Those entities are
//! vault pages, so they are equally editable in a text editor; the CLI
//! is the path that also validates the reference against the store.

use clap::{Subcommand, ValueEnum};
use files_proto::{FilesServiceClient, RootFlavor};

use crate::establish_for_url;
use crate::resolve_org_vox_url;

#[derive(Subcommand)]
pub(crate) enum FilesCmd {
    /// File Root CRUD (create / list / get).
    #[command(subcommand)]
    Root(FilesRootCmd),
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
    /// (a pure lineage cut).
    Restart {
        root_id: uuid::Uuid,
        #[arg(long)]
        label: Option<String>,
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

pub(crate) async fn run_files(cmd: FilesCmd, org_override: Option<&str>) -> eyre::Result<()> {
    let slug = crate::resolve_slug(org_override)?;
    let vox_url = resolve_org_vox_url(None, &slug);
    let client: FilesServiceClient = establish_for_url(&vox_url).await?;

    match cmd {
        FilesCmd::Root(FilesRootCmd::Create {
            path,
            name,
            flavor,
            json,
        }) => {
            let name = name.unwrap_or_else(|| {
                std::path::Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone())
            });
            let root = client
                .create_root(path, name, flavor.into())
                .await
                .map_err(|e| eyre::eyre!("create_root: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&root).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                println!("{} ({})", root.id, placement(&root));
            }
        }
        FilesCmd::Root(FilesRootCmd::List { json }) => {
            let roots = client
                .list_roots()
                .await
                .map_err(|e| eyre::eyre!("list_roots: {e}"))?;
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
        FilesCmd::Root(FilesRootCmd::Get { id, json }) => {
            let root = client
                .get_root(id)
                .await
                .map_err(|e| eyre::eyre!("get_root: {e}"))?;
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
        FilesCmd::Browse {
            root_id,
            subpath,
            json,
        } => {
            let entries = client
                .browse(root_id, subpath)
                .await
                .map_err(|e| eyre::eyre!("browse: {e}"))?;
            print_entries(&entries, json)?;
        }
        FilesCmd::DriveBrowse { path, json } => {
            let entries = client
                .drive_browse(path)
                .await
                .map_err(|e| eyre::eyre!("drive_browse: {e}"))?;
            print_entries(&entries, json)?;
        }
        FilesCmd::Chain {
            root_id,
            path,
            json,
        } => {
            let chain = client
                .chain(root_id, path)
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
            let info = client
                .checkpoint_now(root_id, message)
                .await
                .map_err(|e| eyre::eyre!("checkpoint_now: {e}"))?;
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
            let snapshots = client
                .snapshots(root_id)
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
        FilesCmd::Version(FilesVersionCmd::Name {
            root_id,
            commit_id,
            name,
            json,
        }) => {
            let named = client
                .name_version(root_id, commit_id, name)
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
        FilesCmd::Version(FilesVersionCmd::List { root_id, json }) => {
            let versions = client
                .list_named_versions(root_id)
                .await
                .map_err(|e| eyre::eyre!("list_named_versions: {e}"))?;
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
        FilesCmd::Version(FilesVersionCmd::Resolve { id, json }) => {
            let target = client
                .resolve_named_version(id)
                .await
                .map_err(|e| eyre::eyre!("resolve_named_version: {e}"))?;
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
        FilesCmd::Version(FilesVersionCmd::Remove { id }) => {
            client
                .unname_version(id)
                .await
                .map_err(|e| eyre::eyre!("unname_version: {e}"))?;
            println!("removed {id}");
        }
        FilesCmd::ProjectVersion(FilesProjectVersionCmd::Start {
            root_id,
            label,
            json,
        }) => {
            let pv = client
                .start_project_version(root_id, label)
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
        FilesCmd::ProjectVersion(FilesProjectVersionCmd::List { root_id, json }) => {
            let versions = client
                .list_project_versions(root_id)
                .await
                .map_err(|e| eyre::eyre!("list_project_versions: {e}"))?;
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
        FilesCmd::ProjectVersion(FilesProjectVersionCmd::Restart {
            root_id,
            label,
            empty,
            template,
            carry_forward,
            json,
        }) => {
            let mode = match (empty, template, carry_forward) {
                (true, None, None) => files_proto::RestartMode::Empty,
                (false, Some(source_path), None) => {
                    files_proto::RestartMode::Template { source_path }
                }
                (false, None, Some(paths)) => files_proto::RestartMode::CarryForward { paths },
                _ => eyre::bail!("pick exactly one of --empty / --template / --carry-forward"),
            };
            let pv = client
                .restart_project_version(root_id, mode, label)
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
        FilesCmd::ProjectVersion(FilesProjectVersionCmd::BrowseAt {
            root_id,
            commit_id,
            subpath,
            json,
        }) => {
            let entries = client
                .browse_at(root_id, commit_id, subpath)
                .await
                .map_err(|e| eyre::eyre!("browse_at: {e}"))?;
            print_entries(&entries, json)?;
        }
        FilesCmd::ProjectVersion(FilesProjectVersionCmd::CopyForward {
            root_id,
            commit_id,
            paths,
            json,
        }) => {
            let written = client
                .copy_forward(root_id, commit_id, paths)
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
        FilesCmd::Hint { root_id, paths } => {
            let accepted = client
                .hint_activity(root_id, paths)
                .await
                .map_err(|e| eyre::eyre!("hint_activity: {e}"))?;
            println!("{accepted} hints accepted (the rest are in the Ignore set)");
        }
        FilesCmd::Ignore(FilesIgnoreCmd::Show { root_id, json }) => {
            let patterns = client
                .ignore_set(root_id)
                .await
                .map_err(|e| eyre::eyre!("ignore_set: {e}"))?;
            print_patterns(&patterns, json)?;
        }
        FilesCmd::Ignore(FilesIgnoreCmd::Set {
            root_id,
            patterns,
            json,
        }) => {
            let stored = client
                .set_ignore_set(root_id, patterns)
                .await
                .map_err(|e| eyre::eyre!("set_ignore_set: {e}"))?;
            print_patterns(&stored, json)?;
        }
        FilesCmd::Dehydrate {
            root_id,
            path,
            json,
        } => {
            let entry = client
                .dehydrate(root_id, path)
                .await
                .map_err(|e| eyre::eyre!("dehydrate: {e}"))?;
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
            let entry = client
                .hydrate(root_id, path)
                .await
                .map_err(|e| eyre::eyre!("hydrate: {e}"))?;
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
            let patterns = client
                .hydration_policy(root_id)
                .await
                .map_err(|e| eyre::eyre!("hydration_policy: {e}"))?;
            print_patterns(&patterns, json)?;
        }
        FilesCmd::HydrationPolicy(FilesHydrationPolicyCmd::Set {
            root_id,
            patterns,
            json,
        }) => {
            let stored = client
                .set_hydration_policy(root_id, patterns)
                .await
                .map_err(|e| eyre::eyre!("set_hydration_policy: {e}"))?;
            print_patterns(&stored, json)?;
        }
        FilesCmd::HydrationPolicy(FilesHydrationPolicyCmd::Apply { root_id, json }) => {
            let report = client
                .apply_hydration_policy(root_id)
                .await
                .map_err(|e| eyre::eyre!("apply_hydration_policy: {e}"))?;
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
            let report = client
                .gc_root(root_id, keep_newer_secs)
                .await
                .map_err(|e| eyre::eyre!("gc_root: {e}"))?;
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
