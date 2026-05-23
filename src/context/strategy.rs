use super::compaction::*;
use super::config::*;
use super::token::*;
use crate::types::*;
use chrono::Utc;
#[allow(unused_imports)]
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Compaction strategy
// ---------------------------------------------------------------------------

/// Strategy for compacting messages when context exceeds budget.
///
/// Implement this to customize what happens during compaction:
/// - Index discarded content into a memory store before removal
/// - Apply custom preservation rules (e.g., always keep decisions)
/// - Emit metadata about what was compressed
///
/// See the [Custom Compaction](https://LazyBouy.github.io/phi-core/concepts/agent-loop.html#custom-compaction)
/// docs for examples.
/*
RUST QUIRK: Traits as seams for extensibility (the Strategy pattern)

CompactionStrategy is a classic "strategy pattern" expressed as a Rust trait.
The agent loop calls `strategy.compact(messages, config)` — it doesn't know
whether it's calling DefaultCompaction or a custom user-provided strategy.

This is polymorphism without inheritance:
  - In OOP: you'd subclass a BaseCompaction class
  - In Rust: you implement a trait

The trait object `Arc<dyn CompactionStrategy>` in AgentLoopConfig means:
"store any type that implements CompactionStrategy, dispatched at runtime."

Why `Send + Sync` bounds?
  - The agent loop may run on any tokio thread pool thread
  - The strategy is shared via Arc, so it must be Sync (safe to &-reference across threads)
  - It must be Send (safe to move to another thread)
  - Basically: thread-safe is required because tokio = multi-threaded by default

The `messages: Vec<AgentMessage>` parameter takes ownership (not a borrow).
This is intentional: compaction rewrites the list. Passing by value lets the
implementation freely mutate, filter, and reconstruct without cloning.
*/
pub trait CompactionStrategy: Send + Sync {
    /// Compact messages to fit within the token budget defined by `config`.
    ///
    /// Called before each LLM turn when `context_config` is set.
    fn compact(
        &self,
        messages: Vec<AgentMessage>, // OWNED — taken by value so implementation can freely rewrite without cloning
        config: &ContextConfig, // SETTINGS — token budget, keep_first/keep_recent counts, tool_output_max_lines
    ) -> Vec<AgentMessage>;
}

/// Default 3-level compaction: truncate tool outputs → summarize turns → drop middle.
///
/// This is used automatically when no custom `CompactionStrategy` is set.
/// You can also compose it inside a custom strategy — run your logic first,
/// then delegate to `compact_messages()` for the actual reduction.
pub struct DefaultCompaction;

impl CompactionStrategy for DefaultCompaction {
    fn compact(
        &self,
        messages: Vec<AgentMessage>, // OWNED — passed directly to compact_messages()
        config: &ContextConfig,      // SETTINGS — forwarded to compact_messages()
    ) -> Vec<AgentMessage> {
        super::compact_messages::compact_messages_with_counter(
            messages,
            config,
            config.token_counter.as_ref(),
        )
    }
}

// ---------------------------------------------------------------------------
// Block-based compaction strategy (non-destructive overlay model)
// ---------------------------------------------------------------------------

use crate::session::LoopRecord;

/// Strategy for creating non-destructive `CompactionBlock` overlays.
///
/// Three methods produce the three sections of a `CompactionBlock`:
/// - `keep_first`: turns kept verbatim from the start
/// - `keep_recent`: recent turns with truncated tool outputs
/// - `keep_compacted`: fully summarised section
///
/// The default `compact()` method assembles them. Override individual methods
/// to customise specific sections (e.g. LLM-based summarisation for `keep_compacted`).
pub trait BlockCompactionStrategy: Send + Sync {
    /// Determine the keep_first section: turns kept verbatim from the start.
    /// Only called for the most recent loop.
    fn keep_first(
        &self,
        record: &LoopRecord,
        turn_map: &TurnMap,
        config: &CompactionConfig,
    ) -> Option<TurnRange>;

    /// Create the keep_recent section: recent turns with truncated tool outputs.
    /// Only called for the most recent loop.
    fn keep_recent(
        &self,
        record: &LoopRecord,
        turn_map: &TurnMap,
        config: &CompactionConfig,
    ) -> Option<CompactedSection>;

    /// Create the keep_compacted section: fully summarised turns.
    /// For most recent loop: summarises the middle (between keep_first and keep_recent).
    /// For older loops: summarises the entire loop.
    ///
    /// Implementations should aim to summarise ALL turns in the range within
    /// `config.max_summary_tokens` — e.g. shorter per-turn summaries or an
    /// LLM-generated holistic digest. The token budget is for the total output,
    /// not a per-turn limit.
    fn keep_compacted(
        &self,
        record: &LoopRecord,
        turn_map: &TurnMap,
        config: &CompactionConfig,
        is_most_recent: bool,
    ) -> Option<CompactedSection>;

    /// Assemble a `CompactionBlock` from the three sections.
    /// Default implementation calls the three methods above.
    fn compact(
        &self,
        record: &LoopRecord,
        config: &CompactionConfig,
        is_most_recent: bool,
    ) -> CompactionBlock {
        let turn_map = TurnMap::from_messages(&record.messages);
        CompactionBlock {
            keep_first: if is_most_recent {
                self.keep_first(record, &turn_map, config)
            } else {
                None
            },
            keep_recent: if is_most_recent {
                self.keep_recent(record, &turn_map, config)
            } else {
                None
            },
            keep_compacted: self.keep_compacted(record, &turn_map, config, is_most_recent),
            created_at: Utc::now(),
        }
    }
}

