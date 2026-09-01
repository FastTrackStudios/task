// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! `wiki-proto` — wire contract for the `wiki/` feature.
//!
//! Task's port of Karpathy's LLM-Wiki pattern (see
//! `nashsu/llm_wiki`). The wiki is **not** a separate database —
//! it's a convention layered over the existing file-backed
//! [`vault`](../../../vault/vault) at `<vault>/Wiki/`. Every
//! piece of state lives as either markdown (curatable, agent +
//! human shared surface) or JSON (opaque agent state under
//! `Wiki/_state/`).
//!
//! ## Layers
//!
//! - **schema** — [`schema::SchemaDoc`] + [`schema::PurposeDoc`]
//!   read from `Wiki/schema.md` and `Wiki/purpose.md`. The
//!   contract between the curator (human) and the maintainer
//!   (LLM agent). Each vault has its own.
//! - **index + log** — [`log::WikiIndex`] +
//!   [`log::LogEntry`]: `Wiki/index.md` (content catalog) and
//!   `Wiki/log.md` (append-only timeline). Both are markdown so
//!   they're human-readable and round-trip through Obsidian.
//! - **graph** — [`graph::WikiGraph`] +
//!   [`graph::RelevanceScore`] + [`graph::Cluster`]: the
//!   4-signal relevance model (direct links, source overlap,
//!   Adamic-Adar, type affinity) and Louvain community
//!   detection.
//! - **ingest** — [`ingest::IngestTask`] +
//!   [`ingest::AnalysisDraft`] + [`ingest::PageDraft`]:
//!   scaffolding for the two-step chain-of-thought pipeline.
//!   The LLM glue itself is **not** in this crate; the trait
//!   methods accept the LLM's output as data, so any agent
//!   (claude CLI, Anthropic API, Hermes) can drive the flow via
//!   `task-cli`.
//! - **lint** — [`lint::LintFinding`] +
//!   [`lint::LintScope`]: orphan / contradiction / stale /
//!   missing-cross-ref / data-gap detection.
//! - **review** — [`review::ReviewItem`] +
//!   [`review::ReviewAction`]: async queue where the LLM flags
//!   items for human judgment.
//! - **research** — [`research::ResearchPlan`] +
//!   [`research::RawSource`]: Deep-Research flow. The LLM
//!   proposes search queries, executes them externally, then
//!   submits results back as raw sources to be ingested.
//! - **federation** — [`federation::PeerWiki`] +
//!   [`federation::PeerPullResult`]: multi-wiki replication.
//!   Wikis cite each other across vault boundaries.
//! - **events** — [`event::WikiEvent`]: live notifications
//!   for subscribers (page added, finding raised, ingest
//!   progress).
//!
//! ## The trait
//!
//! [`service::WikiService`] is decorated with
//! `#[architect::rpc]`. Implementations live in sibling crates
//! (`wiki-live` is the canonical file-backed impl, future);
//! callers reach the same surface in-process or remotely via
//! the auto-emitted [`WikiServiceClient`] over vox.
//!
//! Mount the server-side backend with [`serve`] (vox feature),
//! or compose through [`Service`] into an `architect::Services`
//! bundle.

pub mod config;
pub mod error;
pub mod event;
pub mod federation;
pub mod graph;
pub mod health;
pub mod ingest;
pub mod lint;
pub mod log;
pub mod multimodal;
pub mod pages;
pub mod paths;
pub mod raw;
pub mod reference;
pub mod research;
pub mod resolve;
pub mod review;
pub mod schema;
pub mod search;
pub mod service;
pub mod subscription;

pub use config::{NewWiki, ProposerGate, RepoSource, Visibility, WikiConfig};
pub use error::WikiError;
pub use event::{WikiChange, WikiEvent};
pub use reference::Reference;
pub use resolve::{Resolved, resolve};
pub use subscription::{SourceKind, Subscriber, Subscription, Unresolved};

// Per-capability trait re-exports. There is **no**
// umbrella `WikiService` — callers express what they need
// via bounds:
//
// ```ignore
// fn run_ingest<W: Schema + Catalog + Ingest + RawLayer>(wiki: &W, /* ... */) { /* ... */ }
// ```
//
// Each trait is `#[architect::rpc]`-decorated and emits
// its own client + dispatcher + descriptor under the
// `vox` feature.
pub use service::edits::{EditRequest, EditStatus, Editors, NewEditRequest, PageChange, PageDiff};
pub use service::registry::{WikiDescription, WikiSummary};
pub use service::subscriptions::{HeldSubscription, RefreshReport};
pub use service::{
    Catalog, Edits, Events, Federation, Graph, Ingest, Lint, Multimodal, Pages, RawLayer, Registry,
    Research, Review, Schema, Search, Subscriptions, Watcher,
};
