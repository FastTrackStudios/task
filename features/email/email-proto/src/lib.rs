//! `email-proto` — wire contract for the email-sync feature.
//!
//! - [`Account`] / [`AccountId`] — per-mailbox identity
//! - [`Folder`] / [`FolderRole`] — mailbox listing entries
//! - [`Envelope`] / [`Message`] / [`Draft`] / [`Addr`] —
//!   payload shapes
//! - [`Flag`] / [`FlagDelta`] / [`SeqRange`] — operation args
//! - [`EmailEvent`] — live change events for subscribers
//! - [`EmailSyncError`] — trait-boundary error type
//! - [`EmailSync`] — the service trait, decorated with
//!   `#[architect::rpc]`
//!
//! The architect macro derives the async vox face from the
//! sync `EmailSync` trait: backends impl `EmailSync` directly,
//! in-process callers use it as a plain sync API, and remote
//! callers reach the same surface via the auto-emitted
//! [`EmailSyncClient`] over vox.
//!
//! Mount the server-side backend with [`serve`], or compose
//! through [`Service`] into an `architect::Services` bundle.
//! Mirrors the shape of `vault-proto`.

mod account;
mod derivation;
mod draft;
mod envelope;
mod error;
mod event;
mod flag;
mod folder;
mod link;
mod message;
mod outbox;
mod product;
mod range;
mod service;

pub use account::{Account, AccountId};
pub use derivation::{DERIVATION_VERSION, Derivation, DerivationKind, TAG_TAXONOMY};
pub use draft::{Attachment, AttachmentMeta, Draft};
pub use envelope::{Addr, Envelope};
pub use error::EmailSyncError;
pub use event::{EmailChange, EmailEvent};
pub use flag::{Flag, FlagDelta};
pub use folder::{Folder, FolderRole};
pub use message::{Message, MessageId, ThreadId};
pub use outbox::{OutboxEntry, OutboxStatus};
pub use product::{EmailProduct, EmailProductRpc};

pub use range::SeqRange;
pub use service::{EmailSync, EmailSyncRpc};

// architect-emitted vox bits from the auto-generated mirror
// trait. Re-exported with shorter aliases (`Dispatcher`,
// `descriptor`) so consumer mounting code reads
// `email_proto::descriptor()` and `email_proto::serve(state)`
// rather than juggling the underscored mirror names directly.
#[cfg(feature = "vox")]
pub use service::{
    EmailSyncClient, EmailSyncRpcDispatcher as Dispatcher, Service,
    email_sync_rpc_service_descriptor as descriptor, layer, serve,
};

// `#[subscribe] fn changes` stream sibling — live mailbox changes.
// Mount `email_proto::stream_layer(backend)` next to
// `email_proto::serve(backend)`; clients subscribe through
// `EmailSyncStreamClient::changes(tx)` (see `architect::use_stream`).
#[cfg(feature = "vox")]
pub use service::{
    EmailSyncStreamClient, EmailSyncStreamSource, stream_layer, stream_serve,
    email_sync_stream_service_descriptor as stream_descriptor,
};

// `EmailProduct` vox bits — the outbox / product surface. Mounted
// next to the `EmailSync` service under the same "email" plugin;
// its events ride the `EmailSync` changes stream.
#[cfg(feature = "vox")]
pub use product::{
    EmailProductClient, EmailProductRpcDispatcher as ProductDispatcher,
    email_product_rpc_service_descriptor as product_descriptor, layer as product_layer,
    serve as product_serve,
};

// Same aliasing for the link service: `email_proto::links_descriptor()`
// / `links_serve(backend)` at the mount site.
pub use link::{
    EmailLinks, EmailLinksClient, LinkTarget, MessageLink,
    email_links_rpc_service_descriptor as links_descriptor, layer as links_layer,
    serve as links_serve,
};
