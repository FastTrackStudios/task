//! Extended-thinking block. Some providers expose the
//! model's reasoning trace separately from the final
//! response (Anthropic's "thinking" mode, `OpenAI` o1's
//! reasoning summary, Hermes's reasoning effort levels).
//! Stored on the message but distinct so UIs can collapse /
//! redact it independently.

use facet::Facet;

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct ReasoningBlock {
    /// One-line synopsis. Surfaced in the timeline when
    /// the full content is collapsed.
    pub summary: String,
    /// Full reasoning text. May be very long.
    pub content: String,
    /// Effort hint at the time of generation
    /// (`"none" | "minimal" | "low" | "medium" | "high"`).
    /// Empty when not reported.
    pub effort: String,
    /// Whether the provider returned a structured
    /// chain-of-thought (vs. a free-form summary).
    pub structured: bool,
}
