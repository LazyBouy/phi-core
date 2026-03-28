//! Context window management — smart truncation and token counting.
//!
//! The #1 engineering challenge for agents. This module provides:
//! - Token estimation (fast, no external deps)
//! - Tiered compaction (tool output truncation → turn summarization → full summary)
//! - Execution limits (max turns, tokens, duration)
//!
//! Designed based on Claude Code's approach: clear old tool outputs first,
//! then summarize conversation if needed.

use crate::types::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Compaction Block — non-destructive overlay on LoopRecord
// ---------------------------------------------------------------------------

/// Non-destructive compaction overlay. Stored on `LoopRecord` alongside
/// the original messages. When present, the context loader uses this block
/// instead of raw messages.
///
/// Three sections control what gets loaded into context:
/// - `keep_first`: turns kept verbatim from the start (most recent loop only)
/// - `keep_recent`: recent turns with truncated tool outputs (most recent loop only)
/// - `keep_compacted`: fully summarised section (all loops)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionBlock {
    /// Turns kept verbatim from the start of the loop.
    /// Only populated for the MOST RECENT loop. For older loops this is `None`.
    /// During context load: original messages in this range are used as-is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_first: Option<TurnRange>,

    /// Recent turns with tool outputs truncated. Rest unchanged.
    /// Only populated for the MOST RECENT loop. For older loops this is `None`.
    /// Invariant: if a ToolCall is in range, its corresponding ToolResult is too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_recent: Option<CompactedSection>,

    /// Fully summarised middle section (most recent loop) or entire loop (older loops).
    /// Relevant for ALL loops — this is what gets loaded from earlier loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_compacted: Option<CompactedSection>,

    /// When this block was created.
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
}

/// A range of turns within a loop, identified by turn indices.
/// Both bounds are inclusive. These correspond to `TurnId.turn_index` values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnRange {
    #[serde(rename = "startTurn")]
    pub start_turn: u32,
    #[serde(rename = "endTurn")]
    pub end_turn: u32,
}

/// A range of turns plus the compacted replacement messages for that range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactedSection {
    /// The turn range this section replaces.
    pub range: TurnRange,
    /// Replacement messages loaded into context instead of the originals.
    pub messages: Vec<AgentMessage>,
}

// ---------------------------------------------------------------------------
// TurnMap — maps turn indices to message index ranges
// ---------------------------------------------------------------------------

/// Maps turn indices to message index ranges within a message array.
/// Built from messages by grouping on `TurnId.turn_index`.
pub struct TurnMap {
    /// Indexed by position (0-based). Each entry is `(start_msg_idx, end_msg_idx)` inclusive.
    entries: Vec<(usize, usize)>,
}

impl TurnMap {
    /// Build from messages by grouping on `turn_id.turn_index`.
    /// Messages without a `turn_id` are treated as their own single-message group.
    pub fn from_messages(messages: &[AgentMessage]) -> Self {
        let mut entries: Vec<(usize, usize)> = Vec::new();
        let mut current_turn: Option<u32> = None;

        for (i, msg) in messages.iter().enumerate() {
            let turn_idx = msg.turn_id().map(|t| t.turn_index);
            match (turn_idx, current_turn) {
                (Some(idx), Some(cur)) if idx == cur => {
                    // Same turn — extend end index
                    if let Some(last) = entries.last_mut() {
                        last.1 = i;
                    }
                }
                (Some(idx), _) => {
                    // New turn
                    entries.push((i, i));
                    current_turn = Some(idx);
                }
                (None, _) => {
                    // Legacy message without turn_id — treat as its own group
                    entries.push((i, i));
                    current_turn = None;
                }
            }
        }

        Self { entries }
    }

    /// Number of turn groups.
    pub fn turn_count(&self) -> u32 {
        self.entries.len() as u32
    }

