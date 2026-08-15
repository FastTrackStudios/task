//! The contact entity — a vCard-mapped person or organization.
//!
//! Source of truth is a markdown file in the vault
//! (`Records/contacts/<id>.md`): the structured fields ride in
//! frontmatter, the body holds free-form notes. Multi-valued vCard
//! fields (EMAIL, TEL, CATEGORIES) are kept as **newline-joined
//! `String`s** with `*_list()` accessors, so the entity stays
//! all-scalar and derives cleanly — the same trick
//! `finance_proto::Party.address` uses for a postal block. Dates are
//! ISO strings so the proto stays serializer-agnostic and
//! `Facet`-encodable.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// Where a contact came from — its provenance and, for synced
/// contacts, which sync owns it. Free-form on the wire (`source` is a
/// `String`) so a new provider needs no proto change.
pub struct ContactSource;

impl ContactSource {
    /// Authored by hand in the app. Never overwritten by a sync.
    pub const MANUAL: &'static str = "manual";
    /// Imported from a Nextcloud (or generic CardDAV) addressbook.
    pub const NEXTCLOUD: &'static str = "nextcloud";
    /// Imported from an iCloud addressbook.
    pub const ICLOUD: &'static str = "icloud";
    /// Imported from some other CardDAV server.
    pub const CARDDAV: &'static str = "carddav";
}

/// One person or organization in the directory.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[architect(table_name = "contacts", repo)]
pub struct Contact {
    /// Stable id (uuid string). PK — the vault file is
    /// `Records/contacts/<id>.md`.
    #[architect(primary_key, auto_increment = false)]
    pub id: String,
    /// The vCard `UID` this contact syncs against, if any. The sync
    /// identity: a pull matches server cards to local contacts by this,
    /// so re-importing updates in place instead of duplicating. `None`
    /// for hand-authored contacts.
    #[architect(filterable)]
    pub uid: Option<String>,

    /// Display name (vCard `FN`) — always present; the one field the
    /// UI can count on.
    #[architect(fulltext)]
    pub full_name: String,
    /// Given / first name (vCard `N`), if split out.
    pub given_name: Option<String>,
    /// Family / last name (vCard `N`), if split out.
    pub family_name: Option<String>,
    /// Organization / company (vCard `ORG`).
    #[architect(filterable, fulltext)]
    pub organization: Option<String>,
    /// Job title / role (vCard `TITLE`).
    pub title: Option<String>,

    /// Email addresses, newline-joined (vCard `EMAIL`). Primary =
    /// first. Use [`Contact::email_list`] / [`Contact::primary_email`].
    #[architect(fulltext)]
    pub emails: String,
    /// Phone numbers, newline-joined (vCard `TEL`). Primary = first.
    pub phones: String,
    /// Postal address, one line per component (vCard `ADR`). Same
    /// multi-line-`String` shape as `finance_proto::Party.address`.
    pub address: Option<String>,
    /// Birthday, ISO `YYYY-MM-DD` (vCard `BDAY`).
    pub birthday: Option<String>,
    /// URL of an avatar / photo (vCard `PHOTO` when it's a URI).
    pub photo_url: Option<String>,
    /// Groups / categories, newline-joined (vCard `CATEGORIES`). The
    /// project-like filter chips group by these. Use
    /// [`Contact::group_list`].
    #[architect(filterable)]
    pub groups: String,
    /// Free-form notes (vCard `NOTE`); the markdown body mirrors this.
    #[architect(fulltext)]
    pub notes: Option<String>,

    // ── Sync + linkage ──────────────────────────────────────────────
    /// Provenance — one of [`ContactSource`]'s constants. `manual`
    /// contacts are never touched by a pull.
    #[architect(filterable)]
    pub source: String,
    /// The sync account label this contact belongs to (e.g. an
    /// addressbook name), for contacts pulled from CardDAV.
    #[architect(filterable)]
    pub account: Option<String>,
    /// The server ETag last seen for this contact — lets an import
    /// skip unchanged cards. `None` until first synced.
    pub etag: Option<String>,
    /// Linked `finance_proto::Party.id` — the billing identity. Set
    /// when this contact is billed on an invoice.
    #[architect(filterable)]
    pub linked_party_id: Option<String>,
    /// Linked org-member `user_id` — set when this contact IS a
    /// teammate with a login. Ties `/contacts` to `/members`.
    #[architect(filterable)]
    pub linked_user_id: Option<String>,

    /// Hidden from the directory + pickers without deleting the file.
    #[architect(filterable)]
    pub archived: bool,
    /// RFC-3339 creation timestamp.
    #[architect(filterable, sortable)]
    pub created: String,
    /// RFC-3339 last-modified timestamp, if ever edited.
    #[architect(sortable)]
    pub updated: Option<String>,
}

