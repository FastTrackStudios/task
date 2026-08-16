//! The Files domain: the rules, with no RPC anywhere near them.
//!
//! `FilesBackend` grew to 4,860 lines — half its crate in one file —
//! because there was nowhere else for the logic to live. Most of it is
//! domain: how a directory maps to a facet, what an ignore layer covers,
//! what a restored version resolves against. None of that should know
//! that a request arrived over vox.
//!
//! So this crate holds it, and can be tested without standing up a
//! server. It depends on [`files_store`] for the engines and on
//! `files-proto` for shared vocabulary — ids, paths, errors — and on
//! nothing else. No architect, no vox, no org.
//!
//! ## Layout, mirroring `features/files/spec/files.md`
//!
//! | Module | Spec |
//! |---|---|
//! | [`facet`] | `files.facet.*` — tool layouts, project overrides, unmapped |
//! | [`ignore`] | `files.ignore.*` — the two layers |
//! | [`labels`] | `files.version.labels` — read, never parsed |
//!
//! Still to move out of `backend.rs`, in rough dependency order: root
//! identity and the adoption state machine (`files.adopt.*`), catalogue
//! entries and staleness (`files.catalogue.*`), namespace resolution,
//! version chains and divergence, hydration policy, and the cadence
//! engine.

pub mod facet;
pub mod ignore;
pub mod labels;

pub use facet::{Binding, Capability, Facet, FacetMap, Source};
pub use ignore::{IgnoreSet, Layer};
pub use labels::Label;
