use crate::types::*;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// TokenCounter trait (REQ-162)
// ---------------------------------------------------------------------------

/// Pluggable token counting strategy.
///
/// The default implementation ([`HeuristicTokenCounter`]) uses a ~4 chars/token
/// heuristic — fast and sufficient for context budgeting. Provide a custom
/// implementation for model-specific tokenizers (e.g., tiktoken for OpenAI models,
/// or Anthropic's native token-counting API).
///
/// Only `estimate_text` needs to be overridden. The higher-level methods
/// (`estimate_content`, `estimate_message`, `estimate_messages`) have default
/// implementations that delegate to `estimate_text`.
///
/// # Example
///
/// ```
/// use phi_core::context::token::{TokenCounter, HeuristicTokenCounter};
///
/// let counter = HeuristicTokenCounter;
/// assert_eq!(counter.estimate_text("hello"), 2); // 5 chars / 4 = 2 (rounded up)
/// ```
pub trait TokenCounter: Send + Sync {
    /// Estimate tokens for a raw text string.
    fn estimate_text(&self, text: &str) -> usize;

    /// Estimate tokens for a slice of Content blocks.
    fn estimate_content(&self, content: &[Content]) -> usize {
        content
            .iter()
            .map(|c| match c {
                Content::Text { text } => self.estimate_text(text),
                Content::Image { data, .. } => {
                    let raw_bytes = data.len() * 3 / 4;
                    (raw_bytes / 750).clamp(85, 16_000)
                }
                Content::Thinking { thinking, .. } => self.estimate_text(thinking),
                Content::ToolCall {
                    name, arguments, ..
                } => self.estimate_text(name) + self.estimate_text(&arguments.to_string()) + 8,
            })
            .sum()
    }

    /// Estimate tokens for a single message.
    fn estimate_message(&self, msg: &AgentMessage) -> usize {
        match msg {
            AgentMessage::Llm(lm) => match &lm.message {
                Message::User { content, .. } => self.estimate_content(content) + 4,
                Message::Assistant { content, .. } => self.estimate_content(content) + 4,
                Message::ToolResult {
                    content, tool_name, ..
                } => self.estimate_content(content) + self.estimate_text(tool_name) + 8,
            },
            AgentMessage::Extension(ext) => self.estimate_text(&ext.data.to_string()) + 4,
        }
    }

    /// Estimate total tokens for a message list.
    fn estimate_messages(&self, msgs: &[AgentMessage]) -> usize {
        msgs.iter().map(|m| self.estimate_message(m)).sum()
    }
}

// ---------------------------------------------------------------------------
// HeuristicTokenCounter (default)
// ---------------------------------------------------------------------------

/// Default token counter: ~4 chars per token (heuristic for English text).
///
/// Good enough for context budgeting and compaction threshold decisions.
/// Use a model-specific tokenizer (e.g., tiktoken) for precision.
pub struct HeuristicTokenCounter;

impl TokenCounter for HeuristicTokenCounter {
    fn estimate_text(&self, text: &str) -> usize {
        text.len().div_ceil(4)
    }
}

// ---------------------------------------------------------------------------
// Free functions (backward-compatible wrappers)
// ---------------------------------------------------------------------------

/// Rough token estimate: ~4 chars per token for English text.
/// See [`TokenCounter`] for pluggable alternatives.
pub fn estimate_tokens(text: &str) -> usize {
    HeuristicTokenCounter.estimate_text(text)
}

/// Estimate tokens for a single message.
pub fn message_tokens(msg: &AgentMessage) -> usize {
    HeuristicTokenCounter.estimate_message(msg)
}

/// Estimate tokens for a Content slice.
pub fn content_tokens(content: &[Content]) -> usize {
    HeuristicTokenCounter.estimate_content(content)
}

/// Estimate total tokens for a message list.
pub fn total_tokens(messages: &[AgentMessage]) -> usize {
    HeuristicTokenCounter.estimate_messages(messages)
}

// ---------------------------------------------------------------------------
// Helper: resolve counter from optional
// ---------------------------------------------------------------------------

/// Returns the provided counter, or `HeuristicTokenCounter` if `None`.
pub fn resolve_counter(counter: Option<&Arc<dyn TokenCounter>>) -> &dyn TokenCounter {
    counter
        .map(|c| c.as_ref() as &dyn TokenCounter)
        .unwrap_or(&HeuristicTokenCounter)
}
