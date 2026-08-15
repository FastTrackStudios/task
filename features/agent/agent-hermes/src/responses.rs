//! The rich turn transport — `POST /v1/responses` with
//! `stream: true`.
//!
//! `chat/completions` only ever gives us text deltas, so the whole
//! activity timeline stayed empty while the agent worked. The
//! gateway's Responses API emits the *structured* stream instead:
//! `response.output_item.added` with a `function_call` item when a
//! tool starts (name + arguments), a matching `function_call_output`
//! item when it returns (result), `response.output_text.delta` for
//! the answer, and a terminal `response.completed` / `response.failed`
//! carrying usage. Each of those becomes an [`AgentEvent`], so the
//! UI can narrate the turn as it happens.
//!
//! **Session continuity**: the responses handler derives its gateway
//! session id from `previous_response_id` chaining — it doesn't read
//! `X-Hermes-Session-Id` the way `chat/completions` does. So after
//! the first turn we send only the new user message plus the chain
//! id, which keeps the agent's gateway-side memory and skills bound
//! to one conversation. If the gateway forgets the chain (restart,
//! eviction) it answers `404 Previous response not found` and we
//! replay the full transcript, which also re-seeds the chain.
//!
//! Older gateways have no `/v1/responses` at all; [`TurnError::Unsupported`]
//! tells [`crate::stream::run_turn`] to fall back to the legacy path.

use std::collections::HashMap;

use agent_proto::event::{AgentEvent, SessionEvents};
use agent_proto::tool::{ToolCall, ToolStatus};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::stream::{TurnOutcome, pump_sse};
use crate::{Cancel, HermesConfig};

/// Why a turn couldn't run on this transport.
pub(crate) enum TurnError {
    /// The gateway doesn't serve `/v1/responses` — caller falls
    /// back to `chat/completions`.
    Unsupported,
    /// The chained `previous_response_id` is gone; caller retries
    /// with the full transcript.
    ChainLost,
    Failed(String),
}

/// A tool the agent started but hasn't finished.
struct PendingTool {
    name: String,
    args: String,
    started_at: DateTime<Utc>,
}

/// Accumulated state for one streamed turn.
struct RespState {
    outcome: TurnOutcome,
    pending: HashMap<String, PendingTool>,
    /// Tool calls in start order — lets us settle a result that
    /// arrives without a matching `call_id`.
    order: Vec<String>,
    error: Option<String>,
}

/// Human line for a tool call: the tool name plus the argument that
/// actually says what it's doing (`terminal · git status`). Falls
/// back to the bare name when nothing recognizable is present.
pub(crate) fn tool_title(name: &str, args_json: &str) -> String {
    const KEYS: &[&str] = &[
        "command",
        "cmd",
        "file_path",
        "path",
        "file",
        "query",
        "q",
        "url",
        "pattern",
        "expression",
        "text",
        "message",
        "name",
    ];
    let Ok(v) = serde_json::from_str::<Value>(args_json) else {
        return name.to_string();
    };
    let Some(obj) = v.as_object() else {
        return name.to_string();
    };
    // Priority keys first, then the first string value of any key —
    // tool schemas vary and *something* is better than a bare name.
    let picked = KEYS
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))
        .or_else(|| obj.values().find_map(Value::as_str));
    match picked {
        Some(arg) if !arg.trim().is_empty() => format!("{name} · {}", condense(arg, 96)),
        _ => name.to_string(),
    }
}

