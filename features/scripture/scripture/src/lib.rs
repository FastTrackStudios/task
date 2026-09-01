//! `scripture` — read-only Bible spine: USFM ingest + in-memory store.
//!
//! This native crate sits on top of the wasm-clean `scripture_proto`
//! addressing layer (the same split as `locations` over
//! `locations-proto`). It turns USFM source into [`Verse`]s keyed by
//! [`scripture_proto::VerseId`] and serves them from a [`Bible`].
//!
//! Surface:
//! - [`parse_book`] — USFM source → `Vec<Verse>` of clean verse text.
//! - [`install_usfm_dir`] — normalize a source USFM dir into a clean
//!   translation folder in the resource library.
//! - [`Bible`] — an in-memory, ordered store loaded from a translation
//!   folder ([`Bible::load_dir`]); `get` / iterate verses and chapters.
//!
//! The corpus lives in the resource library on disk (e.g.
//! `<org>/resources/bible/WEB/`), not in the repo. The text is never
//! mutated by a user action; later layers (notes, wiki, cross-refs) link
//! in by [`scripture_proto::VerseId`].

#![cfg(not(target_arch = "wasm32"))]

pub mod api;
pub mod backlinks;
pub mod bible;
pub mod compare;
pub mod crossref;
pub mod entities;
pub mod install;
pub mod lexicon;
pub mod morphgnt;
pub mod original;
pub mod oshb;
pub mod pull;
pub mod refs;
pub mod sources;
pub mod stepbible;
pub mod store;
pub mod topics;
pub mod usfm;
pub mod versification;

pub use api::{ApiTranslation, Provider};
pub use backlinks::{RangeBacklink, scan_vault};
pub use bible::{Bible, LoadError};
pub use compare::{CompareSpec, extract_compare_specs};
pub use crossref::{CrossRefEntry, CrossRefs};
pub use entities::{Entity, from_bible_data};
pub use install::{InstallError, install_usfm_dir};
pub use lexicon::{Lexicon, js_to_json, normalize_strongs};
pub use original::{OrigMeta, OrigText, OrigVerse, OrigWord};
pub use pull::{PullError, Pulled, pull, pull_from_archive};
pub use refs::{VerseRefHit, extract_verse_refs};
pub use scripture_proto::{
    Availability, Book, ChapterView, ComparisonRow, ComparisonView, InterlinearWord, LexiconEntry,
    Occurrence, OrigEditionInfo, RefError, ScriptureError, ScriptureRef, ScriptureService,
    TopicTag, Translation, TranslationInfo, VerseBacklink, VerseBacklinks, VerseId, VerseLine,
    VerseRange, WeightedRef, WordStudyReport, WordToken,
};
pub use sources::{Source, SourceError, installable, source_for};
pub use store::Store;
pub use topics::{TopicVerse, Topics};
pub use usfm::{UsfmError, Verse, Word, parse_book};
pub use versification::{Versification, scheme_for_language};

// architect-emitted vox bits, re-exported for the server mount.
#[cfg(feature = "vox")]
pub use scripture_proto::{scripture_service_descriptor, serve_scripture_service};