impl Contact {
    /// A freshly authored contact: manual source, not archived. `id` /
    /// `created` are minted by the caller so the proto stays
    /// clock-agnostic.
    #[must_use]
    pub fn create(
        id: impl Into<String>,
        full_name: impl Into<String>,
        created: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            uid: None,
            full_name: full_name.into(),
            given_name: None,
            family_name: None,
            organization: None,
            title: None,
            emails: String::new(),
            phones: String::new(),
            address: None,
            birthday: None,
            photo_url: None,
            groups: String::new(),
            notes: None,
            source: ContactSource::MANUAL.to_string(),
            account: None,
            etag: None,
            linked_party_id: None,
            linked_user_id: None,
            archived: false,
            created: created.into(),
            updated: None,
        }
    }

    /// Split a newline-joined multi-value field into its non-empty,
    /// trimmed parts.
    fn split_lines(s: &str) -> Vec<&str> {
        s.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
    }

    /// The contact's email addresses, primary first.
    #[must_use]
    pub fn email_list(&self) -> Vec<&str> {
        Self::split_lines(&self.emails)
    }

    /// The primary (first) email, if any.
    #[must_use]
    pub fn primary_email(&self) -> Option<&str> {
        self.email_list().into_iter().next()
    }

    /// The contact's phone numbers, primary first.
    #[must_use]
    pub fn phone_list(&self) -> Vec<&str> {
        Self::split_lines(&self.phones)
    }

    /// The primary (first) phone, if any.
    #[must_use]
    pub fn primary_phone(&self) -> Option<&str> {
        self.phone_list().into_iter().next()
    }

    /// The groups / categories this contact belongs to.
    #[must_use]
    pub fn group_list(&self) -> Vec<&str> {
        Self::split_lines(&self.groups)
    }

    /// Whether this contact came from a CardDAV sync (vs hand-authored)
    /// — synced contacts are refreshed by a pull, manual ones aren't.
    #[must_use]
    pub fn is_synced(&self) -> bool {
        self.source != ContactSource::MANUAL
    }

    /// Up-to-two-letter initials for a deterministic avatar. Prefers
    /// given+family, falls back to the first two words of `full_name`,
    /// then the org, then `?`.
    #[must_use]
    pub fn initials(&self) -> String {
        let first_char = |s: &str| s.chars().find(|c| c.is_alphanumeric());
        if let (Some(g), Some(f)) = (
            self.given_name.as_deref().and_then(first_char),
            self.family_name.as_deref().and_then(first_char),
        ) {
            return format!("{g}{f}").to_uppercase();
        }
        let source = if self.full_name.trim().is_empty() {
            self.organization.as_deref().unwrap_or("?")
        } else {
            self.full_name.as_str()
        };
        let letters: String = source
            .split_whitespace()
            .filter_map(first_char)
            .take(2)
            .collect();
        if letters.is_empty() {
            "?".to_string()
        } else {
            letters.to_uppercase()
        }
    }
}

/// Client-side optimistic store identity (`architect::Store`): keyed
/// by the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for Contact {
    type Key = String;
    fn key(&self) -> String {
        self.id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact() -> Contact {
        let mut c = Contact::create("id-1", "Ada Lovelace", "2026-07-17T09:00:00Z");
        c.given_name = Some("Ada".into());
        c.family_name = Some("Lovelace".into());
        c.emails = "ada@example.com\nada.l@work.com".into();
        c.phones = "+1 555 0100".into();
        c.groups = "Engineering\nFriends".into();
        c
    }

    #[test]
    fn multi_value_accessors() {
        let c = contact();
        assert_eq!(c.email_list(), vec!["ada@example.com", "ada.l@work.com"]);
        assert_eq!(c.primary_email(), Some("ada@example.com"));
        assert_eq!(c.phone_list(), vec!["+1 555 0100"]);
        assert_eq!(c.group_list(), vec!["Engineering", "Friends"]);
    }

    #[test]
    fn initials_prefer_split_name() {
        assert_eq!(contact().initials(), "AL");
    }

    #[test]
    fn initials_fall_back_to_full_name() {
        let c = Contact::create("x", "Grace Hopper", "2026-07-17T09:00:00Z");
        assert_eq!(c.initials(), "GH");
    }

    #[test]
    fn initials_fall_back_to_org() {
        let mut c = Contact::create("x", "", "2026-07-17T09:00:00Z");
        c.organization = Some("Acme Corp".into());
        assert_eq!(c.initials(), "AC");
    }

    #[test]
    fn manual_is_not_synced() {
        assert!(!contact().is_synced());
        let mut c = contact();
        c.source = ContactSource::ICLOUD.to_string();
        assert!(c.is_synced());
    }
}
