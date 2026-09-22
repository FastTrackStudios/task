//! Names [`crate::backend`] and [`crate::scan`] agree on for a File
//! Root's own internals — never surfaced by
//! `TreeService::browse` (root browsing), but visible
//! through `RootsService::browse_area` ("Drive"
//! browsing shows the raw tree, internals included — that's the
//! distinction the glossary draws between the two).
//!
//! The constants themselves live in `files-proto` (issue #262): a
//! Storage agent hosting a root's live tree has to create the same
//! `STORE_DIR` this crate hides, so the names belong to the shared wire
//! crate rather than to one backend. `GIT_DIR` moved there with them
//! when software roots landed (issue #273) — a software root placed on a
//! Storage Location is the same agreement.

pub use files_proto::consts::{GIT_DIR, MARKER_FILE, STORE_DIR};