    /// Slice of messages belonging to a `TurnRange`.
    pub fn messages_for_range<'a>(
        &self,
        range: &TurnRange,
        all_msgs: &'a [AgentMessage],
    ) -> &'a [AgentMessage] {
        if range.start_turn as usize >= self.entries.len()
            || range.end_turn as usize >= self.entries.len()
        {
            return &[];
        }
        let start = self.entries[range.start_turn as usize].0;
        let end = self.entries[range.end_turn as usize].1;
        &all_msgs[start..=end]
    }

    /// Message index range `(start, end)` inclusive for a single turn.
    pub fn turn_msg_range(&self, turn_index: u32) -> Option<(usize, usize)> {
        self.entries.get(turn_index as usize).copied()
    }
}

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
fn content_tokens(content: &[Content]) -> usize {
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

// ---------------------------------------------------------------------------
// Context configuration
// ---------------------------------------------------------------------------

/// Configuration for context management — model constraints + compaction policy.
///
/// `CompactionConfig` is a required field: if you set a context limit,
/// compaction is always ready with sensible defaults. Compaction as a whole
/// is disabled by setting `context_config: None` on `AgentLoopConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Maximum context tokens (the model's context window).
    pub max_context_tokens: usize,
    /// Tokens reserved for the system prompt.
    pub system_prompt_tokens: usize,
    /// Compaction policy — always present when context limits are set.
    pub compaction: CompactionConfig,

    // Legacy fields — kept for backward compatibility with existing configs.
    // New code should use `compaction.*` instead.
    #[serde(default)]
    pub keep_recent: usize,
    #[serde(default)]
    pub keep_first: usize,
    #[serde(default)]
    pub tool_output_max_lines: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 100_000,
            system_prompt_tokens: 4_000,
            compaction: CompactionConfig::default(),
            keep_recent: 10,
            keep_first: 2,
            tool_output_max_lines: 50,
        }
    }
}

/// Controls how many earlier loops are included in compaction and context loading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompactionScope {
    /// Compact a fixed number of earlier loops on the active chain.
    FixedCount(usize),
    /// Walk the chain backward, accumulating per-loop token estimates,
    /// and stop when `max_context_tokens` would be exceeded.
    ///
    /// NOTE: The scope can include loops whose raw messages EXCEED
    /// `max_context_tokens`. This is intentional — the compacted summaries
    /// of those loops will fit in the window, even though the originals
    /// did not. This enables richer context for summarisation strategies
    /// (e.g. LLM summarisers) that compress large loops into compact
    /// representations that then fit within the budget.
    TokenBudget,
}

impl Default for CompactionScope {
    fn default() -> Self {
        Self::FixedCount(3)
    }
}

/// Full compaction policy — controls both WHEN and HOW to compact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    // ── WHEN to compact ──
    /// Fraction of `max_context_tokens` below which headroom is measured.
    /// Compaction fires when headroom drops below `compact_budget_threshold_pct`.
    /// Default: 0.90 (90%).
    pub compact_at_pct: f64,
    /// Minimum remaining headroom fraction before compaction fires.
    /// Default: 0.05 (5%). With defaults at 100k/4k: fires at ~81k tokens.
    pub compact_budget_threshold_pct: f64,
    /// Scope controlling how many earlier loops to compact and load.
    /// Default: `FixedCount(3)`.
    pub compaction_scope: CompactionScope,

    // ── HOW to compact ──
    /// Turns to keep verbatim from the start (most recent loop only). Default: 2.
    pub keep_first_turns: usize,
    /// Minimum turns to keep from the end (most recent loop only).
    /// Extended to turn boundary so ToolCall/ToolResult pairs are never split.
    /// Default: 10.
    pub keep_recent_turns: usize,
    /// Token budget for the summarised middle section. Default: 2_000.
    ///
    /// This is a budget, not a per-turn limit. Implementations of
    /// `BlockCompactionStrategy::keep_compacted()` should aim to summarise
    /// ALL turns in the range within this budget — e.g. by producing shorter
    /// per-turn summaries or an LLM-generated holistic digest.
    /// `DefaultBlockCompaction` is a basic implementation that generates
    /// per-turn one-liners and drops remaining turns when the budget runs out.
    pub max_summary_tokens: usize,
    /// Max lines per tool output in the keep_recent section. Default: 50.
    pub tool_output_max_lines: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            compact_at_pct: 0.90,
            compact_budget_threshold_pct: 0.05,
            compaction_scope: CompactionScope::default(),
            keep_first_turns: 2,
            keep_recent_turns: 10,
            max_summary_tokens: 2_000,
            tool_output_max_lines: 50,
        }
    }
}

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
        compact_messages(messages, config)
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
                                    text: truncate_text_head_tail(
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
    if total_tokens(&messages) <= budget {
        return messages;
    }

    // Level 1: Truncate tool outputs
    let compacted = level1_truncate_tool_outputs(&messages, config.tool_output_max_lines);
    if total_tokens(&compacted) <= budget {
        return compacted;
    }

    // Level 2: Summarize old turns (keep recent N full, summarize the rest)
    let compacted = level2_summarize_old_turns(&compacted, config.keep_recent);
    if total_tokens(&compacted) <= budget {
        return compacted;
    }

    // Level 3: Drop middle messages (keep first + recent)
    level3_drop_middle(&compacted, config, budget)
}

