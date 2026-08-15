//! Disk-backed contacts directory. Contacts live as markdown at
//! `<vault_root>/Records/contacts/<id>.md`; CardDAV sync accounts live
//! at `<vault_root>/Records/contacts/.sync/<id>.md` (a hidden sibling
//! dir so they never show up as contacts). Writes serialize against a
//! coarse `Mutex` so concurrent UI/CLI/sync callers don't race on a
//! file. Mirrors `recall::VaultRecall`.
//!
//! NOTE: account files carry the CardDAV app-password in plaintext.
//! That is acceptable only because the vault is per-org and lives
//! server-side; it should move to a secret store when sync graduates
//! past MVP.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use thiserror::Error;

use contacts_proto::{CardDavAccount, Contact, Contacts, ContactsError, SyncReport};

use crate::parse::{frontmatter_split, parse_contact};
use crate::write::serialize_contact;

/// Contacts live under `Records/contacts/`.
const CONTACTS_DIR: &str = "Records/contacts";
/// Sync accounts live under a hidden sibling dir so the contact scan
/// (which reads `Records/contacts/*.md`, non-recursively) never trips
/// over them.
const ACCOUNTS_DIR: &str = "Records/contacts/.sync";

/// Errors not covered by `ContactsError` (path / root validation).
#[derive(Debug, Error)]
pub enum VaultContactsError {
    #[error("invalid vault root: {0}")]
    BadRoot(String),
}

/// Disk-backed [`Contacts`] implementation. `Clone` is cheap — the root
/// is a `PathBuf` and the lock is `Arc`'d — so the server can hand a
/// clone to the mounted vox descriptor.
#[derive(Clone, architect::HasDispatcher)]
pub struct VaultContacts {
    root: PathBuf,
    write_lock: Arc<Mutex<()>>,
    /// Fan-out hub behind the `#[subscribe] fn events` stream —
    /// every successful contact mutation publishes the post-write
    /// state here ([`contacts_proto::ContactsEvent`]); account edits
    /// don't stream. Sliding mailbox: a slow subscriber loses its
    /// *oldest* queued events, which is correct for state-shaped
    /// payloads. Clones share the hub (`Arc` inside).
    events: architect::PubSub<contacts_proto::ContactsEvent>,
}

impl VaultContacts {
    /// Open a directory rooted at `vault_root`. The subdirs are created
    /// lazily on first write so empty installs don't litter the vault.
    pub fn new(vault_root: impl Into<PathBuf>) -> Result<Self, VaultContactsError> {
        let root = vault_root.into();
        if !root.is_dir() {
            return Err(VaultContactsError::BadRoot(root.display().to_string()));
        }
        Ok(Self {
            root,
            write_lock: Arc::new(Mutex::new(())),
            events: architect::PubSub::sliding(256),
        })
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    fn write_file(&self, rel_path: &str, body: &str) -> Result<(), ContactsError> {
        let _guard = self.write_lock.lock().map_err(|_| ContactsError::Backend {
            message: "vault contacts lock poisoned".into(),
        })?;
        let abs = self.root.join(rel_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(&abs, body).map_err(io)?;
        Ok(())
    }

    fn delete_file(&self, rel_path: &str) -> Result<(), ContactsError> {
        let _guard = self.write_lock.lock().map_err(|_| ContactsError::Backend {
            message: "vault contacts lock poisoned".into(),
        })?;
        let abs = self.root.join(rel_path);
        if abs.exists() {
            std::fs::remove_file(&abs).map_err(io)?;
        }
        Ok(())
    }

    /// Read + parse every `.md` file directly under `dir` (non-recursive).
    fn scan_contacts(&self) -> Result<Vec<Contact>, ContactsError> {
        let dir = self.root.join(CONTACTS_DIR);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let raw = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(?path, error = %e, "contacts: skip unreadable file");
                    continue;
                }
            };
            let Some((fm, body)) = frontmatter_split(&raw) else {
                tracing::warn!(?path, "contacts: skip file with no frontmatter");
                continue;
            };
            let rel = relativize(&self.root, &path);
            match parse_contact(&rel, fm, body) {
                Ok(contact) => out.push(contact),
                Err(e) => tracing::warn!(?path, error = %e, "contacts: parse failed"),
            }
        }
        Ok(out)
    }

    fn scan_accounts(&self) -> Result<Vec<CardDavAccount>, ContactsError> {
        let dir = self.root.join(ACCOUNTS_DIR);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            match std::fs::read_to_string(&path).ok().and_then(|raw| {
                frontmatter_split(&raw).and_then(|(fm, _)| serde_yaml::from_str(fm).ok())
            }) {
                Some(acct) => out.push(acct),
                None => tracing::warn!(?path, "contacts: skip unreadable sync account"),
            }
        }
        Ok(out)
    }

    /// One account with its real (unredacted) password, for the sync +
    /// upsert-preserve paths.
    fn load_account(&self, id: &str) -> Result<Option<CardDavAccount>, ContactsError> {
        Ok(self.scan_accounts()?.into_iter().find(|a| a.id == id))
    }
}

