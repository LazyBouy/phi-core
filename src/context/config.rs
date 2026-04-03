use super::token::TokenCounter;
use super::{BlockCompactionStrategy, CompactionStrategy};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Compaction scope
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Compaction configuration
// ---------------------------------------------------------------------------

/// Full compaction policy — controls both WHEN and HOW to compact.
#[derive(Clone, Serialize, Deserialize)]
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

    // ── Focus message ──
    /// Optional focus message to guide compaction summarization.
    /// When set, prepended to the compacted section to tell the model what to prioritize.
    /// Example: "Focus on specification details, API contracts, and architectural decisions."
    #[serde(default)]
    pub focus_message: Option<String>,

    // ── Strategy objects (G5 — moved from AgentLoopConfig) ──
    /// Custom in-memory compaction strategy. When set, replaces `DefaultCompaction`.
    /// Used when `AgentContext.session` is `None` (sub-agents, tests, sessionless runs).
    #[serde(skip)]
    pub in_memory_strategy: Option<Arc<dyn CompactionStrategy>>,
    /// Block-based compaction strategy for Session-aware compaction.
    /// When set, replaces `DefaultBlockCompaction`.
    /// Used when `AgentContext.session` is `Some`.
    #[serde(skip)]
    pub block_strategy: Option<Arc<dyn BlockCompactionStrategy>>,
}

impl std::fmt::Debug for CompactionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompactionConfig")
            .field("compact_at_pct", &self.compact_at_pct)
            .field(
                "compact_budget_threshold_pct",
                &self.compact_budget_threshold_pct,
            )
            .field("compaction_scope", &self.compaction_scope)
            .field("keep_first_turns", &self.keep_first_turns)
            .field("keep_recent_turns", &self.keep_recent_turns)
            .field("max_summary_tokens", &self.max_summary_tokens)
            .field("tool_output_max_lines", &self.tool_output_max_lines)
            .field("focus_message", &self.focus_message)
            .field(
                "in_memory_strategy",
                &self.in_memory_strategy.as_ref().map(|_| "..."),
            )
            .field(
                "block_strategy",
                &self.block_strategy.as_ref().map(|_| "..."),
            )
            .finish()
    }
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
            focus_message: None,
            in_memory_strategy: None,
            block_strategy: None,
        }
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
#[derive(Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Maximum context tokens (the model's context window).
    pub max_context_tokens: usize,
    /// Tokens reserved for the system prompt.
    pub system_prompt_tokens: usize,
    /// Compaction policy — always present when context limits are set.
    pub compaction: CompactionConfig,

    /// Custom token counter. When `None`, uses `HeuristicTokenCounter` (chars/4).
    /// Set to a custom `TokenCounter` for model-specific tokenization.
    #[serde(skip)]
    pub token_counter: Option<Arc<dyn TokenCounter>>,

    // Legacy fields — kept for backward compatibility with existing configs.
    // New code should use `compaction.*` instead.
    #[serde(default)]
    pub keep_recent: usize,
    #[serde(default)]
    pub keep_first: usize,
    #[serde(default)]
    pub tool_output_max_lines: usize,
}

impl std::fmt::Debug for ContextConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextConfig")
            .field("max_context_tokens", &self.max_context_tokens)
            .field("system_prompt_tokens", &self.system_prompt_tokens)
            .field("compaction", &self.compaction)
            .field("token_counter", &self.token_counter.as_ref().map(|_| "..."))
            .finish()
    }
}

impl ContextConfig {
    /// Returns the configured token counter, or the default heuristic (chars/4).
    pub fn counter(&self) -> &dyn TokenCounter {
        self.token_counter
            .as_deref()
            .unwrap_or(&super::token::HeuristicTokenCounter)
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 100_000,
            system_prompt_tokens: 4_000,
            compaction: CompactionConfig::default(),
            token_counter: None,
            keep_recent: 10,
            keep_first: 2,
            tool_output_max_lines: 50,
        }
    }
}
