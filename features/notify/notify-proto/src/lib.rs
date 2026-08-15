//! Wire contract for the notifications feature.
//!
//! A [`Notification`] is one "something finished / something needs
//! you" record an org's server-side notifier materialized from the
//! event hubs (task completed, agent turn finished, booking landed,
//! forge activity — see `apps/task/server/src/notifier.rs` for the
//! rule catalog). Rows are queue-ish state persisted per org in
//! `notify.sqlite`; the sibling `notify` crate owns the SeaORM store,
//! exactly like `timer` sits on top of `timer-proto`.
//!
//! Click-through: every row carries a [`NotifySource`] whose `href` is
//! an app route (`/tasks`, `/vault?path=…`, `/agents?session=…`) the
//! UI navigates to when the row is clicked.

pub mod error;
pub mod model;
pub mod service;

pub use error::NotifyError;
pub use model::{Notification, NotifyKind, NotifyListFilter, NotifySource};
pub use service::{Notify, NotifyEvent};

// architect-emitted vox bits: the async client / dispatcher /
// descriptor for the one capability. Mount sites stitch the
// descriptor + `serve` into the org router.
#[cfg(feature = "vox")]
pub use service::{
    NotifyClient, NotifyRpcDispatcher, Service as NotifyService, layer as notify_layer,
    notify_rpc_service_descriptor, serve as notify_serve,
};

// `#[subscribe] fn events` stream sibling — live notification changes.
// Mount `notify_stream_layer(backend)` next to the base service;
// subscribers drive a `NotifyStreamClient`.
#[cfg(feature = "vox")]
pub use service::{
    NotifyStream, NotifyStreamClient, NotifyStreamSource,
    notify_stream_service_descriptor as notify_stream_descriptor,
    stream_layer as notify_stream_layer, stream_serve as notify_stream_serve,
};