/// Level 1: Truncate long tool outputs to head + tail.
///
/// This is the cheapest compaction — preserves conversation structure,
/// just removes verbose tool output middles. In practice this saves
/// 50-70% of context in coding sessions.
fn level1_truncate_tool_outputs(
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
fn truncate_text_head_tail(
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

/// Level 3: Drop middle messages, keeping first + recent.
fn level3_drop_middle(
    messages: &[AgentMessage], // SOURCE — full history; middle section will be replaced by a compaction marker
    config: &ContextConfig, // SETTINGS — keep_first and keep_recent counts control what's preserved
    budget: usize, // TOKEN LIMIT — remaining budget after system prompt; used as fallback via keep_within_budget
) -> Vec<AgentMessage> {
    let len = messages.len();
    // .min(len) prevents keep_first from exceeding the actual message count
    // Python analogy: first_end = min(config.keep_first, len)
    let first_end = config.keep_first.min(len);
    // saturating_sub: if keep_recent > len, recent_start = 0 (take all messages as "recent")
    let recent_start = len.saturating_sub(config.keep_recent);

    if first_end >= recent_start {
        // Can't split — just keep as many recent as fit
        return keep_within_budget(messages, budget);
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
    if total_tokens(&result) > budget {
        return keep_within_budget(&result, budget);
    }

    result
}

/// Keep as many recent messages as fit within budget.
fn keep_within_budget(messages: &[AgentMessage], budget: usize) -> Vec<AgentMessage> {
    let mut result = Vec::new();
    let mut remaining = budget;

    for msg in messages.iter().rev() {
        let tokens = message_tokens(msg);
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

// ---------------------------------------------------------------------------
// Execution limits
// ---------------------------------------------------------------------------

/*
ExecutionLimits — a safety net against runaway agent loops.

Without limits, a poorly-designed tool or a confused LLM could loop forever,
burning tokens and money. These three limits provide defense-in-depth:

  max_turns    — catches infinite tool-call loops
  max_total_tokens — catches token budget overruns (cost control)
  max_duration — catches wall-clock hangs (e.g., a bash tool that blocks)

The agent loop checks these BEFORE each turn (in ExecutionTracker::check_limits).
When a limit is hit, it injects a "[Agent stopped: ...]" user message into the
conversation so the LLM (and user) can see what happened, then returns.

RUST QUIRK: `std::time::Duration`

Duration is Rust's type for a span of time (not a point in time — that's Instant/SystemTime).
Constructors:
  Duration::from_secs(600)   → 10 minutes
  Duration::from_millis(100) → 100ms
  Duration::from_nanos(1)    → 1 nanosecond

Internally, Duration is stored as (seconds: u64, nanoseconds: u32) — no floating point,
no overflow risk for reasonable values.

The full path `std::time::Duration` is used instead of a `use` import because it appears
only in this one struct — no need to pollute the module namespace.
*/
/// Execution limits for the agent loop — guards against infinite loops and budget overruns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionLimits {
    /// Maximum number of LLM turns (one turn = one LLM call + its tool results)
    pub max_turns: usize,
    /// Maximum total tokens consumed across all turns (input + output)
    pub max_total_tokens: usize,
    /// Maximum wall-clock duration. Uses std::time::Duration (not f64 seconds) for precision.
    pub max_duration: std::time::Duration,
    /// Maximum cumulative dollar cost for the run. `None` means no cost cap.
    /// Requires `AgentLoopConfig.cost_config` to be set — without pricing rates the
    /// accumulated cost is always 0.0 and this limit has no effect.
    #[serde(default)]
    pub max_cost: Option<f64>,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_turns: 50,
            max_total_tokens: 1_000_000,
            max_duration: std::time::Duration::from_secs(600),
            max_cost: None,
        }
    }
}

/// Tracks execution state against limits
pub struct ExecutionTracker {
    pub limits: ExecutionLimits,
    pub turns: usize,
    pub tokens_used: usize,
    /// Accumulated dollar cost across all turns. Updated via `record_cost()`.
    /// Only non-zero when `AgentLoopConfig.cost_config` is set.
    pub cost_accumulated: f64,
    pub started_at: std::time::Instant,
}

impl ExecutionTracker {
    pub fn new(limits: ExecutionLimits) -> Self {
        Self {
            limits,
            turns: 0,
            tokens_used: 0,
            cost_accumulated: 0.0,
            started_at: std::time::Instant::now(),
        }
    }

    pub fn record_turn(&mut self, tokens: usize) {
        self.turns += 1;
        self.tokens_used += tokens;
    }

    /// Accumulate incremental cost for the current turn.
    pub fn record_cost(&mut self, cost: f64) {
        self.cost_accumulated += cost;
    }

    /// Check if any limit has been exceeded. Returns the reason if so.
    /*
    RUST QUIRK: `Option<String>` as "either an error reason, or nothing"

    `check_limits()` returns:
      Some("Max turns reached (50/50)")  ← a limit was hit
      None                                ← all limits OK

    This is the Rust way to return "optional data" — no exceptions, no sentinel values (-1, ""),
    no separate boolean + string pair. The caller pattern-matches to handle both cases.

    RUST QUIRK: `Instant::elapsed()` for wall-clock timing

    `std::time::Instant` records a moment in time (monotonic clock, not wall clock).
    Monotonic means it never goes backwards — safe to use for durations.
    `started_at.elapsed()` returns a `Duration` = current time - started_at.

    The `>=` comparison between two Durations works because Duration implements PartialOrd.

    RUST QUIRK: `{:.0}` format specifier — zero decimal places for f64

    `format!("Max duration reached ({:.0}s/{:.0}s)", elapsed.as_secs_f64(), ...)`
    `{:.0}` means "format as float with 0 decimal places" → "42" not "42.000000"
    Other examples: {:.2} = 2 decimal places, {:>10.3} = right-aligned, 10 wide, 3 decimal places
    */
    pub fn check_limits(&self) -> Option<String> {
        if self.turns >= self.limits.max_turns {
            return Some(format!(
                "Max turns reached ({}/{})",
                self.turns, self.limits.max_turns
            ));
        }
        if self.tokens_used >= self.limits.max_total_tokens {
            return Some(format!(
                "Max tokens reached ({}/{})",
                self.tokens_used, self.limits.max_total_tokens
            ));
        }
        let elapsed = self.started_at.elapsed(); // Duration since ExecutionTracker::new()
        if elapsed >= self.limits.max_duration {
            return Some(format!(
                "Max duration reached ({:.0}s/{:.0}s)", // {:.0} = 0 decimal places
                elapsed.as_secs_f64(),
                self.limits.max_duration.as_secs_f64()
            ));
        }
        if let Some(max) = self.limits.max_cost {
            if self.cost_accumulated >= max {
                return Some(format!(
                    "Max cost reached (${:.4}/${:.4})",
                    self.cost_accumulated, max
                ));
            }
        }
        None // All limits OK — return None (no reason to stop)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/*
RUST QUIRK: `#[cfg(test)] mod tests { ... }`

`#[cfg(test)]` is a conditional compilation attribute — this module is ONLY
compiled when running `cargo test`. It's completely absent from release builds.
Zero binary size, zero compile overhead in production.

`use super::*;` brings all items from the parent module into scope.
`super` means "the module that contains this module" — here, that's context.rs itself.
This is how tests access private functions like `level1_truncate_tool_outputs`
without making them `pub`.

Python analogy: there's no equivalent — Python test files import the module explicitly.
In Rust, tests live INSIDE the module they test (by convention) and get access to
private items via `use super::*`. This enforces locality and makes tests co-evolve with code.

To run just this module's tests:
  docker run ... cargo test --lib context
To run a single test:
  docker run ... cargo test test_estimate_tokens
*/
// ---------------------------------------------------------------------------
// Compaction orchestration — cross-loop block creation
// ---------------------------------------------------------------------------

use crate::session::Session;

/// Resolve `CompactionScope` to a concrete number of earlier loops to include.
///
/// For `FixedCount(n)`, returns `n` directly.
/// For `TokenBudget`, walks the chain backward from the current loop,
/// accumulating `total_tokens(&record.messages)` per loop, and stops
/// when `max_context_tokens` would be exceeded.
///
/// Note: with `TokenBudget`, the scope can include loops whose raw messages
/// exceed the token budget. This is intentional — the compacted summaries
/// will fit in the window even when the originals don't, enabling richer
/// context for expensive summarisation strategies.
fn resolve_scope(
    session: &Session,
    chain: &[String],
    scope: &CompactionScope,
    max_context_tokens: usize,
) -> usize {
    match scope {
        CompactionScope::FixedCount(n) => *n,
        CompactionScope::TokenBudget => {
            let mut budget = max_context_tokens;
            let mut count = 0usize;
            // Walk backward from the loop before current (chain.last() is current)
            for loop_id in chain.iter().rev().skip(1) {
                if let Some(record) = session.get_loop(loop_id) {
                    let loop_tokens = total_tokens(&record.messages);
                    if loop_tokens > budget {
                        break;
                    }
                    budget -= loop_tokens;
                    count += 1;
                }
            }
            count
        }
    }
}

/// Create `CompactionBlock`s for the current loop and earlier loops within scope.
/// Mutates the session in place.
///
/// The caller is responsible for persisting the session to disk afterward.
pub fn compact_session_loops(
    session: &mut Session,
    current_loop_id: &str,
    strategy: &dyn BlockCompactionStrategy,
    config: &CompactionConfig,
    max_context_tokens: usize,
) {
    let chain = session.loop_chain_to(current_loop_id);

    // 1. Compact current loop (most recent — all three sections)
    if let Some(current) = session.get_loop_mut(current_loop_id) {
        current.compaction_block = Some(strategy.compact(current, config, true));
    }

    // 2. Resolve scope, then compact earlier loops on the chain (only keep_compacted)
    let earlier_count = resolve_scope(
        session,
        &chain,
        &config.compaction_scope,
        max_context_tokens,
    )
    .min(chain.len().saturating_sub(1));
    let earlier_start = chain.len().saturating_sub(1 + earlier_count);
    for loop_id in &chain[earlier_start..chain.len().saturating_sub(1)] {
        if let Some(record) = session.get_loop_mut(loop_id) {
            if record.compaction_block.is_none() {
                record.compaction_block = Some(strategy.compact(record, config, false));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Context builder — loads from CompactionBlocks when available
// ---------------------------------------------------------------------------

/// Build a compacted context by walking the loop chain and loading from
/// `CompactionBlock`s where available, raw messages otherwise.
///
/// For the most recent loop: loads keep_first + keep_compacted + keep_recent.
/// For older loops: loads only keep_compacted.
/// Loops outside the resolved scope are skipped entirely.
pub fn build_context_from_session(
    session: &Session,
    current_loop_id: &str,
    config: &CompactionConfig,
    max_context_tokens: usize,
) -> Vec<AgentMessage> {
    let chain = session.loop_chain_to(current_loop_id);
    let mut context = Vec::new();

    let earlier_count = resolve_scope(
        session,
        &chain,
        &config.compaction_scope,
        max_context_tokens,
    );
    let load_start = chain.len().saturating_sub(earlier_count + 1);

    for (i, loop_id) in chain.iter().enumerate().skip(load_start) {
        let Some(record) = session.get_loop(loop_id) else {
            continue;
        };
        let is_most_recent = i == chain.len() - 1;

        match &record.compaction_block {
            Some(block) => {
                if is_most_recent {
                    // Load keep_first (original messages for that range)
                    if let Some(ref range) = block.keep_first {
                        let turn_map = TurnMap::from_messages(&record.messages);
                        let msgs = turn_map.messages_for_range(range, &record.messages);
                        context.extend_from_slice(msgs);
                    }
                    // Load keep_compacted (summarised middle)
                    if let Some(ref section) = block.keep_compacted {
                        context.extend(section.messages.iter().cloned());
                    }
                    // Load keep_recent (truncated tool outputs)
                    if let Some(ref section) = block.keep_recent {
                        context.extend(section.messages.iter().cloned());
                    }
                } else {
                    // Older loops: only load keep_compacted
                    if let Some(ref section) = block.keep_compacted {
                        context.extend(section.messages.iter().cloned());
                    }
                }
            }
            None => {
                // No compaction block — load raw messages
                context.extend(record.messages.iter().cloned());
            }
        }
    }

    context
}

#[cfg(test)]
mod tests {
    use super::*; // pull in all items from context.rs (including private functions)

    #[test]
    fn test_estimate_tokens() {
        assert!(estimate_tokens("hello world") > 0);
        assert!(estimate_tokens("hello world") < 10);
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_truncate_head_tail() {
        let text = (1..=100)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = truncate_text_head_tail(&text, 10);
        assert!(result.contains("line 1"));
        assert!(result.contains("line 5")); // head
        assert!(result.contains("line 100")); // tail
        assert!(result.contains("truncated"));
        assert!(!result.contains("line 50")); // middle removed
    }

    #[test]
    fn test_level1_truncation() {
        let big_output = (1..=200)
            .map(|i| format!("output line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let messages = vec![
            AgentMessage::Llm(LlmMessage::new(Message::user("do something"))),
            AgentMessage::Llm(LlmMessage::new(Message::ToolResult {
                tool_call_id: "tc-1".into(),
                tool_name: "bash".into(),
                content: vec![Content::Text { text: big_output }],
                is_error: false,
                timestamp: 0,
            })),
        ];

        let compacted = level1_truncate_tool_outputs(&messages, 20);
        let tool_msg = &compacted[1];
        if let AgentMessage::Llm(LlmMessage {
            message: Message::ToolResult { content, .. },
            ..
        }) = tool_msg
        {
            if let Content::Text { text } = &content[0] {
                assert!(text.contains("truncated"));
                assert!(text.contains("output line 1")); // head
                assert!(text.contains("output line 200")); // tail
                assert!(text.lines().count() < 50);
            } else {
                panic!("expected text content");
            }
        } else {
            panic!("expected tool result");
        }
    }

    #[test]
    fn test_compact_within_budget() {
        let messages = vec![
            AgentMessage::Llm(LlmMessage::new(Message::user("Hello"))),
            AgentMessage::Llm(LlmMessage::new(Message::user("World"))),
        ];
        let config = ContextConfig::default();
        let result = compact_messages(messages.clone(), &config);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_compact_drops_middle_when_needed() {
        let mut messages = Vec::new();
        for i in 0..100 {
            messages.push(AgentMessage::Llm(LlmMessage::new(Message::user(format!(
                "Message {} {}",
                i,
                "x".repeat(200)
            )))));
        }

        let config = ContextConfig {
            max_context_tokens: 500,
            system_prompt_tokens: 100,
            compaction: CompactionConfig::default(),
            keep_recent: 5,
            keep_first: 2,
            tool_output_max_lines: 20,
        };

        let result = compact_messages(messages, &config);
        assert!(result.len() < 100);
        assert!(result.len() >= 2);
    }

    #[test]
    fn test_context_tracker_no_usage() {
        let tracker = ContextTracker::new();
        let messages = vec![
            AgentMessage::Llm(LlmMessage::new(Message::user("Hello"))),
            AgentMessage::Llm(LlmMessage::new(Message::user("World"))),
        ];
        // Without usage data, falls back to estimation
        let tokens = tracker.estimate_context_tokens(&messages);
        assert!(tokens > 0);
        assert_eq!(tokens, total_tokens(&messages));
    }

    #[test]
    fn test_context_tracker_with_usage() {
        let mut tracker = ContextTracker::new();
        let messages = vec![
            AgentMessage::Llm(LlmMessage::new(Message::user("Hello"))),
            AgentMessage::Llm(LlmMessage::new(Message::Assistant {
                content: vec![Content::Text {
                    text: "Hi there!".into(),
                }],
                stop_reason: StopReason::Stop,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage {
                    input: 100,
                    output: 50,
                    ..Default::default()
                },
                timestamp: 0,
                error_message: None,
            })),
            AgentMessage::Llm(LlmMessage::new(Message::user("Follow up question here"))),
        ];
        // Record usage at index 1 (assistant message)
        tracker.record_usage(
            &Usage {
                input: 100,
                output: 50,
                ..Default::default()
            },
            1,
        );
        let tokens = tracker.estimate_context_tokens(&messages);
        // Should be 150 (real usage) + estimate for the trailing user message
        let trailing_estimate = message_tokens(&messages[2]);
        assert_eq!(tokens, 150 + trailing_estimate);
    }

    #[test]
    fn test_context_tracker_reset() {
        let mut tracker = ContextTracker::new();
        tracker.record_usage(
            &Usage {
                input: 1000,
                output: 500,
                ..Default::default()
            },
            5,
        );
        tracker.reset();
        let messages = vec![AgentMessage::Llm(LlmMessage::new(Message::user("test")))];
        // After reset, should fall back to estimation
        assert_eq!(
            tracker.estimate_context_tokens(&messages),
            total_tokens(&messages)
        );
    }

    #[test]
    fn test_execution_limits() {
        let limits = ExecutionLimits {
            max_turns: 3,
            max_total_tokens: 1000,
            max_duration: std::time::Duration::from_secs(60),
            max_cost: None,
        };

        let mut tracker = ExecutionTracker::new(limits);
        assert!(tracker.check_limits().is_none());

        tracker.record_turn(100);
        tracker.record_turn(100);
        assert!(tracker.check_limits().is_none());

        tracker.record_turn(100);
        assert!(tracker.check_limits().is_some());
    }
}
