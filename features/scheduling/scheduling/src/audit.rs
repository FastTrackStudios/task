//! Durable booking audit trail.
//!
//! Append-only JSONL inside the vault — the same shape the rest of
//! the tree uses for sidecar history (`links.jsonl`,
//! `collections.jsonl`, scripture's `text.jsonl`). One line per
//! event, oldest first, written next to the booking markdown it
//! describes so a vault copy carries its own history.
//!
//! Entries are additive facts, never rewritten: a booking create and
//! every later status change each append one line.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Vault-relative path of the audit log.
pub const AUDIT_LOG: &str = "Records/audit/booking-events.jsonl";

/// What happened to a booking. Stringly-typed on the wire so old
/// logs stay readable when new event kinds are added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookingAuditEntry {
    /// RFC 3339 UTC stamp of when the entry was appended.
    pub at_utc: String,
    /// `"created"` or `"status"`.
    pub event: String,
    /// The booking this entry is about.
    pub booking_id: String,
    /// New status, for `event == "status"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl BookingAuditEntry {
    #[must_use]
    pub fn created(booking_id: impl Into<String>) -> Self {
        Self {
            at_utc: chrono::Utc::now().to_rfc3339(),
            event: "created".into(),
            booking_id: booking_id.into(),
            status: None,
        }
    }

    #[must_use]
    pub fn status_changed(booking_id: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            at_utc: chrono::Utc::now().to_rfc3339(),
            event: "status".into(),
            booking_id: booking_id.into(),
            status: Some(status.into()),
        }
    }
}

fn log_path(root: &Path) -> PathBuf {
    root.join(AUDIT_LOG)
}

/// Append one entry. Creates `Records/audit/` on demand — an install
/// that never books anything never grows the directory.
pub fn append(root: &Path, entry: &BookingAuditEntry) -> std::io::Result<()> {
    use std::io::Write as _;

    let path = log_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(entry)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{line}")?;
    f.flush()
}

/// Every entry, oldest first. A missing log is an empty history, not
/// an error. Individual malformed lines are logged and skipped — one
/// bad line must not hide the rest of the trail.
pub fn read(root: &Path) -> std::io::Result<Vec<BookingAuditEntry>> {
    let path = log_path(root);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<BookingAuditEntry>(line) {
            Ok(entry) => out.push(entry),
            Err(e) => tracing::warn!(?path, error = %e, "scheduling: skip malformed audit line"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_then_read_round_trips() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        assert!(read(tmp.path()).unwrap().is_empty());

        append(tmp.path(), &BookingAuditEntry::created("b1")).unwrap();
        append(
            tmp.path(),
            &BookingAuditEntry::status_changed("b1", "Cancelled"),
        )
        .unwrap();

        let entries = read(tmp.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].event, "created");
        assert_eq!(entries[0].booking_id, "b1");
        assert_eq!(entries[1].status.as_deref(), Some("Cancelled"));
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        append(tmp.path(), &BookingAuditEntry::created("b1")).unwrap();
        std::fs::write(
            tmp.path().join(AUDIT_LOG),
            format!(
                "{}\nnot json at all\n",
                serde_json::to_string(&BookingAuditEntry::created("b1")).unwrap()
            ),
        )
        .unwrap();
        assert_eq!(read(tmp.path()).unwrap().len(), 1);
    }
}
