use crate::types::*;

// ---------------------------------------------------------------------------
// Token estimation
// ---------------------------------------------------------------------------

/// Rough token estimate: ~4 chars per token for English text.
/// Good enough for context budgeting. Use tiktoken-rs for precision.
/*
RUST QUIRK: `div_ceil` — integer division rounding UP

  text.len() / 4  → rounds DOWN (e.g., 5/4 = 1, but we want 2 tokens for 5 chars)
  text.len().div_ceil(4) → rounds UP (5/4 = 2)

`div_ceil(n)` is equivalent to (len + n - 1) / n but cleaner.
It's a method on integer types (usize, u64, etc.) added in Rust 1.73.

Python analogy: math.ceil(len(text) / 4) or -(-len(text) // 4)
*/
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Estimate tokens for a single message
pub fn message_tokens(msg: &AgentMessage) -> usize {
    match msg {
        AgentMessage::Llm(lm) => match &lm.message {
            Message::User { content, .. } => content_tokens(content) + 4,
            Message::Assistant { content, .. } => content_tokens(content) + 4,
            Message::ToolResult {
                content, tool_name, ..
            } => content_tokens(content) + estimate_tokens(tool_name) + 8,
        },
        AgentMessage::Extension(ext) => estimate_tokens(&ext.data.to_string()) + 4,
    }
}

/*
content_tokens — sum token estimates across a slice of Content items.

RUST QUIRK: Iterator chain `.iter().map(...).sum()`

This is the idiomatic Rust "transform + aggregate" pattern:
  .iter()       → create a lazy iterator over the slice (no allocation)
  .map(|c| ...) → transform each element (still lazy)
  .sum()        → consume the iterator, adding all values together

Python analogy: sum(estimate(c) for c in content)

The chain is lazy: no work happens until `.sum()` is called. This avoids
allocating an intermediate Vec. Think of it as a pipeline specification,
not a sequence of operations.

`.clamp(min, max)` — constrain a value to a range:
  (raw_bytes / 750).clamp(85, 16_000)
  Python analogy: max(85, min(raw_bytes // 750, 16000))
*/
pub fn content_tokens(content: &[Content]) -> usize {
    content
        .iter()
        .map(|c| match c {
            Content::Text { text } => estimate_tokens(text),
            Content::Image { data, .. } => {
                // Estimate tokens from base64 data length:
                // base64 len * 3/4 = raw bytes; ~750 bytes per token for images.
                // Floor at 85 (Anthropic minimum), cap at 16000.
                let raw_bytes = data.len() * 3 / 4;
                (raw_bytes / 750).clamp(85, 16_000)
            }
            Content::Thinking { thinking, .. } => estimate_tokens(thinking),
            Content::ToolCall {
                name, arguments, ..
            } => {
                // +8: overhead for the tool call structure (id field, JSON brackets, etc.)
                estimate_tokens(name) + estimate_tokens(&arguments.to_string()) + 8
            }
        })
        .sum()
}

/// Estimate total tokens for a message list
pub fn total_tokens(messages: &[AgentMessage]) -> usize {
    messages.iter().map(message_tokens).sum()
}
