//! Canonical paths inside a wiki.
//!
//! Backends use these to locate state files; agents use them
//! over RPC to know which files they're reading or writing.
//! Everything is relative to the wiki root path passed into
//! the backend (`<data_root>/orgs/<slug>/wiki/Knowledge/` on
//! the server) — federation peers see the same shape.
//!
//! ## On-disk layout
//!
//! Mirrors `nashsu/llm_wiki`'s project shape — the wiki root
//! IS the entire LLM-Wiki project root, not a subfolder of
//! it. That makes a Task wiki directory portable to
//! `llm_wiki` (and vice versa) with minimal restructuring.
//!
//! ```text
//! <wiki_root>/                  ← typically `<org>/wiki/Knowledge/`
//! ├── schema.md         ← contract / conventions (project root)
//! ├── purpose.md        ← goals / scope / key questions
//! ├── index.md          ← content catalog (LLM-maintained)
//! ├── log.md            ← append-only operation timeline
//! ├── overview.md       ← global synthesis (optional)
//! ├── raw/             ← immutable input layer
//! │   └── sources/      ← imported raw documents
//! ├── media/            ← extracted images, attachments
//! ├── _state/           ← opaque agent state (JSON)
//! │   ├── ingest_queue.json
//! │   ├── review.json
//! │   ├── lint_findings.json
//! │   ├── research_plans.json
//! │   ├── peers.json
//! │   └── snapshot.json (sha256 of every tracked file)
//! └── <Type>/<Slug>.md  ← actual pages (Concepts/, Entities/, ...)
//! ```
//!
//! Wiki pages themselves live as plain markdown anywhere
//! under the wiki root (typically `<Type>/<Slug>.md`, e.g.
//! `Concepts/Spaced repetition.md`). The `type:` frontmatter
//! field is what the graph + lint use, not the folder.

/// Empty — the wiki backend is now rooted directly at the
/// path it was opened with (typically
/// `<org>/wiki/Knowledge/`). Kept as a constant for code that
/// joins paths, so `root.join(WIKI_ROOT)` round-trips to
/// `root` itself.
pub const WIKI_ROOT: &str = "";

/// Schema doc filename (relative to `Wiki/`).
pub const SCHEMA_MD: &str = "schema.md";

/// Purpose doc filename (relative to `Wiki/`).
pub const PURPOSE_MD: &str = "purpose.md";

/// Content catalog filename (relative to `Wiki/`).
pub const INDEX_MD: &str = "index.md";

/// Append-only operation log filename (relative to `Wiki/`).
pub const LOG_MD: &str = "log.md";

/// Global synthesis page (optional; relative to `Wiki/`).
pub const OVERVIEW_MD: &str = "overview.md";

/// Sub-folder under `Wiki/` for the immutable raw layer.
/// Matches `llm_wiki`'s `raw/` directory. Anything nested under
/// here is read-only from the wiki's perspective.
pub const RAW_DIR: &str = "raw";

/// Sub-folder for imported raw source documents, nested under
/// [`RAW_DIR`]. Full path: `Wiki/raw/sources/`.
pub const SOURCES_DIR: &str = "raw/sources";

/// Sub-folder for extracted images + attachments.
pub const MEDIA_DIR: &str = "media";

/// Sub-folder for opaque agent state. JSON-encoded.
pub const STATE_DIR: &str = "_state";

/// Ingest queue state file (relative to `Wiki/_state/`).
pub const INGEST_QUEUE_JSON: &str = "ingest_queue.json";

/// Review queue state file (relative to `Wiki/_state/`).
pub const REVIEW_JSON: &str = "review.json";

/// Open lint findings state file (relative to `Wiki/_state/`).
pub const LINT_FINDINGS_JSON: &str = "lint_findings.json";

/// Research plans state file (relative to `Wiki/_state/`).
pub const RESEARCH_PLANS_JSON: &str = "research_plans.json";

/// Federation peer registry (relative to `Wiki/_state/`).
pub const PEERS_JSON: &str = "peers.json";

/// File-content snapshot (sha256-keyed) for change detection
/// (relative to `Wiki/_state/`).
pub const SNAPSHOT_JSON: &str = "snapshot.json";

/// `_state/wiki.json` — what the wiki declares about itself:
/// visibility, Editors, proposer gate, repository source
/// ([`crate::config::WikiConfig`]).
pub const WIKI_JSON: &str = "wiki.json";

/// `_state/edits/` — one JSON file per Edit Request, named by its id.
/// The tracker row holds status; this holds the change
/// (`wiki.edit.tracked`).
pub const EDITS_DIR: &str = "edits";
