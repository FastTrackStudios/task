//! Parser for the `---ITEM: <id>---` proposal blocks emitted
//! by [`crate::prompts::PROCESS_SYSTEM`].
//!
//! Same line-walker discipline as `agent_wiki::parsers::parse_ingest_blocks`
//! (its hazard list is battle-tested against real LLM output):
//!
//! - H1: CRLF normalized up front.
//! - H2: stream truncation (missing `---END ITEM---`) → error,
//!   never a silent drop.
//! - H3: marker whitespace / case variants accepted
//!   (`--- item: id ---`, `---END ITEM--- `).
//! - H4: prose preamble before the first block skipped.
//! - H5: a literal close marker inside a markdown code fence
//!   in a note BODY does not close the block.
//! - Pure prose with no blocks at all → error (the model
//!   ignored the structured-output ask).

use crate::error::AgentInboxError;

/// One reviewed-by-the-user proposal for one inbox item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// The inbox item id this proposal is for (verbatim from
    /// the block header — the bridge cross-checks it against
    /// the batch).
    pub item_id: String,
    pub action: ProposalAction,
}

/// What the LLM proposes doing with one inbox item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalAction {
    /// Promote to a task via `TaskService`.
    Task {
        /// Clean imperative title (no `#tags` / `[[links]]`).
        title: String,
        /// Project **title** from the provided list, if any.
        /// Resolution to a project id happens caller-side.
        project_title: Option<String>,
        /// `@`-prefixed GTD contexts.
        contexts: Vec<String>,
        /// `YYYY-MM-DD` due date, if the item implies one.
        due: Option<String>,
    },
    /// Keep as a reference note in the vault.
    Note {
        /// Vault-relative path (`notes/<slug>.md`).
        path: String,
        /// Cleaned markdown body.
        body: String,
    },
    /// Nothing to keep — offer to archive.
    Skip {
        /// One-line reason ("duplicate of …", "no action").
        reason: String,
    },
}

/// Parse a full processing-pass response into proposals, in
/// response order. Strict per block (unknown `ACTION:`,
/// missing `TITLE`/`PATH` → error) but tolerant of prose
/// between blocks.
pub fn parse_proposals(response: &str) -> Result<Vec<Proposal>, AgentInboxError> {
    let normalized = response.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.lines().collect();

    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(item_id) = match_item_opener(lines[i]) {
            i += 1;
            let (content, consumed) = collect_until_close(&lines[i..])?;
            i += consumed;
            out.push(parse_item_block(&item_id, &content)?);
        } else {
            i += 1; // prose / preamble / inter-block filler
        }
    }

    if out.is_empty() {
        return Err(AgentInboxError::MalformedResponse(
            "no `---ITEM:` blocks found in response",
            response.chars().take(120).collect(),
        ));
    }
    Ok(out)
}

/// Match a line-anchored `---ITEM: <id>---` opener
/// (case-insensitive, whitespace-tolerant). Returns the id.
fn match_item_opener(line: &str) -> Option<String> {
    let l = line.trim();
    let lower = l.to_ascii_lowercase();
    let rest = lower.strip_prefix("---")?.trim_start();
    if !rest.starts_with("item:") {
        return None;
    }
    // Slice the original (case-preserving) line at the same
    // offsets: past `---`, past `item:`.
    let after_prefix = l[3..].trim_start();
    let after_item = after_prefix.get(5..)?.trim_start();
    let id = after_item.trim_end().trim_end_matches('-').trim();
    if id.is_empty() {
        return None;
    }
    Some(id.to_string())
}

/// True for a line-anchored `---END ITEM---` close marker
/// (case/whitespace variants included).
fn is_close_marker(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    let Some(stripped) = lower.strip_prefix("---") else {
        return false;
    };
    let Some(after_end) = stripped.trim_start().strip_prefix("end") else {
        return false;
    };
    let Some(after_kind) = after_end.trim_start().strip_prefix("item") else {
        return false;
    };
    after_kind.trim().trim_end_matches('-').trim().is_empty()
}

/// Match a CommonMark code-fence open/close (up to 3 leading
/// spaces, ``` or ~~~ runs of ≥3).
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

/// Collect content lines until the fence-aware close marker.
fn collect_until_close(lines: &[&str]) -> Result<(Vec<String>, usize), AgentInboxError> {
    let mut content = Vec::new();
    let mut fence_char: Option<char> = None;
    let mut fence_len = 0usize;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
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
        if fence_char.is_none() && is_close_marker(line) {
            return Ok((content, i + 1));
        }
        content.push(line.to_string());
        i += 1;
    }
    Err(AgentInboxError::MalformedResponse(
        "missing `---END ITEM---` close marker",
        content
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
    ))
}

