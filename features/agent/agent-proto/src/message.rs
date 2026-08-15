//! Message — one turn in a session. Models the
//! multimodal-friendly `role + content blocks` shape used by
//! Anthropic / `OpenAI` / Hermes.
//!
//! For tool-calling, content blocks carry `ToolUse` /
//! `ToolResult` variants inline (matches Hermes's
//! `Message.content` list-form) rather than a parallel
//! tool-calls array. External-monitor backends translate
//! their own representations (e.g. `CodexMonitor`'s
//! `ConversationItem` union) into this shape.

use chrono::{DateTime, Utc};
use facet::Facet;

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: Role,
    /// Ordered content blocks. Single-block string messages
    /// carry one `Text` block; multimodal carries
    /// interleaved `Text` / `Image` / `ToolUse` /
    /// `ToolResult`.
    pub content: Vec<ContentBlock>,
    /// Whether this message is still streaming. Live UIs
    /// render partial state when `true`.
    pub partial: bool,
    /// Error marker — set when the backend produced an
    /// error during this turn. Body in `error_text`.
    pub errored: bool,
    pub error_text: String,
    /// Reasoning trace, if the model exposed one. See
    /// [`crate::reasoning::ReasoningBlock`].
    pub reasoning: Option<crate::reasoning::ReasoningBlock>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum Role {
    User,
    Assistant,
    /// A tool-result turn the agent inserts into context
    /// after invoking a tool. Distinct from `Assistant`
    /// because some providers want them separated.
    Tool,
    /// System / instruction injection. Rarely surfaced in
    /// UI but kept in history for replay.
    System,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub enum ContentBlock {
    /// Plain text. The most common variant.
    Text { text: String },
    /// Image content. Either an inline byte payload or a
    /// reference to an `Attachment`.
    Image {
        mime: String,
        /// Reference to an `Attachment` if the bytes live
        /// off-message; empty when the bytes are inline.
        attachment_id: String,
        /// Inline bytes. Backends prefer attachment refs
        /// for non-tiny images.
        bytes: Vec<u8>,
    },
    /// Agent invoking a tool. Result lands in a later
    /// `ToolResult` block (typically next message).
    ToolUse {
        id: String,
        name: String,
        /// Tool args, opaque JSON.
        input_json: String,
    },
    /// Result of a previous `ToolUse`. `tool_use_id`
    /// matches the originating call.
    ToolResult {
        tool_use_id: String,
        /// Result content — string or list of nested
        /// blocks (matches Anthropic's spec). Encoded as
        /// JSON for transport.
        content_json: String,
        /// Whether the tool reported an error.
        is_error: bool,
    },
    /// Free-form code execution placeholder (some backends
    /// expose this distinctly from `ToolUse`).
    Code { language: String, source: String },
}
