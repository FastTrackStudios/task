//! Maildir folder resolution. INBOX is the account root itself;
//! sub-mailboxes live as Maildir++ sibling dirs (name starts
//! with `.`).

use email_proto::FolderRole;
use std::path::{Path, PathBuf};

/// Folder id used on the wire. `"INBOX"` for the root maildir;
/// for sub-mailboxes, the Maildir++ name with the leading `.`
/// stripped (`Sent` for `.Sent`, `Lists.rust-users` for
/// `.Lists.rust-users`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FolderName(pub String);

impl FolderName {
    pub const INBOX: &'static str = "INBOX";

    #[must_use]
    pub fn inbox() -> Self {
        Self(Self::INBOX.into())
    }

    /// Map a wire folder id to an absolute maildir path under
    /// `account_root`. Returns `None` for paths that fail
    /// basic validation (traversal attempts, empty names).
    #[must_use]
    pub fn to_path(&self, account_root: &Path) -> Option<PathBuf> {
        if self.0 == Self::INBOX {
            return Some(account_root.to_path_buf());
        }
        if self.0.is_empty() || self.0.starts_with('.') || self.0.contains('/') {
            return None;
        }
        Some(account_root.join(format!(".{}", self.0)))
    }

    /// Inverse of [`Self::to_path`] — given a Maildir++ dir
    /// name (`.Sent`, `.Lists.rust-users`), return the wire id.
    /// Returns `None` for entries that aren't sub-mailboxes.
    #[must_use]
    pub fn from_maildir_dir_name(name: &str) -> Option<Self> {
        let stripped = name.strip_prefix('.')?;
        if stripped.is_empty() {
            return None;
        }
        Some(Self(stripped.to_string()))
    }
}

/// Best-effort role inference from a folder name. Honors the
/// usual conventions — case-insensitive matches on common
/// names. Backends may want to override this from server-side
/// SPECIAL-USE annotations later.
pub fn infer_role(folder: &str) -> Option<FolderRole> {
    let lower = folder.to_ascii_lowercase();
    let leaf = lower.rsplit('.').next().unwrap_or(&lower);
    match leaf {
        "inbox" => Some(FolderRole::Inbox),
        "drafts" | "draft" => Some(FolderRole::Drafts),
        "sent" | "sent items" | "sent messages" => Some(FolderRole::Sent),
        "trash" | "deleted" | "deleted items" | "bin" => Some(FolderRole::Trash),
        "junk" | "spam" => Some(FolderRole::Junk),
        "archive" | "archives" | "all mail" => Some(FolderRole::Archive),
        "outbox" => Some(FolderRole::Outbox),
        "flagged" | "starred" => Some(FolderRole::Flagged),
        _ => None,
    }
}