/// Parse one block's body lines: `KEY: value` header lines,
/// then everything after a `BODY:` line is the note body.
fn parse_item_block(item_id: &str, content: &[String]) -> Result<Proposal, AgentInboxError> {
    let mut action: Option<String> = None;
    let mut title: Option<String> = None;
    let mut project: Option<String> = None;
    let mut contexts: Vec<String> = Vec::new();
    let mut due: Option<String> = None;
    let mut path: Option<String> = None;
    let mut reason: Option<String> = None;
    let mut body_lines: Option<Vec<String>> = None;

    for line in content {
        if let Some(b) = &mut body_lines {
            b.push(line.clone());
            continue;
        }
        let l = line.trim();
        if let Some(rest) = strip_key(l, "ACTION:") {
            action = Some(rest.to_lowercase());
        } else if let Some(rest) = strip_key(l, "TITLE:") {
            title = Some(rest.to_string());
        } else if let Some(rest) = strip_key(l, "PROJECT:") {
            if !rest.is_empty() {
                project = Some(rest.to_string());
            }
        } else if let Some(rest) = strip_key(l, "CONTEXTS:") {
            contexts = rest
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    if s.starts_with('@') {
                        s.to_string()
                    } else {
                        format!("@{s}")
                    }
                })
                .collect();
        } else if let Some(rest) = strip_key(l, "DUE:") {
            if !rest.is_empty() {
                due = Some(rest.to_string());
            }
        } else if let Some(rest) = strip_key(l, "PATH:") {
            path = Some(rest.to_string());
        } else if let Some(rest) = strip_key(l, "REASON:") {
            reason = Some(rest.to_string());
        } else if strip_key(l, "BODY:").is_some() {
            // Everything from the next line on is verbatim
            // markdown body (BODY: is always last).
            body_lines = Some(Vec::new());
        }
        // Unknown header lines are ignored — forwards-compatible
        // with prompt additions.
    }

    let action_str =
        action.ok_or_else(|| AgentInboxError::MissingField("ACTION", item_id.to_string()))?;
    let action = match action_str.as_str() {
        "task" => {
            let title = title
                .filter(|t| !t.is_empty())
                .ok_or_else(|| AgentInboxError::MissingField("TITLE", item_id.to_string()))?;
            ProposalAction::Task {
                title,
                project_title: project,
                contexts,
                due,
            }
        }
        "note" => {
            let path = path
                .filter(|p| !p.is_empty())
                .ok_or_else(|| AgentInboxError::MissingField("PATH", item_id.to_string()))?;
            if path.contains("..") || path.starts_with('/') {
                return Err(AgentInboxError::MalformedResponse(
                    "note PATH must be vault-relative, no `..`",
                    path,
                ));
            }
            let body = body_lines
                .map(|b| b.join("\n").trim().to_string())
                .unwrap_or_default();
            ProposalAction::Note { path, body }
        }
        "skip" => ProposalAction::Skip {
            reason: reason
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| "no action proposed".to_string()),
        },
        other => {
            return Err(AgentInboxError::UnknownAction(
                other.to_string(),
                item_id.to_string(),
            ));
        }
    };
    Ok(Proposal {
        item_id: item_id.to_string(),
        action,
    })
}

