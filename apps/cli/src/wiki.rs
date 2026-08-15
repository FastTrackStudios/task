//! `task wiki …` — the `Wiki/` LLM ingest + archive pipeline.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;
use crate::shared::confirm;

/// Wiki id the flat (vault-path) wiki commands address when they
/// route over vox — the one-wiki-per-org convention the server
/// mounts (`WikiBackend::single("default", …)`).
const ORG_WIKI_ID: &str = "default";

/// How a flat wiki command reaches its data.
///
/// The flat commands (`task wiki graph|gaps|search|clusters|
/// health|import|rescan|findings`) predate the vox surface and
/// address a wiki by filesystem path (`--vault`). Unification
/// rule:
///
/// - `--vault` (or its `examples/vault` default) names an
///   EXISTING local directory → FS-native behaviour, unchanged
///   from the pre-vox code path. An explicit on-disk path is
///   authoritative; this is the documented local escape hatch
///   (dev vaults under `examples/`, offline inspection of a
///   copied tree).
/// - the path does NOT exist (previously a hard `canonicalize`
///   error) → route to the active org's wiki over vox — remote
///   server or embedded in-process backend alike — so the same
///   command works wherever the session points, and plugin
///   gating + permissions apply because the data comes through
///   the org router.
enum VaultRoute {
    /// Canonicalized local vault root.
    Local(std::path::PathBuf),
    /// Per-org vox URL (`…/org/<slug>/vox`).
    Vox(String),
}

fn route_vault(vault: &std::path::Path) -> eyre::Result<VaultRoute> {
    if let Ok(p) = vault.canonicalize() {
        return Ok(VaultRoute::Local(p));
    }
    let slug = resolve_active_org(None)?;
    Ok(VaultRoute::Vox(resolve_org_vox_url(None, &slug)))
}

