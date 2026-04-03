use super::config::ContextConfig;
use super::token::*;
use crate::types::*;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Tiered compaction
// ---------------------------------------------------------------------------

/// Compact messages to fit within the token budget using tiered strategy.
///
/// - Level 1: Truncate tool outputs (keep head + tail)
/// - Level 2: Summarize old turns (replace details with one-liner)
/// - Level 3: Drop old messages (keep first + recent only)
///
/// Each level is tried in order. Returns as soon as messages fit.
/*
DESIGN: Why `messages` is owned (Vec) but `config` is borrowed (&ContextConfig)
  `messages` = CONSUMED — tiered compaction rewrites the list; passing by value avoids
               an upfront clone and lets each level freely transform/drop messages
  `config`   = READ-ONLY — just a budget + thresholds; never mutated; borrow is sufficient
*/
pub fn compact_messages(
    messages: Vec<AgentMessage>, // OWNED — rewritten by each compaction level; no upfront clone needed
    config: &ContextConfig, // SETTINGS — token budget derived from max_context_tokens - system_prompt_tokens
) -> Vec<AgentMessage> {
    compact_messages_with_counter(messages, config, config.token_counter.as_ref())
}

/// Compact messages using the provided token counter (or the default heuristic).
pub fn compact_messages_with_counter(
    messages: Vec<AgentMessage>,
    config: &ContextConfig,
    counter: Option<&Arc<dyn TokenCounter>>,
) -> Vec<AgentMessage> {
    let counter = resolve_counter(counter);
    /*
    RUST QUIRK: `saturating_sub` — subtraction that stops at 0, never wraps

    Rust integers are bounded. On u32/usize, 0 - 1 would OVERFLOW (panic in debug, wrap in release).
    `saturating_sub(n)` instead returns 0 if the result would be negative.

    budget = max_context_tokens - system_prompt_tokens
    If someone misconfigured these (system_prompt > max), we'd get underflow.
    saturating_sub makes the budget = 0 (nothing fits) rather than a huge number.

    Python analogy: max(0, max_context_tokens - system_prompt_tokens)

    Alternative: `checked_sub(n)` returns `Option<usize>` — None on underflow.
    Use saturating when 0 is a safe fallback; use checked when you need to handle it explicitly.
    */
    let budget = config
        .max_context_tokens
        .saturating_sub(config.system_prompt_tokens);

    // Already fits?
    if counter.estimate_messages(&messages) <= budget {
        return messages;
    }

    // Level 1: Truncate tool outputs
    let compacted = level1_truncate_tool_outputs(&messages, config.tool_output_max_lines);
    if counter.estimate_messages(&compacted) <= budget {
        return compacted;
    }

    // Level 2: Summarize old turns (keep recent N full, summarize the rest)
    let compacted = level2_summarize_old_turns(&compacted, config.keep_recent);
    if counter.estimate_messages(&compacted) <= budget {
        return compacted;
    }

    // Level 3: Drop middle messages (keep first + recent)
    level3_drop_middle_with_counter(&compacted, config, budget, counter)
}

/// Level 1: Truncate long tool outputs to head + tail.
///
/// This is the cheapest compaction — preserves conversation structure,
/// just removes verbose tool output middles. In practice this saves
/// 50-70% of context in coding sessions.
pub(super) fn level1_truncate_tool_outputs(
    messages: &[AgentMessage], // SOURCE — read-only input; all non-ToolResult messages pass through unchanged
    max_lines: usize, // LIMIT — each ToolResult text block is truncated to this many lines (head+tail)
) -> Vec<AgentMessage> {
    messages
        .iter()
        .map(|msg| match msg {
            // Match only ToolResult messages — destructure all fields so we can reconstruct below
            AgentMessage::Llm(LlmMessage {
                message:
                    Message::ToolResult {
                        tool_call_id,
                        tool_name,
                        content,
                        is_error,
                        timestamp,
                    },
                ..
            }) => {
                let truncated_content: Vec<Content> = content
                    .iter()
                    .map(|c| match c {
                        Content::Text { text } => Content::Text {
                            text: truncate_text_head_tail(text, max_lines),
                        },
                        other => other.clone(), // Images, ToolCalls etc. passed through unchanged
                    })
                    .collect();

                /*
                RUST QUIRK: `*is_error` and `*timestamp` — dereferencing to copy

                Inside a match arm that borrows the enum (we matched `msg` which is `&AgentMessage`),
                the fields `is_error` and `timestamp` are bound as `&bool` and `&u64` — references.

                To use them as plain values (not references) in the new struct literal, we dereference:
                  *is_error  → bool  (Copy type — dereference gives us the value)
                  *timestamp → u64   (Copy type — same)

                For `String` fields (not Copy), we call `.clone()` instead of dereferencing,
                because dereferencing a &String would give us a String borrow — we need owned Strings.

                Python analogy: you never need this in Python because everything is a reference/object
                and copying happens automatically for primitives.
                */
                AgentMessage::Llm(LlmMessage::new(Message::ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    content: truncated_content,
                    is_error: *is_error,   // deref: &bool → bool
                    timestamp: *timestamp, // deref: &u64  → u64
                }))
            }
            other => other.clone(), // Non-ToolResult messages pass through unchanged
        })
        .collect()
}

