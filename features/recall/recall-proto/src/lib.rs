// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the **recall** feature — a general,
//! project-filterable spaced-repetition learning deck.
//!
//! A [`RecallCard`] is one learning card on an FSRS scheduler. Cards
//! carry a `project` (the deck / filter), a `card_type`, and a
//! front/back prompt; the FSRS memory state (`stability`, `difficulty`,
//! …) rides in `sr-*` frontmatter so the whole card round-trips as a
//! plain markdown file (`Records/recall/<id>.md`) — the same
//! vault-as-source-of-truth pattern the inbox uses.
//!
//! Bible study is the seed use case (verse↔reference, first-letter,
//! cloze), but nothing here is Bible-specific: `project` and
//! `card_type` are free-form strings, so any subject is just another
//! deck.
//!
//! The sibling `recall` crate owns the parse / write side and the
//! disk-backed backend; this proto is the wasm-clean wire surface the
//! UI binds to directly.

pub mod error;
pub mod recall_card;
pub mod service;

pub use error::RecallError;
pub use recall_card::{CardType, RecallCard};
pub use service::Recall;
pub use service::recall::RecallEvent;

// architect-emitted vox bits: the async client / dispatcher /
// descriptor for the one capability.
#[cfg(feature = "vox")]
pub use service::recall::{
    RecallClient, RecallRpc, RecallRpcDispatcher, Service as RecallService, layer as recall_layer,
    serve as recall_serve,
};

// `#[subscribe] fn events` stream sibling — live deck changes.
// Mount `recall_stream_layer(backend)` next to the base service;
// subscribers drive a `RecallStreamClient`.
#[cfg(feature = "vox")]
pub use service::recall::{
    RecallStream, RecallStreamClient, RecallStreamSource,
    recall_stream_service_descriptor as recall_stream_descriptor,
    stream_layer as recall_stream_layer, stream_serve as recall_stream_serve,
};
