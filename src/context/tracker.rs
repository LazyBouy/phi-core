use super::token::{message_tokens, total_tokens};
use crate::types::*;

// ---------------------------------------------------------------------------
// Context tracking (real usage + estimates)
// ---------------------------------------------------------------------------

/// Tracks context size using real token counts from provider responses
/// combined with estimates for messages added after the last response.
///
/// This gives more accurate context size tracking than pure estimation,
/// since providers report actual token counts in their usage data.
///
/// # Example
///
/// ```rust
/// use phi_core::context::ContextTracker;
/// use phi_core::types::Usage;
///
/// let mut tracker = ContextTracker::new();
/// // After receiving an assistant response with usage data:
/// tracker.record_usage(&Usage { input: 1500, output: 200, ..Default::default() }, 3);
/// ```
/*
RUST QUIRK: Using `Option<usize>` for "not yet known" state

`last_usage_tokens: Option<usize>` means "either we have a real token count
(Some(n)), or we haven't received one yet (None)".

This is Rust's way of representing nullable data without null pointers.
There is no `null` or `None` in Rust — you must use `Option<T>` explicitly.
The compiler forces you to handle both cases, preventing null pointer exceptions.

Python analogy: last_usage_tokens: Optional[int] = None

The hybrid design strategy:
  - After each LLM response, record the REAL token count from provider usage data
  - For messages added after the last response, ESTIMATE with chars/4
  - Combine: real_base + estimated_trailing = accurate context size

This beats pure estimation because real token counts account for:
  - Unicode characters (multi-byte)
  - Special tokens (BOS, EOS, system prompt formatting)
  - Provider-specific tokenization differences
*/
pub struct ContextTracker {
    /// Last known total token count from provider usage (None = no response yet)
    last_usage_tokens: Option<usize>,
    /// Index into the message list of the last assistant response with usage (None = no response yet)
    last_usage_index: Option<usize>,
}

impl ContextTracker {
    pub fn new() -> Self {
        Self {
            last_usage_tokens: None,
            last_usage_index: None,
        }
    }

    /// Record usage from an assistant response.
    ///
    /// Call this after each assistant message to update the tracker
    /// with real token counts from the provider.
    pub fn record_usage(&mut self, usage: &Usage, message_index: usize) {
        let total = usage.input + usage.output + usage.cache_read + usage.cache_write;
        if total > 0 {
            self.last_usage_tokens = Some(total as usize);
            self.last_usage_index = Some(message_index);
        }
    }

    /// Estimate current context size.
    ///
    /// Uses real usage from the last assistant response as a baseline,
    /// then adds estimates (chars/4) for any messages added since.
    /// Falls back to pure estimation if no usage data is available.
    pub fn estimate_context_tokens(&self, messages: &[AgentMessage]) -> usize {
        match (self.last_usage_tokens, self.last_usage_index) {
            (Some(usage_tokens), Some(idx)) if idx < messages.len() => {
                let trailing: usize = messages[idx + 1..].iter().map(message_tokens).sum();
                usage_tokens + trailing
            }
            _ => total_tokens(messages),
        }
    }

    /// Reset tracking (e.g. after compaction replaces messages).
    pub fn reset(&mut self) {
        self.last_usage_tokens = None;
        self.last_usage_index = None;
    }
}

impl Default for ContextTracker {
    fn default() -> Self {
        Self::new()
    }
}
