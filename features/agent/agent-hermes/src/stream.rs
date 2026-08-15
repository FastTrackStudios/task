//! The streaming turn worker.
//!
//! Two transports, same [`AgentEvent`] output:
//!
//! 1. [`crate::responses`] — `POST /v1/responses`, the structured
//!    stream (tool starts with arguments, tool results, usage). This
//!    is what makes the UI's activity timeline light up, so it's
//!    tried first.
//! 2. This module's `chat/completions` path — text deltas and usage
//!    only, kept as the fallback for gateways predating the
//!    Responses API.
//!
//! Hermes extends the legacy stream with custom event objects (e.g.
//! `hermes.tool.progress`); anything that isn't a
//! `chat.completion.chunk` is surfaced as `ToolProgress` / `Warning`
//! rather than dropped, so the UI never goes silent mid-tool.

use std::sync::atomic::{AtomicBool, Ordering};

use agent_proto::event::{AgentEvent, SessionEvents};
use agent_proto::message::{ContentBlock, Message, Role};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::responses::{TurnError, run_turn_responses};
use crate::{Cancel, HermesConfig};

/// Outcome of one streamed turn.
#[derive(Default)]
pub(crate) struct TurnOutcome {
    pub text: String,
    pub reasoning: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Priced from the models.dev catalog when the effective model
    /// is a known provider model; `0.0` for the gateway's own
    /// façade model (whose cost we can't attribute).
    pub cost_usd: f32,
    /// Chain id for the next turn (Responses transport only).
    pub response_id: String,
}

/// Cost in USD for a turn's token counts, from the cached catalog.
pub(crate) fn price_turn(model: &str, input_tokens: u64, output_tokens: u64) -> f32 {
    let Some((cin, cout)) = crate::discovery::cached_price(model) else {
        return 0.0;
    };
    let dollars =
        (input_tokens as f64 / 1_000_000.0) * cin + (output_tokens as f64 / 1_000_000.0) * cout;
    dollars as f32
}

