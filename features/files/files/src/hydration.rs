//! Per-root **hydration policy** (issue #263): which paths the platform
//! keeps hydrated when a policy pass runs.
//!
//! Same gitignore dialect and storage shape as the Ignore set
//! ([`crate::ignore`]), stored beside it in the root's store dir — but
//! the polarity is the opposite: a path **matching** the policy is kept
//! *hydrated*; everything else is kept dehydrated (glossary "Pointer
//! stub": "the agent hydrates on demand: explicitly, by root policy
//! patterns, or on access"). An empty policy — the default — is fully
//! opt-in: it hydrates nothing and dehydrates nothing, so a root with
//! no policy behaves exactly as before this ticket.
//!
//! Storing patterns changes no file;
//! [`files_proto::FilesService::apply_hydration_policy`] is the pass
//! that does, and dirty files (content differing from the checkpoint
//! head) are never dehydrated by it — see `backend`'s apply
//! implementation.

use std::path::Path;
use std::sync::Arc;

use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::repo_path::RepoPath;

use crate::error::{Error, Result};
use crate::ignore;

/// Stored beside the Ignore set in the root's store dir.
const POLICY_FILE: &str = "hydration-policy.json";

/// The root's stored hydration-policy patterns. Absent file = empty
/// policy, the opt-in default.
pub fn stored_policy(store_dir: &Path) -> Result<Vec<String>> {
    let path = store_dir.join(POLICY_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(&path)?;
    facet_json::from_slice(&bytes)
        .map_err(|e| crate::error::Error::BadRequest(format!("hydration policy: {e}")))
}

/// Replace the root's stored patterns, returning them normalized —
/// same trim/dedup/order rules as the Ignore set (`!` re-includes are
/// order-sensitive in both).
pub fn save_policy(store_dir: &Path, patterns: Vec<String>) -> Result<Vec<String>> {
    let normalized = ignore::normalize(patterns)?;
    std::fs::create_dir_all(store_dir)?;
    let path = store_dir.join(POLICY_FILE);
    let temp = path.with_extension("json.tmp");
    let json = facet_json::to_string(&normalized)
        .map_err(|e| crate::error::Error::BadRequest(format!("hydration policy: {e}")))?;
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, &path)?;
    Ok(normalized)
}

/// The stored policy as one matcher, or `None` for the empty policy —
/// callers must treat `None` as "touch nothing", not "matches nothing"
/// (an empty policy that dehydrated everything would be a destructive
/// default).
pub fn matcher(store_dir: &Path) -> Result<Option<Arc<GitIgnoreFile>>> {
    let stored = stored_policy(store_dir)?;
    if stored.is_empty() {
        return Ok(None);
    }
    let body = stored.join("\n") + "\n";
    GitIgnoreFile::empty()
        .chain(
            RepoPath::root(),
            Path::new("<hydration-policy>"),
            body.as_bytes(),
        )
        .map(Some)
        .map_err(|e| Error::Repo(format!("building the hydration policy: {e}")))
}

/// Does the policy keep `rel_path` hydrated? Matching reuses the
/// gitignore matcher, so pattern semantics (anchoring, `**`, trailing
/// `/`, `!` re-includes) are identical to the Ignore set's — including
/// directory patterns: `stems/` keeps everything under `stems/`
/// hydrated. The scan walker gets that fold-down for free by pruning
/// as it descends; a flat path query has to ask the ancestors itself.
#[must_use]
pub fn keeps_hydrated(policy: &GitIgnoreFile, rel_path: &str) -> bool {
    let Ok(path) = jj_lib::repo_path::RepoPathBuf::from_internal_string(rel_path) else {
        return false;
    };
    if policy.matches_file(&path) {
        return true;
    }
    let mut ancestor = path.parent();
    while let Some(dir) = ancestor {
        if dir.is_root() {
            break;
        }
        if policy.matches_dir(dir) {
            return true;
        }
        ancestor = dir.parent();
    }
    false
}
