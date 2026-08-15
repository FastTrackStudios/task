// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the **contacts** feature — a vault-backed people
//! directory that unifies who you work with and who you bill.
//!
//! A [`Contact`] is one person (or organization) modeled on vCard: a
//! display name, N emails/phones, a postal address, org/title, groups,
//! and notes. Multi-valued fields ride as newline-joined `String`s with
//! `*_list()` accessors so the [`Contact`] entity stays all-scalar (the
//! same shape `finance_proto::Party.address` uses) — clean to derive,
//! clean to store.
//!
//! Source of truth is a markdown file in the vault
//! (`Records/contacts/<id>.md`): the structured fields ride in
//! frontmatter, the body holds free-form notes. The sibling `contacts`
//! crate owns the parse/write side, the disk-backed backend, and the
//! (one-way, first) **CardDAV import** from Nextcloud / iCloud.
//!
//! Contacts are the backbone the rest of the app leans on:
//! `linked_user_id` ties a contact to an org member (→ `/members`), and
//! `linked_party_id` ties it to a finance `Party` (→ `/invoices`), so
//! the same person renders identically wherever they appear.

pub mod account;
pub mod contact;
pub mod error;
pub mod service;

pub use account::{CardDavAccount, CardDavProvider, SyncReport};
pub use contact::{Contact, ContactSource};
pub use error::ContactsError;
pub use service::Contacts;
pub use service::contacts::ContactsEvent;

// architect-emitted vox bits: the async client / dispatcher /
// descriptor for the one capability.
#[cfg(feature = "vox")]
pub use service::contacts::{
    ContactsClient, ContactsRpc, ContactsRpcDispatcher, Service as ContactsService,
    layer as contacts_layer, serve as contacts_serve,
};

// `#[subscribe] fn events` stream sibling — live directory changes.
// Mount `contacts_stream_layer(backend)` next to the base service;
// subscribers drive a `ContactsStreamClient`.
#[cfg(feature = "vox")]
pub use service::contacts::{
    ContactsStream, ContactsStreamClient, ContactsStreamSource,
    contacts_stream_service_descriptor as contacts_stream_descriptor,
    stream_layer as contacts_stream_layer, stream_serve as contacts_stream_serve,
};
