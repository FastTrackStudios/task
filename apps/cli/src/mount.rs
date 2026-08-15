//! `task mount …` — per-machine project content mounts.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum MountCmd {
    /// Register a project's content path on this machine. By
    /// default the path is taken literally; pass `--under-vault`
    /// to resolve it against [`org_proto::default_client_vault_root`]
    /// (`$TASK_VAULT_ROOT` → `$HOME/Documents/Task`).
    Add {
        /// Project UUID (the federation-stable id).
        project_id: uuid::Uuid,
        /// Local path the project's content lives at.
        path: std::path::PathBuf,
        /// Resolve `path` against the client vault root instead
        /// of treating it as already absolute.
        #[arg(long)]
        under_vault: bool,
        /// Optional human-facing label.
        #[arg(long, default_value = "")]
        label: String,
        /// Overwrite an existing mount for this project.
        #[arg(long)]
        replace: bool,
    },
    /// Print every mount in the registry, sorted by project id.
    List,
    /// Remove the mount for a project. Idempotent — removing an
    /// unknown id is not an error.
    Rm { project_id: uuid::Uuid },
    /// Print the resolved path of `mounts.toml`. Useful for
    /// scripting + smoke tests.
    Path,
}

pub(crate) fn run_mount(cmd: MountCmd) -> eyre::Result<()> {
    let mut reg =
        mount::MountRegistry::from_env().map_err(|e| eyre::eyre!("load mounts.toml: {e}"))?;
    match cmd {
        MountCmd::Add {
            project_id,
            path,
            under_vault,
            label,
            replace,
        } => {
            let resolved = if under_vault {
                org_proto::default_client_vault_root()
                    .map_err(|e| eyre::eyre!("client vault root: {e}"))?
                    .join(&path)
            } else {
                path
            };
            let display = resolved.display().to_string();
            let mut mount = mount::Mount::filesystem(project_id, &display);
            mount.label = label;
            reg.add(mount, replace)
                .map_err(|e| eyre::eyre!("register mount: {e}"))?;
            reg.save()
                .map_err(|e| eyre::eyre!("save mounts.toml: {e}"))?;
            println!("Mounted project {project_id}");
            println!("  path:     {display}");
            println!("  registry: {}", reg.path().display());
        }
        MountCmd::List => {
            if reg.is_empty() {
                println!("(no mounts registered at {})", reg.path().display());
                return Ok(());
            }
            println!("registry: {}", reg.path().display());
            for mount in reg.iter() {
                let label = if mount.label.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", mount.label)
                };
                println!(
                    "  {} [{:?}] {}{label}",
                    mount.project_id, mount.backend, mount.path
                );
            }
        }
        MountCmd::Rm { project_id } => match reg.remove(project_id) {
            Some(prev) => {
                reg.save()
                    .map_err(|e| eyre::eyre!("save mounts.toml: {e}"))?;
                println!("Removed mount for {project_id} ({})", prev.path);
            }
            None => {
                println!("(no mount registered for {project_id})");
            }
        },
        MountCmd::Path => {
            println!("{}", reg.path().display());
        }
    }
    Ok(())
}
