//! Orchestration — run ONE Codex turn over a batch of open
//! inbox items and return the parsed proposals.
//!
//! Mirrors `agent_wiki::bridge`: takes a concrete
//! [`CodexBackend`] for now; generalizes to a trait bound
//! once a second backend lands. `access_mode: "read-only"`
//! forces text-only output so the response always routes
//! through [`crate::parsers::parse_proposals`] — no
//! tool-use writes.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use agent_codex::{ChatOpts, CodexBackend};
use agent_proto::event::AgentEvent;
use futures::StreamExt;

use crate::error::AgentInboxError;
use crate::parsers::{Proposal, parse_proposals};
use crate::prompts::{PROCESS_SYSTEM, render};

/// Per-item body cap in the prompt. Inbox captures are
/// fleeting notes — anything longer gets head-truncated with
/// a visible marker so the batch stays inside the turn's
/// output budget (same lesson as agent-wiki's deepen cap).
const ITEM_BODY_CAP: usize = 2_000;

/// One open inbox item, as fed to the prompt.
#[derive(Debug, Clone)]
pub struct ProcessItem {
    /// Inbox item id (uuid string) — echoed back in the
    /// `---ITEM: <id>---` block header.
    pub id: String,
    /// Captured text, verbatim markdown.
    pub body: String,
    /// Capture source label (`cli`, `ui`, `telegram`, …).
    pub source: String,
    /// RFC-3339 capture timestamp.
    pub created: String,
}

/// Input to one processing pass.
#[derive(Debug, Clone)]
pub struct ProcessRequest {
    pub items: Vec<ProcessItem>,
    /// Project titles the LLM may reference in `PROJECT:`
    /// lines (resolution back to ids happens caller-side).
    pub project_titles: Vec<String>,
    /// ISO `YYYY-MM-DD` used to ground relative dates.
    pub today: String,
    /// Model id. `None` ⇒ daemon default.
    pub model: Option<String>,
    /// Per-turn timeout. One turn covers the whole batch.
    pub timeout: Duration,
}

/// Drive ONE turn over the batch and return the proposals,
/// filtered to ids that were actually in the batch (a model
/// that hallucinates an id gets that block dropped, loudly
/// via the returned `unmatched` count being visible to the
/// caller through the shorter Vec).
pub async fn run_process(
    backend: &CodexBackend,
    workspace: &Path,
    req: ProcessRequest,
) -> Result<Vec<Proposal>, AgentInboxError> {
    if req.items.is_empty() {
        return Ok(Vec::new());
    }
    let projects_block = if req.project_titles.is_empty() {
        "(no projects — omit PROJECT lines entirely)".to_string()
    } else {
        req.project_titles
            .iter()
            .map(|t| format!("- {t}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let items_block = render_items_block(&req.items);

    let mut vars = HashMap::new();
    vars.insert("today", req.today.as_str());
    vars.insert("projects", projects_block.as_str());
    vars.insert("inbox_items", items_block.as_str());
    let system = render(PROCESS_SYSTEM, &vars);

    let opts = ChatOpts {
        codex_bin: None,
        codex_args: None,
        codex_home: None,
        model: req.model.clone(),
        effort: None,
        // Text-only output: every proposal must come back as
        // parseable ITEM blocks, never as tool-use writes.
        access_mode: Some("read-only".to_string()),
    };
    let resp = drive_turn_text(backend, workspace, &system, &opts, req.timeout).await?;
    let proposals = parse_proposals(&resp)?;

    // Keep only proposals whose id matches a batch item.
    let known: std::collections::HashSet<&str> = req.items.iter().map(|i| i.id.as_str()).collect();
    Ok(proposals
        .into_iter()
        .filter(|p| known.contains(p.item_id.as_str()))
        .collect())
}

/// Build the `{inbox_items}` block: one YAML-ish stanza per
/// item with the body indented (and capped) so multi-line
/// captures don't break the list shape.
#[must_use]
pub fn render_items_block(items: &[ProcessItem]) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str(&format!(
            "- id: {}\n  source: {}\n  created: {}\n  body: |\n",
            item.id, item.source, item.created
        ));
        let body: String = if item.body.len() > ITEM_BODY_CAP {
            let mut cut = ITEM_BODY_CAP;
            while !item.body.is_char_boundary(cut) {
                cut -= 1;
            }
            format!("{}\n…[truncated for prompt budget]", &item.body[..cut])
        } else {
            item.body.clone()
        };
        for line in body.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// Drive one turn and return the concatenated
/// `MessageDelta` text (same loop as
/// `agent_wiki::bridge::drive_turn_text`).
async fn drive_turn_text(
    backend: &CodexBackend,
    workspace: &Path,
    text: &str,
    opts: &ChatOpts,
    timeout: Duration,
) -> Result<String, AgentInboxError> {
    let handle = backend
        .chat(workspace.to_path_buf(), text.to_string(), opts.clone())
        .await
        .map_err(|e| AgentInboxError::Bridge(format!("backend.chat: {e}")))?;
    let mut events = handle.events;
    let mut out = String::new();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match tokio::time::timeout_at(deadline, events.next()).await {
            Err(_) => {
                return Err(AgentInboxError::Bridge(format!(
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
                return Err(AgentInboxError::Bridge(format!(
                    "turn errored ({kind}): {message}"
                )));
            }
            Ok(Some(_)) => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_block_indents_multiline_bodies() {
        let items = vec![ProcessItem {
            id: "abc".into(),
            body: "first line\nsecond line".into(),
            source: "cli".into(),
            created: "2026-07-03T10:00:00Z".into(),
        }];
        let block = render_items_block(&items);
        assert!(block.contains("- id: abc"));
        assert!(block.contains("    first line\n    second line\n"));
    }

    #[test]
    fn items_block_caps_long_bodies() {
        let items = vec![ProcessItem {
            id: "abc".into(),
            body: "x".repeat(ITEM_BODY_CAP + 500),
            source: "cli".into(),
            created: "2026-07-03T10:00:00Z".into(),
        }];
        let block = render_items_block(&items);
        assert!(block.contains("…[truncated for prompt budget]"));
        assert!(block.len() < ITEM_BODY_CAP + 400);
    }
}
