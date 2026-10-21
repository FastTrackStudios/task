//! Walk a `vault::Vault` and collect every page that looks like
//! a task. Returns the parsed `TaskInfo` rows; pages that don't
//! match the task discriminator are skipped silently.

use vault::Vault;

use crate::model::TaskInfo;
use crate::parse::{looks_like_task_raw, parse_str};

#[must_use]
pub fn scan_vault(vault: &Vault) -> Vec<TaskInfo> {
    vault
        .pages
        .iter()
        // Tested on the raw text, then parsed from the borrowed page: the
        // parser reads only the path, the basename and the text, so
        // there is no need to `to_proto` (clone) the page to hand it over.
        .filter(|p| looks_like_task_raw(&p.raw))
        .filter_map(|p| match parse_str(&p.rel_path, &p.basename, &p.raw) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(path = %p.rel_path, ?e, "task parse failed");
                None
            }
        })
        .collect()
}
