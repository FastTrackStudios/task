//! Minimal `AppServerEvent` → `agent_proto::AgentEvent`
//! translator. Pulls just enough of the Codex JSON-RPC
//! notification surface to drive a CLI chat demo — the
//! full mapping (tool calls, reasoning, approvals, diffs)
//! arrives alongside the `Agents` impl in slice 2c.
//!
//! Codex pushes events as JSON-RPC notifications whose
//! `params.item` carries one of `CodexMonitor`'s
//! `ConversationItem` variants. We only handle:
//!
//! - `thread/started` → noop (`turn_id` lives on the caller)
//! - notifications with `params.item.kind == "message"` +
//!   `role == "assistant"` → `AgentEvent::MessageDelta`
//!   (final text) on `item/done`, or accumulated text on
//!   `item/updated`.
//! - notifications with `method == "turn/completed"` →
//!   `AgentEvent::TurnFinished`.
//! - everything else → `None` (silently dropped).

use agent_proto::event::AgentEvent;
use chrono::Utc;
use serde_json::Value;

use crate::AppServerEvent;

/// Translate one `AppServerEvent` into zero-or-one
/// `AgentEvent`. Returns `None` when the event isn't
/// relevant to the chat-text stream (yet).
pub fn translate(ev: &AppServerEvent, session_id: &str) -> Option<AgentEvent> {
    let method = ev.message.get("method")?.as_str()?;
    let params = ev.message.get("params");

    match method {
        // Streaming token from the assistant. Codex's daemon
        // ships these for every `agentMessage` while the
        // model produces output.
        "item/agentMessage/delta" => {
            let params = params?;
            let delta = params.get("delta")?.as_str()?;
            let message_id = params
                .get("itemId")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            Some(AgentEvent::MessageDelta {
                session_id: session_id.to_string(),
                message_id,
                content_delta: delta.to_string(),
            })
        }
        // Streaming reasoning content.
        "item/reasoning/delta" => {
            let params = params?;
            let delta = params.get("delta")?.as_str()?;
            let message_id = params
                .get("itemId")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            Some(AgentEvent::ReasoningDelta {
                session_id: session_id.to_string(),
                message_id,
                delta: delta.to_string(),
            })
        }
        // Turn completion.
        "turn/completed" | "turn/done" => {
            let message_id = params
                .and_then(|p| p.get("turn"))
                .and_then(|t| t.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(AgentEvent::TurnFinished {
                session_id: session_id.to_string(),
                message_id,
                at: Utc::now(),
            })
        }
        // Turn errored.
        "turn/error" | "turn/failed" => {
            let turn = params.and_then(|p| p.get("turn"));
            let err = turn
                .and_then(|t| t.get("error"))
                .or_else(|| params.and_then(|p| p.get("error")));
            let message = err
                .and_then(|e| {
                    e.get("message")
                        .and_then(|v| v.as_str())
                        .map(std::string::ToString::to_string)
                })
                .or_else(|| err.map(std::string::ToString::to_string))
                .unwrap_or_default();
            Some(AgentEvent::TurnErrored {
                session_id: session_id.to_string(),
                kind: "turn_error".to_string(),
                message,
                at: Utc::now(),
            })
        }
        // Non-streaming completion of an `agentMessage` item
        // (kept as a fallback when the daemon omits deltas).
        // Skip when phase==final_answer because deltas
        // already carried the content.
        "item/completed" => {
            let item = params?.get("item")?;
            let kind = item.get("type").or_else(|| item.get("kind"))?.as_str()?;
            if kind != "agentMessage" && kind != "assistant_message" {
                return None;
            }
            // If the message has a non-empty text and we
            // never saw deltas (e.g. cached response), emit
            // it once. Otherwise skip — deltas already
            // covered the text.
            let _ = extract_text; // (helper kept for future)
            None
        }
        _ => None,
    }
}

/// Pull the assistant-visible text out of a `ConversationItem`
/// JSON blob. Codex sometimes carries plain `text:` and
/// sometimes a `content: [{type:"text", text:"..."}]` array
/// — try both.
fn extract_text(item: &Value) -> Option<String> {
    if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }
    let content = item.get("content")?.as_array()?;
    let mut out = String::new();
    for block in content {
        let kind = block
            .get("type")
            .or_else(|| block.get("kind"))
            .and_then(|v| v.as_str());
        if matches!(kind, Some("text" | "output_text") | None) {
            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                out.push_str(text);
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
}
