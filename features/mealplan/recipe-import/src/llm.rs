//! Stage-3 LLM transport — a minimal Anthropic Messages API client
//! behind the [`LlmClient`] trait so synthesis is unit-testable with
//! a mock (no key, no network).
//!
//! There is deliberately no in-tree LLM client to reuse: the agent
//! feature drives external CLIs (`codex app-server`) over stdio, not
//! HTTP. This is the workspace's first HTTP LLM call site, kept
//! feature-local on purpose.

use std::time::Duration;

use serde::Serialize;

use crate::error::ImportError;

/// One conversation turn. The retry path appends the model's failed
/// attempt + the parser diagnostics, so the trait takes a transcript
/// rather than a single prompt.
#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

impl ChatMessage {
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// Anything that can turn (system, transcript) into model text.
/// Implemented by [`AnthropicClient`] for real use and by mocks in
/// the synthesis tests.
pub trait LlmClient {
    fn complete(
        &self,
        system: &str,
        messages: &[ChatMessage],
    ) -> impl std::future::Future<Output = Result<String, ImportError>> + Send;
}

/// Raw-HTTP Anthropic Messages API client (`POST /v1/messages`).
/// Rust has no official Anthropic SDK, so this speaks the wire format
/// directly: `x-api-key` + `anthropic-version: 2023-06-01`.
pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: String,
    model: String,
    base_url: String,
}

/// Default model for recipe synthesis. Override with
/// `TASK_RECIPE_IMPORT_MODEL`.
pub const DEFAULT_MODEL: &str = "claude-opus-4-8";

impl AnthropicClient {
    /// Build from the environment: requires `ANTHROPIC_API_KEY`;
    /// honors `TASK_RECIPE_IMPORT_MODEL` and `ANTHROPIC_BASE_URL`.
    /// `None` when no key is configured — the CLI then asks the user
    /// to either export a key or pass `--offline`.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())?;
        let model = std::env::var("TASK_RECIPE_IMPORT_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());
        Some(Self::new(api_key, model, base_url))
    }

    #[must_use]
    pub fn new(api_key: String, model: String, base_url: String) -> Self {
        let http = reqwest::Client::builder()
            // Synthesis is one mid-sized completion; generous but
            // bounded. (Anthropic SDK default is 10 minutes.)
            .timeout(Duration::from_secs(300))
            .build()
            .expect("reqwest client");
        Self {
            http,
            api_key,
            model,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: &'a [ChatMessage],
}

impl LlmClient for AnthropicClient {
    async fn complete(
        &self,
        system: &str,
        messages: &[ChatMessage],
    ) -> Result<String, ImportError> {
        let body = MessagesRequest {
            model: &self.model,
            // A whole .cook file is small; 8K leaves ample room.
            max_tokens: 8192,
            system,
            messages,
        };
        let resp = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| ImportError::Llm(format!("request: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ImportError::Llm(format!("read response: {e}")))?;
        if !status.is_success() {
            return Err(ImportError::Llm(format!("HTTP {status}: {text}")));
        }

        let json: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| ImportError::Llm(format!("bad json: {e}")))?;

        if json["stop_reason"].as_str() == Some("refusal") {
            return Err(ImportError::Llm("model refused the request".into()));
        }

        // Concatenate the text blocks; thinking blocks (if any) are
        // skipped by the type filter.
        let out: String = json["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b["type"].as_str() == Some("text"))
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        if out.trim().is_empty() {
            return Err(ImportError::Llm(format!(
                "empty completion (stop_reason: {})",
                json["stop_reason"].as_str().unwrap_or("?")
            )));
        }
        if json["stop_reason"].as_str() == Some("max_tokens") {
            return Err(ImportError::Llm(
                "completion truncated at max_tokens".into(),
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_matches_the_wire_format() {
        let messages = vec![ChatMessage::user("hi"), ChatMessage::assistant("yo")];
        let req = MessagesRequest {
            model: DEFAULT_MODEL,
            max_tokens: 8192,
            system: "sys",
            messages: &messages,
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["model"], "claude-opus-4-8");
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][1]["role"], "assistant");
        assert_eq!(v["system"], "sys");
    }
}
