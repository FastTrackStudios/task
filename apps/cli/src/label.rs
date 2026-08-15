//! `task label …` — org-scoped labels.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::resolve_active_org;

/// `task label *` — org-scoped colored tags.
#[derive(Subcommand)]
pub(crate) enum LabelCmd {
    /// Create a label (idempotent on name within the org).
    Create {
        name: String,
        /// 6-char hex color without `#` (e.g. `d73a4a`).
        #[arg(long)]
        color: Option<String>,
        /// Optional group (e.g. `priority`, `area`).
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Scope the label to one project (UUID). Omit for an
        /// org-wide label available across every project.
        #[arg(long)]
        project: Option<uuid::Uuid>,
        #[arg(long)]
        org: Option<String>,
    },
    /// List labels in the org. By default shows every label;
    /// `--project` narrows to that project's labels plus the
    /// org-wide ones.
    List {
        /// Only labels visible to this project (UUID): the
        /// project's own labels plus org-wide (unscoped) ones.
        #[arg(long)]
        project: Option<uuid::Uuid>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Remove a label by name.
    Rm {
        name: String,
        #[arg(long)]
        org: Option<String>,
    },
}

/// Path to the per-org label store JSON.
///
/// Vox-unification judgment: labels ARE org data, but no label
/// service exists to route them through (label-proto is types-only
/// and the org router mounts nothing for it) — a known gap. Until
/// that surface lands, the store stays this machine-local JSON in
/// the per-org dir, which co-resides with the local/embedded
/// server's data root; a remote-only session cannot see it.
fn label_store_path(org_slug: &str) -> eyre::Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| eyre::eyre!("HOME not set"))?;
    Ok(std::path::Path::new(&home)
        .join(".task")
        .join("orgs")
        .join(org_slug)
        .join("labels.json"))
}

fn load_labels(org_slug: &str) -> eyre::Result<Vec<label_proto::Label>> {
    let p = label_store_path(org_slug)?;
    if !p.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(&p).map_err(|e| eyre::eyre!("read {}: {e}", p.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| eyre::eyre!("parse labels.json: {e}"))
}

fn save_labels(org_slug: &str, labels: &[label_proto::Label]) -> eyre::Result<()> {
    let p = label_store_path(org_slug)?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| eyre::eyre!("mkdir: {e}"))?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(labels)?)
        .map_err(|e| eyre::eyre!("write: {e}"))?;
    std::fs::rename(&tmp, &p).map_err(|e| eyre::eyre!("rename: {e}"))?;
    Ok(())
}

pub(crate) fn run_label(cmd: LabelCmd) -> eyre::Result<()> {
    match cmd {
        LabelCmd::Create {
            name,
            color,
            group,
            description,
            project,
            org,
        } => {
            let slug = resolve_active_org(org)?;
            let mut labels = load_labels(&slug)?;
            if let Some(existing) = labels
                .iter_mut()
                .find(|l| l.name.eq_ignore_ascii_case(&name))
            {
                // Idempotent: update color/group/description/scope on re-create.
                existing.color = color.or(existing.color.take());
                existing.group = group.or(existing.group.take());
                existing.description = description.or(existing.description.take());
                existing.project_id = project.or(existing.project_id.take());
                existing.updated_at = chrono::Utc::now();
                save_labels(&slug, &labels)?;
                println!("updated label `{name}`");
                return Ok(());
            }
            // org-scoped: workspace_id is nil (no Workspace entity).
            let mut l = label_proto::Label::new(uuid::Uuid::nil(), &name);
            l.color = color;
            l.group = group;
            l.description = description;
            l.project_id = project;
            labels.push(l);
            save_labels(&slug, &labels)?;
            println!("created label `{name}`");
        }
        LabelCmd::List { project, org, json } => {
            let slug = resolve_active_org(org)?;
            let mut labels = load_labels(&slug)?;
            // `--project` narrows to labels visible to that project:
            // its own labels plus the org-wide (unscoped) ones.
            if let Some(pid) = project {
                labels.retain(|l| l.project_id.is_none() || l.project_id == Some(pid));
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&labels)?);
                return Ok(());
            }
            if labels.is_empty() {
                println!("(no labels)");
                return Ok(());
            }
            for l in &labels {
                let color = l
                    .color
                    .as_deref()
                    .map_or(String::new(), |c| format!(" #{c}"));
                let group = l
                    .group
                    .as_deref()
                    .map_or(String::new(), |g| format!(" [{g}]"));
                // Mark project-scoped labels so they're distinguishable
                // from org-wide ones in the plain listing.
                let scope = l
                    .project_id
                    .map_or(String::new(), |p| format!(" (project {p})"));
                println!("{}{group}{color}{scope}", l.name);
            }
        }
        LabelCmd::Rm { name, org } => {
            let slug = resolve_active_org(org)?;
            let mut labels = load_labels(&slug)?;
            let before = labels.len();
            labels.retain(|l| !l.name.eq_ignore_ascii_case(&name));
            if labels.len() == before {
                return Err(eyre::eyre!("no label named `{name}`"));
            }
            save_labels(&slug, &labels)?;
            println!("removed label `{name}`");
        }
    }
    Ok(())
}
