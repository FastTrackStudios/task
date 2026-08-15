//! Output parsers for the wiki-flavored prompts.
//!
//! See [`crate::prompts`] for the prompts these consume.
//! Each parser is strict by design — `llm_wiki`'s pipeline
//! drops responses that don't match the expected format
//! and the port preserves that contract.

use crate::error::AgentWikiError;

// ─────────────────────────── Ingest blocks ───────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestBlocks {
    pub files: Vec<FileBlock>,
    pub reviews: Vec<ReviewBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileBlock {
    /// Vault-relative path (e.g. `Concepts/Spaced repetition.md`
    /// — `llm_wiki` uses `wiki/...` prefix which we strip).
    pub path: String,
    /// Full markdown including frontmatter.
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewBlock {
    pub kind: ReviewBlockKind,
    pub title: String,
    pub description: String,
    pub options: Vec<String>,
    pub pages: Vec<String>,
    pub search_queries: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewBlockKind {
    Contradiction,
    Duplicate,
    MissingPage,
    Suggestion,
}

impl ReviewBlockKind {
    fn parse(s: &str) -> Result<Self, AgentWikiError> {
        match s.trim().to_lowercase().as_str() {
            "contradiction" => Ok(Self::Contradiction),
            "duplicate" => Ok(Self::Duplicate),
            "missing-page" => Ok(Self::MissingPage),
            "suggestion" => Ok(Self::Suggestion),
            other => Err(AgentWikiError::UnknownReviewKind(other.to_string())),
        }
    }
}

/// Parse an ingest step-2 LLM response into FILE + REVIEW
/// blocks.
///
/// The response must begin with `---FILE:` (after optional
/// leading whitespace). Reviews follow files. Anything
/// between blocks is ignored — common LLM filler like
/// blank lines doesn't break the parse.
///
/// Format reference:
///
/// ```text
/// ---FILE: wiki/path/to/page.md---
/// (markdown body)
/// ---END FILE---
///
/// ---REVIEW: <kind> | <title>---
/// Description.
/// OPTIONS: Create Page | Skip
/// PAGES: a.md, b.md
/// SEARCH: q1 | q2
/// ---END REVIEW---
/// ```
pub fn parse_ingest_blocks(response: &str) -> Result<IngestBlocks, AgentWikiError> {
    // Line-walker parser ported from nashsu/llm_wiki's
    // `parseFileBlocks` hazard list — battle-tested against
    // every shape real LLMs produce:
    //
    //   H1. CRLF line endings → normalize to LF up front.
    //   H2. Stream truncation (missing close) → surface as
    //       MalformedResponse rather than silent drop.
    //   H3. Marker whitespace / case variants — `--- FILE: …`,
    //       `---END FILE--- `, `---file: …`, etc. — all
    //       accepted via case-insensitive line-anchored match.
    //   H4. Preamble prose before the first block → skip.
    //   H5. Literal `---END FILE---` inside a fenced code
    //       block (the LLM writes about our format) →
    //       fence-aware skip.
    //   H6. Empty path → InvalidFileTarget error.
    //
    // Pure prose with NO openers still errors so we don't
    // silently swallow a model that ignored the structured-
    // output ask entirely.
    let normalized = response.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.lines().collect();

    let mut out = IngestBlocks {
        files: Vec::new(),
        reviews: Vec::new(),
    };

    let mut i = 0;
    while i < lines.len() {
        if let Some(path) = match_file_opener(lines[i]) {
            i += 1;
            let (content, consumed) = collect_until_close(&lines[i..], "FILE")?;
            i += consumed;
            out.files.push(parse_file_block_lines(&path, &content)?);
        } else if let Some((kind_raw, title)) = match_review_opener(lines[i]) {
            i += 1;
            let (content, consumed) = collect_until_close(&lines[i..], "REVIEW")?;
            i += consumed;
            out.reviews
                .push(parse_review_block_lines(&kind_raw, &title, &content)?);
        } else {
            // Skip prose / preamble / inter-block filler.
            i += 1;
        }
    }

    if out.files.is_empty() && out.reviews.is_empty() {
        return Err(AgentWikiError::MalformedResponse(
            "no `---FILE:` / `---REVIEW:` blocks found in response",
            response.chars().take(120).collect::<String>(),
        ));
    }
    Ok(out)
}

/// Match a line-anchored `---FILE: <path>---` opener
/// (case-insensitive, whitespace-tolerant). Returns the
/// extracted path on success.
fn match_file_opener(line: &str) -> Option<String> {
    let l = line.trim();
    let lower = l.to_ascii_lowercase();
    let rest = lower.strip_prefix("---")?.trim_start();
    if !rest.starts_with("file:") {
        return None;
    }
    // Slice the original (case-preserving) line at the same offsets.
    let after_prefix = l[3..].trim_start();
    let after_file = after_prefix
        .get(5..)? // "file:" (case-insensitive — we know matched)
        .trim_start();
    let trimmed = after_file.trim_end();
    // Strip the trailing `---` (with optional whitespace).
    let path = trimmed.trim_end_matches('-').trim();
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

/// Match a line-anchored `---REVIEW: <kind> | <title>---`
/// opener. Returns `(kind, title)`.
fn match_review_opener(line: &str) -> Option<(String, String)> {
    let l = line.trim();
    let lower = l.to_ascii_lowercase();
    let rest = lower.strip_prefix("---")?.trim_start();
    if !rest.starts_with("review:") {
        return None;
    }
    let after_prefix = l[3..].trim_start();
    let after_review = after_prefix
        .get(7..)? // "review:"
        .trim_start()
        .trim_end()
        .trim_end_matches('-')
        .trim();
    let (kind, title) = after_review.split_once('|').map_or_else(
        || (after_review.to_string(), String::new()),
        |(k, t)| (k.trim().to_string(), t.trim().to_string()),
    );
    Some((kind, title))
}

/// True if the line is a line-anchored close marker for
/// `kind` (either `FILE` or `REVIEW`). Matches `---END FILE---`,
/// `--- end file ---`, etc.
fn is_close_marker(line: &str, kind: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    let stripped = match lower.strip_prefix("---") {
        Some(s) => s.trim_start(),
        None => return false,
    };
    let after_end = match stripped.strip_prefix("end") {
        Some(s) => s.trim_start(),
        None => return false,
    };
    let kind_lower = kind.to_ascii_lowercase();
    let after_kind = match after_end.strip_prefix(kind_lower.as_str()) {
        Some(s) => s,
        None => return false,
    };
    after_kind.trim().trim_end_matches('-').trim().is_empty()
}

/// Match a CommonMark code-fence open / close. Returns
/// `Some(("```", 3))` for a `\`\`\`rust` line or similar.
/// Allows up to 3 leading spaces (CommonMark fence rule).
fn match_fence_open(line: &str) -> Option<(char, usize)> {
    let leading = line.chars().take_while(|c| *c == ' ').count();
    if leading > 3 {
        return None;
    }
    let body = &line[leading..];
    let first = body.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let run = body.chars().take_while(|c| *c == first).count();
    if run < 3 {
        return None;
    }
    Some((first, run))
}

/// Walk `lines` collecting content until the close marker
/// for `kind`. Tracks fence depth so a literal close marker
/// quoted inside a code fence doesn't end the block early
/// (nashsu's H5).
fn collect_until_close(lines: &[&str], kind: &str) -> Result<(Vec<String>, usize), AgentWikiError> {
    let mut content = Vec::new();
    let mut fence_char: Option<char> = None;
    let mut fence_len = 0usize;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // Fence state update.
        if let Some((ch, run)) = match_fence_open(line) {
            match fence_char {
                None => {
                    fence_char = Some(ch);
                    fence_len = run;
                }
                Some(open_ch) if open_ch == ch && run >= fence_len => {
                    fence_char = None;
                    fence_len = 0;
                }
                _ => {}
            }
        }
        if fence_char.is_none() && is_close_marker(line, kind) {
            return Ok((content, i + 1));
        }
        content.push(line.to_string());
        i += 1;
    }
    Err(AgentWikiError::MalformedResponse(
        "missing close marker",
        content
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
    ))
}

fn parse_file_block_lines(path_raw: &str, content: &[String]) -> Result<FileBlock, AgentWikiError> {
    if path_raw.is_empty() {
        return Err(AgentWikiError::InvalidFileTarget(
            path_raw.to_string(),
            "empty path",
        ));
    }
    // Preserve the `wiki/` prefix when present — nashsu/llm_wiki's
    // canonical convention is `wiki/concepts/X.md`. Stripping it
    // (the old behavior) caused pages to land at the vault root
    // (`concepts/X.md`) while Codex's autonomous tool-use wrote
    // them at `wiki/concepts/X.md`, bifurcating the wiki.
    // Normalize `Wiki/` → `wiki/` so case variants land in the
    // same place.
    let normalized = if let Some(rest) = path_raw.strip_prefix("Wiki/") {
        format!("wiki/{rest}")
    } else {
        path_raw.to_string()
    };
    // If the path is wiki-rooted but missing the prefix entirely
    // (e.g. legacy `concepts/X.md`), prepend `wiki/` so all writes
    // land under `wiki/`. Schema/index/log/overview live at the
    // wiki root (no `wiki/` prefix) and are recognized by name.
    let canonical = if matches!(
        normalized.as_str(),
        "schema.md" | "purpose.md" | "index.md" | "log.md" | "overview.md"
    ) || normalized.starts_with("wiki/")
        || normalized.starts_with("_state/")
        || normalized.starts_with("raw/")
        || normalized.starts_with("media/")
    {
        normalized
    } else if normalized.starts_with("concepts/")
        || normalized.starts_with("entities/")
        || normalized.starts_with("sources/")
    {
        format!("wiki/{normalized}")
    } else {
        normalized
    };
    if canonical.is_empty() || canonical.contains("..") {
        return Err(AgentWikiError::InvalidFileTarget(
            canonical,
            "must be wiki-relative, no `..` allowed",
        ));
    }
    let body = content.join("\n");
    Ok(FileBlock {
        path: canonical,
        content: body.trim_end_matches('\n').to_string() + "\n",
    })
}

fn parse_review_block_lines(
    kind_raw: &str,
    title: &str,
    content: &[String],
) -> Result<ReviewBlock, AgentWikiError> {
    let kind = ReviewBlockKind::parse(kind_raw)?;
    let mut description = String::new();
    let mut options: Vec<String> = Vec::new();
    let mut pages: Vec<String> = Vec::new();
    let mut search_queries: Vec<String> = Vec::new();
    for line in content {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("OPTIONS:") {
            options = rest.split('|').map(|s| s.trim().to_string()).collect();
        } else if let Some(rest) = l.strip_prefix("PAGES:") {
            pages = rest.split(',').map(|s| s.trim().to_string()).collect();
        } else if let Some(rest) = l.strip_prefix("SEARCH:") {
            search_queries = rest.split('|').map(|s| s.trim().to_string()).collect();
        } else if !l.is_empty() {
            if !description.is_empty() {
                description.push('\n');
            }
            description.push_str(line);
        }
    }
    Ok(ReviewBlock {
        kind,
        title: title.to_string(),
        description,
        options,
        pages,
        search_queries,
    })
}

/// Return the slice between the open header at `start`
/// and the matching close marker. Returns the slice
/// (header + body + close) and the absolute end index.
fn parse_one<'a>(
    src: &'a str,
    start: usize,
    open: &str,
    close: &str,
) -> Result<(&'a str, usize), AgentWikiError> {
    debug_assert!(src[start..].starts_with(open));
    let close_rel = src[start..].find(close).ok_or_else(|| {
        AgentWikiError::MalformedResponse(
            "missing close marker",
            format!("{}…", &src[start..src.len().min(start + 80)]),
        )
    })?;
    let end = start + close_rel + close.len();
    Ok((&src[start..end], end))
}

#[allow(dead_code)] // Replaced by line-walker; kept for any future direct callers.
fn parse_file_block(raw: &str) -> Result<FileBlock, AgentWikiError> {
    // Strip the header line.
    let rest = raw
        .strip_prefix("---FILE:")
        .ok_or(AgentWikiError::MalformedResponse(
            "expected ---FILE: prefix",
            raw.chars().take(60).collect(),
        ))?;
    let (header, body) = rest
        .split_once('\n')
        .ok_or(AgentWikiError::MalformedResponse(
            "FILE block missing newline after header",
            raw.chars().take(60).collect(),
        ))?;
    // Header is `<path>---` — strip the trailing `---`.
    let path_raw = header.trim().trim_end_matches('-').trim();
    if path_raw.is_empty() {
        return Err(AgentWikiError::InvalidFileTarget(
            path_raw.to_string(),
            "empty path",
        ));
    }
    // Strip an optional `wiki/` prefix (llm_wiki's
    // convention) so our paths are Wiki/-relative.
    let path = path_raw
        .strip_prefix("wiki/")
        .or_else(|| path_raw.strip_prefix("Wiki/"))
        .unwrap_or(path_raw)
        .to_string();
    if path.is_empty() || path.contains("..") {
        return Err(AgentWikiError::InvalidFileTarget(
            path,
            "must be wiki-relative, no `..` allowed",
        ));
    }

    // Strip the closing `---END FILE---` from the body.
    let content = body
        .strip_suffix("---END FILE---")
        .or_else(|| body.strip_suffix("---END FILE---\n"))
        .unwrap_or_else(|| {
            body.trim_end_matches('\n')
                .strip_suffix("---END FILE---")
                .unwrap_or(body)
        });
    Ok(FileBlock {
        path,
        content: content.trim_end_matches('\n').to_string() + "\n",
    })
}

#[allow(dead_code)] // Replaced by line-walker; kept for any future direct callers.
fn parse_review_block(raw: &str) -> Result<ReviewBlock, AgentWikiError> {
    let rest = raw
        .strip_prefix("---REVIEW:")
        .ok_or(AgentWikiError::MalformedResponse(
            "expected ---REVIEW: prefix",
            raw.chars().take(60).collect(),
        ))?;
    let (header, body) = rest
        .split_once('\n')
        .ok_or(AgentWikiError::MalformedResponse(
            "REVIEW block missing newline after header",
            raw.chars().take(60).collect(),
        ))?;
    // Header is `<kind> | <title>---`.
    let header = header.trim().trim_end_matches('-').trim();
    let (kind_str, title) = header
        .split_once('|')
        .ok_or(AgentWikiError::MalformedResponse(
            "REVIEW header must be `<kind> | <title>`",
            header.to_string(),
        ))?;
    let kind = ReviewBlockKind::parse(kind_str)?;
    let title = title.trim().to_string();

    let body = body
        .strip_suffix("---END REVIEW---")
        .or_else(|| body.strip_suffix("---END REVIEW---\n"))
        .unwrap_or(body);

    let mut description = String::new();
    let mut options = Vec::new();
    let mut pages = Vec::new();
    let mut search_queries = Vec::new();
    for line in body.lines() {
        let l = line.trim_end();
        if let Some(rest) = l.strip_prefix("OPTIONS:") {
            options = rest
                .split('|')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        } else if let Some(rest) = l.strip_prefix("PAGES:") {
            pages = rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        } else if let Some(rest) = l.strip_prefix("SEARCH:") {
            search_queries = rest
                .split('|')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        } else {
            description.push_str(l);
            description.push('\n');
        }
    }

    Ok(ReviewBlock {
        kind,
        title,
        description: description.trim().to_string(),
        options,
        pages,
        search_queries,
    })
}

// ─────────────────────────── Lint, dedup, research ───────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintBlock {
    pub kind: LintBlockKind,
    pub severity: LintSeverity,
    pub title: String,
    pub description: String,
    pub pages: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintBlockKind {
    Contradiction,
    Stale,
    MissingPage,
    Suggestion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintSeverity {
    Warning,
    Info,
}

impl LintBlockKind {
    fn parse(s: &str) -> Result<Self, AgentWikiError> {
        match s.trim().to_lowercase().as_str() {
            "contradiction" => Ok(Self::Contradiction),
            "stale" => Ok(Self::Stale),
            "missing-page" => Ok(Self::MissingPage),
            "suggestion" => Ok(Self::Suggestion),
            other => Err(AgentWikiError::UnknownLintKind(other.to_string())),
        }
    }
}

impl LintSeverity {
    fn parse(s: &str) -> LintSeverity {
        match s.trim().to_lowercase().as_str() {
            "info" => Self::Info,
            _ => Self::Warning,
        }
    }
}

/// Parse `---LINT: <kind> | <severity> | <title>---` /
/// `---END LINT---` blocks. Matches `llm_wiki`'s lint
/// emit format.
pub fn parse_lint_blocks(response: &str) -> Result<Vec<LintBlock>, AgentWikiError> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < response.len() {
        let after = &response[cursor..];
        let Some(rel) = after.find("---LINT:") else {
            break;
        };
        let block_start = cursor + rel;
        let (block, end) = parse_one(response, block_start, "---LINT:", "---END LINT---")?;
        out.push(parse_lint_block(block)?);
        cursor = end;
    }
    Ok(out)
}

fn parse_lint_block(raw: &str) -> Result<LintBlock, AgentWikiError> {
    let rest = raw
        .strip_prefix("---LINT:")
        .ok_or(AgentWikiError::MalformedResponse(
            "expected ---LINT: prefix",
            raw.chars().take(60).collect(),
        ))?;
    let (header, body) = rest
        .split_once('\n')
        .ok_or(AgentWikiError::MalformedResponse(
            "LINT block missing newline after header",
            raw.chars().take(60).collect(),
        ))?;
    let header = header.trim().trim_end_matches('-').trim();
    let mut parts = header.splitn(3, '|');
    let kind_str = parts.next().unwrap_or("").trim();
    let sev_str = parts.next().unwrap_or("warning").trim();
    let title = parts.next().unwrap_or("").trim().to_string();
    let kind = LintBlockKind::parse(kind_str)?;
    let severity = LintSeverity::parse(sev_str);

    let body = body
        .strip_suffix("---END LINT---")
        .or_else(|| body.strip_suffix("---END LINT---\n"))
        .unwrap_or(body);

    let mut description = String::new();
    let mut pages = Vec::new();
    for line in body.lines() {
        let l = line.trim_end();
        if let Some(rest) = l.strip_prefix("PAGES:") {
            pages = rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        } else {
            description.push_str(l);
            description.push('\n');
        }
    }

    Ok(LintBlock {
        kind,
        severity,
        title,
        description: description.trim().to_string(),
        pages,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub slugs: Vec<String>,
    pub reason: String,
    pub confidence: DuplicateConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateConfidence {
    High,
    Medium,
    Low,
}

/// Parse `{"groups":[{"slugs":[...],"reason":"…",
/// "confidence":"high"}]}` JSON.
pub fn parse_dedup_groups(response: &str) -> Result<Vec<DuplicateGroup>, AgentWikiError> {
    // Trim markdown fences in case the LLM wrapped despite
    // instructions.
    let body = strip_json_fence(response);
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| {
        AgentWikiError::MalformedResponse(
            "expected JSON object",
            format!("{e}: {}", body.chars().take(80).collect::<String>()),
        )
    })?;
    let groups =
        v.get("groups")
            .and_then(|g| g.as_array())
            .ok_or(AgentWikiError::MalformedResponse(
                "missing `groups` array",
                body.chars().take(80).collect(),
            ))?;
    let mut out = Vec::with_capacity(groups.len());
    for g in groups {
        let slugs: Vec<String> = g
            .get("slugs")
            .and_then(|s| s.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(std::string::ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if slugs.len() < 2 {
            continue;
        }
        let reason = g
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let confidence = match g.get("confidence").and_then(|v| v.as_str()) {
            Some("high") => DuplicateConfidence::High,
            Some("medium") => DuplicateConfidence::Medium,
            Some("low") => DuplicateConfidence::Low,
            _ => DuplicateConfidence::Medium,
        };
        out.push(DuplicateGroup {
            slugs,
            reason,
            confidence,
        });
    }
    Ok(out)
}

fn strip_json_fence(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```json") {
        rest.trim_start_matches('\n')
            .trim_end()
            .strip_suffix("```")
            .unwrap_or(rest)
            .trim()
    } else if let Some(rest) = t.strip_prefix("```") {
        rest.trim_start_matches('\n')
            .trim_end()
            .strip_suffix("```")
            .unwrap_or(rest)
            .trim()
    } else {
        t
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchTopicPlan {
    pub topic: String,
    pub queries: Vec<String>,
}

/// Parse the 4-line `TOPIC:` + 3× `QUERY:` response from
/// `OPTIMIZE_RESEARCH_SYSTEM`. Tolerant: accepts < 3
/// queries, but `TOPIC:` is required.
pub fn parse_research_plan(response: &str) -> Result<ResearchTopicPlan, AgentWikiError> {
    let mut topic = String::new();
    let mut queries = Vec::new();
    for line in response.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("TOPIC:") {
            topic = rest.trim().to_string();
        } else if let Some(rest) = l.strip_prefix("QUERY:") {
            let q = rest.trim().to_string();
            if !q.is_empty() {
                queries.push(q);
            }
        }
    }
    if topic.is_empty() {
        return Err(AgentWikiError::MalformedResponse(
            "missing TOPIC: line",
            response.chars().take(120).collect(),
        ));
    }
    Ok(ResearchTopicPlan { topic, queries })
}

/// Parse `{"resolved":["id1","id2"]}` from
/// `SWEEP_REVIEWS_SYSTEM`.
pub fn parse_sweep_resolved(response: &str) -> Result<Vec<String>, AgentWikiError> {
    let body = strip_json_fence(response);
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| {
        AgentWikiError::MalformedResponse(
            "expected JSON object",
            format!("{e}: {}", body.chars().take(80).collect::<String>()),
        )
    })?;
    let arr =
        v.get("resolved")
            .and_then(|r| r.as_array())
            .ok_or(AgentWikiError::MalformedResponse(
                "missing `resolved` array",
                body.chars().take(80).collect(),
            ))?;
    Ok(arr
        .iter()
        .filter_map(|v| v.as_str().map(std::string::ToString::to_string))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_file_block() {
        let resp = r"---FILE: wiki/Concepts/Foo.md---
---
type: concept
title: Foo
---

# Foo

Body.
---END FILE---";
        let parsed = parse_ingest_blocks(resp).expect("parse");
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].path, "wiki/Concepts/Foo.md");
        assert!(parsed.files[0].content.starts_with("---\n"));
        assert!(parsed.files[0].content.contains("# Foo"));
    }

    #[test]
    fn parses_file_then_review() {
        let resp = "---FILE: wiki/Entities/X.md---\n---\ntype: entity\ntitle: X\n---\n\n# X\n---END FILE---\n\n---REVIEW: contradiction | Foo conflicts with bar---\nDescription line.\nOPTIONS: Create Page | Skip\nPAGES: Concepts/Foo.md, Concepts/Bar.md\n---END REVIEW---\n";
        let parsed = parse_ingest_blocks(resp).expect("parse");
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.reviews.len(), 1);
        assert_eq!(parsed.reviews[0].kind, ReviewBlockKind::Contradiction);
        assert_eq!(parsed.reviews[0].title, "Foo conflicts with bar");
        assert_eq!(parsed.reviews[0].options, vec!["Create Page", "Skip"]);
        assert_eq!(parsed.reviews[0].pages.len(), 2);
    }

    #[test]
    fn accepts_preamble_before_first_block() {
        // Real LLMs often emit 1-2 sentences of preface even
        // when told to start with `-`. Treat that as harmless;
        // the parser advances to the first `---FILE:` header.
        let resp = "Sure! Here are the files:\n\n---FILE: wiki/foo.md---\nbody\n---END FILE---";
        let parsed = parse_ingest_blocks(resp).expect("should accept preamble");
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].path, "wiki/foo.md");
    }

    // ── Ported nashsu/llm_wiki hazards ──────────────────────

    #[test]
    fn h1_normalizes_crlf() {
        // Windows line endings would defeat a regex anchored
        // on bare `\n`. Normalize early.
        let resp = "---FILE: foo.md---\r\nbody line 1\r\nbody line 2\r\n---END FILE---\r\n";
        let parsed = parse_ingest_blocks(resp).unwrap();
        assert_eq!(parsed.files.len(), 1);
        assert!(parsed.files[0].content.contains("body line 1"));
        assert!(parsed.files[0].content.contains("body line 2"));
    }

    #[test]
    fn h3_marker_whitespace_and_case() {
        // Real LLMs emit lots of small variants. All should
        // parse — `--- FILE: foo.md ---`, `---file: bar.md---`,
        // `---END FILE--- ` (trailing space).
        let resp = "--- FILE: foo.md ---\nbody\n---END FILE--- \n\n---file: bar.md---\nbody2\n--- end file ---\n";
        let parsed = parse_ingest_blocks(resp).unwrap();
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(parsed.files[0].path, "foo.md");
        assert_eq!(parsed.files[1].path, "bar.md");
    }

    #[test]
    fn h5_fenced_close_marker_does_not_close_outer_block() {
        // When the LLM is writing a wiki page ABOUT our ingest
        // format, the literal `---END FILE---` inside a code
        // fence MUST NOT close the outer block. nashsu's H5.
        let resp = "---FILE: docs/ingest-format.md---\n# Format\n\n```\nresponse = \"---FILE: x.md---\\nbody\\n---END FILE---\"\n```\n\nMore prose after the fence.\n---END FILE---\n";
        let parsed = parse_ingest_blocks(resp).unwrap();
        assert_eq!(parsed.files.len(), 1);
        assert!(
            parsed.files[0]
                .content
                .contains("More prose after the fence")
        );
    }

    #[test]
    fn h2_missing_close_marker_errors() {
        // Stream truncation should surface as an error, not
        // silently drop the block.
        let resp = "---FILE: foo.md---\nbody with no close marker\n";
        let err = parse_ingest_blocks(resp).unwrap_err();
        assert!(matches!(
            err,
            AgentWikiError::MalformedResponse("missing close marker", _)
        ));
    }

    #[test]
    fn h6_empty_path_errors() {
        let resp = "---FILE: ---\nbody\n---END FILE---\n";
        // Empty path between `FILE:` and `---` — opener match
        // would yield an empty string; we reject pre-walk so
        // the line isn't treated as an opener at all.
        assert!(parse_ingest_blocks(resp).is_err());
    }

    #[test]
    fn canonicalizes_legacy_paths_under_wiki_prefix() {
        // Without `wiki/` prefix, paths under the canonical
        // tiers (concepts/ entities/ sources/) get auto-rooted
        // under `wiki/` so they don't bifurcate the layout when
        // the LLM forgets the prefix.
        let resp = "---FILE: concepts/x.md---\nbody\n---END FILE---\n\n---FILE: entities/y.md---\nb2\n---END FILE---\n\n---FILE: sources/z.md---\nb3\n---END FILE---";
        let parsed = parse_ingest_blocks(resp).unwrap();
        assert_eq!(parsed.files[0].path, "wiki/concepts/x.md");
        assert_eq!(parsed.files[1].path, "wiki/entities/y.md");
        assert_eq!(parsed.files[2].path, "wiki/sources/z.md");
    }

    #[test]
    fn reserved_root_paths_stay_unprefixed() {
        // Schema/index/log/overview live at the wiki root, not
        // under `wiki/`. Don't auto-prefix them.
        for name in &[
            "index.md",
            "log.md",
            "overview.md",
            "schema.md",
            "purpose.md",
        ] {
            let resp = format!("---FILE: {name}---\nbody\n---END FILE---");
            let parsed = parse_ingest_blocks(&resp).unwrap();
            assert_eq!(
                parsed.files[0].path, *name,
                "reserved path stripped wrongly"
            );
        }
    }

    #[test]
    fn case_variant_wiki_normalized_to_lower() {
        let resp = "---FILE: Wiki/concepts/x.md---\nbody\n---END FILE---";
        let parsed = parse_ingest_blocks(resp).unwrap();
        assert_eq!(parsed.files[0].path, "wiki/concepts/x.md");
    }

    #[test]
    fn rejects_pure_prose() {
        // Still error when there are no blocks at all — that
        // means the LLM ignored the structured-output ask.
        let resp = "I'm checking the existing wiki pages now…";
        assert!(parse_ingest_blocks(resp).is_err());
    }
}