/// Default block-based compaction strategy.
///
/// Stateless — all parameters come from `CompactionConfig`.
/// - `keep_first`: returns turn range `0..keep_first_turns`
/// - `keep_compacted`: one-liner summaries of the middle section, bounded by `max_summary_tokens`
/// - `keep_recent`: truncates tool outputs in the recent section to `tool_output_max_lines`
pub struct DefaultBlockCompaction;

impl BlockCompactionStrategy for DefaultBlockCompaction {
    fn keep_first(
        &self,
        _record: &LoopRecord,
        turn_map: &TurnMap,
        config: &CompactionConfig,
    ) -> Option<TurnRange> {
        let total = turn_map.turn_count();
        if total == 0 {
            return None;
        }
        let end = (config.keep_first_turns as u32)
            .min(total)
            .saturating_sub(1);
        Some(TurnRange {
            start_turn: 0,
            end_turn: end,
        })
    }

    fn keep_recent(
        &self,
        record: &LoopRecord,
        turn_map: &TurnMap,
        config: &CompactionConfig,
    ) -> Option<CompactedSection> {
        let total = turn_map.turn_count();
        if total == 0 {
            return None;
        }
        let recent_start = total.saturating_sub(config.keep_recent_turns as u32);
        let range = TurnRange {
            start_turn: recent_start,
            end_turn: total - 1,
        };
        let msgs = turn_map.messages_for_range(&range, &record.messages);
        // Truncate tool outputs in the recent section
        let truncated: Vec<AgentMessage> = msgs
            .iter()
            .map(|m| {
                if let AgentMessage::Llm(lm) = m {
                    if let Message::ToolResult {
                        tool_call_id,
                        tool_name,
                        content,
                        is_error,
                        timestamp,
                    } = &lm.message
                    {
                        let truncated_content: Vec<Content> = content
                            .iter()
                            .map(|c| match c {
                                Content::Text { text } => Content::Text {
                                    text: super::compact_messages::truncate_text_head_tail(
                                        text,
                                        config.tool_output_max_lines,
                                    ),
                                },
                                other => other.clone(),
                            })
                            .collect();
                        return AgentMessage::Llm(LlmMessage {
                            message: Message::ToolResult {
                                tool_call_id: tool_call_id.clone(),
                                tool_name: tool_name.clone(),
                                content: truncated_content,
                                is_error: *is_error,
                                timestamp: *timestamp,
                            },
                            turn_id: lm.turn_id.clone(),
                            // Preserve Composition I identity + tags through
                            // tool-output truncation. Identity is a property of
                            // the node, not its body bytes; tags ride along.
                            node_id: lm.node_id,
                            parent_id: lm.parent_id,
                            tags: lm.tags.clone(),
                        });
                    }
                }
                m.clone()
            })
            .collect();
        Some(CompactedSection {
            range,
            messages: truncated,
        })
    }

    /// Basic implementation: generates per-turn one-liner summaries until
    /// `max_summary_tokens` is exhausted. Remaining turns are dropped.
    ///
    /// More sophisticated strategies (e.g. LLM-based) should produce a holistic
    /// summary of ALL turns within the budget rather than dropping turns.
    ///
    /// Summaries use `Message::User` role to maintain valid LLM message alternation
    /// (user→assistant→user→...). A summary replaces a full turn sequence
    /// (user + assistant + tool results) with a single user-role "[Summary]" message.
    fn keep_compacted(
        &self,
        record: &LoopRecord,
        turn_map: &TurnMap,
        config: &CompactionConfig,
        is_most_recent: bool,
    ) -> Option<CompactedSection> {
        let total = turn_map.turn_count();
        if total == 0 {
            return None;
        }

        let (start, end) = if is_most_recent {
            let first_end = (config.keep_first_turns as u32).min(total);
            let recent_start = total.saturating_sub(config.keep_recent_turns as u32);
            if first_end >= recent_start {
                return None; // No middle section
            }
            (first_end, recent_start.saturating_sub(1))
        } else {
            // Summarise the entire loop
            (0, total.saturating_sub(1))
        };

        let range = TurnRange {
            start_turn: start,
            end_turn: end,
        };
        let msgs = turn_map.messages_for_range(&range, &record.messages);

        // Generate one-liner summaries per assistant message
        let mut summaries: Vec<AgentMessage> = Vec::new();
        let mut token_budget = config.max_summary_tokens;

        for msg in msgs {
            if let AgentMessage::Llm(lm) = msg {
                if let Message::Assistant { content, .. } = &lm.message {
                    let text_parts: Vec<&str> = content
                        .iter()
                        .filter_map(|c| match c {
                            Content::Text { text } if text.len() <= 200 => Some(text.as_str()),
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
                    let summary_text = format!("[Summary] {}", summary);
                    let est_tokens = estimate_tokens(&summary_text);
                    if est_tokens > token_budget {
                        break; // Budget exhausted
                    }
                    token_budget -= est_tokens;
                    summaries.push(AgentMessage::Llm(LlmMessage::new(Message::user(
                        &summary_text,
                    ))));
                }
            }
        }

        if summaries.is_empty() {
            return None;
        }
        Some(CompactedSection {
            range,
            messages: summaries,
        })
    }
}