/// Collapse whitespace to single spaces and cap the length — tool
/// arguments and results are frequently multi-line blobs.
pub(crate) fn condense(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let head: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// The `output` of a `function_call_output` item — either a plain
/// string or the spec's content-part array.
fn output_text(item: &Value) -> String {
    match item.get("output") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Interpret a tool result: what to show, and whether it failed.
///
/// Hermes tools return a JSON envelope inside the text part —
/// `{"output": "...", "exit_code": 0, "error": null}` — so the real
/// output and the real failure signal are one level in. Tools that
/// return bare text fall back to the conventional error prefixes.
pub(crate) fn result_summary(text: &str) -> (String, bool) {
    if let Ok(Value::Object(env)) = serde_json::from_str::<Value>(text.trim()) {
        let err = env.get("error").filter(|v| !v.is_null());
        let failed = err.is_some()
            || env
                .get("exit_code")
                .and_then(Value::as_i64)
                .is_some_and(|c| c != 0);
        // Prefer the error text on failure, then the output field,
        // then the envelope itself (an unfamiliar tool schema).
        let body = err
            .and_then(Value::as_str)
            .or_else(|| env.get("output").and_then(Value::as_str))
            .map(str::to_string);
        if let Some(body) = body {
            return (body, failed);
        }
        if failed || env.contains_key("output") || env.contains_key("exit_code") {
            return (text.to_string(), failed);
        }
    }
    let head = text.trim_start().to_lowercase();
    let failed =
        head.starts_with("error") || head.starts_with("traceback") || head.starts_with("exception");
    (text.to_string(), failed)
}

/// Run one turn over the Responses API.
///
/// `messages` is the full transcript, oldest first, with this turn's
/// user message last; `previous_response_id` chains onto the prior
/// turn (empty replays everything).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_turn_responses(
    http: &reqwest::Client,
    cfg: &HermesConfig,
    session_id: &str,
    session_key: &str,
    message_id: &str,
    model: &str,
    messages: &[Value],
    previous_response_id: &str,
    events_tx: &SessionEvents,
    cancel: &Cancel,
) -> Result<TurnOutcome, TurnError> {
    let Some(last) = messages.last() else {
        return Err(TurnError::Failed("hermes: empty turn".to_string()));
    };
    let mut body = json!({
        "model": model,
        "stream": true,
        // Persist so the next turn can chain off this response and
        // keep the agent's gateway session (memory, skills) intact.
        "store": true,
    });
    if previous_response_id.is_empty() {
        body["input"] = Value::Array(messages.to_vec());
    } else {
        body["input"] = Value::Array(vec![last.clone()]);
        body["previous_response_id"] = Value::String(previous_response_id.to_string());
    }

    let mut req = http
        .post(format!("{}/responses", cfg.base_url))
        .header("Content-Type", "application/json")
        // Ignored by this handler today, but harmless and correct if
        // a future release honors it here too.
        .header("X-Hermes-Session-Id", session_key)
        .json(&body);
    if !cfg.api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", cfg.api_key));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| TurnError::Failed(format!("hermes: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        // 404/405 on the endpoint itself = old gateway. 404 naming a
        // missing previous response = a dropped chain, which the
        // caller retries with the full transcript.
        if status == reqwest::StatusCode::NOT_FOUND && text.contains("Previous response") {
            return Err(TurnError::ChainLost);
        }
        if matches!(
            status,
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED
        ) {
            return Err(TurnError::Unsupported);
        }
        return Err(TurnError::Failed(format!("hermes: HTTP {status}: {text}")));
    }

    let mut state = RespState {
        outcome: TurnOutcome::default(),
        pending: HashMap::new(),
        order: Vec::new(),
        error: None,
    };
    pump_sse(resp, cancel, |v| {
        handle_event(v, session_id, message_id, events_tx, &mut state);
        true
    })
    .await
    .map_err(TurnError::Failed)?;

    if let Some(e) = state.error {
        return Err(TurnError::Failed(e));
    }
    Ok(state.outcome)
}

