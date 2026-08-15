//! Orchestration helpers — sequences agent turns with
//! `wiki_live::WikiLive` writes to run whole pipelines.
//!
//! Today: ingest. Lint / dedup / research follow the same
//! pattern once their parser bodies land.
//!
//! Bridge takes a concrete `&CodexBackend` for now;
//! once a second backend (Hermes) lands the signature
//! generalizes to `A: Sessions + TurnDispatch +
//! Subscriptions`.

use std::collections::HashMap;
use std::time::Duration;

use agent_codex::{ChatOpts, CodexBackend};
use agent_proto::event::AgentEvent;
use chrono::Utc;
use futures::StreamExt;
use wiki_live::WikiLive;

use crate::error::AgentWikiError;
use crate::parsers::{IngestBlocks, parse_ingest_blocks};
use crate::prompts::{self, INGEST_ANALYZE_SYSTEM, INGEST_GENERATE_SYSTEM, language_directive};

/// Input to one ingest run.
#[derive(Debug, Clone)]
pub struct IngestRequest {
    /// Filename to record under `Wiki/raw/sources/`.
    pub source_filename: String,
    pub source_mime: String,
    /// Optional title (used in log + analysis prompt).
    pub source_title: String,
    pub source_bytes: Vec<u8>,
    /// Model id (`gpt-5.4-mini`, `o3`, ...). `None` ⇒
    /// daemon default.
    pub model: Option<String>,
    /// Per-turn timeout. Default 5 minutes.
    pub timeout: Duration,
    /// Output language (`English`, `中文`, ...).
    pub language: String,
}