/// Truncate text keeping first N/2 and last N/2 lines.
pub(super) fn truncate_text_head_tail(
    text: &str,       // SOURCE — the full tool output text to truncate
    max_lines: usize, // LIMIT — keep first max_lines/2 and last max_lines/2; omitted middle shown as "[... N lines truncated ...]"
) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.to_string();
    }

    let head = max_lines / 2;
    let tail = max_lines - head;
    let omitted = lines.len() - head - tail;

    let mut result = lines[..head].join("\n");
    result.push_str(&format!("\n\n[... {} lines truncated ...]\n\n", omitted));
    result.push_str(&lines[lines.len() - tail..].join("\n"));
    result
}

/// Level 2: Summarize old assistant turns.
///
/// Keeps the last `keep_recent` messages in full detail.
/// For older messages: assistant messages with tool calls get replaced
/// with a short summary, and their tool results get dropped.
fn level2_summarize_old_turns(
    messages: &[AgentMessage], // SOURCE — full conversation history to be summarized
    keep_recent: usize, // WINDOW — last N messages kept verbatim; everything before is summarized/dropped
) -> Vec<AgentMessage> {
    let len = messages.len();
    if len <= keep_recent {
        return messages.to_vec();
    }

    let boundary = len - keep_recent;
    let mut result = Vec::new();

    let mut i = 0;
    while i < boundary {
        let msg = &messages[i];
        match msg {
            AgentMessage::Llm(LlmMessage {
                message: Message::Assistant { content, .. },
                ..
            }) => {
                // Summarize: extract text content, skip tool call details
                let text_parts: Vec<&str> = content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text } => {
                            if text.len() > 200 {
                                None // Too long, will be replaced
                            } else {
                                Some(text.as_str())
                            }
                        }
                        _ => None,
                    })
                    .collect();

                let tool_count = content
                    .iter()
                    .filter(|c| matches!(c, Content::ToolCall { .. }))
                    .count();

                let summary = if !text_parts.is_empty() {
                    text_parts.join(" ")
                } else if tool_count > 0 {
                    format!("[Assistant used {} tool(s)]", tool_count)
                } else {
                    "[Assistant response]".into()
                };

                result.push(AgentMessage::Llm(LlmMessage::new(Message::User {
                    content: vec![Content::Text {
                        text: format!("[Summary] {}", summary),
                    }],
                    timestamp: now_ms(),
                })));

                // Skip following tool results that belong to this turn
                i += 1;
                while i < boundary {
                    if let AgentMessage::Llm(LlmMessage {
                        message: Message::ToolResult { .. },
                        ..
                    }) = &messages[i]
                    {
                        i += 1;
                    } else {
                        break;
                    }
                }
                continue;
            }
            AgentMessage::Llm(LlmMessage {
                message: Message::ToolResult { .. },
                ..
            }) => {
                // Skip orphaned tool results in old section
                i += 1;
                continue;
            }
            other => {
                // Keep user messages as-is (they provide intent)
                result.push(other.clone());
            }
        }
        i += 1;
    }

    // Append recent messages in full
    result.extend_from_slice(&messages[boundary..]);
    result
}

/// Level 3: Drop middle messages with a pluggable token counter.
fn level3_drop_middle_with_counter(
    messages: &[AgentMessage],
    config: &ContextConfig,
    budget: usize,
    counter: &dyn TokenCounter,
) -> Vec<AgentMessage> {
    let len = messages.len();
    // .min(len) prevents keep_first from exceeding the actual message count
    // Python analogy: first_end = min(config.keep_first, len)
    let first_end = config.keep_first.min(len);
    // saturating_sub: if keep_recent > len, recent_start = 0 (take all messages as "recent")
    let recent_start = len.saturating_sub(config.keep_recent);

    if first_end >= recent_start {
        // Can't split — just keep as many recent as fit
        return keep_within_budget_with_counter(messages, budget, counter);
    }

    let first_msgs = &messages[..first_end];
    let recent_msgs = &messages[recent_start..];
    let removed = recent_start - first_end;

    let marker = AgentMessage::Llm(LlmMessage::new(Message::User {
        content: vec![Content::Text {
            text: format!(
                "[Context compacted: {} messages removed to fit context window]",
                removed
            ),
        }],
        timestamp: now_ms(),
    }));

    let mut result = first_msgs.to_vec();
    result.push(marker);
    result.extend_from_slice(recent_msgs);

    // If still too big, progressively drop from recent
    if counter.estimate_messages(&result) > budget {
        return keep_within_budget_with_counter(&result, budget, counter);
    }

    result
}

/// Keep as many recent messages as fit within budget using a pluggable counter.
fn keep_within_budget_with_counter(
    messages: &[AgentMessage],
    budget: usize,
    counter: &dyn TokenCounter,
) -> Vec<AgentMessage> {
    let mut result = Vec::new();
    let mut remaining = budget;

    for msg in messages.iter().rev() {
        let tokens = counter.estimate_message(msg);
        if tokens > remaining {
            break;
        }
        remaining -= tokens;
        result.push(msg.clone());
    }

    result.reverse();

    if result.len() < messages.len() {
        let removed = messages.len() - result.len();
        result.insert(
            0,
            AgentMessage::Llm(LlmMessage::new(Message::User {
                content: vec![Content::Text {
                    text: format!("[Context compacted: {} messages removed]", removed),
                }],
                timestamp: now_ms(),
            })),
        );
    }

    result
}