/// Translate one Responses SSE payload into events + accumulation.
fn handle_event(
    v: &Value,
    session_id: &str,
    message_id: &str,
    events_tx: &SessionEvents,
    state: &mut RespState,
) {
    let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "response.created" => {
            if let Some(id) = v.pointer("/response/id").and_then(Value::as_str) {
                state.outcome.response_id = id.to_string();
            }
        }
        "response.output_text.delta" => {
            if let Some(d) = v.get("delta").and_then(Value::as_str) {
                if !d.is_empty() {
                    state.outcome.text.push_str(d);
                    let _ = events_tx.send(AgentEvent::MessageDelta {
                        session_id: session_id.to_string(),
                        message_id: message_id.to_string(),
                        content_delta: d.to_string(),
                    });
                }
            }
        }
        // Forward-compat: reasoning summaries stream under several
        // `response.reasoning*.delta` names across releases.
        k if k.starts_with("response.reasoning") && k.ends_with(".delta") => {
            if let Some(d) = v.get("delta").and_then(Value::as_str) {
                if !d.is_empty() {
                    state.outcome.reasoning.push_str(d);
                    let _ = events_tx.send(AgentEvent::ReasoningDelta {
                        session_id: session_id.to_string(),
                        message_id: message_id.to_string(),
                        delta: d.to_string(),
                    });
                }
            }
        }
        "response.output_item.added" => {
            let Some(item) = v.get("item") else { return };
            match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                "function_call" => start_tool(item, session_id, message_id, events_tx, state),
                "function_call_output" => {
                    finish_tool(item, session_id, message_id, events_tx, state);
                }
                _ => {}
            }
        }
        "response.completed" | "response.failed" => {
            if let Some(id) = v.pointer("/response/id").and_then(Value::as_str) {
                state.outcome.response_id = id.to_string();
            }
            let input = v
                .pointer("/response/usage/input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let output = v
                .pointer("/response/usage/output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if input > 0 || output > 0 {
                state.outcome.input_tokens = input;
                state.outcome.output_tokens = output;
                let _ = events_tx.send(AgentEvent::Metering {
                    session_id: session_id.to_string(),
                    input_tokens: input,
                    output_tokens: output,
                    estimated_cost_usd: state.outcome.cost_usd,
                });
            }
            if kind == "response.failed" {
                state.error = Some(
                    v.pointer("/response/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("the agent run failed")
                        .to_string(),
                );
            }
        }
        // `response.output_item.done` for a function_call carries no
        // new information (we settle on the result item), and the
        // message item's `done` duplicates the accumulated deltas.
        _ => {}
    }
}

fn start_tool(
    item: &Value,
    session_id: &str,
    message_id: &str,
    events_tx: &SessionEvents,
    state: &mut RespState,
) {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_string();
    let args = item
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let started_at = Utc::now();
    let _ = events_tx.send(AgentEvent::ToolStarted {
        tool_call: ToolCall {
            id: call_id.clone(),
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            name: name.clone(),
            category: String::new(),
            input_json: args.clone(),
            title: tool_title(&name, &args),
            status: ToolStatus::InProgress,
            output_json: String::new(),
            preview: String::new(),
            changes: Vec::new(),
            duration_ms: 0,
            started_at,
            finished_at: None,
            collab: None,
        },
    });
    state.order.push(call_id.clone());
    state.pending.insert(
        call_id,
        PendingTool {
            name,
            args,
            started_at,
        },
    );
}