impl IngestRequest {
    pub fn new(filename: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            source_filename: filename.into(),
            source_mime: "text/markdown".to_string(),
            source_title: String::new(),
            source_bytes: bytes,
            model: None,
            timeout: Duration::from_secs(300),
            language: "English".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IngestRunResult {
    pub task_id: String,
    pub raw_source_path: String,
    pub pages_written: Vec<String>,
    pub reviews_raised: Vec<String>,
    pub analysis: String,
}

/// Drive one full ingest pipeline (two-step `CoT`) against
/// `codex app-server`.
pub async fn run_ingest(
    backend: &CodexBackend,
    wiki: &WikiLive,
    req: IngestRequest,
) -> Result<IngestRunResult, AgentWikiError> {
    wiki.bootstrap()
        .map_err(|e| AgentWikiError::Bridge(format!("bootstrap: {e}")))?;

    let raw_ref = wiki
        .import_raw_source(wiki_proto::raw::ImportRawSource {
            filename: req.source_filename.clone(),
            mime: req.source_mime.clone(),
            title: req.source_title.clone(),
            bytes: req.source_bytes.clone(),
            auto_enqueue: false,
        })
        .map_err(|e| AgentWikiError::Bridge(format!("import_raw_source: {e}")))?;

    let task = wiki
        .enqueue_ingest(
            &raw_ref.path,
            wiki_live::queue::SourceChange::Created,
            &req.source_bytes,
        )
        .map_err(|e| AgentWikiError::Bridge(format!("enqueue_ingest: {e}")))?;

    let claimed = wiki
        .claim_next_ingest()
        .map_err(|e| AgentWikiError::Bridge(format!("claim_next_ingest: {e}")))?
        .ok_or_else(|| AgentWikiError::Bridge("claim_next_ingest returned None".into()))?;
    debug_assert_eq!(claimed.id, task.id);

    let ctx = wiki
        .read_context()
        .map_err(|e| AgentWikiError::Bridge(format!("read_context: {e}")))?;

    // Archived timestamped media (provenance frontmatter
    // `content_type: video|audio` from `task wiki archive`)
    // get the `^t<sec>` anchor-citation directive appended
    // to BOTH turns: chaptered source page, [mm:ss] deep
    // links, curator-owned ## Notes preservation.
    let archive_directive = sniff_media_content_type(&req.source_bytes)
        .map(|ct| prompts::archive_media_directive(&req.source_filename, &ct));

    // ── Step 1: analyze ────────────────────────────────
    let lang = language_directive(&req.language);
    let mut vars1 = HashMap::new();
    vars1.insert("language_directive", lang.as_str());
    vars1.insert("wiki_purpose", ctx.purpose_markdown.as_str());
    vars1.insert("wiki_index", ctx.index_markdown.as_str());
    let mut analyze_system = prompts::render(INGEST_ANALYZE_SYSTEM, &vars1);
    if let Some(d) = &archive_directive {
        analyze_system.push_str("\n\n");
        analyze_system.push_str(d);
    }

    let source_text = String::from_utf8_lossy(&req.source_bytes).to_string();
    let user1 = format!(
        "Source filename: {}\n\nSource content:\n\n{source_text}",
        req.source_filename
    );
    let full_msg_1 = format!("{analyze_system}\n\n---\n\n{user1}");
    let analysis = drive_one_turn(backend, wiki, &req, &full_msg_1).await?;

    wiki.record_analysis(&task.id, analysis.clone())
        .map_err(|e| AgentWikiError::Bridge(format!("record_analysis: {e}")))?;

    // ── Step 2: generate ───────────────────────────────
    let source_basename = req
        .source_filename
        .rsplit_once('.')
        .map_or(req.source_filename.as_str(), |(stem, _)| stem);
    let mut vars2 = HashMap::new();
    vars2.insert("language_directive", lang.as_str());
    vars2.insert("source_filename", req.source_filename.as_str());
    vars2.insert("source_basename", source_basename);
    vars2.insert("wiki_schema", ctx.schema_markdown.as_str());
    vars2.insert("wiki_purpose", ctx.purpose_markdown.as_str());
    vars2.insert("wiki_index", ctx.index_markdown.as_str());
    vars2.insert("wiki_overview", ctx.overview_markdown.as_str());
    let mut generate_system = prompts::render(INGEST_GENERATE_SYSTEM, &vars2);
    if let Some(d) = &archive_directive {
        generate_system.push_str("\n\n");
        generate_system.push_str(d);
    }

    let user2 = format!(
        "Analysis:\n\n{analysis}\n\nNow emit FILE/REVIEW blocks per the rules. \
         Remember: the FIRST character of your response must be `-` (start of `---FILE:`)."
    );
    let full_msg_2 = format!("{generate_system}\n\n---\n\n{user2}");
    let generation = drive_one_turn(backend, wiki, &req, &full_msg_2).await?;

    let blocks: IngestBlocks = parse_ingest_blocks(&generation)?;

    // Source traceability: inject `sources: ["<raw_path>"]`
    // into each generated page's YAML frontmatter so every
    // wiki page traces back to the raw file(s) that produced
    // it (matches nashsu/llm_wiki's `sources:` field).
    let mut drafts: Vec<wiki_live::queue::PageDraft> = blocks
        .files
        .iter()
        .map(|fb| wiki_live::queue::PageDraft {
            path: fb.path.clone(),
            markdown: inject_sources_frontmatter(&fb.content, &raw_ref.path),
            overwrite: true,
        })
        .collect();

    // Guaranteed source-summary fallback (matches
    // nashsu/llm_wiki's "always create wiki/sources/<slug>.md
    // even if the LLM omits it" behavior). Two guards:
    //   1. Skip if the LLM already named the page in its
    //      FILE blocks (drafts already covers it).
    //   2. Skip if the page already exists on disk — Codex
    //      may have written it directly via tool use during
    //      the turn, in which case we should NOT clobber with
    //      a stub.
    // Without these, a mis-formatted LLM response can drop
    // the source pointer and downstream sweeps eventually
    // reap orphans.
    let summary_path = format!("wiki/sources/{source_basename}.md");
    let summary_exists_on_disk = wiki.vault_root().join(&summary_path).exists();
    if !drafts.iter().any(|d| d.path == summary_path) && !summary_exists_on_disk {
        let title = if req.source_title.is_empty() {
            req.source_filename.clone()
        } else {
            req.source_title.clone()
        };
        let stub = format!(
            "---\ntype: source\ntitle: \"Source: {title}\"\nsources:\n  - {raw_path}\n---\n\n# Source: {title}\n\n*The LLM did not emit a source-summary page on this ingest run; this stub is a fallback so the source has a reachable wiki page.*\n",
            raw_path = raw_ref.path,
        );
        drafts.push(wiki_live::queue::PageDraft {
            path: summary_path,
            markdown: stub,
            overwrite: false,
        });
    }
    wiki.record_pages(&task.id, &drafts)
        .map_err(|e| AgentWikiError::Bridge(format!("record_pages: {e}")))?;

    let title = if req.source_title.is_empty() {
        req.source_filename.clone()
    } else {
        req.source_title.clone()
    };
    wiki.append_log(wiki_live::log_md::LogEntry {
        at: Utc::now(),
        op: wiki_live::log_md::LogOp::Ingest,
        title,
        body: format!(
            "Ingested via agent-wiki bridge.\n\n- Source: `{}`\n- Pages: {}\n- Reviews: {}",
            raw_ref.path,
            blocks.files.len(),
            blocks.reviews.len()
        ),
        pages_touched: blocks.files.iter().map(|f| f.path.clone()).collect(),
    })
    .map_err(|e| AgentWikiError::Bridge(format!("append_log: {e}")))?;

    wiki.rebuild_index()
        .map_err(|e| AgentWikiError::Bridge(format!("rebuild_index: {e}")))?;

    wiki.complete_ingest(&task.id)
        .map_err(|e| AgentWikiError::Bridge(format!("complete_ingest: {e}")))?;

    Ok(IngestRunResult {
        task_id: task.id,
        raw_source_path: raw_ref.path,
        pages_written: blocks.files.into_iter().map(|f| f.path).collect(),
        reviews_raised: blocks.reviews.into_iter().map(|r| r.title).collect(),
        analysis,
    })
}

/// Drive one Codex turn and return the full assistant
/// response (concatenated `MessageDelta`s) when the turn
/// completes.
async fn drive_one_turn(
    backend: &CodexBackend,
    wiki: &WikiLive,
    req: &IngestRequest,
    user_text: &str,
) -> Result<String, AgentWikiError> {
    let opts = ChatOpts {
        codex_bin: None,
        codex_args: None,
        codex_home: None,
        model: req.model.clone(),
        effort: None,
        access_mode: Some("current".to_string()),
    };
    let handle = backend
        .chat(wiki.vault_root().to_path_buf(), user_text.to_string(), opts)
        .await
        .map_err(|e| AgentWikiError::Bridge(format!("backend.chat: {e}")))?;
    let mut events = handle.events;
    let mut out = String::new();
    let deadline = tokio::time::Instant::now() + req.timeout;
    loop {
        match tokio::time::timeout_at(deadline, events.next()).await {
            Err(_) => {
                return Err(AgentWikiError::Bridge(format!(
                    "turn timed out after {}s",
                    req.timeout.as_secs()
                )));
            }
            Ok(None) => break,
            Ok(Some(AgentEvent::MessageDelta { content_delta, .. })) => {
                out.push_str(&content_delta);
            }
            Ok(Some(AgentEvent::TurnFinished { .. })) => break,
            Ok(Some(AgentEvent::TurnErrored { kind, message, .. })) => {
                return Err(AgentWikiError::Bridge(format!(
                    "turn errored ({kind}): {message}"
                )));
            }
            Ok(Some(_)) => {}
        }
    }
    Ok(out)
}

// ────────────────────── Lint ──────────────────────

#[derive(Debug, Clone)]
pub struct LintRequest {
    pub model: Option<String>,
    pub timeout: Duration,
    pub language: String,
}

impl Default for LintRequest {
    fn default() -> Self {
        Self {
            model: None,
            timeout: Duration::from_secs(180),
            language: "English".to_string(),
        }
    }
}

/// Run one semantic-lint pass. Reads page summaries (title +
/// first ~500 chars), drives the LLM through
/// `LINT_SEMANTIC_SYSTEM`, parses the LINT blocks,
/// persists newly-raised findings.
pub async fn run_lint(
    backend: &CodexBackend,
    wiki: &WikiLive,
    req: LintRequest,
) -> Result<Vec<wiki_live::LintFinding>, AgentWikiError> {
    use crate::parsers::{LintBlockKind, LintSeverity, parse_lint_blocks};

    let pages = collect_page_summaries(wiki)?;
    let lang = language_directive(&req.language);
    let mut vars = HashMap::new();
    vars.insert("language_directive", lang.as_str());
    vars.insert("page_summaries", pages.as_str());
    let system = prompts::render(prompts::LINT_SEMANTIC_SYSTEM, &vars);

    let opts = ChatOpts {
        codex_bin: None,
        codex_args: None,
        codex_home: None,
        model: req.model.clone(),
        effort: None,
        access_mode: Some("read-only".to_string()),
    };
    let resp = drive_turn_text(backend, wiki, &system, &opts, req.timeout).await?;
    let blocks = parse_lint_blocks(&resp)?;

    let now = chrono::Utc::now();
    let items = blocks.into_iter().map(|b| wiki_live::LintFinding {
        id: String::new(),
        kind: match b.kind {
            LintBlockKind::Contradiction => wiki_live::LintKind::Contradiction,
            LintBlockKind::Stale => wiki_live::LintKind::Stale,
            LintBlockKind::MissingPage => wiki_live::LintKind::MissingPage,
            LintBlockKind::Suggestion => wiki_live::LintKind::Suggestion,
        },
        severity: match b.severity {
            LintSeverity::Warning => wiki_live::LintSeverity::Warning,
            LintSeverity::Info => wiki_live::LintSeverity::Info,
        },
        title: b.title,
        description: b.description,
        pages: b.pages,
        status: wiki_live::FindingStatus::Open,
        raised_at: now,
        resolved_at: None,
    });
    let raised = wiki
        .raise_findings(items)
        .map_err(|e| AgentWikiError::Bridge(format!("raise_findings: {e}")))?;
    wiki.append_log(wiki_live::log_md::LogEntry {
        at: now,
        op: wiki_live::log_md::LogOp::Lint,
        title: format!("Lint pass — {} new finding(s)", raised.len()),
        body: String::new(),
        pages_touched: Vec::new(),
    })
    .map_err(|e| AgentWikiError::Bridge(format!("append_log: {e}")))?;
    Ok(raised)
}

// ────────────────────── Deep research ──────────────────────

/// Propose a research plan for one knowledge gap. Returns
/// the parsed `TOPIC: …` + queries; caller dispatches the
/// actual web search.
#[allow(clippy::too_many_arguments)]
pub async fn run_propose_research(
    backend: &CodexBackend,
    wiki: &WikiLive,
    gap_kind: &str,
    gap_title: &str,
    gap_description: &str,
    model: Option<String>,
    timeout: Duration,
    language: &str,
) -> Result<crate::parsers::ResearchTopicPlan, AgentWikiError> {
    let ctx = wiki
        .read_context()
        .map_err(|e| AgentWikiError::Bridge(format!("read_context: {e}")))?;
    let lang = language_directive(language);
    let mut vars = HashMap::new();
    vars.insert("language_directive", lang.as_str());
    vars.insert("wiki_purpose", ctx.purpose_markdown.as_str());
    vars.insert("wiki_overview", ctx.overview_markdown.as_str());
    vars.insert("gap_type", gap_kind);
    vars.insert("gap_title", gap_title);
    vars.insert("gap_description", gap_description);
    let system = prompts::render(prompts::OPTIMIZE_RESEARCH_SYSTEM, &vars);
    let opts = ChatOpts {
        codex_bin: None,
        codex_args: None,
        codex_home: None,
        model,
        effort: None,
        access_mode: Some("read-only".to_string()),
    };
    let resp = drive_turn_text(backend, wiki, &system, &opts, timeout).await?;
    crate::parsers::parse_research_plan(&resp)
}

// ────────────────────── Sweep reviews ──────────────────────

#[derive(Debug, Clone)]
pub struct ReviewSummary {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub description: String,
    pub pages: Vec<String>,
}

/// Ask the LLM which review items are now stale given the
/// current wiki state. Returns ids the curator can mark
/// resolved.
pub async fn run_sweep_reviews(
    backend: &CodexBackend,
    wiki: &WikiLive,
    pending: &[ReviewSummary],
    model: Option<String>,
    timeout: Duration,
) -> Result<Vec<String>, AgentWikiError> {
    let pages = collect_page_index_lines(wiki)?;
    let mut items_block = String::new();
    for item in pending {
        items_block.push_str(&format!(
            "- id: {}\n  kind: {}\n  title: {}\n  description: {}\n  pages: {}\n",
            item.id,
            item.kind,
            item.title,
            item.description,
            item.pages.join(", ")
        ));
    }
    let mut vars = HashMap::new();
    vars.insert("page_list", pages.as_str());
    vars.insert("review_items", items_block.as_str());
    let system = prompts::render(prompts::SWEEP_REVIEWS_SYSTEM, &vars);

    let opts = ChatOpts {
        codex_bin: None,
        codex_args: None,
        codex_home: None,
        model,
        effort: None,
        access_mode: Some("read-only".to_string()),
    };
    let resp = drive_turn_text(backend, wiki, &system, &opts, timeout).await?;
    crate::parsers::parse_sweep_resolved(&resp)
}

// ────────────────────── Dedup ──────────────────────

pub async fn run_dedup_detect(
    backend: &CodexBackend,
    wiki: &WikiLive,
    model: Option<String>,
    timeout: Duration,
) -> Result<Vec<crate::parsers::DuplicateGroup>, AgentWikiError> {
    let pages = collect_dedup_input(wiki)?;
    let mut vars = HashMap::new();
    vars.insert("pages", pages.as_str());
    let system = prompts::render(prompts::DEDUP_DETECT_SYSTEM, &vars);
    let user_appendix = format!("Page list:\n\n{pages}");

    let opts = ChatOpts {
        codex_bin: None,
        codex_args: None,
        codex_home: None,
        model,
        effort: None,
        access_mode: Some("read-only".to_string()),
    };
    let full = format!("{system}\n\n---\n\n{user_appendix}");
    let resp = drive_turn_text(backend, wiki, &full, &opts, timeout).await?;
    crate::parsers::parse_dedup_groups(&resp)
}

/// Ask the LLM to merge a duplicate group into one page.
/// Returns the resulting `(path, markdown)` ready for
/// `record_pages`. Caller does the write so this fn stays
/// pure.
pub async fn run_dedup_merge(
    backend: &CodexBackend,
    wiki: &WikiLive,
    group: &crate::parsers::DuplicateGroup,
    target_path: &str,
    model: Option<String>,
    timeout: Duration,
) -> Result<(String, String), AgentWikiError> {
    let mut inputs = String::new();
    for slug in &group.slugs {
        // Slug → try a few common locations.
        for candidate in [
            format!("Concepts/{slug}.md"),
            format!("concepts/{slug}.md"),
            format!("Entities/{slug}.md"),
            format!("entities/{slug}.md"),
            format!("{slug}.md"),
        ] {
            let abs = wiki.wiki_root().join(&candidate);
            if abs.is_file() {
                let body = std::fs::read_to_string(&abs)
                    .map_err(|e| AgentWikiError::Bridge(format!("read {candidate}: {e}")))?;
                inputs.push_str(&format!("\n### {candidate}\n\n{body}\n"));
                break;
            }
        }
    }
    let system = prompts::render(prompts::DEDUP_MERGE_SYSTEM, &HashMap::new());
    let user = format!(
        "Pages to merge:\n{inputs}\n\nReason for grouping: {}",
        group.reason
    );

    let opts = ChatOpts {
        codex_bin: None,
        codex_args: None,
        codex_home: None,
        model,
        effort: None,
        access_mode: Some("read-only".to_string()),
    };
    let full = format!("{system}\n\n---\n\n{user}");
    let merged = drive_turn_text(backend, wiki, &full, &opts, timeout).await?;
    // First char must be `-` per the prompt contract.
    let trimmed = merged.trim_start();
    if !trimmed.starts_with("---") {
        return Err(AgentWikiError::MalformedResponse(
            "dedup merge response must start with `---`",
            trimmed.chars().take(80).collect(),
        ));
    }
    Ok((target_path.to_string(), trimmed.to_string()))
}

// ────────────────────── Vision caption ──────────────────────

/// Build the vision-caption prompt for one image. The
/// caller supplies prose context excerpts (before / after
/// the image in the source); passes empty strings for the
/// no-context "pinned" variant.
///
/// Returning just the prompt — not driving the LLM —
/// keeps this layer backend-agnostic. The Codex chat
/// surface today doesn't carry image bytes through
/// `turn/start`'s `input` array end-to-end; once a vision-
/// capable backend (or a direct Anthropic-API helper)
/// lands, it consumes this prompt + the
/// `ExtractedImage::bytes` and returns the caption.
#[must_use]
pub fn vision_caption_prompt(before: &str, after: &str) -> String {
    if before.is_empty() && after.is_empty() {
        prompts::VISION_CAPTION_PINNED.to_string()
    } else {
        let mut vars = HashMap::new();
        let b = if before.is_empty() { "(none)" } else { before };
        let a = if after.is_empty() { "(none)" } else { after };
        vars.insert("before", b);
        vars.insert("after", a);
        prompts::render(prompts::VISION_CAPTION_CONTEXTUAL, &vars)
    }
}

// ────────────────────── Deepen page ──────────────────────

/// Result of a [`run_deepen`] call.
#[derive(Debug, Clone)]
pub struct DeepenResult {
    /// Page that was rewritten.
    pub page_path: String,
    /// Word count before.
    pub before_words: usize,
    /// Word count after.
    pub after_words: usize,
}

/// Rewrite a thin wiki page into a proper reference article.
/// Reads the existing page + the raw sources listed in its
/// `sources:` frontmatter, prompts the LLM to expand with code
/// examples + sharper structure, then overwrites the page.
///
/// Idempotent in the sense that re-running on an already-deep
/// page just makes it slightly better (or no-op if the LLM
/// considers it complete). Unsafe to invoke concurrently on
/// the same page from two callers.
pub async fn run_deepen(
    backend: &CodexBackend,
    wiki: &WikiLive,
    page_path: &str,
    model: Option<String>,
    timeout: Duration,
    language: &str,
) -> Result<DeepenResult, AgentWikiError> {
    let abs = wiki.vault_root().join(page_path);
    let existing = std::fs::read_to_string(&abs)
        .map_err(|e| AgentWikiError::Bridge(format!("read {page_path}: {e}")))?;
    let before_words = existing.split_whitespace().count();

    // Pull `sources:` from the frontmatter so we can include
    // the original source material in the prompt — but **cap
    // each source at ~6KB**. Earlier versions dumped entire
    // raw sources (17KB Apollo chapters), which pushed the
    // prompt past Codex's output budget and the response got
    // stream-truncated mid-FILE-block (we'd see "missing close
    // marker" errors every time). 6KB per source is enough
    // for the LLM to pull a few code examples + API signatures
    // while leaving room in the output budget for a 300-800
    // word page.
    const SOURCE_CAP_BYTES: usize = 6_000;
    let source_paths = extract_sources_field(&existing);
    let mut source_content = String::new();
    for src in &source_paths {
        let src_abs = wiki.vault_root().join("raw/sources").join(src);
        if let Ok(bytes) = std::fs::read(&src_abs) {
            if let Ok(s) = std::str::from_utf8(&bytes) {
                source_content.push_str(&format!("\n\n--- source: {src} ---\n\n"));
                if s.len() > SOURCE_CAP_BYTES {
                    // Take the head of the source — usually
                    // the most representative chunk (intro +
                    // first sections). Add a marker so the
                    // LLM knows it's not the full text.
                    source_content.push_str(&s[..SOURCE_CAP_BYTES]);
                    source_content.push_str(&format!(
                        "\n\n…[truncated; original was {} bytes, capped at {SOURCE_CAP_BYTES} for prompt budget]\n",
                        s.len()
                    ));
                } else {
                    source_content.push_str(s);
                }
            } else {
                source_content
                    .push_str(&format!("\n\n--- source: {src} (binary, skipped) ---\n\n"));
            }
        }
    }
    if source_content.is_empty() {
        source_content
            .push_str("(no raw source content available; rewrite from the existing page alone)");
    }

    let lang = language_directive(language);
    let mut vars = HashMap::new();
    vars.insert("language_directive", lang.as_str());
    vars.insert("existing_page", existing.as_str());
    vars.insert("source_content", source_content.as_str());
    let system = prompts::render(prompts::DEEPEN_PAGE_SYSTEM, &vars);

    // `read-only` so Codex CAN'T write the page via tool-use
    // — forces text-only output so we always get a parseable
    // FILE block back. With `current`, tool-use was non-
    // deterministic: sometimes Codex wrote the page,
    // sometimes it chatted, sometimes both. Read-only routes
    // every write through `parse_ingest_blocks` → on-disk
    // write below.
    let opts = ChatOpts {
        codex_bin: None,
        codex_args: None,
        codex_home: None,
        model,
        effort: None,
        access_mode: Some("read-only".to_string()),
    };
    // Snapshot the file's pre-turn state — kept as a safety
    // net in case Codex ignores `read-only` (which has
    // happened) and writes anyway. If the file changed
    // during the turn, treat that as the canonical result and
    // don't insist on a FILE block.
    let before_mtime = std::fs::metadata(&abs).and_then(|m| m.modified()).ok();
    let before_size = existing.len();

    let resp = drive_turn_text(backend, wiki, &system, &opts, timeout).await?;

    // Two success paths:
    //   1. Codex wrote the file directly (tool-use). Detect by
    //      mtime/size change. Trust the on-disk content.
    //   2. The LLM emitted a FILE block. Parse + write.
    // If neither happened, surface the prose response as an
    // error so the caller knows nothing landed.
    let after_existing = std::fs::read_to_string(&abs)
        .map_err(|e| AgentWikiError::Bridge(format!("re-read {page_path}: {e}")))?;
    let after_mtime = std::fs::metadata(&abs).and_then(|m| m.modified()).ok();
    let codex_wrote_directly = match (before_mtime, after_mtime) {
        (Some(b), Some(a)) => a > b,
        _ => after_existing.len() != before_size,
    } && after_existing != existing;

    let final_content = if codex_wrote_directly {
        after_existing
    } else {
        // Fall back to FILE-block parse. Errors here surface
        // when the LLM chatted but didn't actually write.
        let blocks = parse_ingest_blocks(&resp)?;
        let file = blocks.files.into_iter().next().ok_or_else(|| {
            AgentWikiError::Bridge("deepen: response had no FILE block".to_string())
        })?;
        // Path-match by basename — Codex often returns just the
        // file's basename (`object-safety.md`) rather than the
        // full canonical path (`wiki/concepts/object-safety.md`).
        // Accept anything whose basename matches; the WRITE
        // target is always the expected (canonical) location.
        let expected_basename = std::path::Path::new(page_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let returned_basename = std::path::Path::new(&file.path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if expected_basename != returned_basename {
            return Err(AgentWikiError::Bridge(format!(
                "deepen: LLM returned path `{}`, expected basename `{expected_basename}`",
                file.path
            )));
        }
        let target = wiki.vault_root().join(canonicalize_path(page_path));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AgentWikiError::Bridge(format!("mkdir: {e}")))?;
        }
        std::fs::write(&target, &file.content)
            .map_err(|e| AgentWikiError::Bridge(format!("write {page_path}: {e}")))?;
        file.content
    };

    let after_words = final_content.split_whitespace().count();

    wiki.append_log(wiki_live::log_md::LogEntry {
        at: Utc::now(),
        op: wiki_live::log_md::LogOp::Ingest,
        title: format!("Deepen: {page_path}"),
        body: format!("Rewrote {page_path}: {before_words}w → {after_words}w"),
        pages_touched: vec![page_path.to_string()],
    })
    .map_err(|e| AgentWikiError::Bridge(format!("append_log: {e}")))?;

    Ok(DeepenResult {
        page_path: page_path.to_string(),
        before_words,
        after_words,
    })
}

/// Pull the `sources:` array value out of a page's YAML
/// frontmatter. Returns the filenames (e.g. `apollo-ch01.md`).
fn extract_sources_field(markdown: &str) -> Vec<String> {
    let Some(rest) = markdown.strip_prefix("---\n") else {
        return Vec::new();
    };
    let Some(end) = rest.find("\n---\n") else {
        return Vec::new();
    };
    let fm = &rest[..end];
    for line in fm.lines() {
        let trimmed = line.trim_start();
        if let Some(after) = trimmed.strip_prefix("sources:") {
            let v = after.trim();
            if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                return inner
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
    }
    Vec::new()
}

/// Normalize `wiki/concepts/X.md` ↔ `concepts/X.md` ↔
/// `Wiki/concepts/X.md` to one canonical form for comparison.
fn canonicalize_path(p: &str) -> String {
    let lower_pref = if let Some(rest) = p.strip_prefix("Wiki/") {
        format!("wiki/{rest}")
    } else {
        p.to_string()
    };
    if lower_pref.starts_with("wiki/")
        || matches!(
            lower_pref.as_str(),
            "index.md" | "log.md" | "overview.md" | "schema.md" | "purpose.md"
        )
    {
        lower_pref
    } else if lower_pref.starts_with("concepts/")
        || lower_pref.starts_with("entities/")
        || lower_pref.starts_with("sources/")
    {
        format!("wiki/{lower_pref}")
    } else {
        lower_pref
    }
}

// ────────────────────── Shared helpers ──────────────────────

/// Drive one turn with an LLM and return its full text
/// (concatenated `MessageDelta`s). The session+task it
/// runs under is throwaway — Lint/Dedup/Research don't
/// need persistent session state.
async fn drive_turn_text(
    backend: &CodexBackend,
    wiki: &WikiLive,
    text: &str,
    opts: &ChatOpts,
    timeout: Duration,
) -> Result<String, AgentWikiError> {
    let handle = backend
        .chat(
            wiki.vault_root().to_path_buf(),
            text.to_string(),
            opts.clone(),
        )
        .await
        .map_err(|e| AgentWikiError::Bridge(format!("backend.chat: {e}")))?;
    let mut events = handle.events;
    let mut out = String::new();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match tokio::time::timeout_at(deadline, events.next()).await {
            Err(_) => {
                return Err(AgentWikiError::Bridge(format!(
                    "turn timed out after {}s",
                    timeout.as_secs()
                )));
            }
            Ok(None) => break,
            Ok(Some(AgentEvent::MessageDelta { content_delta, .. })) => {
                out.push_str(&content_delta);
            }
            Ok(Some(AgentEvent::TurnFinished { .. })) => break,
            Ok(Some(AgentEvent::TurnErrored { kind, message, .. })) => {
                return Err(AgentWikiError::Bridge(format!(
                    "turn errored ({kind}): {message}"
                )));
            }
            Ok(Some(_)) => {}
        }
    }
    Ok(out)
}

/// Build a compact "title — first 500 chars" listing for
/// the lint prompt.
fn collect_page_summaries(wiki: &WikiLive) -> Result<String, AgentWikiError> {
    let root = wiki.wiki_root();
    let mut out = String::new();
    const SKIP: &[&str] = &[
        wiki_proto::paths::SCHEMA_MD,
        wiki_proto::paths::PURPOSE_MD,
        wiki_proto::paths::INDEX_MD,
        wiki_proto::paths::LOG_MD,
        wiki_proto::paths::OVERVIEW_MD,
    ];
    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
    {
        let p = entry.path();
        if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let rel = p
            .strip_prefix(&root)
            .map(|r| r.to_string_lossy().to_string())
            .unwrap_or_default();
        if SKIP.contains(&rel.as_str())
            || rel.starts_with("raw/")
            || rel.starts_with("_state/")
            || rel.starts_with("media/")
        {
            continue;
        }
        let body = std::fs::read_to_string(p)
            .map_err(|e| AgentWikiError::Bridge(format!("read {rel}: {e}")))?;
        let snippet: String = body.chars().take(500).collect();
        out.push_str(&format!("\n## {rel}\n\n{snippet}\n"));
    }
    Ok(out)
}

fn collect_page_index_lines(wiki: &WikiLive) -> Result<String, AgentWikiError> {
    let root = wiki.wiki_root();
    let mut out = String::new();
    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
    {
        let p = entry.path();
        if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let rel = p
            .strip_prefix(&root)
            .map(|r| r.to_string_lossy().to_string())
            .unwrap_or_default();
        if rel.starts_with("raw/") || rel.starts_with("_state/") || rel.starts_with("media/") {
            continue;
        }
        out.push_str(&format!("- {rel}\n"));
    }
    Ok(out)
}

fn collect_dedup_input(wiki: &WikiLive) -> Result<String, AgentWikiError> {
    // Same shape as page_index_lines for now — slugs +
    // title. The LLM groups by name similarity.
    collect_page_index_lines(wiki)
}

/// Inject `sources: ["<path>"]` into a page's YAML
/// frontmatter. Idempotent — if the page already has a
/// `sources:` list, appends to it (de-duped by path).
/// If the page has no frontmatter, wraps the body in a
/// fresh `---` fence.
#[must_use]
pub(crate) fn inject_sources_frontmatter(markdown: &str, source_path: &str) -> String {
    // Match a leading `---\n...\n---\n` fence (the standard
    // YAML frontmatter convention). Anything else gets a
    // fresh fence prepended.
    let trimmed = markdown.trim_start_matches('\n');
    if let Some(rest) = trimmed.strip_prefix("---\n") {
        if let Some(end_idx) = rest.find("\n---\n") {
            let fm = &rest[..end_idx];
            let body = &rest[end_idx + 5..];
            let new_fm = merge_sources_into_fm(fm, source_path);
            return format!("---\n{new_fm}---\n{body}");
        }
    }
    // No frontmatter — wrap.
    format!("---\nsources:\n  - {source_path}\n---\n\n{markdown}")
}

/// Merge `<source_path>` into the `sources:` list inside a
/// YAML frontmatter string. Preserves all other keys.
fn merge_sources_into_fm(fm: &str, source_path: &str) -> String {
    let mut out = String::new();
    let mut in_sources_block = false;
    let mut had_sources = false;
    let mut already_present = false;
    let mut emitted_path = false;
    for line in fm.lines() {
        if in_sources_block {
            // Continuation of the sources block when the line
            // starts with `  -` or `  ` (a list item or
            // continuation). Anything else closes the block.
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix('-') {
                if rest.trim() == source_path {
                    already_present = true;
                }
                out.push_str(line);
                out.push('\n');
                continue;
            }
            if !emitted_path && !already_present {
                out.push_str(&format!("  - {source_path}\n"));
                emitted_path = true;
            }
            in_sources_block = false;
            // Fall through to write the current (non-list) line.
        }
        if line.trim_start().starts_with("sources:") {
            had_sources = true;
            in_sources_block = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    // Sources block ran to end of frontmatter.
    if in_sources_block && !emitted_path && !already_present {
        out.push_str(&format!("  - {source_path}\n"));
    }
    if !had_sources {
        out.push_str(&format!("sources:\n  - {source_path}\n"));
    }
    out
}

/// Sniff the provenance `content_type:` out of an archived
/// source's leading YAML frontmatter. Returns it only for
/// timestamped media (`video` / `audio`) — the kinds whose
/// transcripts carry `^t<sec>` anchors. Plain line scan, no
/// YAML dep: `task wiki archive` writes one `key: value` per
/// line.
fn sniff_media_content_type(source_bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(source_bytes).ok()?;
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    for line in rest[..end].lines() {
        if let Some(v) = line.strip_prefix("content_type:") {
            let v = v.trim();
            if matches!(v, "video" | "audio") {
                return Some(v.to_string());
            }
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sniffs_video_content_type() {
        let src = b"---\ntitle: \"X\"\ncontent_type: video\nduration: 213\n---\n\n# X\n";
        assert_eq!(sniff_media_content_type(src).as_deref(), Some("video"));
    }
    #[test]
    fn ignores_articles_and_plain_sources() {
        let article = b"---\ntitle: \"X\"\ncontent_type: article\n---\nBody";
        assert_eq!(sniff_media_content_type(article), None);
        assert_eq!(sniff_media_content_type(b"no frontmatter here"), None);
        // content_type mentioned in the body, not frontmatter.
        let body_only = b"---\ntitle: \"X\"\n---\ncontent_type: video\n";
        assert_eq!(sniff_media_content_type(body_only), None);
    }
    #[test]
    fn injects_into_existing_frontmatter() {
        let src = "---\ntitle: Foo\n---\n\nBody.\n";
        let out = inject_sources_frontmatter(src, "raw/sources/x.md");
        assert!(out.contains("sources:"));
        assert!(out.contains("raw/sources/x.md"));
        assert!(out.contains("title: Foo"));
    }
    #[test]
    fn wraps_when_no_frontmatter() {
        let src = "Just a body.\n";
        let out = inject_sources_frontmatter(src, "raw/sources/x.md");
        assert!(out.starts_with("---\nsources:\n  - raw/sources/x.md\n---\n"));
    }
    #[test]
    fn idempotent_when_already_listed() {
        let src = "---\nsources:\n  - raw/sources/x.md\n---\nBody.\n";
        let out = inject_sources_frontmatter(src, "raw/sources/x.md");
        let count = out.matches("raw/sources/x.md").count();
        assert_eq!(count, 1, "should not duplicate the source path");
    }
    #[test]
    fn appends_to_existing_list() {
        let src = "---\nsources:\n  - raw/sources/old.md\n---\nBody.\n";
        let out = inject_sources_frontmatter(src, "raw/sources/new.md");
        assert!(out.contains("raw/sources/old.md"));
        assert!(out.contains("raw/sources/new.md"));
    }
}