/// Drive an SSE body, handing each decoded `data:` payload to
/// `on_data` (returning `false` stops the pump). Frames may straddle
/// TCP chunks, so the tail stays buffered; `event:` lines are
/// ignored — every payload we care about names its own `type`.
pub(crate) async fn pump_sse<F>(
    resp: reqwest::Response,
    cancel: &Cancel,
    mut on_data: F,
) -> Result<(), String>
where
    F: FnMut(&Value) -> bool,
{
    let mut buf = String::new();
    let mut stream = resp.bytes_stream();
    loop {
        // Race the body against Stop. Polling between chunks would
        // leave a cancel unnoticed for as long as the agent stays
        // quiet — i.e. the whole of a long tool call.
        let chunk = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            next = stream.next() => match next {
                Some(c) => c,
                None => break,
            },
        };
        let bytes = chunk.map_err(|e| format!("hermes stream: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim().to_string();
            buf.drain(..=pos);
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                return Ok(());
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if !on_data(&v) {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Serialize the Task-side transcript into the OpenAI `messages`
/// array. Only text blocks travel; tool blocks are backend-internal.
pub(crate) fn wire_messages(history: &[Message]) -> Vec<Value> {
    history
        .iter()
        .filter(|m| !m.errored)
        .filter_map(|m| {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
                Role::Tool => return None,
            };
            let text: String = m
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() {
                return None;
            }
            Some(json!({ "role": role, "content": text }))
        })
        .collect()
}

/// Run one streaming turn against the gateway, preferring the
/// structured Responses transport and falling back to
/// `chat/completions` when the gateway doesn't serve it.
///
/// `previous_response_id` chains this turn onto the last one so the
/// agent keeps its gateway-side session; a dropped chain silently
/// replays the full transcript. Emits `MessageDelta` /
/// `ReasoningDelta` / `ToolStarted` / `ToolFinished` / `Metering` on
/// `events_tx` as the turn runs; a cancelled turn returns what it
/// has with no error.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_turn(
    http: &reqwest::Client,
    cfg: &HermesConfig,
    session_id: &str,
    session_key: &str,
    message_id: &str,
    model: &str,
    messages: Vec<Value>,
    previous_response_id: &str,
    legacy_transport: &AtomicBool,
    events_tx: &SessionEvents,
    cancel: &Cancel,
) -> Result<TurnOutcome, String> {
    let mut chain = previous_response_id;
    // Two attempts at the rich transport: the second only happens when
    // the gateway lost our chain and we replay the transcript.
    if !legacy_transport.load(Ordering::Relaxed) {
        for attempt in 0..2 {
            match run_turn_responses(
                http,
                cfg,
                session_id,
                session_key,
                message_id,
                model,
                &messages,
                chain,
                events_tx,
                cancel,
            )
            .await
            {
                Ok(mut outcome) => {
                    outcome.cost_usd =
                        price_turn(model, outcome.input_tokens, outcome.output_tokens);
                    if outcome.cost_usd > 0.0 {
                        let _ = events_tx.send(AgentEvent::Metering {
                            session_id: session_id.to_string(),
                            input_tokens: outcome.input_tokens,
                            output_tokens: outcome.output_tokens,
                            estimated_cost_usd: outcome.cost_usd,
                        });
                    }
                    return Ok(outcome);
                }
                // The gateway forgot the chain (restart, eviction) —
                // replay everything, which also re-seeds it.
                Err(TurnError::ChainLost) if attempt == 0 => {
                    let _ = events_tx.send(AgentEvent::Warning {
                        session_id: session_id.to_string(),
                        kind: "session".to_string(),
                        message: "gateway session expired — replaying the transcript".to_string(),
                    });
                    chain = "";
                }
                Err(TurnError::ChainLost) => {
                    return Err("hermes: lost the session chain".to_string());
                }
                // Pre-Responses gateway: latch the verdict so later
                // turns don't pay the failed round-trip.
                Err(TurnError::Unsupported) => {
                    legacy_transport.store(true, Ordering::Relaxed);
                    break;
                }
                Err(TurnError::Failed(e)) => return Err(e),
            }
        }
    }
    run_turn_chat(
        http,
        cfg,
        session_id,
        session_key,
        message_id,
        model,
        messages,
        events_tx,
        cancel,
    )
    .await
}

/// The legacy `chat/completions` transport — text and usage only.
#[allow(clippy::too_many_arguments)]
async fn run_turn_chat(
    http: &reqwest::Client,
    cfg: &HermesConfig,
    session_id: &str,
    session_key: &str,
    message_id: &str,
    model: &str,
    messages: Vec<Value>,
    events_tx: &SessionEvents,
    cancel: &Cancel,
) -> Result<TurnOutcome, String> {
    let url = format!("{}/chat/completions", cfg.base_url);
    let body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    let mut req = http
        .post(&url)
        .header("Content-Type", "application/json")
        // Scope the gateway's own memory/skills to this Task
        // session (stable across turns).
        .header("X-Hermes-Session-Id", session_key)
        .header("X-Hermes-Session-Key", session_key)
        .json(&body);
    if !cfg.api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", cfg.api_key));
    }

    let resp = req.send().await.map_err(|e| format!("hermes: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("hermes: HTTP {status}: {text}"));
    }

    let mut outcome = TurnOutcome::default();
    pump_sse(resp, cancel, |v| {
        handle_chunk(v, session_id, message_id, events_tx, &mut outcome);
        true
    })
    .await?;
    outcome.cost_usd = price_turn(model, outcome.input_tokens, outcome.output_tokens);
    Ok(outcome)
}

/// Translate one parsed stream object into events + accumulation.
fn handle_chunk(
    v: &Value,
    session_id: &str,
    message_id: &str,
    events_tx: &SessionEvents,
    outcome: &mut TurnOutcome,
) {
    let object = v.get("object").and_then(Value::as_str).unwrap_or_default();

    // Hermes custom events ride the same stream. Known today:
    // `hermes.tool.progress`; render anything hermes.* unknown as a
    // status warning so the UI never goes silent mid-tool.
    if let Some(kind) = object.strip_prefix("hermes.") {
        let preview = v
            .get("message")
            .or_else(|| v.get("preview"))
            .or_else(|| v.get("detail"))
            .and_then(Value::as_str)
            .unwrap_or(kind)
            .to_string();
        if kind.starts_with("tool") {
            let _ = events_tx.send(AgentEvent::ToolProgress {
                tool_call_id: v
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or(message_id)
                    .to_string(),
                preview,
                progress: v
                    .get("progress")
                    .and_then(Value::as_f64)
                    .map_or(-1.0, |p| p as f32),
            });
        } else {
            let _ = events_tx.send(AgentEvent::Warning {
                session_id: session_id.to_string(),
                kind: format!("hermes.{kind}"),
                message: preview,
            });
        }
        return;
    }

    if let Some(delta) = v.pointer("/choices/0/delta") {
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                outcome.text.push_str(text);
                let _ = events_tx.send(AgentEvent::MessageDelta {
                    session_id: session_id.to_string(),
                    message_id: message_id.to_string(),
                    content_delta: text.to_string(),
                });
            }
        }
        // Hybrid reasoners (Hermes 4) stream thinking separately.
        if let Some(think) = delta.get("reasoning_content").and_then(Value::as_str) {
            if !think.is_empty() {
                outcome.reasoning.push_str(think);
                let _ = events_tx.send(AgentEvent::ReasoningDelta {
                    session_id: session_id.to_string(),
                    message_id: message_id.to_string(),
                    delta: think.to_string(),
                });
            }
        }
    }

    if let Some(usage) = v.get("usage") {
        let input = usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if input > 0 || output > 0 {
            outcome.input_tokens = input;
            outcome.output_tokens = output;
            let _ = events_tx.send(AgentEvent::Metering {
                session_id: session_id.to_string(),
                input_tokens: input,
                output_tokens: output,
                estimated_cost_usd: 0.0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Not re-exported by the parent module — `use super::*` doesn't
    // reach it, so the tap type is named directly.
    use agent_proto::event::EventTap;

    /// Test event sink + its tap. `SessionEvents::send` mirrors into
    /// the tap synchronously, so `tap.try_next()` sees whatever the
    /// code under test just published.
    fn sink() -> (SessionEvents, EventTap) {
        SessionEvents::tapped("s1")
    }

    use chrono::Utc;

    fn msg(role: Role, text: &str) -> Message {
        Message {
            id: "m".into(),
            session_id: "s".into(),
            role,
            content: vec![ContentBlock::Text { text: text.into() }],
            partial: false,
            errored: false,
            error_text: String::new(),
            reasoning: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn wire_messages_maps_roles_and_skips_tool_and_errored() {
        let mut errored = msg(Role::Assistant, "boom");
        errored.errored = true;
        let history = vec![
            msg(Role::User, "hi"),
            msg(Role::Assistant, "hello"),
            msg(Role::Tool, "tool noise"),
            errored,
        ];
        let wire = wire_messages(&history);
        assert_eq!(wire.len(), 2);
        assert_eq!(wire[0]["role"], "user");
        assert_eq!(wire[1]["role"], "assistant");
    }

    #[test]
    fn chunk_deltas_accumulate_and_emit() {
        let (tx, rx) = sink();
        let mut outcome = TurnOutcome::default();
        let chunk: Value = serde_json::json!({
            "object": "chat.completion.chunk",
            "choices": [{"delta": {"content": "Hel"}}],
        });
        handle_chunk(&chunk, "s1", "m1", &tx, &mut outcome);
        let chunk2: Value = serde_json::json!({
            "object": "chat.completion.chunk",
            "choices": [{"delta": {"content": "lo"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2},
        });
        handle_chunk(&chunk2, "s1", "m1", &tx, &mut outcome);
        assert_eq!(outcome.text, "Hello");
        assert_eq!(outcome.input_tokens, 10);
        assert!(matches!(
            rx.try_next().unwrap(),
            AgentEvent::MessageDelta { .. }
        ));
    }

    #[test]
    fn hermes_tool_event_becomes_tool_progress() {
        let (tx, rx) = sink();
        let mut outcome = TurnOutcome::default();
        let ev: Value = serde_json::json!({
            "object": "hermes.tool.progress",
            "message": "running terminal command",
            "progress": 0.5,
        });
        handle_chunk(&ev, "s1", "m1", &tx, &mut outcome);
        match rx.try_next().unwrap() {
            AgentEvent::ToolProgress {
                preview, progress, ..
            } => {
                assert_eq!(preview, "running terminal command");
                assert!((progress - 0.5).abs() < f32::EPSILON);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
