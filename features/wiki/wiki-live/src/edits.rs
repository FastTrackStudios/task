//! The Edit Request store: one JSON file per request under
//! `<wiki>/_state/edits/<id>.json`.
//!
//! The tracker row holds the request's *status* as an issue
//! (`wiki.edit.tracked`); this holds what the tracker cannot — the
//! change itself, the claim, the base the proposer saw. The whole
//! [`EditRequest`] is written here too, so a wiki read with no tracker
//! at hand (a mounted folder, a peer's copy) still shows every request
//! and who made it.
//!
//! Writes are temp + rename, like `config.rs`: a crash mid-write leaves
//! the previous request on disk rather than half of a new one.

use std::path::{Path, PathBuf};

use wiki_proto::error::WikiError;
use wiki_proto::paths;
use wiki_proto::service::edits::EditRequest;

/// Where a wiki's requests live.
#[must_use]
pub fn edits_dir(wiki_root: &Path) -> PathBuf {
    wiki_root.join(paths::STATE_DIR).join(paths::EDITS_DIR)
}

/// The file one request lives in.
#[must_use]
pub fn request_path(wiki_root: &Path, id: uuid::Uuid) -> PathBuf {
    edits_dir(wiki_root).join(format!("{id}.json"))
}

/// One request, or `NotFound` naming the id.
pub fn load(wiki_root: &Path, id: uuid::Uuid) -> Result<EditRequest, WikiError> {
    let path = request_path(wiki_root, id);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| WikiError::Backend(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(WikiError::NotFound(format!("edit request {id}")))
        }
        Err(e) => Err(WikiError::Io(format!("{}: {e}", path.display()))),
    }
}

/// Write one request. Atomic.
pub fn save(wiki_root: &Path, request: &EditRequest) -> Result<(), WikiError> {
    let path = request_path(wiki_root, request.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WikiError::Io(e.to_string()))?;
    }
    let json = serde_json::to_vec_pretty(request).map_err(|e| WikiError::Backend(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| WikiError::Io(format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path).map_err(|e| WikiError::Io(format!("{}: {e}", path.display())))
}

/// Every request against a wiki, oldest first. A wiki that has never
/// had one lists empty rather than failing.
pub fn list(wiki_root: &Path) -> Result<Vec<EditRequest>, WikiError> {
    let dir = edits_dir(wiki_root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(WikiError::Io(format!("{}: {e}", dir.display()))),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let bytes =
            std::fs::read(&path).map_err(|e| WikiError::Io(format!("{}: {e}", path.display())))?;
        let request: EditRequest = serde_json::from_slice(&bytes)
            .map_err(|e| WikiError::Backend(format!("{}: {e}", path.display())))?;
        out.push(request);
    }
    out.sort_by(|a, b| a.opened_at.cmp(&b.opened_at).then(a.id.cmp(&b.id)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiki_proto::service::edits::{EditStatus, PageChange};

    fn request(title: &str, opened_at: &str) -> EditRequest {
        EditRequest {
            id: uuid::Uuid::new_v4(),
            wiki: "theory".into(),
            title: title.into(),
            summary: String::new(),
            proposer: "sam".into(),
            opened_at: opened_at.into(),
            status: EditStatus::Open,
            resolved_by: String::new(),
            resolved_at: String::new(),
            resolution: String::new(),
            claimed_by: String::new(),
            claimed_until: String::new(),
            auto_approved: false,
            held: false,
            landing: String::new(),
            changes: vec![PageChange {
                path: "Concepts/Ionian.md".into(),
                base_sha256: "abc".into(),
                base_markdown: "before".into(),
                markdown: "after".into(),
                delete: false,
            }],
        }
    }

    #[test]
    fn an_empty_wiki_lists_nothing_and_a_missing_id_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list(dir.path()).unwrap().is_empty());
        let err = load(dir.path(), uuid::Uuid::new_v4()).unwrap_err();
        assert!(matches!(err, WikiError::NotFound(_)), "{err:?}");
    }

    #[test]
    fn save_then_load_round_trips_and_lists_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let later = request("later", "2026-09-02T00:00:00Z");
        let earlier = request("earlier", "2026-09-01T00:00:00Z");
        save(dir.path(), &later).unwrap();
        save(dir.path(), &earlier).unwrap();
        assert_eq!(load(dir.path(), later.id).unwrap(), later);
        let titles: Vec<String> = list(dir.path())
            .unwrap()
            .into_iter()
            .map(|r| r.title)
            .collect();
        assert_eq!(titles, vec!["earlier", "later"]);
        assert!(
            !request_path(dir.path(), later.id)
                .with_extension("json.tmp")
                .exists()
        );
    }

    #[test]
    fn saving_again_replaces_rather_than_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = request("one", "2026-09-01T00:00:00Z");
        save(dir.path(), &r).unwrap();
        r.status = EditStatus::Accepted;
        save(dir.path(), &r).unwrap();
        let all = list(dir.path()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, EditStatus::Accepted);
    }
}
