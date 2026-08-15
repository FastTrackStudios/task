//! Vault-wide scanner.

use thiserror::Error;
use vault::Vault;
use vault_entity::store::VaultEntity;

use crate::entity::Projects;
use crate::model::ProjectInfo;
use crate::parse::ParseError;

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("vault: {0}")]
    Vault(String),
}

/// Collect every project page in the vault. Logs + skips
/// files whose frontmatter fails to parse — one malformed
/// project shouldn't nuke the whole list.
///
/// Pages without a persisted `id:` (e.g. just-created by hand
/// without running `write_project` first) get a deterministic
/// uuid-v5 of their vault-relative path (see `parse_page`), so
/// repeated scans — and `get(id)` against a just-fetched
/// `list()` — always agree. The on-disk file is **not**
/// rewritten here; the id persists the next time the page is
/// saved (`write_project` always emits `id:`).
///
/// Hand-written rather than [`vault_entity::VaultEntityStore::scan`]
/// because a project page that has the discriminator but no complete
/// frontmatter is skipped *silently* (it happens mid-edit), where the
/// shared scan warns on every parse failure.
pub fn scan_vault(vault: &Vault) -> Result<Vec<ProjectInfo>, ScanError> {
    let mut out = Vec::new();
    for page in &vault.pages {
        if !Projects::matches(page) {
            continue;
        }
        match Projects::from_page(page) {
            Ok(p) => out.push(p),
            Err(ParseError::NoFrontmatter) => {
                // Project discriminator without a complete
                // frontmatter — skip silently. Could happen
                // mid-edit.
            }
            Err(e) => {
                tracing::warn!(path = %page.rel_path, error = %e, "project: parse failed");
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_idless_project(root: &std::path::Path) {
        let dir = root.join("Projects");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Scheduling.md"),
            "---\ntype: project\ntitle: Scheduling\n---\nbody\n",
        )
        .unwrap();
    }

    /// The live-data bug: a page with no persisted `id:` must
    /// resolve to the SAME id on every scan (deep links,
    /// `task.project_id` pointers, and `get(list()[i].id)` all
    /// depend on it).
    #[test]
    fn idless_page_gets_stable_id_across_scans() {
        let tmp = tempfile::tempdir().unwrap();
        write_idless_project(tmp.path());

        let first = scan_vault(&Vault::open(tmp.path()).unwrap()).unwrap();
        let second = scan_vault(&Vault::open(tmp.path()).unwrap()).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert!(!first[0].id.is_nil(), "scan must surface a usable id");
        assert_eq!(
            first[0].id, second[0].id,
            "two consecutive scans must agree on an id-less page's id"
        );
        // Deterministic derivation: v5 of the vault-relative path,
        // matching the task / milestone / goal / workstream parsers.
        assert_eq!(
            first[0].id,
            uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_URL,
                "Projects/Scheduling.md".as_bytes()
            )
        );
    }

    /// A persisted `id:` always wins over the derived fallback.
    #[test]
    fn persisted_id_wins_over_derived() {
        let tmp = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4();
        let dir = tmp.path().join("Projects");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Pinned.md"),
            format!("---\ntype: project\nid: {id}\ntitle: Pinned\n---\n"),
        )
        .unwrap();
        let got = scan_vault(&Vault::open(tmp.path()).unwrap()).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, id);
    }
}
