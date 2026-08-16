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
//! | [`adopt`] | `files.adopt.*` — structure first, addresses later |
//! | [`cadence`] | `files.version.cadence` — quiescence, save points, snapshots |
//! | [`catalogue`] | `files.catalogue.*` — entries, staleness, cursors |
//! | [`facet`] | `files.facet.*` — tool layouts, project overrides, unmapped |
//! | [`hydration`] | `files.sync.selective`, `files.device.control` |
//! | [`ignore`] | `files.ignore.*` — the two layers |
//! | [`labels`] | `files.version.labels` — read, never parsed |
//!
//! [`facet`] classifies and [`hydration`] decides: together they are what
//! selective sync was blocked on, since nothing previously defined what a
//! facet *was*.
//!
//! Still to move out of `backend.rs`, in rough dependency order: root
//! identity and the marker file, namespace resolution over the org tree,
//! version chains and divergence, and the cadence engine.

pub mod adopt;
pub mod cadence;
pub mod catalogue;
pub mod facet;
pub mod hydration;
pub mod ignore;
pub mod labels;

pub use adopt::Adoption;
pub use cadence::{CadenceConfig, CadenceEngine, Clock, SystemClock, TestClock};
pub use catalogue::{Catalogue, Change};
pub use facet::{Binding, Capability, Facet, FacetMap, Source};
pub use hydration::{Decision, Reason, Subscription};
pub use ignore::{IgnoreSet, Layer};
pub use labels::Label;