impl Contacts for VaultContacts {
    fn list_contacts(&self) -> Result<Vec<Contact>, ContactsError> {
        let mut out = self.scan_contacts()?;
        // Alphabetical by display name — stable order for the directory.
        out.sort_by(|a, b| a.full_name.to_lowercase().cmp(&b.full_name.to_lowercase()));
        Ok(out)
    }

    fn get_contact(&self, id: String) -> Result<Option<Contact>, ContactsError> {
        Ok(self.scan_contacts()?.into_iter().find(|c| c.id == id))
    }

    fn upsert_contact(&self, contact: &Contact) -> Result<(), ContactsError> {
        if contact.id.trim().is_empty() {
            return Err(ContactsError::Invalid {
                field: "contact.id".into(),
                reason: "id must be non-empty".into(),
            });
        }
        let body = serialize_contact(contact).map_err(|e| ContactsError::Backend {
            message: e.to_string(),
        })?;
        self.write_file(&format!("{CONTACTS_DIR}/{}.md", sanitize(&contact.id)), &body)?;
        // Publish only after the write landed — subscribers fold
        // these into state fetched via `list_contacts()`, so a
        // phantom event would desync them. `sync_account` funnels
        // through here, so each pulled contact publishes too.
        self.events
            .publish(contacts_proto::ContactsEvent::Upserted(contact.clone()));
        Ok(())
    }

    fn delete_contact(&self, id: &str) -> Result<(), ContactsError> {
        self.delete_file(&format!("{CONTACTS_DIR}/{}.md", sanitize(id)))?;
        self.events
            .publish(contacts_proto::ContactsEvent::Deleted(id.to_owned()));
        Ok(())
    }

    fn list_accounts(&self) -> Result<Vec<CardDavAccount>, ContactsError> {
        let mut out: Vec<CardDavAccount> =
            self.scan_accounts()?.iter().map(CardDavAccount::redacted).collect();
        out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
        Ok(out)
    }

    fn upsert_account(&self, account: &CardDavAccount) -> Result<(), ContactsError> {
        if account.id.trim().is_empty() {
            return Err(ContactsError::Invalid {
                field: "account.id".into(),
                reason: "id must be non-empty".into(),
            });
        }
        // A blank password on an existing account keeps the stored one
        // (the UI never reads the secret back, so it round-trips blank).
        let mut to_store = account.clone();
        if to_store.password.is_empty() {
            if let Some(existing) = self.load_account(&account.id)? {
                to_store.password = existing.password;
            }
        }
        let yaml =
            serde_yaml::to_string(&to_store).map_err(|e| ContactsError::Backend {
                message: e.to_string(),
            })?;
        self.write_file(
            &format!("{ACCOUNTS_DIR}/{}.md", sanitize(&account.id)),
            &format!("---\n{yaml}---\n"),
        )
    }

    fn delete_account(&self, id: &str) -> Result<(), ContactsError> {
        self.delete_file(&format!("{ACCOUNTS_DIR}/{}.md", sanitize(id)))
    }

