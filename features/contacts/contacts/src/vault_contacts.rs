//! Disk-backed contacts directory. Contacts live as markdown at
//! `<vault_root>/Records/contacts/<id>.md`; CardDAV sync accounts live
//! at `<vault_root>/Records/contacts/.sync/<id>.md` (a hidden sibling
//! dir so they never show up as contacts). Writes serialize against a
//! coarse `Mutex` so concurrent UI/CLI/sync callers don't race on a
//! file. Mirrors `recall::VaultRecall`.
//!
//! # The account directory is not vault content
//!
//! An account file holds connection details and a [`PasswordRef`] —
//! **never the password** (`contacts.secret.out-of-vault`). The
//! password itself goes to a [`CredentialStore`]: the OS keyring, or
//! the `auth::crypto` envelope where there is none (see
//! [`crate::credentials`]).
//!
//! And the directory is node-private: [`VaultContacts`] writes
//! `files_proto`'s `PRIVATE_MARKER` into it before anything else, so
//! Files never versions it (no history, no replica), the vault manifest
//! leaves it out, and no lane serves it by name
//! (`vault.private.node-only`). A peer syncing this vault gets the
//! contacts and nothing about the accounts they came from.
//!
//! # Migration
//!
//! Account files written before this existed carried the password in
//! plaintext. Opening a [`VaultContacts`] moves any it finds into the
//! credential store and rewrites the file with a reference in its
//! place; where no store will take it, the password is removed anyway
//! and the account says it needs re-entering. Plaintext never survives
//! an open.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use contacts_proto::{CardDavAccount, Contact, Contacts, ContactsError, SyncReport};

use crate::carddav::Transport;
use crate::credentials::{CredentialStore, NativeCredentials, PasswordRef, Preference};
use crate::parse::{frontmatter_split, parse_contact};
use crate::write::serialize_contact;

/// Contacts live under `Records/contacts/`.
const CONTACTS_DIR: &str = "Records/contacts";
/// Sync accounts live under a hidden sibling dir so the contact scan
/// (which reads `Records/contacts/*.md`, non-recursively) never trips
/// over them. Node-private: see the module docs.
pub const ACCOUNTS_DIR: &str = "Records/contacts/.sync";

/// What an account that lost its plaintext password in the migration
/// says until somebody re-enters it.
pub const REENTER_STATUS: &str =
    "password removed from the vault by a security upgrade; re-enter it to sync";

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
    /// Where account passwords go instead of the vault.
    credentials: Arc<dyn CredentialStore>,
    /// How the CardDAV pull reaches a server (trusted roots).
    transport: Transport,
    /// Prefix of every credential key this vault files, so two vaults
    /// on one machine never share a keyring entry.
    key_prefix: String,
}

/// One account as it sits on disk: the wire shape minus the password,
/// plus the reference that replaces it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredAccount {
    id: String,
    label: String,
    provider: String,
    server_url: String,
    username: String,
    /// Where the password is. `None` until one is entered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password_ref: Option<PasswordRef>,
    /// The legacy plaintext field. Read so the migration can move it
    /// out; **never written**.
    #[serde(default, skip_serializing)]
    password: Option<String>,
    #[serde(default)]
    addressbook_url: Option<String>,
    #[serde(default = "enabled_default")]
    enabled: bool,
    #[serde(default)]
    last_sync: Option<String>,
    #[serde(default)]
    last_status: Option<String>,
    #[serde(default)]
    created: String,
}

fn enabled_default() -> bool {
    true
}

impl StoredAccount {
    fn from_wire(a: &CardDavAccount, password_ref: Option<PasswordRef>) -> Self {
        Self {
            id: a.id.clone(),
            label: a.label.clone(),
            provider: a.provider.clone(),
            server_url: a.server_url.clone(),
            username: a.username.clone(),
            password_ref,
            password: None,
            addressbook_url: a.addressbook_url.clone(),
            enabled: a.enabled,
            last_sync: a.last_sync.clone(),
            last_status: a.last_status.clone(),
            created: a.created.clone(),
        }
    }