/// Case-insensitive `KEY:` prefix strip; returns the trimmed
/// remainder.
fn strip_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    if line.len() >= key.len() && line[..key.len()].eq_ignore_ascii_case(key) {
        Some(line[key.len()..].trim())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture: a realistic full response — preamble prose,
    /// one of each action kind, marker case/whitespace
    /// variants, and a code fence quoting the close marker.
    const FIXTURE_RESPONSE: &str = r#"Here are my proposals:

---ITEM: aaaa1111-0000-0000-0000-000000000001---
ACTION: task
TITLE: Call the dentist to reschedule
PROJECT: Health
CONTEXTS: @phone, morning
DUE: 2026-07-04
---END ITEM---

--- item: bbbb2222-0000-0000-0000-000000000002 ---
ACTION: note
PATH: notes/loro-crdt-reading.md
BODY:
# Loro CRDT reading list

A fence quoting our own format:

```
---END ITEM---
```

- movable trees paper
--- end item ---

---ITEM: cccc3333-0000-0000-0000-000000000003---
ACTION: skip
REASON: duplicate of "Call the dentist to reschedule"
---END ITEM---
"#;

    #[test]
    fn parses_fixture_all_three_kinds() {
        let props = parse_proposals(FIXTURE_RESPONSE).expect("parse");
        assert_eq!(props.len(), 3);

        assert_eq!(props[0].item_id, "aaaa1111-0000-0000-0000-000000000001");
        match &props[0].action {
            ProposalAction::Task {
                title,
                project_title,
                contexts,
                due,
            } => {
                assert_eq!(title, "Call the dentist to reschedule");
                assert_eq!(project_title.as_deref(), Some("Health"));
                // Bare `morning` gets the `@` prefix normalized on.
                assert_eq!(contexts, &["@phone", "@morning"]);
                assert_eq!(due.as_deref(), Some("2026-07-04"));
            }
            other => panic!("expected task, got {other:?}"),
        }

        assert_eq!(props[1].item_id, "bbbb2222-0000-0000-0000-000000000002");
        match &props[1].action {
            ProposalAction::Note { path, body } => {
                assert_eq!(path, "notes/loro-crdt-reading.md");
                assert!(body.starts_with("# Loro CRDT reading list"));
                // H5: the fenced close marker stays IN the body …
                assert!(body.contains("---END ITEM---"));
                // … and content after the fence survived.
                assert!(body.contains("movable trees paper"));
            }
            other => panic!("expected note, got {other:?}"),
        }

        match &props[2].action {
            ProposalAction::Skip { reason } => {
                assert!(reason.contains("duplicate"));
            }
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn h1_normalizes_crlf() {
        let resp = "---ITEM: x---\r\nACTION: task\r\nTITLE: Buy milk\r\n---END ITEM---\r\n";
        let props = parse_proposals(resp).unwrap();
        assert_eq!(props.len(), 1);
        assert!(matches!(
            &props[0].action,
            ProposalAction::Task { title, .. } if title == "Buy milk"
        ));
    }

    #[test]
    fn h2_missing_close_marker_errors() {
        let resp = "---ITEM: x---\nACTION: task\nTITLE: truncated mid-stream\n";
        let err = parse_proposals(resp).unwrap_err();
        assert!(matches!(
            err,
            AgentInboxError::MalformedResponse("missing `---END ITEM---` close marker", _)
        ));
    }

    #[test]
    fn optional_task_fields_default_empty() {
        let resp = "---ITEM: x---\nACTION: task\nTITLE: Buy milk\n---END ITEM---";
        let props = parse_proposals(resp).unwrap();
        match &props[0].action {
            ProposalAction::Task {
                project_title,
                contexts,
                due,
                ..
            } => {
                assert!(project_title.is_none());
                assert!(contexts.is_empty());
                assert!(due.is_none());
            }
            other => panic!("expected task, got {other:?}"),
        }
    }

    #[test]
    fn unknown_action_errors() {
        let resp = "---ITEM: x---\nACTION: event\nTITLE: t\n---END ITEM---";
        let err = parse_proposals(resp).unwrap_err();
        assert!(matches!(err, AgentInboxError::UnknownAction(a, id)
            if a == "event" && id == "x"));
    }

    #[test]
    fn task_without_title_errors() {
        let resp = "---ITEM: x---\nACTION: task\n---END ITEM---";
        let err = parse_proposals(resp).unwrap_err();
        assert!(matches!(err, AgentInboxError::MissingField("TITLE", _)));
    }

    #[test]
    fn note_without_path_errors() {
        let resp = "---ITEM: x---\nACTION: note\nBODY:\nbody\n---END ITEM---";
        let err = parse_proposals(resp).unwrap_err();
        assert!(matches!(err, AgentInboxError::MissingField("PATH", _)));
    }

    #[test]
    fn note_path_traversal_rejected() {
        let resp = "---ITEM: x---\nACTION: note\nPATH: ../../etc/notes.md\n---END ITEM---";
        assert!(parse_proposals(resp).is_err());
    }

    #[test]
    fn skip_without_reason_gets_default() {
        let resp = "---ITEM: x---\nACTION: skip\n---END ITEM---";
        let props = parse_proposals(resp).unwrap();
        assert!(matches!(
            &props[0].action,
            ProposalAction::Skip { reason } if reason == "no action proposed"
        ));
    }

    #[test]
    fn rejects_pure_prose() {
        let err = parse_proposals("Let me look at your inbox…").unwrap_err();
        assert!(matches!(err, AgentInboxError::MalformedResponse(_, _)));
    }

    #[test]
    fn missing_action_errors() {
        let resp = "---ITEM: x---\nTITLE: no action line\n---END ITEM---";
        let err = parse_proposals(resp).unwrap_err();
        assert!(matches!(err, AgentInboxError::MissingField("ACTION", _)));
    }
}