    fn sync_account(&self, id: String) -> Result<SyncReport, ContactsError> {
        let account = self
            .load_account(&id)?
            .ok_or_else(|| ContactsError::NotFound { id: id.clone() })?;

        // One-way pull: fetch + map the server's vCards, then reconcile
        // against local contacts by vCard UID. `manual` contacts are
        // never touched; a matching synced contact is updated in place.
        let pulled = crate::carddav::import(&account)?;
        let existing = self.scan_contacts()?;
        let mut report = SyncReport::empty(&id);

        for incoming in pulled {
            let matched = incoming.uid.as_deref().and_then(|uid| {
                existing
                    .iter()
                    .find(|c| c.uid.as_deref() == Some(uid) && c.is_synced())
            });
            match matched {
                Some(local) => {
                    // ETag match → unchanged, skip.
                    if local.etag.is_some() && local.etag == incoming.etag {
                        report.skipped += 1;
                        continue;
                    }
                    let mut merged = incoming.clone();
                    merged.id = local.id.clone();
                    merged.created = local.created.clone();
                    merged.linked_party_id = local.linked_party_id.clone();
                    merged.linked_user_id = local.linked_user_id.clone();
                    self.upsert_contact(&merged)?;
                    report.updated += 1;
                }
                None => {
                    self.upsert_contact(&incoming)?;
                    report.added += 1;
                }
            }
        }
        report.message = format!(
            "{} added, {} updated, {} unchanged",
            report.added, report.updated, report.skipped
        );
        Ok(report)
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in the
/// [`Contacts`] impl above, on every successful contact mutation.
impl contacts_proto::ContactsStreamSource for VaultContacts {
    fn events_hub(&self) -> &architect::PubSub<contacts_proto::ContactsEvent> {
        &self.events
    }
}

fn io(e: std::io::Error) -> ContactsError {
    ContactsError::Backend {
        message: format!("io: {e}"),
    }
}

/// Vault-relative forward-slash path.
fn relativize(root: &std::path::Path, abs: &std::path::Path) -> String {
    abs.strip_prefix(root).map_or_else(
        |_| abs.display().to_string(),
        |p| p.to_string_lossy().replace('\\', "/"),
    )
}

/// Strip anything that doesn't belong in a vault filename so a hostile
/// id can't escape its subdir.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '-',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use contacts_proto::CardDavProvider;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, VaultContacts) {
        let tmp = TempDir::new().expect("tempdir");
        let contacts = VaultContacts::new(tmp.path()).expect("new contacts");
        (tmp, contacts)
    }

    fn contact(id: &str, name: &str) -> Contact {
        Contact::create(id, name, "2026-07-17T09:00:00Z")
    }

    #[test]
    fn upsert_then_list_sorted() {
        let (_tmp, c) = fixture();
        c.upsert_contact(&contact("b", "Zed")).unwrap();
        c.upsert_contact(&contact("a", "Ada")).unwrap();
        let list = c.list_contacts().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].full_name, "Ada");
        assert_eq!(list[1].full_name, "Zed");
    }

    #[test]
    fn get_and_delete() {
        let (_tmp, c) = fixture();
        c.upsert_contact(&contact("x", "Ada")).unwrap();
        assert!(c.get_contact("x".into()).unwrap().is_some());
        c.delete_contact("x").unwrap();
        assert!(c.get_contact("x".into()).unwrap().is_none());
    }

    #[test]
    fn accounts_redact_password_but_not_contacts() {
        let (_tmp, c) = fixture();
        let mut acct = CardDavAccount::create("a1", "iCloud", CardDavProvider::ICLOUD, "2026-07-17T09:00:00Z");
        acct.username = "me@icloud.com".into();
        acct.password = "app-secret".into();
        c.upsert_account(&acct).unwrap();

        // Listed accounts are redacted…
        let listed = c.list_accounts().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].password, "");
        assert_eq!(listed[0].username, "me@icloud.com");
        // …but the stored secret survives.
        assert_eq!(c.load_account("a1").unwrap().unwrap().password, "app-secret");
        // Accounts never leak into the contact directory.
        assert!(c.list_contacts().unwrap().is_empty());
    }

    #[test]
    fn upsert_account_blank_password_preserves_secret() {
        let (_tmp, c) = fixture();
        let mut acct = CardDavAccount::create("a1", "iCloud", CardDavProvider::ICLOUD, "2026-07-17T09:00:00Z");
        acct.password = "app-secret".into();
        c.upsert_account(&acct).unwrap();
        // Re-upsert with a blank password (the redacted round-trip).
        let mut edited = acct.clone();
        edited.password = String::new();
        edited.label = "iCloud (personal)".into();
        c.upsert_account(&edited).unwrap();
        let stored = c.load_account("a1").unwrap().unwrap();
        assert_eq!(stored.password, "app-secret");
        assert_eq!(stored.label, "iCloud (personal)");
    }
}