    /// The wire shape. The password field is always blank: the account
    /// never carries its credential, on the wire or in memory.
    fn to_wire(&self) -> CardDavAccount {
        CardDavAccount {
            id: self.id.clone(),
            label: self.label.clone(),
            provider: self.provider.clone(),
            server_url: self.server_url.clone(),
            username: self.username.clone(),
            password: String::new(),
            addressbook_url: self.addressbook_url.clone(),
            enabled: self.enabled,
            last_sync: self.last_sync.clone(),
            last_status: self.last_status.clone(),
            created: self.created.clone(),
        }
    }

    fn to_markdown(&self) -> Result<String, ContactsError> {
        let yaml = serde_yaml::to_string(self).map_err(|e| ContactsError::Backend {
            message: e.to_string(),
        })?;
        Ok(format!("---\n{yaml}---\n"))
    }
}

impl VaultContacts {
    /// Open a directory rooted at `vault_root`, keeping passwords in the
    /// platform's default store ([`Preference::from_env`]) with no
    /// envelope key, and trusting the public CA roots plus
    /// `TASK_CARDDAV_EXTRA_CA`. A server passes its auth secret through
    /// [`VaultContacts::with_secrets`] instead, so the envelope is
    /// there when the keyring is not.
    pub fn new(vault_root: impl Into<PathBuf>) -> Result<Self, VaultContactsError> {
        Self::with_secrets(
            vault_root,
            Arc::new(NativeCredentials::new(Preference::from_env(), None)),
            Transport::from_env(),
        )
    }

