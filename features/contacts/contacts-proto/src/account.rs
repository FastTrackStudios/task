//! CardDAV sync accounts + the sync report.
//!
//! A [`CardDavAccount`] is one addressbook the app pulls contacts from
//! — Nextcloud, iCloud, or any generic CardDAV server. It carries the
//! connection details (URL + username + app-specific password) and the
//! last-sync bookkeeping. The password is a **secret**: it lives only
//! server-side, and `list_accounts` blanks it before it goes on the
//! wire, so the UI can show/edit an account without ever reading the
//! credential back.
//!
//! The actual pull (principal discovery → addressbook-query → vCard
//! parse → [`crate::Contact`]) lives in the native `contacts` crate;
//! this proto only fixes the wire shapes.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// The known CardDAV providers. Free-form on the wire (`provider` is a
/// `String`) so a new one needs no proto change; these drive the
/// discovery quirks (iCloud needs principal discovery from a fixed
/// well-known root; Nextcloud exposes a predictable path).
pub struct CardDavProvider;

impl CardDavProvider {
    /// Nextcloud (or any generic CardDAV server) — `server_url` is the
    /// base URL; discovery walks `/.well-known/carddav`.
    pub const NEXTCLOUD: &'static str = "nextcloud";
    /// Apple iCloud — fixed root `https://contacts.icloud.com`, needs
    /// principal discovery + an app-specific password.
    pub const ICLOUD: &'static str = "icloud";
    /// Any other CardDAV server, treated generically.
    pub const GENERIC: &'static str = "carddav";
}

/// One CardDAV addressbook the app imports contacts from.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[architect(table_name = "contact_sync_accounts", repo)]
pub struct CardDavAccount {
    /// Stable id (uuid string). PK.
    #[architect(primary_key, auto_increment = false)]
    pub id: String,
    /// User-facing name for this account ("Personal iCloud").
    pub label: String,
    /// One of [`CardDavProvider`]'s constants.
    #[architect(filterable)]
    pub provider: String,
    /// Base / discovery URL. For iCloud this is left to the provider
    /// default; for Nextcloud it's the server root.
    pub server_url: String,
    /// Login user (often an email).
    pub username: String,
    /// App-specific password. **Secret** — blanked by `list_accounts`;
    /// only ever set, never read back over the wire.
    pub password: String,
    /// The resolved addressbook collection URL, filled in by discovery
    /// on the first successful sync.
    pub addressbook_url: Option<String>,
    /// Contacts pulled from this account are stamped with this label in
    /// [`crate::Contact::account`], so a re-sync can scope its updates.
    #[architect(filterable)]
    pub enabled: bool,
    /// RFC-3339 timestamp of the last successful sync.
    pub last_sync: Option<String>,
    /// Human-readable outcome of the last sync attempt (`ok` or an
    /// error message).
    pub last_status: Option<String>,
    /// RFC-3339 creation timestamp.
    pub created: String,
}

impl CardDavAccount {
    /// A new, enabled account. `id` / `created` minted by the caller.
    #[must_use]
    pub fn create(
        id: impl Into<String>,
        label: impl Into<String>,
        provider: impl Into<String>,
        created: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            provider: provider.into(),
            server_url: String::new(),
            username: String::new(),
            password: String::new(),
            addressbook_url: None,
            enabled: true,
            last_sync: None,
            last_status: None,
            created: created.into(),
        }
    }

    /// A copy with the secret password blanked — what goes on the wire
    /// out of `list_accounts`.
    #[must_use]
    pub fn redacted(&self) -> Self {
        Self {
            password: String::new(),
            ..self.clone()
        }
    }
}

/// Client-side optimistic store identity: keyed by the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for CardDavAccount {
    type Key = String;
    fn key(&self) -> String {
        self.id.clone()
    }
}

/// What a single [`crate::Contacts::sync_account`] pull did.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
pub struct SyncReport {
    /// The account that was synced.
    pub account_id: String,
    /// New contacts created.
    pub added: i64,
    /// Existing contacts updated in place (matched by vCard `UID`).
    pub updated: i64,
    /// Unchanged contacts skipped (ETag match).
    pub skipped: i64,
    /// Contacts on the server that failed to parse.
    pub failed: i64,
    /// Human-readable summary.
    pub message: String,
}

impl SyncReport {
    /// An empty report for `account_id` — the baseline a sync folds
    /// counts into.
    #[must_use]
    pub fn empty(account_id: impl Into<String>) -> Self {
        Self {
            account_id: account_id.into(),
            added: 0,
            updated: 0,
            skipped: 0,
            failed: 0,
            message: String::new(),
        }
    }
}