// A clap command enum: constructed once per invocation, so the
// inter-variant size gap is irrelevant, and boxing a variant's
// args fights the `Subcommand` derive / flattening.
// ── Wiki RPC handlers ────────────────────────────────────────────────

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub(crate) enum WikiCmd {
    /// Build the 4-signal wiki graph and dump it as JSON or
    /// a terse text summary. No LLM — pure walk + compute.
    Graph {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        /// Filter by substring (matches title or path).
        #[arg(long, default_value = "")]
        query: String,
        /// Filter by `type:` frontmatter (`concept`,
        /// `entity`, ...).
        #[arg(long, default_value = "")]
        node_type: String,
        /// Cap on node count. `0` = no cap.
        #[arg(long, default_value_t = 0)]
        limit: u32,
        /// Emit JSON instead of the text summary.
        #[arg(long)]
        json: bool,
    },
    /// Build a **token-budgeted context subgraph** for an
    /// LLM prompt. Returns relevance-ranked page summaries +
    /// outbound connections as markdown — paste the output
    /// straight into a system / user message. The agent
    /// gets a structural view of the wiki instead of having
    /// to grep raw files.
    ///
    /// Inspired by graphify's "query the graph instead of
    /// grepping" pitch, but for the entire wiki (concepts +
    /// entities + sources, not just code).
    Context {
        /// Free-text query. Empty = top-centrality view.
        query: String,
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        /// `concept` / `entity` / `source` filter on
        /// `type:` frontmatter.
        #[arg(long, default_value = "")]
        node_type: String,
        /// Soft token cap. `chars/4` heuristic. Default
        /// 8000 (~32k chars, comfortably fits in any
        /// modern context window).
        #[arg(long, default_value_t = 8000)]
        budget_tokens: usize,
        /// Hard node ceiling. `0` = unlimited.
        #[arg(long, default_value_t = 0)]
        max_nodes: usize,
        /// Chars of body kept per page summary. Default
        /// 600 (~150 tokens).
        #[arg(long, default_value_t = 600)]
        summary_chars: usize,
        /// Also merge prose notes from this tree (typically
        /// the org vault) into the graph as `note:/<path>`
        /// nodes — notes and wiki pages cross-link through
        /// the usual title/stem wikilink resolution.
        #[arg(long)]
        notes: Option<std::path::PathBuf>,
        /// Overlay a typed-link store (`links.jsonl`) as
        /// direct-link edges — picks up user-asserted
        /// note↔note links that aren't wikilinks in any
        /// body. Pair with `--notes` (the endpoints must be
        /// nodes to land).
        #[arg(long)]
        links: Option<std::path::PathBuf>,
    },
    /// Tree-sitter-extracted **code-symbol graph** for a
    /// project root. Walks `.rs` files (TS/JS/Python come in
    /// follow-up PRs), emits functions / structs / traits /
    /// impls as nodes, plus `Imports` / `Calls` /
    /// `Implements` / `Defines` edges with `extracted` /
    /// `inferred` confidence labels (matching graphify's
    /// schema).
    ///
    /// This is the structural complement to the markdown
    /// wiki — agents can ask "what calls `foo`?" and get a
    /// real graph, not a grep.
    Code {
        /// Project root. Default: current directory.
        #[arg(short, long, default_value = ".")]
        root: std::path::PathBuf,
        /// Emit JSON (the full graph) instead of the text
        /// summary.
        #[arg(long)]
        json: bool,
        /// Render the full `GRAPH_REPORT.md` shape (god
        /// nodes, fan-out hubs, kind histogram). Mutually
        /// exclusive with `--json`.
        #[arg(long, conflicts_with = "json")]
        report: bool,
        /// Top-N node summary in the text + report views.
        #[arg(long, default_value_t = 25)]
        top: usize,
    },
    /// Surface knowledge gaps — orphan pages (degree ≤ 1)
    /// and missing-page wikilinks. No LLM.
    Gaps {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Louvain communities — partition the wiki graph and
    /// print each cluster with its cohesion score.
    Clusters {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
    },
    /// Token search over `Wiki/`. TF-IDF over page bodies;
    /// `--hybrid` opts into vector retrieval where the
    /// `vector` feature has been built in (else
    /// downgrades to token).
    Search {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        /// Query string.
        query: String,
        /// Filter by `type:` frontmatter.
        #[arg(long, default_value = "")]
        node_type: String,
        /// Result cap. `0` = unbounded.
        #[arg(long, default_value_t = 10)]
        top_k: u32,
        /// Include full page content in the response
        /// (skip for normal listings — the snippet usually
        /// suffices).
        #[arg(long)]
        include_content: bool,
        /// Use hybrid (token + vector) mode. Requires the
        /// `vector` feature build; otherwise downgrades
        /// transparently.
        #[arg(long)]
        hybrid: bool,
    },
    /// Watch `Wiki/raw/sources/` for FS events; on each
    /// debounced burst, rescan + (optionally) enqueue.
    /// Runs until interrupted.
    WatchSources {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        #[arg(long, default_value_t = 2)]
        debounce_secs: u64,
        /// Don't auto-enqueue diffs — just print them.
        #[arg(long)]
        dry_run: bool,
    },
    /// One-shot wiki health snapshot — queue depth, open
    /// findings, source count, last ingest/rescan.
    Health {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
    },
    /// Recursively import a directory of files into
    /// `Wiki/raw/sources/`. Doesn't enqueue ingest tasks —
    /// follow with `task wiki rescan` to do that.
    Import {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        /// Directory to walk.
        #[arg(short, long)]
        dir: std::path::PathBuf,
        /// Flatten (drop subdirectory structure).
        #[arg(long)]
        flatten: bool,
        /// Extensions to include (comma-separated, no dot).
        /// Default: `md,txt,pdf`.
        #[arg(long, default_value = "md,txt,pdf")]
        ext: String,
    },
    /// Walk `Wiki/raw/sources/`, diff against the
    /// `snapshot.json`, and report new/modified/deleted
    /// files. Doesn't enqueue (yet) — pass `--enqueue` to
    /// also push diffs into the ingest queue.
    Rescan {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        #[arg(long)]
        enqueue: bool,
    },
    /// Run one semantic lint pass via the LLM. Persists
    /// new findings under `Wiki/_state/lint_findings.json`.
    Lint {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        #[arg(short, long)]
        model: Option<String>,
        #[arg(long, default_value_t = 180)]
        timeout_secs: u64,
        #[arg(long, default_value = "English")]
        language: String,
    },
    /// List open lint findings.
    Findings {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
    },
    /// Layered-access wikilink lint. Scans
    /// `<org_root>/wiki/Knowledge/` + `wiki/LLM/` for
    /// wikilinks that escape their tier (Knowledge linking
    /// out, LLM linking into vault). Pure walk, no LLM.
    /// Exit code is non-zero when violations are present
    /// so the command works in pre-commit / CI.
    LintTiers {
        /// `<data_root>/orgs/<slug>/`. Defaults to the
        /// active session's org root.
        #[arg(long)]
        org_root: Option<std::path::PathBuf>,
    },
    /// Detect duplicate pages via the LLM. Prints groups;
    /// pass `--merge <slug-csv>` to merge one (writes via
    /// `record_pages`).
    Dedup {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        #[arg(short, long)]
        model: Option<String>,
        #[arg(long, default_value_t = 180)]
        timeout_secs: u64,
    },
    /// Propose a research plan for a knowledge gap (output
    /// of `task wiki gaps`).
    Research {
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        /// Gap kind (`Orphan`, `MissingPage`, `SparseCluster`, `Bridge`).
        #[arg(long, default_value = "MissingPage")]
        gap_kind: String,
        /// Short gap title.
        #[arg(long)]
        gap_title: String,
        /// Gap description.
        #[arg(long, default_value = "")]
        gap_description: String,
        #[arg(short, long)]
        model: Option<String>,
        #[arg(long, default_value_t = 120)]
        timeout_secs: u64,
        #[arg(long, default_value = "English")]
        language: String,
    },
    /// Ingest one source file into `<vault>/Wiki/` via the
    /// two-step `CoT` pipeline (analyze → generate). Drops
    /// the source under `Wiki/raw/sources/`, runs the
    /// agent, parses FILE/REVIEW blocks, writes pages,
    /// updates `index.md` + `log.md`.
    ///
    /// Example:
    ///   task wiki ingest \
    ///     -v examples/vault \
    ///     -s examples/vault/Wiki/raw/sources/karpathy-llm-wiki.md \
    ///     -m gpt-5.4-mini
    Ingest {
        /// Vault root.
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        /// Path to the source file to ingest. Bytes get
        /// copied into `Wiki/raw/sources/<filename>`.
        #[arg(short, long)]
        source: std::path::PathBuf,
        /// Override the filename used under `raw/sources/`.
        /// Default: source's basename.
        #[arg(long)]
        filename: Option<String>,
        /// MIME type. Default `text/markdown`.
        #[arg(long, default_value = "text/markdown")]
        mime: String,
        /// Human title for the log entry.
        #[arg(long)]
        title: Option<String>,
        /// Model id.
        #[arg(short, long)]
        model: Option<String>,
        /// Output language. Default `English`.
        #[arg(long, default_value = "English")]
        language: String,
        /// Per-turn timeout (seconds). Default 300.
        #[arg(long, default_value_t = 300)]
        timeout_secs: u64,
    },
    /// Archive a URL (or local file) into `Wiki/raw/sources/`
    /// with provenance frontmatter, then enqueue an ingest
    /// task. The front door of the wiki archive feature:
    /// routes by content type — articles → readability
    /// extraction, Google Docs → markdown export, YouTube /
    /// video → yt-dlp transcript with `^t<sec>` block
    /// anchors. Canonical-URL dedup: re-archiving the same
    /// resource (even via a differently-tracked link) is a
    /// no-op unless `--force`.
    Archive(WikiArchiveArgs),
    /// Rewrite a thin wiki page into a proper reference
    /// article. Reads the existing page + its `sources:`,
    /// prompts the LLM to expand with code examples + sharper
    /// structure, overwrites the page in place.
    Deepen {
        /// Vault root (the `Wiki/Knowledge/` parent).
        #[arg(short, long, default_value = "examples/vault")]
        vault: std::path::PathBuf,
        /// Page path relative to the wiki root, e.g.
        /// `wiki/concepts/borrowing-over-cloning.md`.
        #[arg(short, long)]
        page: String,
        #[arg(short, long)]
        model: Option<String>,
        #[arg(long, default_value_t = 600)]
        timeout_secs: u64,
        #[arg(long, default_value = "English")]
        language: String,
    },
    /// `Wiki/schema.md` + `Wiki/purpose.md` operations.
    /// Talks to the server over vox (per-org); these are
    /// the canonical authoring + bootstrap entry points.
    #[command(subcommand)]
    Schema(WikiSchemaCmd),
    /// `Wiki/index.md` catalog operations.
    #[command(subcommand)]
    Catalog(WikiCatalogCmd),
    /// `Wiki/raw/sources/` — listing + reading + deleting
    /// raw sources via the server.
    #[command(subcommand)]
    Raw(WikiRawCmd),
    /// LLM ingest queue — list, retry, cancel pending
    /// ingestion tasks. The actual `enqueue` happens via
    /// `wiki import` + `wiki rescan` (existing FS verbs).
    #[command(subcommand)]
    IngestQueue(WikiIngestCmd),
    /// Lint findings (RPC) — list + resolve via the server.
    /// The lint *runner* (LLM pass) is `wiki lint`. This
    /// sub-tree marks existing findings resolved /
    /// dismissed / deferred.
    #[command(subcommand)]
    LintFindings(WikiFindingsCmd),
    /// Review queue — LLM-proposed page changes await curator
    /// approval here. `list` shows pending items; `apply`
    /// accepts the proposal (rewrite-page / append-note / etc).
    #[command(subcommand)]
    Review(WikiReviewCmd),
    /// Research plans — list + status. The proposer is the
    /// existing flat `wiki research` (LLM call). This sub-tree
    /// manages the plans the server tracks.
    #[command(subcommand)]
    ResearchPlans(WikiResearchCmd),
    /// Filesystem watcher — re-ingest on external edits.
    #[command(subcommand)]
    Watch(WikiWatchCmd),
}

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub(crate) struct WikiArchiveArgs {
    /// Bulk importers (`task wiki archive import <kind>`).
    #[command(subcommand)]
    cmd: Option<WikiArchiveSub>,
    /// URL (http/https) or local file path to archive.
    #[arg(required = true)]
    target: Option<String>,
    /// Override the recorded title (default: extracted from
    /// the content).
    #[arg(long)]
    title: Option<String>,
    /// Re-archive even when the canonical URL already exists
    /// under `raw/sources/`.
    #[arg(long)]
    force: bool,
    /// Import + record only — don't enqueue an ingest task.
    #[arg(long)]
    no_enqueue: bool,
    /// yt-dlp binary used for video routes. Also settable
    /// via `TASK_YTDLP`.
    #[arg(long, env = "TASK_YTDLP", default_value = "yt-dlp")]
    yt_dlp: String,
    /// pdftotext binary (poppler) — the PDF-extraction
    /// fallback when pdfium isn't available. The pdfium path
    /// itself loads `libpdfium` from the `TASK_PDFIUM`
    /// directory or the system library path at runtime.
    #[arg(long, env = "TASK_PDFTOTEXT", default_value = "pdftotext")]
    pdftotext: String,
    /// Podcast episode picker for show-level URLs: a title
    /// (or substring) matched against the feed. Episode
    /// links (`?i=` on Apple) resolve without this; without
    /// either, the latest episode is archived.
    #[arg(long)]
    episode: Option<String>,
    /// Podcast transcript strategy: `auto` (feed transcript
    /// tag, then local whisper when compiled in), `tag`
    /// (feed tag only), `groq` (Groq API backfill,
    /// needs GROQ_API_KEY, ~$0.04/audio-hour), `whisper`
    /// (local model — requires a `--features whisper` build),
    /// `none` (metadata + show notes only).
    #[arg(long, default_value = "auto")]
    transcribe: String,
    /// Whisper model: a name (`small` dev default;
    /// `large-v3-turbo` for production quality) cached under
    /// ~/.cache/task/whisper/ and downloaded on first use, or
    /// a path to a ggml .bin file.
    #[arg(long, env = "TASK_WHISPER_MODEL", default_value = "small")]
    whisper_model: String,
    /// ffmpeg binary for enclosure → PCM decode (whisper
    /// path only).
    #[arg(long, env = "TASK_FFMPEG", default_value = "ffmpeg")]
    ffmpeg: String,
    #[arg(long, default_value = "default")]
    wiki_id: String,
    #[arg(long)]
    org: Option<String>,
    #[arg(long)]
    server: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
pub(crate) enum WikiArchiveSub {
    /// Bookmark-service importers — batch front-ends to the
    /// archive router. Canonical-URL dedup makes re-runs (and
    /// cross-service overlap) idempotent.
    #[command(subcommand)]
    Import(WikiArchiveImportCmd),
    /// Extractor health: which archive routes currently work,
    /// which are broken, and the last error seen — from the
    /// per-org ledger every archive attempt records into.
    /// The phase-3 social routes are accept-fragility by
    /// design; this surface is how their breakage stays
    /// honest instead of silent.
    Health {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Re-attempt every unarchived stub (`unarchived-*.md`
    /// under raw/sources/ — written when an accept-fragility
    /// route was blocked). Cron-friendly: throttled per
    /// route, always exits 0, prints a summary. A success
    /// imports the real source and deletes the stub.
    Retry {
        /// Max stubs to attempt this run.
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        /// Import retried sources but don't enqueue ingest.
        #[arg(long)]
        no_enqueue: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum WikiArchiveImportCmd {
    /// Readwise classic highlights (v2 /export/ API; Token
    /// auth, 20 req/min — pages are throttled automatically).
    Readwise {
        /// Readwise access token (readwise.io/access_token).
        #[arg(long, env = "READWISE_TOKEN")]
        token: String,
        /// Incremental cursor: only books with highlights
        /// updated after this RFC3339 instant. The previous
        /// run prints the value to pass here.
        #[arg(long)]
        updated_after: Option<String>,
        #[command(flatten)]
        common: WikiArchiveImportCommon,
    },
    /// Readwise Reader documents (v3 /list/ API with
    /// withHtmlContent=true — full stored article HTML).
    Reader {
        #[arg(long, env = "READWISE_TOKEN")]
        token: String,
        #[arg(long)]
        updated_after: Option<String>,
        #[command(flatten)]
        common: WikiArchiveImportCommon,
    },
    /// Karakeep (self-hosted) via its REST API with
    /// includeContent=true. The JSON export is lossy (drops
    /// crawled htmlContent) — the API is the real source.
    Karakeep {
        /// Instance base URL, e.g. <https://keep.example.com>
        #[arg(long, env = "KARAKEEP_ENDPOINT")]
        endpoint: String,
        /// API key (ak2_… — Settings → API Keys).
        #[arg(long, env = "KARAKEEP_TOKEN")]
        token: String,
        #[command(flatten)]
        common: WikiArchiveImportCommon,
    },
    /// Pocket export zip (the service is dead — exports
    /// only). Reads part_*.csv saves + annotations/*.json
    /// highlights.
    Pocket {
        /// Path to the Pocket export zip.
        zip: std::path::PathBuf,
        #[command(flatten)]
        common: WikiArchiveImportCommon,
    },
    /// Netscape bookmarks HTML (what every browser exports).
    Bookmarks {
        /// Path to bookmarks.html.
        html: std::path::PathBuf,
        #[command(flatten)]
        common: WikiArchiveImportCommon,
    },
}

#[derive(clap::Args, Clone)]
pub(crate) struct WikiArchiveImportCommon {
    /// Max items to archive this run (after dedup).
    #[arg(long)]
    limit: Option<usize>,
    /// Parse + report only; nothing is written to the wiki.
    #[arg(long)]
    dry_run: bool,
    /// Import sources but don't enqueue ingest tasks (use
    /// `task wiki raw rescan` later to enqueue in bulk).
    #[arg(long)]
    no_enqueue: bool,
    #[arg(long, default_value = "default")]
    wiki_id: String,
    #[arg(long)]
    org: Option<String>,
    #[arg(long)]
    server: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum WikiReviewCmd {
    /// List every open review item.
    List {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Accept the LLM's proposed action on a review item.
    ///
    /// Actions:
    /// - `rewrite-page <path> <body|->`  — replace a page's body
    /// - `append-note <path> <body|->`   — append to the page
    /// - `research <query>`              — convert to a ResearchPlan
    ///
    /// Body args read stdin when given as `-`.
    Apply {
        item_id: String,
        /// `rewrite-page` / `append-note` / `research`.
        action: String,
        /// First positional arg: page path (for rewrite/append)
        /// or query text (for research).
        arg: String,
        /// Second positional: markdown body for rewrite /
        /// append (`-` = stdin). Unused for `research`.
        #[arg(default_value = "")]
        body: String,
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum WikiResearchCmd {
    /// List every research plan and its status.
    List {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Set the status of a research plan.
    /// `proposed|running|awaiting|integrated|cancelled`.
    SetStatus {
        plan_id: String,
        status: String,
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum WikiWatchCmd {
    /// Enable filesystem watch on `Wiki/raw/sources/` so
    /// dropping a file there auto-enqueues an ingest.
    On {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    Off {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    Status {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum WikiSchemaCmd {
    /// Print `Wiki/schema.md`.
    Show {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print `Wiki/purpose.md`.
    Purpose {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Replace `Wiki/schema.md`. Read body from `<path>` or
    /// `-` for stdin.
    WriteSchema {
        path: String,
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Replace `Wiki/purpose.md`.
    WritePurpose {
        path: String,
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Initialize `Wiki/` if missing. Idempotent.
    Bootstrap {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Server-side health snapshot (orphan count, lint
    /// queue depth, last ingest mtime, …).
    Health {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum WikiCatalogCmd {
    /// Dump the catalog (`Wiki/index.md` parsed).
    Show {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Force-rebuild the catalog by re-scanning the vault.
    Rebuild {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum WikiRawCmd {
    /// List every raw source the wiki carries.
    List {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print the raw source bytes to stdout.
    Read {
        path: String,
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Delete a raw source. Returns the review items the
    /// server enqueued for any pages that depended on it.
    Delete {
        path: String,
        #[arg(long, short = 'y')]
        yes: bool,
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Rescan `Wiki/raw/sources/`; enqueues fresh ingest
    /// tasks for any new files since last scan.
    Rescan {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum WikiIngestCmd {
    List {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Retry a previously-failed ingest task.
    Retry {
        task_id: String,
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Cancel a pending or running ingest task.
    Cancel {
        task_id: String,
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum WikiFindingsCmd {
    /// All open lint findings.
    List {
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Mark a finding resolved (or `dismiss` / `defer`).
    Resolve {
        finding_id: String,
        /// `resolved` / `dismissed` / `deferred`.
        action: String,
        #[arg(long, default_value = "default")]
        wiki_id: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

pub(crate) async fn run_wiki(cmd: WikiCmd) -> eyre::Result<()> {
    use agent_codex::CodexBackend;
    use agent_wiki::bridge::{IngestRequest, run_ingest};
    use std::time::Duration;
    use wiki_live::WikiLive;

    match cmd {
        WikiCmd::Graph {
            vault,
            query,
            node_type,
            limit,
            json,
        } => {
            let opts = wiki_proto::graph::GraphOpts {
                query,
                node_type,
                limit,
                weights: None,
            };
            // Same `WikiGraph` shape either way — the server runs
            // the identical `wiki_graph::build_graph` over its
            // vault root.
            let graph = match route_vault(&vault)? {
                VaultRoute::Local(vault) => wiki_graph::build_graph(&vault, opts)
                    .map_err(|e| eyre::eyre!("build_graph: {e}"))?,
                VaultRoute::Vox(url) => {
                    let c: wiki_proto::service::graph::GraphClient =
                        establish_for_url(&url).await?;
                    c.build_graph(ORG_WIKI_ID.to_string(), opts)
                        .await
                        .map_err(|e| eyre::eyre!("build_graph: {e:?}"))?
                }
            };
            if json {
                let payload = serde_json::json!({
                    "nodes": graph.nodes.iter().map(|n| serde_json::json!({
                        "id": n.id,
                        "label": n.label,
                        "type": n.node_type,
                        "link_count": n.link_count,
                    })).collect::<Vec<_>>(),
                    "edges": graph.edges.iter().map(|e| serde_json::json!({
                        "source": e.source,
                        "target": e.target,
                        "weight": e.weight,
                        "signals": {
                            "direct_link": e.signals.direct_link,
                            "source_overlap": e.signals.source_overlap,
                            "adamic_adar": e.signals.adamic_adar,
                            "type_affinity": e.signals.type_affinity,
                        },
                    })).collect::<Vec<_>>(),
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("nodes={}  edges={}", graph.nodes.len(), graph.edges.len());
                let mut top = graph.nodes.clone();
                top.sort_by(|a, b| b.link_count.cmp(&a.link_count));
                for n in top.iter().take(20) {
                    println!("  {:3}  [{}]  {}", n.link_count, n.node_type, n.label);
                }
                if graph.nodes.len() > 20 {
                    println!("  … {} more", graph.nodes.len() - 20);
                }
            }
            Ok(())
        }
        // `context` stays FS-native: `build_context` composes the
        // wiki graph with LOCAL overlays (`--notes` tree, `--links`
        // jsonl) that only exist on this machine's disk; no RPC
        // carries that composition (gap — the Graph service covers
        // plain `build_graph` only).
        WikiCmd::Context {
            query,
            vault,
            node_type,
            budget_tokens,
            max_nodes,
            summary_chars,
            notes,
            links,
        } => {
            let vault = vault
                .canonicalize()
                .map_err(|e| eyre::eyre!("vault {}: {e}", vault.display()))?;
            // Typed-link overlay: note↔note links become
            // direct-link edges (verses aren't graph nodes;
            // endpoints missing from the graph are dropped
            // inside `build_context`).
            let extra_edges = match links {
                Some(path) => {
                    use links::LinksService as _;
                    links::Store::open(path)
                        .graph(links::Confidence::Speculative, true)
                        .map_err(|e| eyre::eyre!("links store: {e}"))?
                        .into_iter()
                        .filter(|l| {
                            l.source.kind == links::NodeKind::Note
                                && l.target.kind == links::NodeKind::Note
                        })
                        .map(|l| {
                            (
                                format!("{}{}", wiki_graph::NOTE_ID_PREFIX, l.source.id),
                                format!("{}{}", wiki_graph::NOTE_ID_PREFIX, l.target.id),
                            )
                        })
                        .collect()
                }
                None => Vec::new(),
            };
            let result = wiki_graph::build_context(
                &vault,
                wiki_graph::ContextOpts {
                    query,
                    node_type,
                    budget_tokens,
                    max_nodes,
                    summary_chars,
                    notes_root: notes,
                    extra_edges,
                },
            )
            .map_err(|e| eyre::eyre!("build_context: {e}"))?;
            // The whole point is that the markdown is the
            // output — print it to stdout so the caller can
            // pipe it directly into an LLM prompt.
            print!("{}", result.markdown);
            eprintln!(
                "\n[wiki context] {}/{} nodes, ~{} tokens (budget {})",
                result.included.len(),
                result.nodes_considered,
                result.tokens_estimate,
                budget_tokens
            );
            Ok(())
        }
        WikiCmd::Code {
            root,
            json,
            report,
            top,
        } => {
            let root = root
                .canonicalize()
                .map_err(|e| eyre::eyre!("root {}: {e}", root.display()))?;
            let extractions = wiki_graph::scan_code_tree(&root);

            if report {
                // Cross-file analysis + GRAPH_REPORT.md
                // render. This is the agent-context payload
                // for "give me a high-level map of this
                // codebase".
                let g = wiki_graph::analyze(&extractions);
                print!("{}", wiki_graph::render_report(&g, top));
                eprintln!(
                    "[wiki code report] {} nodes, {} edges ({} resolved, {} unresolved)",
                    g.nodes.len(),
                    g.edges.len(),
                    g.resolved_edges,
                    g.unresolved_edges
                );
                return Ok(());
            }
            let mut total_nodes = 0usize;
            let mut total_edges = 0usize;
            let mut by_kind: std::collections::HashMap<&str, usize> =
                std::collections::HashMap::new();
            let mut all_nodes: Vec<wiki_graph::CodeNode> = Vec::new();
            let mut all_edges: Vec<wiki_graph::CodeEdge> = Vec::new();
            let mut errors = Vec::new();
            for ex in extractions {
                total_nodes += ex.nodes.len();
                total_edges += ex.edges.len();
                for n in &ex.nodes {
                    *by_kind.entry(n.kind.as_str()).or_insert(0) += 1;
                }
                all_nodes.extend(ex.nodes);
                all_edges.extend(ex.edges);
                errors.extend(ex.errors);
            }
            if json {
                let payload = serde_json::json!({
                    "root": root.display().to_string(),
                    "totals": {
                        "nodes": total_nodes,
                        "edges": total_edges,
                        "errors": errors.len(),
                    },
                    "by_kind": by_kind,
                    "nodes": all_nodes.iter().map(|n| serde_json::json!({
                        "id": n.id,
                        "label": n.label,
                        "kind": n.kind.as_str(),
                        "language": n.language.tag(),
                        "source_file": n.source_file.display().to_string(),
                        "line_start": n.line_start,
                        "line_end": n.line_end,
                    })).collect::<Vec<_>>(),
                    "edges": all_edges.iter().map(|e| serde_json::json!({
                        "source": e.source,
                        "target": e.target,
                        "relation": e.relation.as_str(),
                        "confidence": e.confidence.as_str(),
                    })).collect::<Vec<_>>(),
                    "errors": errors,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("root: {}", root.display());
                println!("nodes: {total_nodes}, edges: {total_edges}");
                if !by_kind.is_empty() {
                    let mut kinds: Vec<_> = by_kind.iter().collect();
                    kinds.sort_by(|a, b| b.1.cmp(a.1));
                    println!("by kind:");
                    for (k, n) in kinds {
                        println!("  {n:>5} {k}");
                    }
                }
                // Top-N nodes by how often they appear as a
                // target — i.e. most-called / most-imported.
                use std::collections::HashMap;
                let mut indeg: HashMap<&str, usize> = HashMap::new();
                for e in &all_edges {
                    *indeg.entry(e.target.as_str()).or_insert(0) += 1;
                }
                let mut ranked: Vec<_> = all_nodes
                    .iter()
                    .map(|n| (n, indeg.get(n.id.as_str()).copied().unwrap_or(0)))
                    .collect();
                ranked.sort_by(|a, b| b.1.cmp(&a.1));
                println!("\ntop {top} by in-degree (high = referenced a lot):");
                for (n, deg) in ranked.iter().take(top) {
                    println!(
                        "  {deg:>3} {:<8} {} ({}:L{})",
                        n.kind.as_str(),
                        n.label,
                        n.source_file.display(),
                        n.line_start + 1
                    );
                }
                if !errors.is_empty() {
                    eprintln!("\n{} extraction error(s):", errors.len());
                    for e in errors.iter().take(5) {
                        eprintln!("  - {e}");
                    }
                }
            }
            Ok(())
        }
        WikiCmd::Gaps { vault, json } => {
            let gaps = match route_vault(&vault)? {
                VaultRoute::Local(vault) => {
                    wiki_graph::find_gaps(&vault).map_err(|e| eyre::eyre!("find_gaps: {e}"))?
                }
                VaultRoute::Vox(url) => {
                    let c: wiki_proto::service::graph::GraphClient =
                        establish_for_url(&url).await?;
                    c.gaps(ORG_WIKI_ID.to_string())
                        .await
                        .map_err(|e| eyre::eyre!("find_gaps: {e:?}"))?
                }
            };
            if json {
                let payload: Vec<_> = gaps
                    .iter()
                    .map(|g| {
                        serde_json::json!({
                            "id": g.id,
                            "kind": format!("{:?}", g.kind),
                            "subjects": g.subjects,
                            "explanation": g.explanation,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                let orphans = gaps
                    .iter()
                    .filter(|g| matches!(g.kind, wiki_proto::graph::GapKind::Orphan))
                    .count();
                let missing = gaps
                    .iter()
                    .filter(|g| matches!(g.kind, wiki_proto::graph::GapKind::MissingPage))
                    .count();
                println!(
                    "gaps={} (orphans={} missing-pages={})",
                    gaps.len(),
                    orphans,
                    missing
                );
                for g in &gaps {
                    println!("  [{:?}] {}", g.kind, g.explanation);
                }
            }
            Ok(())
        }
        WikiCmd::Search {
            vault,
            query,
            node_type,
            top_k,
            include_content,
            hybrid,
        } => {
            let opts = wiki_proto::search::SearchOpts {
                query: query.clone(),
                top_k,
                include_content,
                mode: if hybrid {
                    wiki_proto::search::SearchMode::Hybrid
                } else {
                    wiki_proto::search::SearchMode::Token
                },
                node_type,
            };
            let hits = match route_vault(&vault)? {
                VaultRoute::Local(vault) => {
                    wiki_search::search(&vault, opts).map_err(|e| eyre::eyre!("search: {e}"))?
                }
                VaultRoute::Vox(url) => {
                    let c: wiki_proto::service::search::SearchClient =
                        establish_for_url(&url).await?;
                    c.search(ORG_WIKI_ID.to_string(), opts)
                        .await
                        .map_err(|e| eyre::eyre!("search: {e:?}"))?
                }
            };
            println!(
                "mode={:?}  token={}  vector={}  total={}",
                hits.mode,
                hits.token_count,
                hits.vector_count,
                hits.hits.len()
            );
            for h in &hits.hits {
                println!("  {:>5.2}  [{}]  {}", h.score, h.path, h.title);
                if !h.snippet.is_empty() {
                    println!("         {}", h.snippet);
                }
            }
            Ok(())
        }
        WikiCmd::Clusters { vault } => {
            let clusters = match route_vault(&vault)? {
                VaultRoute::Local(vault) => wiki_graph::build_clusters(&vault)
                    .map_err(|e| eyre::eyre!("build_clusters: {e}"))?,
                VaultRoute::Vox(url) => {
                    let c: wiki_proto::service::graph::GraphClient =
                        establish_for_url(&url).await?;
                    c.clusters(ORG_WIKI_ID.to_string())
                        .await
                        .map_err(|e| eyre::eyre!("build_clusters: {e:?}"))?
                }
            };
            println!("clusters: {}", clusters.len());
            for c in &clusters {
                println!(
                    "  {}  ({:>3} members, cohesion {:.2})  — {}",
                    c.id,
                    c.members.len(),
                    c.cohesion,
                    c.name
                );
            }
            Ok(())
        }
        WikiCmd::WatchSources {
            vault,
            debounce_secs,
            dry_run,
        } => {
            // Stays FS-native by design: this is a long-running
            // inotify watcher over a LOCAL directory. The server's
            // Watcher service is a toggle for its own co-resident
            // watcher (`task wiki watch on|off`), and no RPC
            // streams FS events — watching a remote org's disk
            // from here is not a thing.
            let vault = vault
                .canonicalize()
                .map_err(|e| eyre::eyre!("vault {}: {e}", vault.display()))?;
            let wiki = wiki_live::WikiLive::open(&vault);
            wiki.bootstrap()
                .map_err(|e| eyre::eyre!("bootstrap: {e}"))?;
            let opts = wiki_live::WatchSourcesOpts {
                debounce: Duration::from_secs(debounce_secs),
                auto_enqueue: !dry_run,
            };
            let (rx, _guard) = wiki
                .watch_sources(opts)
                .map_err(|e| eyre::eyre!("watch_sources: {e}"))?;
            eprintln!(
                "Watching {}/Wiki/raw/sources/ (debounce {}s, auto_enqueue={})",
                vault.display(),
                debounce_secs,
                !dry_run
            );
            for event in rx {
                println!(
                    "diff: +{} ~{} -{}  enqueued={}",
                    event.diff.created.len(),
                    event.diff.modified.len(),
                    event.diff.deleted.len(),
                    event.enqueued_task_ids.len(),
                );
                for c in &event.diff.created {
                    println!("  + {c}");
                }
                for m in &event.diff.modified {
                    println!("  ~ {m}");
                }
                for d in &event.diff.deleted {
                    println!("  - {d}");
                }
            }
            Ok(())
        }
        WikiCmd::Health { vault } => {
            match route_vault(&vault)? {
                VaultRoute::Local(vault) => {
                    let wiki = wiki_live::WikiLive::open(&vault);
                    let h = wiki.health().map_err(|e| eyre::eyre!("health: {e}"))?;
                    println!("bootstrapped:    {}", h.bootstrap_done);
                    println!("schema_present:  {}", h.schema_present);
                    println!("purpose_present: {}", h.purpose_present);
                    println!("pages:           {}", h.page_count);
                    println!("sources:         {}", h.source_count);
                    println!("queue_depth:     {}", h.queue_depth);
                    println!("queue_failed:    {}", h.queue_failed);
                    if let Some(t) = h.last_ingest_at {
                        println!("last_ingest_at:  {t}");
                    }
                    if let Some(t) = h.last_rescan_at {
                        println!("last_rescan_at:  {t}");
                    }
                }
                VaultRoute::Vox(url) => {
                    // Same snapshot via the Schema service. The
                    // wire shape carries no `last_rescan_at`, so
                    // that (conditional) line is simply absent
                    // here.
                    let c: wiki_proto::service::schema::SchemaClient =
                        establish_for_url(&url).await?;
                    let h = c
                        .health(ORG_WIKI_ID.to_string())
                        .await
                        .map_err(|e| eyre::eyre!("health: {e:?}"))?;
                    println!("bootstrapped:    {}", h.bootstrap_done);
                    println!("schema_present:  {}", h.schema_present);
                    println!("purpose_present: {}", h.purpose_present);
                    println!("pages:           {}", h.page_count);
                    println!("sources:         {}", h.source_count);
                    println!("queue_depth:     {}", h.queue_depth);
                    println!("queue_failed:    {}", h.queue_failed);
                    if let Some(t) = h.last_ingest_at {
                        println!("last_ingest_at:  {t}");
                    }
                }
            }
            Ok(())
        }
        WikiCmd::Import {
            vault,
            dir,
            flatten,
            ext,
        } => {
            let include_exts: Vec<String> =
                ext.split(',').map(|s| s.trim().to_lowercase()).collect();
            let exclude_substrings = [".git/", "node_modules/", "target/"];
            let refs = match route_vault(&vault)? {
                VaultRoute::Local(vault) => {
                    let wiki = wiki_live::WikiLive::open(&vault);
                    wiki.bootstrap()
                        .map_err(|e| eyre::eyre!("bootstrap: {e}"))?;
                    let opts = wiki_live::ImportFolderOpts {
                        preserve_structure: !flatten,
                        include_exts,
                        exclude_substrings: exclude_substrings
                            .iter()
                            .map(|s| (*s).to_owned())
                            .collect(),
                    };
                    wiki.import_folder(&dir, opts)
                        .map_err(|e| eyre::eyre!("import_folder: {e}"))?
                }
                VaultRoute::Vox(url) => {
                    // The directory being imported is local by
                    // definition; the WIKI is not. Mirror
                    // `WikiLive::import_folder`'s walk + filters
                    // here, then land each file over the wire via
                    // `import_raw_source` (sha-deduped
                    // server-side), exactly what the local helper
                    // does underneath.
                    use wiki_proto::service::raw_layer::RawLayerClient;
                    if !dir.is_dir() {
                        return Err(eyre::eyre!(
                            "import_folder: {} is not a directory",
                            dir.display()
                        ));
                    }
                    let schema: wiki_proto::service::schema::SchemaClient =
                        establish_for_url(&url).await?;
                    schema
                        .bootstrap(ORG_WIKI_ID.to_string())
                        .await
                        .map_err(|e| eyre::eyre!("bootstrap: {e:?}"))?;
                    let raw: RawLayerClient = establish_for_url(&url).await?;
                    let mut refs = Vec::new();
                    for entry in walkdir::WalkDir::new(&dir)
                        .into_iter()
                        .filter_map(Result::ok)
                    {
                        let path = entry.path();
                        if !path.is_file() {
                            continue;
                        }
                        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                            continue;
                        };
                        if name.starts_with('.') {
                            continue;
                        }
                        let rel = path.strip_prefix(&dir).map_or_else(
                            |_| name.to_string(),
                            |p| p.to_string_lossy().to_string(),
                        );
                        if exclude_substrings.iter().any(|sub| rel.contains(sub)) {
                            continue;
                        }
                        if !include_exts.is_empty() {
                            let e = path
                                .extension()
                                .and_then(|s| s.to_str())
                                .map(str::to_ascii_lowercase)
                                .unwrap_or_default();
                            if !include_exts.contains(&e) {
                                continue;
                            }
                        }
                        let bytes = std::fs::read(path)?;
                        let filename = if flatten {
                            rel.replace(['/', '\\'], "_")
                        } else {
                            rel.replace('\\', "/")
                        };
                        let mime = archive_mime_for_filename(name);
                        let r = raw
                            .import_raw_source(
                                ORG_WIKI_ID.to_string(),
                                wiki_proto::raw::ImportRawSource {
                                    filename,
                                    mime,
                                    title: String::new(),
                                    bytes,
                                    auto_enqueue: false,
                                },
                            )
                            .await
                            .map_err(|e| eyre::eyre!("import_raw_source {rel}: {e:?}"))?;
                        refs.push(r);
                    }
                    refs
                }
            };
            println!("Imported {} file(s) from {}", refs.len(), dir.display());
            for r in refs.iter().take(40) {
                println!("  {} ({} bytes)", r.path, r.size);
            }
            if refs.len() > 40 {
                println!("  … {} more", refs.len() - 40);
            }
            Ok(())
        }
        WikiCmd::Rescan { vault, enqueue } => {
            match route_vault(&vault)? {
                VaultRoute::Local(vault) => {
                    let wiki = wiki_live::WikiLive::open(&vault);
                    let diff = wiki
                        .rescan_sources()
                        .map_err(|e| eyre::eyre!("rescan: {e}"))?;
                    println!(
                        "created={} modified={} deleted={}",
                        diff.created.len(),
                        diff.modified.len(),
                        diff.deleted.len()
                    );
                    for c in &diff.created {
                        println!("  + {c}");
                    }
                    for m in &diff.modified {
                        println!("  ~ {m}");
                    }
                    for d in &diff.deleted {
                        println!("  - {d}");
                    }
                    if enqueue {
                        let mut count = 0;
                        for c in diff.created.iter().chain(diff.modified.iter()) {
                            let abs = wiki.wiki_root().join(c);
                            let bytes = std::fs::read(&abs)?;
                            let kind = if diff.created.contains(c) {
                                wiki_live::queue::SourceChange::Created
                            } else {
                                wiki_live::queue::SourceChange::Modified
                            };
                            wiki.enqueue_ingest(c, kind, &bytes)
                                .map_err(|e| eyre::eyre!("enqueue {c}: {e}"))?;
                            count += 1;
                        }
                        println!("enqueued {count} ingest task(s)");
                    }
                }
                VaultRoute::Vox(url) => {
                    // `rescan_diff` is the read-only sibling of
                    // the raw layer's `rescan_sources` RPC —
                    // added precisely so this command keeps its
                    // "diff first, enqueue only on --enqueue"
                    // contract over the wire.
                    use wiki_proto::service::ingest::IngestClient;
                    use wiki_proto::service::raw_layer::RawLayerClient;
                    let raw: RawLayerClient = establish_for_url(&url).await?;
                    let diff = raw
                        .rescan_diff(ORG_WIKI_ID.to_string())
                        .await
                        .map_err(|e| eyre::eyre!("rescan: {e:?}"))?;
                    println!(
                        "created={} modified={} deleted={}",
                        diff.created.len(),
                        diff.modified.len(),
                        diff.deleted.len()
                    );
                    for c in &diff.created {
                        println!("  + {c}");
                    }
                    for m in &diff.modified {
                        println!("  ~ {m}");
                    }
                    for d in &diff.deleted {
                        println!("  - {d}");
                    }
                    if enqueue {
                        let ing: IngestClient = establish_for_url(&url).await?;
                        let mut count = 0;
                        for c in diff.created.iter().chain(diff.modified.iter()) {
                            let kind = if diff.created.contains(c) {
                                wiki_proto::ingest::SourceChange::Created
                            } else {
                                wiki_proto::ingest::SourceChange::Modified
                            };
                            ing.enqueue_ingest(ORG_WIKI_ID.to_string(), c.clone(), kind)
                                .await
                                .map_err(|e| eyre::eyre!("enqueue {c}: {e:?}"))?;
                            count += 1;
                        }
                        println!("enqueued {count} ingest task(s)");
                    }
                }
            }
            Ok(())
        }
        // `lint` / `dedup` / `research` / `ingest` / `deepen` stay
        // FS-native: they RUN the LLM here (agent-codex is a local
        // process supervisor) and `agent_wiki::bridge` drives it
        // against a concrete `WikiLive` handle. Putting their data
        // over vox means either running the agent server-side (the
        // server's `Lint::lint` is an explicit stub) or re-basing
        // the bridge on the wiki RPC clients — a real refactor,
        // reported as a gap, not smuggled in here. Their *outputs*
        // (findings, queue rows, pages) are visible over vox via
        // `findings` / `ingest-queue` / `review`.
        WikiCmd::Lint {
            vault,
            model,
            timeout_secs,
            language,
        } => {
            let vault = vault
                .canonicalize()
                .map_err(|e| eyre::eyre!("vault {}: {e}", vault.display()))?;
            let wiki = wiki_live::WikiLive::open(&vault);
            wiki.bootstrap()
                .map_err(|e| eyre::eyre!("bootstrap: {e}"))?;
            let backend = agent_codex::CodexBackend::new();
            let req = agent_wiki::bridge::LintRequest {
                model,
                timeout: Duration::from_secs(timeout_secs),
                language,
            };
            let raised = agent_wiki::bridge::run_lint(&backend, &wiki, req)
                .await
                .map_err(|e| eyre::eyre!("lint: {e}"))?;
            println!("New findings: {}", raised.len());
            for f in &raised {
                println!("  [{:?} {:?}] {}", f.kind, f.severity, f.title);
            }
            Ok(())
        }
        WikiCmd::Findings { vault } => {
            match route_vault(&vault)? {
                VaultRoute::Local(vault) => {
                    let wiki = wiki_live::WikiLive::open(&vault);
                    let open = wiki
                        .list_findings(Some(wiki_live::FindingStatus::Open))
                        .map_err(|e| eyre::eyre!("list_findings: {e}"))?;
                    println!("Open findings: {}", open.len());
                    for f in &open {
                        println!("  {}  [{:?} {:?}] {}", f.id, f.kind, f.severity, f.title);
                        if !f.pages.is_empty() {
                            println!("      pages: {}", f.pages.join(", "));
                        }
                    }
                }
                VaultRoute::Vox(url) => {
                    // The wire shape names things differently
                    // (kind→scope, title→message, pages→subjects)
                    // but carries the same finding.
                    let c: wiki_proto::service::lint::LintClient = establish_for_url(&url).await?;
                    let open = c
                        .list_findings(ORG_WIKI_ID.to_string())
                        .await
                        .map_err(|e| eyre::eyre!("list_findings: {e:?}"))?;
                    println!("Open findings: {}", open.len());
                    for f in &open {
                        println!("  {}  [{:?} {:?}] {}", f.id, f.scope, f.severity, f.message);
                        if !f.subjects.is_empty() {
                            println!("      pages: {}", f.subjects.join(", "));
                        }
                    }
                }
            }
            Ok(())
        }
        WikiCmd::LintTiers { org_root } => {
            let org_root = if let Some(p) = org_root {
                p
            } else {
                let ctx = crate::org_ctx::resolve_active(None)?;
                ctx.root.path().to_path_buf()
            };
            let violations = wiki_graph::lint_org_tree(&org_root);
            if violations.is_empty() {
                println!("OK — no tier violations in {}", org_root.display());
                return Ok(());
            }
            println!(
                "{} violation{} in {}\n",
                violations.len(),
                if violations.len() == 1 { "" } else { "s" },
                org_root.display(),
            );
            for v in &violations {
                println!(
                    "  {} ({}) → [[{}]] resolves to {} ({})",
                    v.source.display(),
                    v.source_tier.as_str(),
                    v.target_link,
                    v.resolved.display(),
                    v.resolved_tier.as_str(),
                );
            }
            std::process::exit(1);
        }
        WikiCmd::Dedup {
            vault,
            model,
            timeout_secs,
        } => {
            let vault = vault
                .canonicalize()
                .map_err(|e| eyre::eyre!("vault {}: {e}", vault.display()))?;
            let wiki = wiki_live::WikiLive::open(&vault);
            let backend = agent_codex::CodexBackend::new();
            let groups = agent_wiki::bridge::run_dedup_detect(
                &backend,
                &wiki,
                model,
                Duration::from_secs(timeout_secs),
            )
            .await
            .map_err(|e| eyre::eyre!("dedup_detect: {e}"))?;
            println!("Dedup groups: {}", groups.len());
            for g in &groups {
                println!(
                    "  [{:?}] {} — {}",
                    g.confidence,
                    g.slugs.join(", "),
                    g.reason
                );
            }
            Ok(())
        }
        WikiCmd::Research {
            vault,
            gap_kind,
            gap_title,
            gap_description,
            model,
            timeout_secs,
            language,
        } => {
            let vault = vault
                .canonicalize()
                .map_err(|e| eyre::eyre!("vault {}: {e}", vault.display()))?;
            let wiki = wiki_live::WikiLive::open(&vault);
            let backend = agent_codex::CodexBackend::new();
            let plan = agent_wiki::bridge::run_propose_research(
                &backend,
                &wiki,
                &gap_kind,
                &gap_title,
                &gap_description,
                model,
                Duration::from_secs(timeout_secs),
                &language,
            )
            .await
            .map_err(|e| eyre::eyre!("propose_research: {e}"))?;
            println!("TOPIC: {}", plan.topic);
            for q in &plan.queries {
                println!("QUERY: {q}");
            }
            Ok(())
        }
        WikiCmd::Ingest {
            vault,
            source,
            filename,
            mime,
            title,
            model,
            language,
            timeout_secs,
        } => {
            let vault = vault
                .canonicalize()
                .map_err(|e| eyre::eyre!("vault {}: {e}", vault.display()))?;
            let bytes = std::fs::read(&source)
                .map_err(|e| eyre::eyre!("read source {}: {e}", source.display()))?;
            let fname = filename.unwrap_or_else(|| {
                source
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("source.md")
                    .to_string()
            });
            let wiki = WikiLive::open(&vault);
            let backend = CodexBackend::new();
            let req = IngestRequest {
                source_filename: fname,
                source_mime: mime,
                source_title: title.unwrap_or_default(),
                source_bytes: bytes,
                model: model.clone(),
                timeout: Duration::from_secs(timeout_secs),
                language,
            };
            eprintln!(
                "› ingest@{} vault={}",
                model.as_deref().unwrap_or("default"),
                vault.display()
            );
            let result = run_ingest(&backend, &wiki, req)
                .await
                .map_err(|e| eyre::eyre!("ingest: {e}"))?;
            println!("Ingest done.");
            println!("  task:   {}", result.task_id);
            println!("  source: {}", result.raw_source_path);
            println!("  pages:  {}", result.pages_written.len());
            for p in &result.pages_written {
                println!("    - {p}");
            }
            if !result.reviews_raised.is_empty() {
                println!("  reviews:");
                for r in &result.reviews_raised {
                    println!("    - {r}");
                }
            }
            Ok(())
        }
        WikiCmd::Deepen {
            vault,
            page,
            model,
            timeout_secs,
            language,
        } => {
            let vault = vault
                .canonicalize()
                .map_err(|e| eyre::eyre!("vault {}: {e}", vault.display()))?;
            let wiki = WikiLive::open(&vault);
            let backend = CodexBackend::new();
            eprintln!(
                "› deepen@{} page={page}",
                model.as_deref().unwrap_or("default")
            );
            let result = agent_wiki::bridge::run_deepen(
                &backend,
                &wiki,
                &page,
                model,
                Duration::from_secs(timeout_secs),
                &language,
            )
            .await
            .map_err(|e| eyre::eyre!("deepen: {e}"))?;
            println!("Deepen done.");
            println!("  page:  {}", result.page_path);
            println!(
                "  words: {} → {} ({:+})",
                result.before_words,
                result.after_words,
                result.after_words as i64 - result.before_words as i64
            );
            Ok(())
        }
        WikiCmd::Archive(args) => run_wiki_archive(args).await,
        WikiCmd::Schema(c) => run_wiki_schema(c).await,
        WikiCmd::Catalog(c) => run_wiki_catalog(c).await,
        WikiCmd::Raw(c) => run_wiki_raw(c).await,
        WikiCmd::IngestQueue(c) => run_wiki_ingest(c).await,
        WikiCmd::LintFindings(c) => run_wiki_lint_findings(c).await,
        WikiCmd::Review(c) => run_wiki_review(c).await,
        WikiCmd::ResearchPlans(c) => run_wiki_research_plans(c).await,
        WikiCmd::Watch(c) => run_wiki_watch(c).await,
    }
}

async fn run_wiki_schema(cmd: WikiSchemaCmd) -> eyre::Result<()> {
    use wiki_proto::service::schema::SchemaClient;
    async fn connect(url: &str) -> eyre::Result<SchemaClient> {
        establish_for_url(url).await
    }
    match cmd {
        WikiSchemaCmd::Show {
            wiki_id,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let doc = c
                .read_schema(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("read_schema: {e:?}"))?;
            if json {
                println!("{doc:#?}");
            } else {
                println!("{}", doc.markdown);
            }
        }
        WikiSchemaCmd::Purpose {
            wiki_id,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let doc = c
                .read_purpose(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("read_purpose: {e:?}"))?;
            if json {
                println!("{doc:#?}");
            } else {
                println!("{}", doc.markdown);
            }
        }
        WikiSchemaCmd::WriteSchema {
            path,
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let body = if path == "-" {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s
            } else {
                std::fs::read_to_string(&path)?
            };
            c.write_schema(wiki_id, body)
                .await
                .map_err(|e| eyre::eyre!("write_schema: {e:?}"))?;
            println!("wrote schema");
        }
        WikiSchemaCmd::WritePurpose {
            path,
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let body = if path == "-" {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s
            } else {
                std::fs::read_to_string(&path)?
            };
            c.write_purpose(wiki_id, body)
                .await
                .map_err(|e| eyre::eyre!("write_purpose: {e:?}"))?;
            println!("wrote purpose");
        }
        WikiSchemaCmd::Bootstrap {
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            c.bootstrap(wiki_id.clone())
                .await
                .map_err(|e| eyre::eyre!("bootstrap: {e:?}"))?;
            println!("bootstrapped {wiki_id}");
        }
        WikiSchemaCmd::Health {
            wiki_id,
            org,
            server,
            json: _,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let h = c
                .health(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("health: {e:?}"))?;
            println!("{h:#?}");
        }
    }
    Ok(())
}

async fn run_wiki_catalog(cmd: WikiCatalogCmd) -> eyre::Result<()> {
    use wiki_proto::service::catalog::CatalogClient;
    async fn connect(url: &str) -> eyre::Result<CatalogClient> {
        establish_for_url(url).await
    }
    match cmd {
        WikiCatalogCmd::Show {
            wiki_id,
            org,
            server,
            json: _,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let idx = c
                .read_index(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("read_index: {e:?}"))?;
            println!("{idx:#?}");
        }
        WikiCatalogCmd::Rebuild {
            wiki_id,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let idx = c
                .rebuild_index(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("rebuild_index: {e:?}"))?;
            if json {
                println!("{idx:#?}");
            } else {
                println!("rebuilt catalog");
            }
        }
    }
    Ok(())
}

async fn run_wiki_raw(cmd: WikiRawCmd) -> eyre::Result<()> {
    use wiki_proto::service::raw_layer::RawLayerClient;
    async fn connect(url: &str) -> eyre::Result<RawLayerClient> {
        establish_for_url(url).await
    }
    match cmd {
        WikiRawCmd::List {
            wiki_id,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let rows = c
                .list_raw_sources(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("list_raw_sources: {e:?}"))?;
            if json {
                println!("{rows:#?}");
            } else {
                for r in &rows {
                    println!("{r:?}");
                }
            }
        }
        WikiRawCmd::Read {
            path,
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let bytes = c
                .read_raw_source(wiki_id, path)
                .await
                .map_err(|e| eyre::eyre!("read_raw_source: {e:?}"))?;
            std::io::Write::write_all(&mut std::io::stdout(), &bytes)?;
        }
        WikiRawCmd::Delete {
            path,
            yes,
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            if !yes && !confirm(&format!("delete raw source `{path}`?"))? {
                println!("aborted");
                return Ok(());
            }
            let reviews = c
                .delete_raw_source(wiki_id, path.clone())
                .await
                .map_err(|e| eyre::eyre!("delete_raw_source: {e:?}"))?;
            println!("deleted {path} ({} review items enqueued)", reviews.len());
        }
        WikiRawCmd::Rescan {
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let tasks = c
                .rescan_sources(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("rescan_sources: {e:?}"))?;
            println!("enqueued {} ingest task(s)", tasks.len());
        }
    }
    Ok(())
}

/// `task wiki archive <url|file>` — extract locally, then
/// feed the UNCHANGED raw→ingest pipeline over RPC:
/// `import_raw_source` (sha-dedup server-side) +
/// `enqueue_ingest`. Canonical-URL dedup happens client-side
/// by filename scan — archived sources are named
/// `<slug>-<canon8>.md` so the same resource reached via
/// different links collapses to one file.
async fn run_wiki_archive(mut args: WikiArchiveArgs) -> eyre::Result<()> {
    use wiki_proto::raw::ImportRawSource;
    use wiki_proto::service::ingest::IngestClient;
    use wiki_proto::service::raw_layer::RawLayerClient;

    match args.cmd.take() {
        Some(WikiArchiveSub::Import(cmd)) => return run_wiki_archive_import(cmd).await,
        Some(WikiArchiveSub::Health { org, json }) => {
            return run_wiki_archive_health(org, json);
        }
        Some(WikiArchiveSub::Retry {
            limit,
            wiki_id,
            org,
            server,
            no_enqueue,
        }) => {
            return run_wiki_archive_retry(limit, wiki_id, org, server, no_enqueue).await;
        }
        None => {}
    }
    let target = args
        .target
        .clone()
        .ok_or_else(|| eyre::eyre!("a URL or file path is required"))?;

    let slug = resolve_active_org(args.org.clone())?;
    let vox_url = resolve_org_vox_url(args.server.clone(), &slug);

    // ── Resolve the target into import-ready parts ──────
    let local = std::path::Path::new(&target);
    let (filename, mime, title, bytes, canon8) = if local.exists() {
        // Local file: bytes go in as-is (binary originals
        // included) — the ingest pipeline + wiki-extract
        // already handle md/txt/pdf/docx. No provenance
        // frontmatter is injected into foreign bytes.
        let name = local
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("source")
            .to_string();
        let mime = archive_mime_for_filename(&name);
        let title = args.title.clone().unwrap_or_else(|| name.clone());
        (name, mime, title, std::fs::read(local)?, None)
    } else {
        // Health bookkeeping — every attempt lands in the
        // per-org extractor ledger (`task wiki archive health`).
        let route_label = wiki_archive::classify(&target)
            .ok()
            .map(|(_, r)| wiki_archive::health::route_label(&r));
        let health_path = archive_health_path(&slug);
        match archive_extract_url(&target, &args).await {
            Ok((prov, body)) => {
                if let (Some(p), Some(label)) = (&health_path, route_label) {
                    wiki_archive::health::record(p, label, Ok(()));
                }
                let md = wiki_archive::compose_source_markdown(&prov, &body);
                (
                    prov.filename(),
                    "text/markdown".to_string(),
                    prov.title.clone(),
                    md.into_bytes(),
                    Some(prov.canon8()),
                )
            }
            Err(e) => {
                if let (Some(p), Some(label)) = (&health_path, route_label) {
                    wiki_archive::health::record(p, label, Err(&e.to_string()));
                }
                // Accept-fragility routes (Reddit/X) store an
                // honest unarchived stub instead of failing —
                // `task wiki archive retry` sweeps them later.
                let Some((prov, body)) = unarchived_stub(&target, &e.to_string()) else {
                    return Err(eyre::eyre!("archive {target}: {e}"));
                };
                println!("could not archive {target}: {e}");
                println!(
                    "storing an unarchived stub — `task wiki archive retry` re-attempts later"
                );
                let md = wiki_archive::compose_source_markdown(&prov, &body);
                (
                    format!("unarchived-{}", prov.filename()),
                    "text/markdown".to_string(),
                    prov.title.clone(),
                    md.into_bytes(),
                    Some(prov.canon8()),
                )
            }
        }
    };

    let raw: RawLayerClient = establish_for_url(&vox_url).await?;

    // ── Canonical-URL dedup ─────────────────────────────
    let mut stale_stub: Option<String> = None;
    if let Some(c8) = &canon8 {
        let existing = raw
            .list_raw_sources(args.wiki_id.clone())
            .await
            .map_err(|e| eyre::eyre!("list_raw_sources: {e:?}"))?;
        let hit =
            wiki_archive::find_canonical_match(existing.iter().map(|r| r.filename.as_str()), c8)
                .map(ToString::to_string);
        if let Some(hit) = hit {
            let importing_stub = filename.starts_with("unarchived-");
            if importing_stub {
                // Something (stub or real) already tracks this
                // canonical URL — never stack stubs.
                println!("already tracked as raw/sources/{hit} — not writing another stub");
                return Ok(());
            }
            if hit.starts_with("unarchived-") {
                // A failed earlier attempt left a stub and this
                // run extracted successfully — replace it.
                println!("replacing unarchived stub {hit}");
                stale_stub = Some(hit);
            } else if args.force {
                println!("note: canonical URL already archived as {hit} (continuing: --force)");
            } else {
                if args.json {
                    println!(
                        "{}",
                        serde_json::json!({ "deduped": true, "existing": hit, "canon8": c8 })
                    );
                } else {
                    println!("already archived (canonical-url dedup): raw/sources/{hit}");
                    println!("pass --force to archive a fresh copy");
                }
                return Ok(());
            }
        }
    }

    // ── Import (server sha-dedups byte-identical content) ──
    let r = raw
        .import_raw_source(
            args.wiki_id.clone(),
            ImportRawSource {
                filename,
                mime,
                title,
                bytes,
                auto_enqueue: false, // explicit enqueue below
            },
        )
        .await
        .map_err(|e| eyre::eyre!("import_raw_source: {e:?}"))?;

    // The successful archive supersedes any unarchived stub.
    if let Some(stub) = stale_stub {
        if let Err(e) = raw
            .delete_raw_source(args.wiki_id.clone(), format!("raw/sources/{stub}"))
            .await
        {
            println!("note: could not delete stale stub {stub}: {e:?}");
        }
    }

    // ── Enqueue ingest ──────────────────────────────────
    let task_id = if args.no_enqueue {
        None
    } else {
        let ing: IngestClient = establish_for_url(&vox_url).await?;
        let task = ing
            .enqueue_ingest(
                args.wiki_id.clone(),
                r.path.clone(),
                wiki_proto::ingest::SourceChange::Created,
            )
            .await
            .map_err(|e| eyre::eyre!("enqueue_ingest: {e:?}"))?;
        Some(task.id)
    };

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "path": r.path,
                "sha256": r.sha256,
                "size": r.size,
                "title": r.title,
                "ingest_task": task_id,
            })
        );
    } else {
        println!(
            "archived: {} ({} bytes, sha256 {})",
            r.path,
            r.size,
            &r.sha256[..12]
        );
        match task_id {
            Some(id) => println!("ingest task enqueued: {id}"),
            None => println!("ingest not enqueued (--no-enqueue)"),
        }
    }
    Ok(())
}

/// Per-org extractor-health ledger location (best-effort —
/// health is advisory and never blocks an archive).
fn archive_health_path(slug: &str) -> Option<std::path::PathBuf> {
    org_proto::DataRoot::from_env()
        .ok()
        .map(|root| root.org(slug).path().join("wiki-archive-health.json"))
}

/// Build the unarchived-stub `(Provenance, body)` for a
/// failed extraction — but ONLY for accept-fragility routes
/// (Reddit/X), where blocks are expected and a retry sweep
/// exists. Everything else keeps fail-loud semantics.
fn unarchived_stub(target: &str, error: &str) -> Option<(wiki_archive::Provenance, String)> {
    let (url, route) = wiki_archive::classify(target).ok()?;
    if !matches!(
        route,
        wiki_archive::Route::Reddit { .. } | wiki_archive::Route::Tweet { .. }
    ) {
        return None;
    }
    let canonical = wiki_archive::canonicalize(&url, &route);
    let label = wiki_archive::health::route_label(&route);
    let mut prov = wiki_archive::Provenance::new(
        target,
        target,
        canonical,
        wiki_archive::content_type_for(&route),
        "unarchived",
    );
    prov.archive_status = Some("unarchived".into());
    prov.archive_error = Some(error.to_string());
    let body = format!(
        "## Archive status\n\nNot archived yet — {error}\n\nThe `{label}` route is \
         accept-fragility tier: blocks and shape drift are expected, and the content \
         is still only a retry away. Re-run `task wiki archive retry` later \
         (cron-friendly), or re-archive this URL directly once the block clears."
    );
    Some((prov, body))
}

/// `task wiki archive health` — render the per-org ledger.
fn run_wiki_archive_health(org: Option<String>, json: bool) -> eyre::Result<()> {
    let slug = resolve_active_org(org)?;
    let path = archive_health_path(&slug)
        .ok_or_else(|| eyre::eyre!("no data root — set TASK_DATA_ROOT or run `task org init`"))?;
    let ledger = wiki_archive::health::load(&path);
    if json {
        println!("{}", serde_json::to_string_pretty(&ledger)?);
    } else {
        println!("{}", wiki_archive::health::render(&ledger));
    }
    Ok(())
}

/// `task wiki archive retry` — sweep `unarchived-*` stubs and
/// re-attempt them, throttled per route. Always exits 0 so a
/// cron line stays quiet; the summary tells the story.
async fn run_wiki_archive_retry(
    limit: Option<usize>,
    wiki_id: String,
    org: Option<String>,
    server: Option<String>,
    no_enqueue: bool,
) -> eyre::Result<()> {
    use wiki_proto::raw::ImportRawSource;
    use wiki_proto::service::ingest::IngestClient;
    use wiki_proto::service::raw_layer::RawLayerClient;

    let slug = resolve_active_org(org.clone())?;
    let vox_url = resolve_org_vox_url(server.clone(), &slug);
    let raw: RawLayerClient = establish_for_url(&vox_url).await?;
    let health_path = archive_health_path(&slug);

    let stubs: Vec<String> = raw
        .list_raw_sources(wiki_id.clone())
        .await
        .map_err(|e| eyre::eyre!("list_raw_sources: {e:?}"))?
        .into_iter()
        .map(|r| r.filename)
        .filter(|f| f.starts_with("unarchived-"))
        .collect();
    if stubs.is_empty() {
        println!("no unarchived stubs — nothing to retry");
        return Ok(());
    }
    let limit = limit.unwrap_or(usize::MAX);
    println!("{} unarchived stub(s); retrying up to {limit}", stubs.len());

    // Default single-shot args for re-extraction.
    let extract_args = WikiArchiveArgs {
        cmd: None,
        target: None,
        title: None,
        force: false,
        no_enqueue,
        yt_dlp: std::env::var("TASK_YTDLP").unwrap_or_else(|_| "yt-dlp".into()),
        pdftotext: std::env::var("TASK_PDFTOTEXT").unwrap_or_else(|_| "pdftotext".into()),
        episode: None,
        transcribe: "auto".into(),
        whisper_model: std::env::var("TASK_WHISPER_MODEL").unwrap_or_else(|_| "small".into()),
        ffmpeg: std::env::var("TASK_FFMPEG").unwrap_or_else(|_| "ffmpeg".into()),
        wiki_id: wiki_id.clone(),
        org,
        server,
        json: false,
    };

    let (mut recovered, mut still_blocked, mut skipped) = (0usize, 0usize, 0usize);
    let mut first = true;
    for stub in stubs.iter().take(limit) {
        // Pull the original URL out of the stub frontmatter.
        let bytes = match raw
            .read_raw_source(wiki_id.clone(), format!("raw/sources/{stub}"))
            .await
        {
            Ok(b) => b,
            Err(e) => {
                println!("  skip {stub}: read failed: {e:?}");
                skipped += 1;
                continue;
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        let Some(source_url) = text
            .lines()
            .find_map(|l| l.strip_prefix("source_url:"))
            .map(|v| v.trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
        else {
            println!("  skip {stub}: no source_url in frontmatter");
            skipped += 1;
            continue;
        };

        // Throttle BETWEEN requests, per route — Reddit
        // tolerates ~10/min anonymously.
        let route = wiki_archive::classify(&source_url).ok().map(|(_, r)| r);
        if !first {
            let pause = match route {
                Some(wiki_archive::Route::Reddit { .. }) => {
                    wiki_archive::reddit::MIN_REQUEST_INTERVAL
                }
                _ => std::time::Duration::from_secs(1),
            };
            tokio::time::sleep(pause).await;
        }
        first = false;

        let label = route.as_ref().map(wiki_archive::health::route_label);
        match archive_extract_url(&source_url, &extract_args).await {
            Ok((prov, body)) => {
                if let (Some(p), Some(label)) = (&health_path, label) {
                    wiki_archive::health::record(p, label, Ok(()));
                }
                let md = wiki_archive::compose_source_markdown(&prov, &body);
                let r = raw
                    .import_raw_source(
                        wiki_id.clone(),
                        ImportRawSource {
                            filename: prov.filename(),
                            mime: "text/markdown".into(),
                            title: prov.title.clone(),
                            bytes: md.into_bytes(),
                            auto_enqueue: false,
                        },
                    )
                    .await
                    .map_err(|e| eyre::eyre!("import_raw_source {source_url}: {e:?}"))?;
                if !no_enqueue {
                    let ing: IngestClient = establish_for_url(&vox_url).await?;
                    ing.enqueue_ingest(
                        wiki_id.clone(),
                        r.path.clone(),
                        wiki_proto::ingest::SourceChange::Created,
                    )
                    .await
                    .map_err(|e| eyre::eyre!("enqueue_ingest {}: {e:?}", r.path))?;
                }
                if let Err(e) = raw
                    .delete_raw_source(wiki_id.clone(), format!("raw/sources/{stub}"))
                    .await
                {
                    println!("  note: stub {stub} not deleted: {e:?}");
                }
                println!("  ✓ {source_url} → {}", r.path);
                recovered += 1;
            }
            Err(e) => {
                if let (Some(p), Some(label)) = (&health_path, label) {
                    wiki_archive::health::record(p, label, Err(&e.to_string()));
                }
                println!("  ✗ {source_url}: still blocked ({e})");
                still_blocked += 1;
            }
        }
    }
    println!(
        "retry sweep: {recovered} recovered, {still_blocked} still blocked, {skipped} skipped \
         (of {})",
        stubs.len()
    );
    Ok(())
}

/// Route a URL to its extractor and run it. Returns the
/// provenance + extracted markdown body.
async fn archive_extract_url(
    target: &str,
    args: &WikiArchiveArgs,
) -> eyre::Result<(wiki_archive::Provenance, String)> {
    let title_override = args.title.clone();
    let yt_dlp = args.yt_dlp.as_str();
    let (url, route) = wiki_archive::classify(target).map_err(|e| eyre::eyre!("{e}"))?;
    let canonical = wiki_archive::canonicalize(&url, &route);
    let content_type = wiki_archive::content_type_for(&route);
    match route {
        wiki_archive::Route::Article => {
            let client = wiki_archive::article::http_client().map_err(|e| eyre::eyre!("{e}"))?;
            // Fetch once as bytes: extensionless PDF links
            // (arxiv `/pdf/…`, DOI redirects) divert to the
            // PDF extractor on content-type or magic bytes.
            let (ct, bytes) = wiki_archive::article::fetch_bytes(
                &client,
                url.as_str(),
                "text/html,application/xhtml+xml,application/pdf;q=0.9,*/*;q=0.8",
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
            if ct == "application/pdf" || bytes.starts_with(b"%PDF-") {
                return archive_pdf_from_bytes(target, canonical, args, &bytes).await;
            }
            let html = String::from_utf8_lossy(&bytes);
            let a = wiki_archive::article::extract_article(&html, url.as_str())
                .map_err(|e| eyre::eyre!("{e}"))?;
            let title = title_override
                .or_else(|| (!a.title.is_empty()).then(|| a.title.clone()))
                .unwrap_or_else(|| target.to_string());
            let prov = wiki_archive::Provenance::new(
                title,
                target,
                canonical,
                content_type,
                "dom_smoothie",
            );
            let body = match &a.byline {
                Some(byline) => format!("_{byline}_\n\n{}", a.markdown),
                None => a.markdown,
            };
            Ok((prov, body))
        }
        wiki_archive::Route::Pdf => {
            let client = wiki_archive::article::http_client().map_err(|e| eyre::eyre!("{e}"))?;
            let (_ct, bytes) = wiki_archive::article::fetch_bytes(
                &client,
                url.as_str(),
                "application/pdf,*/*;q=0.8",
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
            archive_pdf_from_bytes(target, canonical, args, &bytes).await
        }
        wiki_archive::Route::GoogleDoc { doc_id } => {
            let client = wiki_archive::article::http_client().map_err(|e| eyre::eyre!("{e}"))?;
            let (doc_title, md) = wiki_archive::article::fetch_google_doc(&client, &doc_id)
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
            let title = title_override.unwrap_or(doc_title);
            let prov = wiki_archive::Provenance::new(
                title,
                target,
                canonical,
                content_type,
                "gdocs-export",
            );
            Ok((prov, md))
        }
        wiki_archive::Route::ApplePodcast {
            podcast_id,
            episode_id,
        } => {
            let client = wiki_archive::article::http_client().map_err(|e| eyre::eyre!("{e}"))?;
            let show = wiki_archive::podcast::apple_lookup_feed(&client, &podcast_id)
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
            // Episode links need the entity=podcastEpisode
            // listing — a direct lookup of the episode id
            // returns 0 results.
            let episode_hint = match (&episode_id, &args.episode) {
                (Some(ep), _) => {
                    let hint =
                        wiki_archive::podcast::apple_lookup_episode_title(&client, &podcast_id, ep)
                            .await
                            .map_err(|e| eyre::eyre!("{e}"))?;
                    if hint.is_none() {
                        println!(
                            "note: episode id {ep} not in the show's latest 200 — archiving the latest episode instead (pass --episode <title> to pick)"
                        );
                    }
                    hint
                }
                (None, Some(title)) => Some(title.clone()),
                (None, None) => None,
            };
            archive_podcast_from_feed(
                target,
                canonical,
                args,
                &client,
                &show.feed_url,
                show.show_title.as_deref(),
                episode_hint.as_deref(),
            )
            .await
        }
        wiki_archive::Route::SpotifyPodcast { .. } => {
            let client = wiki_archive::article::http_client().map_err(|e| eyre::eyre!("{e}"))?;
            let oembed_title = wiki_archive::podcast::spotify_oembed_title(&client, target)
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
            // Podcast Index title-search: the only route from a
            // Spotify URL back to public RSS. Best-effort.
            let pi_key = std::env::var("PODCASTINDEX_API_KEY")
                .ok()
                .filter(|s| !s.is_empty());
            let pi_secret = std::env::var("PODCASTINDEX_API_SECRET")
                .ok()
                .filter(|s| !s.is_empty());
            if let (Some(key), Some(secret)) = (pi_key, pi_secret) {
                match wiki_archive::podcast::podcastindex_feed_by_title(
                    &client,
                    &key,
                    &secret,
                    &oembed_title,
                )
                .await
                {
                    Ok(Some(feed_url)) => {
                        println!("podcastindex resolved a public feed: {feed_url}");
                        return archive_podcast_from_feed(
                            target,
                            canonical,
                            args,
                            &client,
                            &feed_url,
                            None,
                            Some(&oembed_title),
                        )
                        .await;
                    }
                    Ok(None) => println!(
                        "podcastindex: no feed matched `{oembed_title}` — falling back to metadata-only"
                    ),
                    Err(e) => {
                        println!(
                            "podcastindex lookup failed ({e}) — falling back to metadata-only"
                        );
                    }
                }
            }
            // Honest metadata-only archive: Spotify exposes no
            // public audio stream or transcript.
            let item = wiki_archive::feed::FeedItem {
                title: oembed_title.clone(),
                ..Default::default()
            };
            let body = wiki_archive::podcast::render_podcast_markdown(
                None,
                &item,
                &[],
                Some(
                    "Spotify-exclusive: Spotify exposes no public audio stream or \
                     transcript for this episode, so only metadata could be archived. \
                     If the show also publishes a public RSS feed, archive its Apple \
                     Podcasts page or feed URL instead — or set PODCASTINDEX_API_KEY / \
                     PODCASTINDEX_API_SECRET so Task can resolve it by title.",
                ),
            );
            let title = args.title.clone().unwrap_or(oembed_title);
            let mut prov = wiki_archive::Provenance::new(
                title,
                target,
                canonical,
                content_type,
                "spotify-oembed",
            );
            prov.media = Some(target.to_string());
            Ok((prov, body))
        }
        wiki_archive::Route::Reddit { permalink } => {
            // Dedicated client: Reddit 403s self-identifying
            // UAs (no loid cookie) — see reddit.rs.
            let client = wiki_archive::reddit::client().map_err(|e| eyre::eyre!("{e}"))?;
            let thread = wiki_archive::reddit::fetch_thread(&client, &permalink)
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
            let body = wiki_archive::reddit::render_reddit_markdown(&thread);
            let title = title_override.unwrap_or_else(|| thread.title.clone());
            let prov = wiki_archive::Provenance::new(
                title,
                target,
                canonical,
                content_type,
                "reddit-json",
            );
            Ok((prov, body))
        }
        wiki_archive::Route::Tweet { status_id } => {
            let client = wiki_archive::article::http_client().map_err(|e| eyre::eyre!("{e}"))?;
            let result = wiki_archive::x::fetch_tweet(&client, &status_id)
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
            let body = wiki_archive::x::render_tweet_markdown(&result.tweet, target);
            let title = title_override.unwrap_or_else(|| {
                let head: String = result.tweet.text.chars().take(60).collect();
                match &result.tweet.author_handle {
                    Some(h) => format!("@{h}: {head}"),
                    None => head,
                }
            });
            // `extractor:` records which ladder rung answered
            // — fragility honesty for later debugging.
            let prov =
                wiki_archive::Provenance::new(title, target, canonical, content_type, result.rung);
            Ok((prov, body))
        }
        wiki_archive::Route::YouTube { .. } | wiki_archive::Route::Video => {
            let yt = wiki_archive::youtube::YtDlp::new(yt_dlp);
            let meta = yt.probe(target).await.map_err(|e| eyre::eyre!("{e}"))?;
            let blocks = match &meta.json3_track {
                Some((lang, track_url)) => {
                    let client =
                        wiki_archive::article::http_client().map_err(|e| eyre::eyre!("{e}"))?;
                    let json3 =
                        wiki_archive::article::fetch_text(&client, track_url, "application/json")
                            .await
                            .map_err(|e| eyre::eyre!("subtitle track ({lang}): {e}"))?;
                    let cues = wiki_archive::youtube::parse_json3_cues(&json3)
                        .map_err(|e| eyre::eyre!("{e}"))?;
                    wiki_archive::youtube::coalesce_cues(
                        &cues,
                        wiki_archive::youtube::DEFAULT_BLOCK_SECS,
                    )
                }
                None => Vec::new(),
            };
            let body = wiki_archive::youtube::render_video_markdown(&meta, &blocks);
            let title = title_override.unwrap_or_else(|| meta.title.clone());
            let mut prov = wiki_archive::Provenance::new(
                title,
                target,
                canonical.clone(),
                content_type,
                "yt-dlp",
            );
            // `media:` = what the SourceViewer should embed.
            prov.media = Some(canonical);
            prov.duration_secs = meta.duration_secs;
            Ok((prov, body))
        }
    }
}

/// PDF bytes → per-page text with `^p<page>` anchors.
/// Engine order: pdfium (feature `pdf`, dynamic libpdfium
/// binding) then `pdftotext -layout`. Title precedence:
/// `--title` → PDF metadata title → URL filename stem.
async fn archive_pdf_from_bytes(
    target: &str,
    canonical: String,
    args: &WikiArchiveArgs,
    bytes: &[u8],
) -> eyre::Result<(wiki_archive::Provenance, String)> {
    let (text, engine) = wiki_archive::pdf::extract_pdf(bytes, &args.pdftotext)
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;
    let title = args
        .title
        .clone()
        .or_else(|| text.title.clone())
        .unwrap_or_else(|| {
            // Last path segment without the extension.
            target
                .rsplit('/')
                .next()
                .unwrap_or(target)
                .split('?')
                .next()
                .unwrap_or(target)
                .trim_end_matches(".pdf")
                .trim_end_matches(".PDF")
                .to_string()
        });
    let page_count = text.pages.len();
    let body = wiki_archive::pdf::render_pdf_markdown(&text.pages);
    let body = format!("_{page_count} page(s)_\n\n{body}");
    let prov = wiki_archive::Provenance::new(title, target, canonical, "pdf", engine);
    Ok((prov, body))
}

/// Archive one podcast episode from its RSS feed: pick the
/// episode, resolve a transcript (feed `<podcast:transcript>`
/// tag fast path → Groq backfill → local whisper, governed by
/// `--transcribe`), render `^t<sec>`-anchored markdown.
async fn archive_podcast_from_feed(
    target: &str,
    canonical: String,
    args: &WikiArchiveArgs,
    client: &reqwest::Client,
    feed_url: &str,
    show_title: Option<&str>,
    episode_hint: Option<&str>,
) -> eyre::Result<(wiki_archive::Provenance, String)> {
    let xml = wiki_archive::article::fetch_text(client, feed_url, "application/rss+xml,*/*")
        .await
        .map_err(|e| eyre::eyre!("feed {feed_url}: {e}"))?;
    let feed = wiki_archive::feed::parse_feed(&xml).map_err(|e| eyre::eyre!("{e}"))?;
    let show_title = show_title.or(if feed.title.is_empty() {
        None
    } else {
        Some(&feed.title)
    });
    let item = wiki_archive::feed::pick_episode(&feed, episode_hint).ok_or_else(|| {
        let sample: Vec<&str> = feed
            .items
            .iter()
            .take(5)
            .map(|i| i.title.as_str())
            .collect();
        eyre::eyre!(
            "no episode matched `{}` — recent episodes: {}",
            episode_hint.unwrap_or("<latest>"),
            sample.join(" | ")
        )
    })?;

    let (cues, extractor, note) = podcast_transcript_cues(args, client, item).await?;
    let blocks =
        wiki_archive::youtube::coalesce_cues(&cues, wiki_archive::youtube::DEFAULT_BLOCK_SECS);
    let body =
        wiki_archive::podcast::render_podcast_markdown(show_title, item, &blocks, note.as_deref());
    let title = args.title.clone().unwrap_or_else(|| item.title.clone());
    let mut prov = wiki_archive::Provenance::new(title, target, canonical, "podcast", extractor);
    // `media:` = the playable enclosure — the SourceViewer's
    // audio player + seek-on-anchor reads this.
    prov.media = item
        .enclosure_url
        .clone()
        .or_else(|| Some(target.to_string()));
    prov.duration_secs = item.duration_secs;
    Ok((prov, body))
}

/// Transcript ladder for one episode, honest at every rung.
/// Returns `(cues, extractor-label, note-when-empty)`.
async fn podcast_transcript_cues(
    args: &WikiArchiveArgs,
    client: &reqwest::Client,
    item: &wiki_archive::feed::FeedItem,
) -> eyre::Result<(Vec<wiki_archive::youtube::Cue>, String, Option<String>)> {
    let mode = args.transcribe.as_str();
    if !matches!(mode, "auto" | "tag" | "groq" | "whisper" | "none") {
        return Err(eyre::eyre!(
            "--transcribe must be auto|tag|groq|whisper|none (got `{mode}`)"
        ));
    }
    if mode == "none" {
        return Ok((
            Vec::new(),
            "podcast-feed".into(),
            Some("Transcript skipped (--transcribe none).".into()),
        ));
    }

    // ── Fast path: the feed's own transcript tag ────────
    if matches!(mode, "auto" | "tag") {
        for tref in wiki_archive::transcript::pick_transcripts(&item.transcripts) {
            let accept = if tref.mime.is_empty() {
                "*/*"
            } else {
                &tref.mime
            };
            match wiki_archive::article::fetch_text(client, &tref.url, accept).await {
                Ok(content) => {
                    match wiki_archive::transcript::parse_transcript(&content, &tref.mime) {
                        Ok(cues) => {
                            return Ok((cues, "podcast-transcript-tag".into(), None));
                        }
                        Err(e) => println!("note: transcript {} unusable: {e}", tref.url),
                    }
                }
                Err(e) => println!("note: transcript fetch {} failed: {e}", tref.url),
            }
        }
        if mode == "tag" {
            return Ok((
                Vec::new(),
                "podcast-feed".into(),
                Some(
                    "No usable transcript tag in the feed (--transcribe tag stops here; \
                     try groq or whisper)."
                        .into(),
                ),
            ));
        }
    }

    // ── Backfills need the audio itself ─────────────────
    let fetch_audio = || async {
        let url = item
            .enclosure_url
            .as_deref()
            .ok_or_else(|| eyre::eyre!("episode has no enclosure URL — nothing to transcribe"))?;
        let enc = wiki_archive::podcast::enclosure_client().map_err(|e| eyre::eyre!("{e}"))?;
        // Stacked redirect trackers are normal here; the
        // enclosure client follows a deeper chain.
        let (_ct, bytes) = wiki_archive::article::fetch_bytes(&enc, url, "audio/*,*/*")
            .await
            .map_err(|e| eyre::eyre!("enclosure {url}: {e}"))?;
        eyre::Ok(bytes)
    };

    if mode == "groq" {
        let key = std::env::var("GROQ_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| eyre::eyre!("--transcribe groq needs GROQ_API_KEY"))?;
        let audio = fetch_audio().await?;
        println!(
            "transcribing via Groq ({} MB upload)…",
            audio.len() / 1_048_576
        );
        let cues = wiki_archive::podcast::groq_transcribe(client, &key, audio, "episode.mp3")
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        return Ok((cues, "groq-whisper-large-v3-turbo".into(), None));
    }

    // mode is now `whisper` or `auto`-falling-through.
    #[cfg(feature = "whisper")]
    {
        let audio = fetch_audio().await?;
        println!(
            "transcribing locally (whisper, model `{}`)…",
            args.whisper_model
        );
        let cues = wiki_archive::whisper::transcribe_enclosure(
            client,
            &args.whisper_model,
            &args.ffmpeg,
            audio,
        )
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;
        let label = format!("whisper-rs-{}", args.whisper_model);
        Ok((cues, label, None))
    }
    #[cfg(not(feature = "whisper"))]
    {
        let _ = &args.whisper_model;
        let _ = &args.ffmpeg;
        let _ = fetch_audio; // audio only fetched when a backfill can use it
        if mode == "whisper" {
            return Err(eyre::eyre!(
                "this build has no local whisper — rebuild the CLI with `--features whisper`, \
                 or use --transcribe groq (GROQ_API_KEY)"
            ));
        }
        Ok((
            Vec::new(),
            "podcast-feed".to_string(),
            Some(
                "No transcript tag in the feed. Re-run with --transcribe groq \
                 (GROQ_API_KEY, ~$0.04/audio-hour) or a `--features whisper` build \
                 for local transcription."
                    .to_string(),
            ),
        ))
    }
}

/// `task wiki archive import <kind>` — run one importer and
/// feed every item through the same provenance + dedup +
/// import + enqueue path as a single-URL archive.
async fn run_wiki_archive_import(cmd: WikiArchiveImportCmd) -> eyre::Result<()> {
    use wiki_archive::import as imp;

    let client = wiki_archive::article::http_client().map_err(|e| eyre::eyre!("{e}"))?;
    let started_at = chrono::Utc::now();
    let (items, common, next_cursor_hint): (Vec<imp::ImportedItem>, _, Option<String>) = match cmd {
        WikiArchiveImportCmd::Readwise {
            token,
            updated_after,
            common,
        } => {
            let items = imp::readwise::fetch_export(&client, &token, updated_after.as_deref())
                .await
                .map_err(|e| eyre::eyre!("readwise: {e}"))?;
            (items, common, Some(started_at.to_rfc3339()))
        }
        WikiArchiveImportCmd::Reader {
            token,
            updated_after,
            common,
        } => {
            let items = imp::readwise::fetch_reader(&client, &token, updated_after.as_deref())
                .await
                .map_err(|e| eyre::eyre!("readwise reader: {e}"))?;
            (items, common, Some(started_at.to_rfc3339()))
        }
        WikiArchiveImportCmd::Karakeep {
            endpoint,
            token,
            common,
        } => {
            let (items, skipped) = imp::karakeep::fetch_bookmarks(&client, &endpoint, &token)
                .await
                .map_err(|e| eyre::eyre!("karakeep: {e}"))?;
            if skipped > 0 {
                println!("note: skipped {skipped} non-link bookmark(s) (text/asset)");
            }
            (items, common, None)
        }
        WikiArchiveImportCmd::Pocket { zip, common } => {
            let items = imp::pocket::import_zip(&zip).map_err(|e| eyre::eyre!("pocket: {e}"))?;
            (items, common, None)
        }
        WikiArchiveImportCmd::Bookmarks { html, common } => {
            let body = std::fs::read_to_string(&html)?;
            let items =
                imp::netscape::parse_bookmarks_html(&body).map_err(|e| eyre::eyre!("{e}"))?;
            (items, common, None)
        }
    };

    println!("{} item(s) from the importer", items.len());
    if common.dry_run {
        for item in items.iter().take(25) {
            println!("  [{}] {}  {}", item.origin, item.title, item.url);
        }
        if items.len() > 25 {
            println!("  … {} more", items.len() - 25);
        }
        println!("dry run — nothing written");
        return Ok(());
    }

    archive_imported_items(items, &common).await?;
    if let Some(cursor) = next_cursor_hint {
        println!("incremental: next run pass --updated-after {cursor}");
    }
    Ok(())
}

/// Shared importer back-half: dedup against the wiki's
/// existing sources (canonical-URL filename scan, plus
/// in-batch), import over RPC, enqueue ingest.
async fn archive_imported_items(
    items: Vec<wiki_archive::import::ImportedItem>,
    common: &WikiArchiveImportCommon,
) -> eyre::Result<()> {
    use wiki_proto::raw::ImportRawSource;
    use wiki_proto::service::ingest::IngestClient;
    use wiki_proto::service::raw_layer::RawLayerClient;

    let slug = resolve_active_org(common.org.clone())?;
    let vox_url = resolve_org_vox_url(common.server.clone(), &slug);
    let raw: RawLayerClient = establish_for_url(&vox_url).await?;
    let ing: IngestClient = establish_for_url(&vox_url).await?;

    let mut existing: Vec<String> = raw
        .list_raw_sources(common.wiki_id.clone())
        .await
        .map_err(|e| eyre::eyre!("list_raw_sources: {e:?}"))?
        .into_iter()
        .map(|r| r.filename)
        .collect();

    let limit = common.limit.unwrap_or(usize::MAX);
    let (mut imported, mut deduped, mut skipped) = (0usize, 0usize, 0usize);
    for item in &items {
        if imported >= limit {
            break;
        }
        let (prov, body) = match wiki_archive::import::item_to_source(item) {
            Ok(v) => v,
            Err(e) => {
                println!("  skip {}: {e}", item.url);
                skipped += 1;
                continue;
            }
        };
        let canon8 = prov.canon8();
        if wiki_archive::find_canonical_match(existing.iter().map(String::as_str), &canon8)
            .is_some()
        {
            deduped += 1;
            continue;
        }
        let md = wiki_archive::compose_source_markdown(&prov, &body);
        let filename = prov.filename();
        let r = raw
            .import_raw_source(
                common.wiki_id.clone(),
                ImportRawSource {
                    filename: filename.clone(),
                    mime: "text/markdown".to_string(),
                    title: prov.title.clone(),
                    bytes: md.into_bytes(),
                    auto_enqueue: false,
                },
            )
            .await
            .map_err(|e| eyre::eyre!("import_raw_source {}: {e:?}", item.url))?;
        existing.push(filename);
        if !common.no_enqueue {
            ing.enqueue_ingest(
                common.wiki_id.clone(),
                r.path.clone(),
                wiki_proto::ingest::SourceChange::Created,
            )
            .await
            .map_err(|e| eyre::eyre!("enqueue_ingest {}: {e:?}", r.path))?;
        }
        println!("  + {}", r.path);
        imported += 1;
    }
    println!(
        "imported {imported}, deduped {deduped}, skipped {skipped} (of {})",
        items.len()
    );
    if common.no_enqueue && imported > 0 {
        println!("ingest not enqueued (--no-enqueue) — `task wiki raw rescan` enqueues later");
    }
    Ok(())
}

/// Extension → MIME for local-file archives. Mirrors
/// `wiki_extract::extract_path`'s table.
fn archive_mime_for_filename(name: &str) -> String {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" => "text/markdown",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        "json" => "application/json",
        "csv" => "text/csv",
        _ => "application/octet-stream",
    }
    .to_string()
}

async fn run_wiki_ingest(cmd: WikiIngestCmd) -> eyre::Result<()> {
    use wiki_proto::service::ingest::IngestClient;
    async fn connect(url: &str) -> eyre::Result<IngestClient> {
        establish_for_url(url).await
    }
    match cmd {
        WikiIngestCmd::List {
            wiki_id,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let rows = c
                .list_ingest(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("list_ingest: {e:?}"))?;
            if json {
                println!("{rows:#?}");
            } else {
                for t in &rows {
                    println!("{t:#?}");
                }
            }
        }
        WikiIngestCmd::Retry {
            task_id,
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let t = c
                .retry_ingest(wiki_id, task_id)
                .await
                .map_err(|e| eyre::eyre!("retry_ingest: {e:?}"))?;
            println!("retrying {t:#?}");
        }
        WikiIngestCmd::Cancel {
            task_id,
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            c.cancel_ingest(wiki_id, task_id.clone())
                .await
                .map_err(|e| eyre::eyre!("cancel_ingest: {e:?}"))?;
            println!("cancelled {task_id}");
        }
    }
    Ok(())
}

async fn run_wiki_lint_findings(cmd: WikiFindingsCmd) -> eyre::Result<()> {
    use wiki_proto::service::lint::LintClient;
    async fn connect(url: &str) -> eyre::Result<LintClient> {
        establish_for_url(url).await
    }
    match cmd {
        WikiFindingsCmd::List {
            wiki_id,
            org,
            server,
            json,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let rows = c
                .list_findings(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("list_findings: {e:?}"))?;
            if json {
                println!("{rows:#?}");
            } else {
                for f in &rows {
                    println!("{f:#?}");
                }
            }
        }
        WikiFindingsCmd::Resolve {
            finding_id,
            action,
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let parsed = match action.as_str() {
                "resolve" | "resolved" => wiki_proto::lint::FindingAction::Resolve,
                "dismiss" | "dismissed" => wiki_proto::lint::FindingAction::Dismiss {
                    reason: String::new(),
                },
                "promote-review" | "review" => wiki_proto::lint::FindingAction::PromoteToReview,
                "promote-research" | "research" => {
                    wiki_proto::lint::FindingAction::PromoteToResearch
                }
                other => {
                    return Err(eyre::eyre!(
                        "unknown action `{other}` — try resolve / dismiss / promote-review / promote-research"
                    ));
                }
            };
            c.resolve_finding(wiki_id, finding_id.clone(), parsed)
                .await
                .map_err(|e| eyre::eyre!("resolve_finding: {e:?}"))?;
            println!("{finding_id} → {action}");
        }
    }
    Ok(())
}

async fn run_wiki_review(cmd: WikiReviewCmd) -> eyre::Result<()> {
    use wiki_proto::review::{ReviewAction, ReviewItem};
    use wiki_proto::service::review::ReviewClient;
    async fn connect(url: &str) -> eyre::Result<ReviewClient> {
        establish_for_url(url).await
    }
    let read_body = |s: String| -> eyre::Result<String> {
        if s == "-" {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            Ok(buf)
        } else if std::path::Path::new(&s).exists() {
            Ok(std::fs::read_to_string(&s)?)
        } else {
            Ok(s)
        }
    };
    match cmd {
        WikiReviewCmd::List {
            wiki_id,
            org,
            server,
            json: _,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let rows: Vec<ReviewItem> = c
                .list_review(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("list_review: {e:?}"))?;
            if rows.is_empty() {
                println!("(no pending review items)");
            }
            for r in &rows {
                println!("{r:#?}");
            }
        }
        WikiReviewCmd::Apply {
            item_id,
            action,
            arg,
            body,
            wiki_id,
            org,
            server,
        } => {
            let parsed = match action.as_str() {
                "rewrite-page" => ReviewAction::RewritePage {
                    path: arg.clone(),
                    markdown: read_body(body)?,
                },
                "append-note" => ReviewAction::AppendNote {
                    path: arg.clone(),
                    body: read_body(body)?,
                },
                "research" => ReviewAction::Research { query: arg.clone() },
                other => {
                    return Err(eyre::eyre!(
                        "unknown action `{other}` — try rewrite-page / append-note / research"
                    ));
                }
            };
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            c.apply_review(wiki_id, item_id.clone(), parsed)
                .await
                .map_err(|e| eyre::eyre!("apply_review: {e:?}"))?;
            println!("applied {action} on {item_id}");
        }
    }
    Ok(())
}

async fn run_wiki_research_plans(cmd: WikiResearchCmd) -> eyre::Result<()> {
    use wiki_proto::research::ResearchStatus;
    use wiki_proto::service::research::ResearchClient;
    async fn connect(url: &str) -> eyre::Result<ResearchClient> {
        establish_for_url(url).await
    }
    match cmd {
        WikiResearchCmd::List {
            wiki_id,
            org,
            server,
            json: _,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let rows = c
                .list_research(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("list_research: {e:?}"))?;
            if rows.is_empty() {
                println!("(no research plans)");
            }
            for p in &rows {
                println!("{p:#?}");
            }
        }
        WikiResearchCmd::SetStatus {
            plan_id,
            status,
            wiki_id,
            org,
            server,
        } => {
            let parsed = match status.to_ascii_lowercase().as_str() {
                "proposed" => ResearchStatus::Proposed,
                "running" => ResearchStatus::Running,
                "awaiting" => ResearchStatus::Awaiting,
                "submitted" => ResearchStatus::Submitted,
                "cancelled" | "canceled" => ResearchStatus::Cancelled,
                other => {
                    return Err(eyre::eyre!(
                        "unknown status `{other}` — try proposed / running / awaiting / submitted / cancelled"
                    ));
                }
            };
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            c.set_research_status(wiki_id, plan_id.clone(), parsed)
                .await
                .map_err(|e| eyre::eyre!("set_research_status: {e:?}"))?;
            println!("{plan_id} → {status}");
        }
    }
    Ok(())
}

async fn run_wiki_watch(cmd: WikiWatchCmd) -> eyre::Result<()> {
    use wiki_proto::service::watcher::WatcherClient;
    async fn connect(url: &str) -> eyre::Result<WatcherClient> {
        establish_for_url(url).await
    }
    match cmd {
        WikiWatchCmd::On {
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let r = c
                .set_watch(wiki_id, true)
                .await
                .map_err(|e| eyre::eyre!("set_watch: {e:?}"))?;
            println!("watch enabled: {r}");
        }
        WikiWatchCmd::Off {
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let r = c
                .set_watch(wiki_id, false)
                .await
                .map_err(|e| eyre::eyre!("set_watch: {e:?}"))?;
            println!("watch disabled: {r}");
        }
        WikiWatchCmd::Status {
            wiki_id,
            org,
            server,
        } => {
            let slug = resolve_active_org(org)?;
            let u = resolve_org_vox_url(server, &slug);
            let c = connect(&u).await?;
            let r = c
                .is_watching(wiki_id)
                .await
                .map_err(|e| eyre::eyre!("is_watching: {e:?}"))?;
            println!("watching: {r}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_mime_for_filename_maps_the_known_extensions() {
        assert_eq!(archive_mime_for_filename("notes.md"), "text/markdown");
        assert_eq!(archive_mime_for_filename("notes.markdown"), "text/markdown");
        assert_eq!(archive_mime_for_filename("page.HTML"), "text/html");
        assert_eq!(archive_mime_for_filename("page.htm"), "text/html");
        assert_eq!(archive_mime_for_filename("paper.pdf"), "application/pdf");
        assert_eq!(archive_mime_for_filename("shot.JPEG"), "image/jpeg");
        assert_eq!(archive_mime_for_filename("shot.jpg"), "image/jpeg");
        assert_eq!(archive_mime_for_filename("data.csv"), "text/csv");
    }

    #[test]
    fn archive_mime_for_filename_falls_back_to_octet_stream() {
        // No extension, an unknown one, and a dotfile (whose "name"
        // after the last dot is the whole thing) must all be storable
        // rather than erroring.
        assert_eq!(
            archive_mime_for_filename("README"),
            "application/octet-stream"
        );
        assert_eq!(
            archive_mime_for_filename("archive.tar.zst"),
            "application/octet-stream"
        );
        assert_eq!(
            archive_mime_for_filename(".gitignore"),
            "application/octet-stream"
        );
    }

    #[test]
    fn archive_mime_for_filename_uses_only_the_final_extension() {
        // `.tar.gz`-style names must not match on an inner segment.
        assert_eq!(archive_mime_for_filename("report.pdf.zip"), "application/zip");
        assert_eq!(archive_mime_for_filename("a.zip.pdf"), "application/pdf");
    }

    #[test]
    fn route_vault_prefers_an_existing_local_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        match route_vault(dir.path()).expect("route") {
            VaultRoute::Local(p) => {
                assert_eq!(p, dir.path().canonicalize().unwrap());
            }
            VaultRoute::Vox(url) => panic!("existing dir must route local, got {url}"),
        }
    }
}