    /// Open with an explicit credential store and transport. Runs the
    /// plaintext migration (see the module docs) before returning.
    pub fn with_secrets(
        vault_root: impl Into<PathBuf>,
        credentials: Arc<dyn CredentialStore>,
        transport: Transport,
    ) -> Result<Self, VaultContactsError> {
        let root = vault_root.into();
        if !root.is_dir() {
            return Err(VaultContactsError::BadRoot(root.display().to_string()));
        }
        let key_prefix = format!(
            "carddav:{:016x}",
            fnv1a(
                root.canonicalize()
                    .unwrap_or_else(|_| root.clone())
                    .to_string_lossy()
                    .as_bytes()
            )
        );
        let this = Self {
            root,
            write_lock: Arc::new(Mutex::new(())),
            events: architect::PubSub::sliding(256),
            credentials,
            transport,
            key_prefix,
        };
        if let Err(e) = this.secure_accounts() {
            // Never fatal to boot: contacts still serve, and the next
            // open tries again. Logged loudly because a failure here can
            // mean plaintext is still on disk.
            tracing::error!(%e, root = %this.root.display(), "contacts: securing sync accounts failed");
        }
        Ok(this)
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    fn credential_key(&self, account_id: &str) -> String {
        format!("{}:{}", self.key_prefix, sanitize(account_id))
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

    // t[impl vault.private.node-only]
    /// Make the account directory node-private: create it and drop the
    /// marker in **before** any account file lands, so there is no
    /// moment when a scan could version an account.
    fn private_accounts_dir(&self) -> Result<PathBuf, ContactsError> {
        let dir = self.root.join(ACCOUNTS_DIR);
        std::fs::create_dir_all(&dir).map_err(io)?;
        let marker = dir.join(vault_live::private::PRIVATE_MARKER);
        if !marker.is_file() {
            std::fs::write(
                &marker,
                "CardDAV sync accounts: node-private. Never versioned, replicated or served.\n",
            )
            .map_err(io)?;
        }
        Ok(dir)
    }

    /// Write one account, atomically, into the private directory.
    fn write_account(&self, stored: &StoredAccount) -> Result<(), ContactsError> {
        let body = stored.to_markdown()?;
        let _guard = self.write_lock.lock().map_err(|_| ContactsError::Backend {
            message: "vault contacts lock poisoned".into(),
        })?;
        let dir = self.private_accounts_dir()?;
        let path = dir.join(format!("{}.md", sanitize(&stored.id)));
        let tmp = dir.join(format!(".{}.md.tmp", sanitize(&stored.id)));
        std::fs::write(&tmp, body).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)?;
        Ok(())
    }

    // t[impl contacts.secret.out-of-vault]
    /// The migration: mark the directory private, and move every
    /// plaintext password found in it into the credential store.
    fn secure_accounts(&self) -> Result<(), ContactsError> {
        let dir = self.root.join(ACCOUNTS_DIR);
        if !dir.is_dir() {
            return Ok(());
        }
        {
            let _guard = self.write_lock.lock().map_err(|_| ContactsError::Backend {
                message: "vault contacts lock poisoned".into(),
            })?;
            self.private_accounts_dir()?;
        }
        let mut moved = 0usize;
        let mut dropped = 0usize;
        for (path, mut stored) in self.read_accounts()? {
            let Some(plaintext) = stored.password.take() else {
                continue;
            };
            if !plaintext.is_empty() {
                match self
                    .credentials
                    .store(&self.credential_key(&stored.id), &plaintext)
                {
                    Ok(reference) => {
                        stored.password_ref = Some(reference);
                        moved += 1;
                    }
                    Err(e) => {
                        tracing::warn!(?path, %e, "contacts: no store took a legacy CardDAV password; removing it anyway");
                        stored.password_ref = None;
                        stored.last_status = Some(REENTER_STATUS.into());
                        dropped += 1;
                    }
                }
            }
            self.write_account(&stored)?;
        }
        if moved + dropped > 0 {
            tracing::info!(
                moved,
                dropped,
                "contacts: plaintext CardDAV passwords moved out of the vault"
            );
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

    /// Every account file, parsed, with its path.
    fn read_accounts(&self) -> Result<Vec<(PathBuf, StoredAccount)>, ContactsError> {
        let dir = self.root.join(ACCOUNTS_DIR);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let path = entry.path();
            let is_md = path.extension().and_then(|s| s.to_str()) == Some("md");
            let hidden = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            if !path.is_file() || !is_md || hidden {
                continue;
            }
            match std::fs::read_to_string(&path).ok().and_then(|raw| {
                frontmatter_split(&raw)
                    .and_then(|(fm, _)| serde_yaml::from_str::<StoredAccount>(fm).ok())
            }) {
                Some(acct) => out.push((path, acct)),
                None => tracing::warn!(?path, "contacts: skip unreadable sync account"),
            }
        }
        Ok(out)
    }

    fn load_account(&self, id: &str) -> Result<Option<StoredAccount>, ContactsError> {
        Ok(self
            .read_accounts()?
            .into_iter()
            .map(|(_, a)| a)
            .find(|a| a.id == id))
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
        self.write_file(
            &format!("{CONTACTS_DIR}/{}.md", sanitize(&contact.id)),
            &body,
        )?;
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
        let mut out: Vec<CardDavAccount> = self
            .read_accounts()?
            .iter()
            .map(|(_, a)| a.to_wire())
            .collect();
        out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
        Ok(out)
    }

    // t[impl contacts.secret.out-of-vault]
    // t[impl contacts.transport.https-only]
    fn upsert_account(&self, account: &CardDavAccount) -> Result<(), ContactsError> {
        if account.id.trim().is_empty() {
            return Err(ContactsError::Invalid {
                field: "account.id".into(),
                reason: "id must be non-empty".into(),
            });
        }
        // Refused at save: an http origin, a URL with credentials in
        // it, an addressbook on another server.
        crate::carddav::validate_account(account)?;

        let existing = self.load_account(&account.id)?;
        let previous = existing.as_ref().and_then(|e| e.password_ref.clone());
        // A blank password on an existing account keeps the stored
        // reference (the UI never reads the secret back, so it
        // round-trips blank). A new one goes to the credential store —
        // never to this file.
        let password_ref = if account.password.is_empty() {
            previous
        } else {
            let fresh = self
                .credentials
                .store(&self.credential_key(&account.id), &account.password)?;
            if let Some(old) = previous.filter(|old| *old != fresh) {
                self.credentials.forget(&old);
            }
            Some(fresh)
        };
        let mut stored = StoredAccount::from_wire(account, password_ref);
        // A re-entered password clears the migration's note.
        if !account.password.is_empty() && stored.last_status.as_deref() == Some(REENTER_STATUS) {
            stored.last_status = None;
        }
        self.write_account(&stored)
    }

    fn delete_account(&self, id: &str) -> Result<(), ContactsError> {
        if let Some(reference) = self.load_account(id)?.and_then(|a| a.password_ref) {
            self.credentials.forget(&reference);
        }
        self.delete_file(&format!("{ACCOUNTS_DIR}/{}.md", sanitize(id)))
    }

    fn sync_account(&self, id: String) -> Result<SyncReport, ContactsError> {
        let stored = self
            .load_account(&id)?
            .ok_or_else(|| ContactsError::NotFound { id: id.clone() })?;
        let account = stored.to_wire();
        // Refused at sync too: a file edited by hand to an http origin
        // gets the same answer the save would have given.
        crate::carddav::validate_account(&account).map_err(|e| ContactsError::Sync {
            message: e.to_string(),
        })?;
        let reference = stored.password_ref.ok_or_else(|| ContactsError::Sync {
            message: format!(
                "no password is stored for '{}'; enter it to sync",
                account.label
            ),
        })?;
        let password = self.credentials.resolve(&reference)?;

        // One-way pull: fetch + map the server's vCards, then reconcile
        // against local contacts by vCard UID. `manual` contacts are
        // never touched; a matching synced contact is updated in place.
        let pulled = crate::carddav::import(&account, password.as_str(), &self.transport)?;
        drop(password);
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

/// 64-bit FNV-1a — a stable, dependency-free fingerprint of the vault
/// path for credential keys. Not a security boundary.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Vault-relative forward-slash path.
fn relativize(root: &Path, abs: &Path) -> String {
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
    use crate::credentials::TestSecrets;
    use contacts_proto::CardDavProvider;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, VaultContacts) {
        let tmp = TempDir::new().expect("tempdir");
        let contacts = open(tmp.path());
        (tmp, contacts)
    }

    fn open(root: &Path) -> VaultContacts {
        VaultContacts::with_secrets(root, TestSecrets::shared(), Transport::default())
            .expect("new contacts")
    }

    fn contact(id: &str, name: &str) -> Contact {
        Contact::create(id, name, "2026-07-17T09:00:00Z")
    }

    fn icloud(id: &str, password: &str) -> CardDavAccount {
        let mut acct = CardDavAccount::create(
            id,
            "iCloud",
            CardDavProvider::ICLOUD,
            "2026-07-17T09:00:00Z",
        );
        acct.username = "me@icloud.com".into();
        acct.password = password.into();
        acct
    }

    /// Every byte under the vault, as text — what a peer copying the
    /// whole directory would be holding.
    fn whole_vault(root: &Path) -> String {
        let mut out = String::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push_str(&String::from_utf8_lossy(&std::fs::read(&p).unwrap()));
                }
            }
        }
        out
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

    // t[verify contacts.secret.out-of-vault]
    #[test]
    fn the_password_never_touches_the_vault() {
        let (tmp, c) = fixture();
        c.upsert_account(&icloud("a1", "app-secret-a1")).unwrap();

        // Listed accounts are blank…
        let listed = c.list_accounts().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].password, "");
        assert_eq!(listed[0].username, "me@icloud.com");
        // …the file holds a reference…
        let stored = c.load_account("a1").unwrap().unwrap();
        assert!(matches!(
            stored.password_ref,
            Some(PasswordRef::Test { .. })
        ));
        // …which still resolves to the secret…
        assert_eq!(
            TestSecrets
                .resolve(stored.password_ref.as_ref().unwrap())
                .unwrap()
                .as_str(),
            "app-secret-a1"
        );
        // …and no byte anywhere in the vault is the password.
        assert!(!whole_vault(tmp.path()).contains("app-secret-a1"));
        // The directory is node-private.
        assert!(vault_live::private::is_private_dir(
            &tmp.path().join(ACCOUNTS_DIR)
        ));
        // Accounts never leak into the contact directory.
        assert!(c.list_contacts().unwrap().is_empty());
    }

    #[test]
    fn upsert_account_blank_password_preserves_the_reference() {
        let (_tmp, c) = fixture();
        c.upsert_account(&icloud("a2", "app-secret-a2")).unwrap();
        let before = c.load_account("a2").unwrap().unwrap().password_ref;
        // Re-upsert with a blank password (the redacted round-trip).
        let mut edited = icloud("a2", "");
        edited.label = "iCloud (personal)".into();
        c.upsert_account(&edited).unwrap();
        let stored = c.load_account("a2").unwrap().unwrap();
        assert_eq!(stored.password_ref, before);
        assert_eq!(stored.label, "iCloud (personal)");
    }

    // t[verify contacts.transport.https-only]
    #[test]
    fn an_http_origin_is_refused_at_save() {
        let (tmp, c) = fixture();
        let mut acct = icloud("a3", "pw-a3");
        acct.provider = CardDavProvider::NEXTCLOUD.into();
        acct.server_url = "http://cloud.example.com".into();
        let err = c.upsert_account(&acct).unwrap_err();
        assert!(matches!(err, ContactsError::Invalid { .. }), "{err:?}");
        assert!(c.list_accounts().unwrap().is_empty());
        assert!(!whole_vault(tmp.path()).contains("pw-a3"));
    }

    // t[verify contacts.secret.out-of-vault]
    #[test]
    fn opening_a_vault_strips_legacy_plaintext() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(ACCOUNTS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        // The shape the old backend wrote.
        std::fs::write(
            dir.join("legacy.md"),
            "---\nid: legacy\nlabel: Old\nprovider: nextcloud\nserver_url: https://cloud.example.com\n\
             username: me\npassword: legacy-plaintext-pw\naddressbook_url: null\nenabled: true\n\
             last_sync: null\nlast_status: null\ncreated: 2026-01-01T00:00:00Z\n---\n",
        )
        .unwrap();

        let c = open(tmp.path());
        assert!(!whole_vault(tmp.path()).contains("legacy-plaintext-pw"));
        let stored = c.load_account("legacy").unwrap().unwrap();
        let reference = stored.password_ref.expect("moved, not lost");
        assert_eq!(
            TestSecrets.resolve(&reference).unwrap().as_str(),
            "legacy-plaintext-pw"
        );
        assert!(vault_live::private::is_private_dir(&dir));
    }

    #[test]
    fn with_no_store_the_plaintext_is_dropped_and_flagged() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(ACCOUNTS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("legacy.md"),
            "---\nid: legacy\nlabel: Old\nprovider: nextcloud\nserver_url: https://cloud.example.com\n\
             username: me\npassword: nowhere-to-go-pw\nenabled: true\ncreated: x\n---\n",
        )
        .unwrap();
        let c = VaultContacts::with_secrets(
            tmp.path(),
            Arc::new(NativeCredentials::new(Preference::Envelope, None)),
            Transport::default(),
        )
        .unwrap();
        assert!(!whole_vault(tmp.path()).contains("nowhere-to-go-pw"));
        let listed = c.list_accounts().unwrap();
        assert_eq!(listed[0].last_status.as_deref(), Some(REENTER_STATUS));
        let err = c.sync_account("legacy".into()).unwrap_err();
        assert!(matches!(err, ContactsError::Sync { .. }), "{err:?}");
    }

    #[test]
    fn delete_account_forgets_its_secret() {
        let (_tmp, c) = fixture();
        c.upsert_account(&icloud("a4", "pw-a4")).unwrap();
        let reference = c.load_account("a4").unwrap().unwrap().password_ref.unwrap();
        c.delete_account("a4").unwrap();
        assert!(TestSecrets.resolve(&reference).is_err());
        assert!(c.list_accounts().unwrap().is_empty());
    }
}