fn finish_tool(
    item: &Value,
    session_id: &str,
    message_id: &str,
    events_tx: &SessionEvents,
    state: &mut RespState,
) {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // Prefer the exact call; otherwise settle the oldest one still
    // open, which is the order the gateway emits results in.
    let key = if state.pending.contains_key(&call_id) {
        call_id.clone()
    } else {
        state
            .order
            .iter()
            .find(|id| state.pending.contains_key(*id))
            .cloned()
            .unwrap_or(call_id.clone())
    };
    let pending = state.pending.remove(&key);
    let output = output_text(item);
    let now = Utc::now();
    let (name, args, started_at) = match pending {
        Some(p) => (p.name, p.args, p.started_at),
        None => ("tool".to_string(), String::new(), now),
    };
    let duration_ms = u32::try_from((now - started_at).num_milliseconds().max(0)).unwrap_or(0);
    let (output, failed) = result_summary(&output);
    let _ = events_tx.send(AgentEvent::ToolFinished {
        tool_call: ToolCall {
            id: key,
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            name: name.clone(),
            category: String::new(),
            input_json: args.clone(),
            title: tool_title(&name, &args),
            status: if failed {
                ToolStatus::Error
            } else {
                ToolStatus::Done
            },
            output_json: condense(&output, 4000),
            preview: condense(&output, 160),
            changes: Vec::new(),
            duration_ms,
            started_at,
            finished_at: Some(now),
            collab: None,
        },
    });
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


    fn state() -> RespState {
        RespState {
            outcome: TurnOutcome::default(),
            pending: HashMap::new(),
            order: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn tool_title_picks_the_telling_argument() {
        assert_eq!(
            tool_title("terminal", r#"{"command":"git status"}"#),
            "terminal · git status"
        );
        assert_eq!(
            tool_title("read_file", r#"{"file_path":"src/main.rs","offset":0}"#),
            "read_file · src/main.rs"
        );
        // No recognizable key: any string value beats a bare name.
        assert_eq!(tool_title("odd", r#"{"zzz":"payload"}"#), "odd · payload");
        // Nothing usable at all.
        assert_eq!(tool_title("think", "{}"), "think");
        assert_eq!(tool_title("think", "not json"), "think");
    }

    #[test]
    fn condense_flattens_and_caps() {
        assert_eq!(condense("  a\n\n b \t c ", 80), "a b c");
        assert_eq!(condense("abcdef", 4), "abc…");
    }

    #[test]
    fn output_text_accepts_string_and_parts() {
        assert_eq!(output_text(&json!({"output": "hi"})), "hi");
        assert_eq!(
            output_text(&json!({"output": [{"type":"input_text","text":"a"},{"text":"b"}]})),
            "a\nb"
        );
        assert_eq!(output_text(&json!({})), "");
    }

    #[test]
    fn tool_lifecycle_emits_started_then_finished_with_duration() {
        let (tx, rx) = sink();
        let mut st = state();
        handle_event(
            &json!({
                "type": "response.output_item.added",
                "item": {
                    "type": "function_call",
                    "call_id": "c1",
                    "name": "terminal",
                    "arguments": "{\"command\":\"ls\"}"
                }
            }),
            "s1",
            "m1",
            &tx,
            &mut st,
        );
        handle_event(
            &json!({
                "type": "response.output_item.added",
                "item": {
                    "type": "function_call_output",
                    "call_id": "c1",
                    "output": [{"type": "input_text", "text": "a.rs\nb.rs"}]
                }
            }),
            "s1",
            "m1",
            &tx,
            &mut st,
        );
        match rx.try_next().unwrap() {
            AgentEvent::ToolStarted { tool_call } => {
                assert_eq!(tool_call.title, "terminal · ls");
                assert_eq!(tool_call.status, ToolStatus::InProgress);
            }
            other => panic!("unexpected: {other:?}"),
        }
        match rx.try_next().unwrap() {
            AgentEvent::ToolFinished { tool_call } => {
                assert_eq!(tool_call.status, ToolStatus::Done);
                assert_eq!(tool_call.preview, "a.rs b.rs");
                assert!(tool_call.finished_at.is_some());
            }
            other => panic!("unexpected: {other:?}"),
        }
        assert!(st.pending.is_empty());
    }

    #[test]
    fn result_summary_unwraps_the_hermes_envelope() {
        // Verbatim from a live `terminal` call against the deployed
        // gateway (hermes-agent 0.19.0).
        let (body, failed) =
            result_summary(r#"{"output": "hi-from-task", "exit_code": 0, "error": null}"#);
        assert_eq!(body, "hi-from-task");
        assert!(!failed);

        let (body, failed) =
            result_summary(r#"{"output": "", "exit_code": 127, "error": "command not found"}"#);
        assert_eq!(body, "command not found");
        assert!(failed);

        // A non-zero exit with no error text still reads as a failure.
        let (_, failed) = result_summary(r#"{"output": "nope", "exit_code": 1, "error": null}"#);
        assert!(failed);
    }

    #[test]
    fn result_summary_falls_back_to_prefix_heuristics() {
        let (body, failed) = result_summary("Error: no such file");
        assert_eq!(body, "Error: no such file");
        assert!(failed);
        assert_eq!(result_summary("all good"), ("all good".to_string(), false));
        // An unfamiliar JSON schema passes through unchanged.
        let (body, failed) = result_summary(r#"{"rows": 3}"#);
        assert_eq!(body, r#"{"rows": 3}"#);
        assert!(!failed);
    }

    #[test]
    fn error_shaped_output_marks_the_tool_failed() {
        let (tx, rx) = sink();
        let mut st = state();
        handle_event(
            &json!({
                "type": "response.output_item.added",
                "item": {"type": "function_call", "call_id": "c1", "name": "terminal", "arguments": "{}"}
            }),
            "s1",
            "m1",
            &tx,
            &mut st,
        );
        let _ = rx.try_next();
        handle_event(
            &json!({
                "type": "response.output_item.added",
                "item": {"type": "function_call_output", "call_id": "c1", "output": "Error: no such file"}
            }),
            "s1",
            "m1",
            &tx,
            &mut st,
        );
        match rx.try_next().unwrap() {
            AgentEvent::ToolFinished { tool_call } => {
                assert_eq!(tool_call.status, ToolStatus::Error);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn result_without_a_matching_call_id_settles_the_oldest_open_tool() {
        let (tx, _rx) = sink();
        let mut st = state();
        for id in ["c1", "c2"] {
            handle_event(
                &json!({
                    "type": "response.output_item.added",
                    "item": {"type": "function_call", "call_id": id, "name": "t", "arguments": "{}"}
                }),
                "s1",
                "m1",
                &tx,
                &mut st,
            );
        }
        handle_event(
            &json!({
                "type": "response.output_item.added",
                "item": {"type": "function_call_output", "call_id": "unknown", "output": "ok"}
            }),
            "s1",
            "m1",
            &tx,
            &mut st,
        );
        assert!(!st.pending.contains_key("c1"));
        assert!(st.pending.contains_key("c2"));
    }

    #[test]
    fn text_deltas_accumulate_and_completion_carries_usage() {
        let (tx, _rx) = sink();
        let mut st = state();
        handle_event(
            &json!({"type": "response.created", "response": {"id": "resp_1"}}),
            "s1",
            "m1",
            &tx,
            &mut st,
        );
        for d in ["Hel", "lo"] {
            handle_event(
                &json!({"type": "response.output_text.delta", "delta": d}),
                "s1",
                "m1",
                &tx,
                &mut st,
            );
        }
        handle_event(
            &json!({
                "type": "response.completed",
                "response": {"id": "resp_1", "usage": {"input_tokens": 120, "output_tokens": 8}}
            }),
            "s1",
            "m1",
            &tx,
            &mut st,
        );
        assert_eq!(st.outcome.text, "Hello");
        assert_eq!(st.outcome.response_id, "resp_1");
        assert_eq!(st.outcome.input_tokens, 120);
        assert_eq!(st.outcome.output_tokens, 8);
        assert!(st.error.is_none());
    }

    #[test]
    fn failed_response_records_the_error_message() {
        let (tx, _rx) = sink();
        let mut st = state();
        handle_event(
            &json!({
                "type": "response.failed",
                "response": {"id": "r", "error": {"message": "model unavailable"}}
            }),
            "s1",
            "m1",
            &tx,
            &mut st,
        );
        assert_eq!(st.error.as_deref(), Some("model unavailable"));
    }

    #[test]
    fn reasoning_deltas_are_forwarded() {
        let (tx, rx) = sink();
        let mut st = state();
        handle_event(
            &json!({"type": "response.reasoning_summary_text.delta", "delta": "hmm"}),
            "s1",
            "m1",
            &tx,
            &mut st,
        );
        assert_eq!(st.outcome.reasoning, "hmm");
        assert!(matches!(
            rx.try_next().unwrap(),
            AgentEvent::ReasoningDelta { .. }
        ));
    }
}
