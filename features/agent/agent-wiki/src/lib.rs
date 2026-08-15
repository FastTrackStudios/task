//! `agent-wiki` — wires `agent-proto` agent loops to
//! `wiki-proto` operations.
//!
//! The crate is a *binding library*, not a backend. Backends
//! (Hermes, Codex monitor, claude CLI bridge) implement
//! `agent_proto::Agents` in their own way; this crate
//! supplies the **prompts** they send and the **parsers**
//! that turn LLM output into [`wiki_proto`] calls.
//!
//! Ported from `nashsu/llm_wiki`:
//!
//! - [`prompts`] — system + user prompt templates for the
//!   wiki workflows. Modeled on `llm_wiki`'s
//!   `src/lib/ingest.ts`, `deep-research.ts`, `lint.ts`,
//!   `dedup.ts`, `optimize-research-topic.ts`,
//!   `vision-caption.ts`, `sweep-reviews.ts`.
//! - [`parsers`] — `FILE`-block, `REVIEW`-block, `LINT`-block,
//!   and JSON parsers. Each takes the raw LLM response and
//!   returns typed values ready to feed back through
//!   `WikiService`.
//! - [`bridge`] — orchestration helpers: "run one ingest
//!   loop", "run one lint pass". They sequence `Agents`
//!   dispatches and `WikiService` writes.
//! - [`error`] — `AgentWikiError`.
//!
//! ## Why a separate crate
//!
//! `wiki-proto` knows nothing about LLMs. `agent-proto`
//! knows nothing about wikis. Bindings live here because
//! they're the *application* of the two — flexible enough
//! that we can swap the prompts or parsers without churning
//! either trait.

pub mod bridge;
pub mod error;
pub mod parsers;
pub mod prompts;

pub use error::AgentWikiError;
